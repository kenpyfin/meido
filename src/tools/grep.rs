use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

pub struct GrepTool {
    working_dir: PathBuf,
}

impl GrepTool {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
        }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "grep",
            "Search file contents using a regex pattern. Returns matching lines with file paths and line numbers. Prefer a narrow `path` and `glob` filter (e.g. path `shared/`, glob `*.md`) — avoid searching all of `shared/` without a glob.",
            schema_object(
                json!({
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in (default: current directory)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g., '*.rs')"
                    }
                }),
                &["pattern"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing 'pattern' parameter".into()),
        };
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let auth = super::auth_context_from_input(&input);
        let working_dir =
            super::resolve_tool_working_dir_for_auth(&self.working_dir, auth.as_ref());
        let workspace_root = &self.working_dir;
        let resolved_path = super::resolve_tool_path(workspace_root, &working_dir, path);
        if !is_path_within_workspace_root(&resolved_path, workspace_root) {
            return ToolResult::error(format!(
                "Path '{}' is outside the workspace root '{}'. \
grep is limited to workspace files.",
                resolved_path.display(),
                workspace_root.display()
            ));
        }
        let resolved_path_str = resolved_path.to_string_lossy().to_string();
        if let Err(msg) = crate::tools::path_guard::check_path(&resolved_path_str) {
            return ToolResult::error(msg);
        }
        if let Err(msg) = super::assert_persona_tool_path_allowed(
            workspace_root,
            &resolved_path,
            auth.as_ref(),
            false,
        ) {
            return ToolResult::error(msg);
        }
        let file_glob = input.get("glob").and_then(|v| v.as_str());

        info!("Grep: {} in {}", pattern, resolved_path.display());

        let re = match regex::Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Invalid regex: {e}")),
        };

        let mut results = Vec::new();
        let mut file_count = 0;

        if let Err(e) = grep_recursive(
            &resolved_path,
            file_glob,
            &re,
            &mut results,
            &mut file_count,
        ) {
            return ToolResult::error(format!("Search error: {e}"));
        }

        if results.is_empty() {
            ToolResult::success("No matches found.".into())
        } else {
            truncate_grep_results(&mut results);
            ToolResult::success(results.join("\n"))
        }
    }
}

const MAX_GREP_OUTPUT_CHARS: usize = 32_000;
const MAX_GREP_MATCH_LINES: usize = 500;
const MAX_GREP_FILES_SCANNED: usize = 3_000;

fn is_path_within_workspace_root(path: &Path, workspace_root: &Path) -> bool {
    let resolved_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    resolved_path.starts_with(resolved_root)
}

fn truncate_grep_results(results: &mut Vec<String>) {
    if results.len() > MAX_GREP_MATCH_LINES {
        results.truncate(MAX_GREP_MATCH_LINES);
        results.push("... (line matches truncated)".into());
    }
    let mut total = 0usize;
    let mut keep = 0usize;
    for line in results.iter() {
        let next = total.saturating_add(line.len()).saturating_add(1);
        if next > MAX_GREP_OUTPUT_CHARS {
            break;
        }
        total = next;
        keep += 1;
    }
    if keep < results.len() {
        results.truncate(keep);
        results.push("... (output size truncated)".into());
    }
}

fn should_skip_grep_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "vault_db" | "__pycache__" | ".git"
        )
}

fn should_skip_grep_file(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "safetensors"
            | "bin"
            | "pdf"
            | "zip"
            | "gz"
            | "sqlite"
            | "db"
    )
}

fn grep_recursive(
    path: &Path,
    file_glob: Option<&str>,
    re: &regex::Regex,
    results: &mut Vec<String>,
    file_count: &mut usize,
) -> std::io::Result<()> {
    let metadata = std::fs::metadata(path)?;

    if metadata.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !should_skip_grep_file(&name) {
            grep_file(path, re, results)?;
        }
    } else if metadata.is_dir() {
        let glob_pattern = file_glob.and_then(|g| glob::Pattern::new(g).ok());

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if should_skip_grep_dir(&name) {
                continue;
            }

            if entry_path.is_dir() {
                grep_recursive(&entry_path, file_glob, re, results, file_count)?;
            } else if entry_path.is_file() {
                if crate::tools::path_guard::is_blocked(&entry_path) {
                    continue;
                }
                if should_skip_grep_file(&name) {
                    continue;
                }
                if let Some(ref pat) = glob_pattern {
                    if !pat.matches(&name) {
                        continue;
                    }
                }
                *file_count += 1;
                if *file_count > MAX_GREP_FILES_SCANNED {
                    results.push("... (file scan limit reached)".into());
                    return Ok(());
                }
                grep_file(&entry_path, re, results)?;
            }
        }
    }
    Ok(())
}

fn grep_file(path: &Path, re: &regex::Regex, results: &mut Vec<String>) -> std::io::Result<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()), // Skip binary / unreadable files
    };

    for (line_num, line) in content.lines().enumerate() {
        if re.is_match(line) {
            results.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
            if results.len() >= MAX_GREP_MATCH_LINES {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup_grep_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_grep_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("world.txt"), "hello world\ngoodbye world\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn test_grep_finds_matches() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep_ws_{}",
            uuid::Uuid::new_v4()
        ));
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let dir = setup_grep_dir();
        std::fs::rename(&dir, shared.join("sample")).unwrap();
        let tool = GrepTool::new(root.to_str().unwrap());
        let result = tool
            .execute(json!({"pattern": "hello", "path": "sample"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("hello"));
        // Should have file:line format
        assert!(result.content.contains(":"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep_ws_{}",
            uuid::Uuid::new_v4()
        ));
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let dir = setup_grep_dir();
        std::fs::rename(&dir, shared.join("sample")).unwrap();
        let tool = GrepTool::new(root.to_str().unwrap());
        let result = tool
            .execute(json!({"pattern": "zzzzzzz", "path": "sample"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("No matches"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_grep_with_file_glob() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep_ws_{}",
            uuid::Uuid::new_v4()
        ));
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let dir = setup_grep_dir();
        std::fs::rename(&dir, shared.join("sample")).unwrap();
        let tool = GrepTool::new(root.to_str().unwrap());
        // Only search .txt files
        let result = tool
            .execute(json!({
                "pattern": "hello",
                "path": "sample",
                "glob": "*.txt"
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("world.txt"));
        // Should NOT match the .rs file
        assert!(!result.content.contains("hello.rs"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let tool = GrepTool::new(".");
        let result = tool
            .execute(json!({"pattern": "[invalid", "path": "."}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_grep_missing_pattern() {
        let tool = GrepTool::new(".");
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'pattern'"));
    }

    #[test]
    fn test_grep_file_function() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_gf_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "foo bar\nbaz qux\nfoo again\n").unwrap();

        let re = regex::Regex::new("foo").unwrap();
        let mut results = Vec::new();
        grep_file(&file, &re, &mut results).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].contains(":1:"));
        assert!(results[1].contains(":3:"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_recursive_skips_hidden_dirs() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_gr_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join(".hidden").join("secret.txt"), "match_me").unwrap();
        std::fs::write(dir.join("visible.txt"), "match_me").unwrap();

        let re = regex::Regex::new("match_me").unwrap();
        let mut results = Vec::new();
        let mut count = 0;
        grep_recursive(&dir, None, &re, &mut results, &mut count).unwrap();

        // Should only find in visible.txt
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("visible.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_grep_defaults_to_working_dir() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep2_{}",
            uuid::Uuid::new_v4()
        ));
        let work = root.join("workspace");
        let shared = work.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("a.txt"), "needle").unwrap();

        let tool = GrepTool::new(work.to_str().unwrap());
        let result = tool.execute(json!({"pattern":"needle"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("a.txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_grep_rejects_path_outside_workspace() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep_outside_{}",
            uuid::Uuid::new_v4()
        ));
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let outside_dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_grep_external_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("a.txt"), "needle").unwrap();

        let tool = GrepTool::new(root.to_str().unwrap());
        let result = tool
            .execute(json!({
                "pattern":"needle",
                "path": outside_dir.to_string_lossy().to_string()
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("outside the workspace root"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside_dir);
    }
}
