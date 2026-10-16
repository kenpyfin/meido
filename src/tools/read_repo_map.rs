use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

pub struct ReadRepoMapTool {
    working_dir: PathBuf,
}

impl ReadRepoMapTool {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
        }
    }
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".idea"
            | ".cursor"
            | "runtime"
            | "__pycache__"
    )
}

fn is_source_like(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "rb"
            | "php"
            | "swift"
            | "m"
            | "mm"
            | "sql"
            | "sh"
    )
}

fn collect_files(root: &Path, max_files: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !is_skipped_dir(name) {
                    stack.push(path);
                }
                continue;
            }
            if is_source_like(&path) {
                out.push(path);
                if out.len() >= max_files {
                    return out;
                }
            }
        }
    }
    out
}

fn extract_symbols(path: &Path, content: &str, max_symbols_per_file: usize) -> Vec<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut symbols = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let matched = match ext {
            "rs" => {
                trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("enum ")
            }
            "ts" | "tsx" | "js" | "jsx" => {
                trimmed.starts_with("function ")
                    || trimmed.starts_with("export function ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("export class ")
                    || trimmed.starts_with("const ")
                    || trimmed.starts_with("export const ")
            }
            "py" => trimmed.starts_with("def ") || trimmed.starts_with("class "),
            _ => {
                trimmed.starts_with("fn ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("def ")
            }
        };
        if matched {
            symbols.push(format!("L{}: {}", idx + 1, trimmed));
            if symbols.len() >= max_symbols_per_file {
                break;
            }
        }
    }
    symbols
}

#[async_trait]
impl Tool for ReadRepoMapTool {
    fn name(&self) -> &str {
        "read_repo_map"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_repo_map",
            "Build a lightweight repository map with source files and top-level symbol signatures.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "Optional directory path to map (defaults to current workspace root)."
                    },
                    "max_files": {
                        "type": "integer",
                        "description": "Maximum number of source files to include."
                    },
                    "max_symbols_per_file": {
                        "type": "integer",
                        "description": "Maximum symbol lines to show per file."
                    },
                    "include_symbols": {
                        "type": "boolean",
                        "description": "Whether to include symbol signatures."
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let max_files = input
            .get("max_files")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(200)
            .clamp(1, 1000);
        let max_symbols_per_file = input
            .get("max_symbols_per_file")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(8)
            .clamp(1, 40);
        let include_symbols = input
            .get("include_symbols")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let auth = super::auth_context_from_input(&input);
        let tool_working_dir =
            super::resolve_tool_working_dir_for_auth(&self.working_dir, auth.as_ref());
        let root = match input.get("path").and_then(|v| v.as_str()) {
            Some(path) => super::resolve_tool_path(&self.working_dir, &tool_working_dir, path),
            None => tool_working_dir,
        };
        if let Err(msg) =
            super::assert_persona_tool_path_allowed(&self.working_dir, &root, auth.as_ref(), false)
        {
            return ToolResult::error(msg);
        }

        let files = collect_files(&root, max_files);
        if files.is_empty() {
            return ToolResult::success(format!("Repository map is empty for {}.", root.display()));
        }

        let mut lines = Vec::new();
        lines.push(format!("Repository map root: {}", root.display()));
        lines.push(format!("Files indexed: {}", files.len()));
        lines.push(String::new());

        for file in files {
            let rel = file
                .strip_prefix(&root)
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| file.display().to_string());
            lines.push(format!("- {}", rel));
            if include_symbols {
                if let Ok(content) = fs::read_to_string(&file) {
                    let symbols = extract_symbols(&file, &content, max_symbols_per_file);
                    for symbol in symbols {
                        lines.push(format!("  - {}", symbol));
                    }
                }
            }
        }

        ToolResult::success(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_repo_map_basic() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_repo_map_{}",
            uuid::Uuid::new_v4()
        ));
        // Tools resolve their working dir to `<root>/shared`, so place sources there.
        let src = root.join("shared").join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(src.join("lib.rs"), "pub fn helper() {}\n").unwrap();

        let tool = ReadRepoMapTool::new(root.to_str().unwrap());
        let result = tool.execute(json!({"max_files": 10})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("main.rs"));
        assert!(result.content.contains("fn main()"));

        let _ = std::fs::remove_dir_all(root);
    }
}
