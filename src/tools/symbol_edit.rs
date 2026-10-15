use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

pub struct SymbolEditTool {
    working_dir: PathBuf,
    enabled: bool,
}

impl SymbolEditTool {
    pub fn new(working_dir: &str, enabled: bool) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
            enabled,
        }
    }
}

fn detect_language(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "py" => "python",
        "rs" => "rust",
        "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "kt" | "kts" | "c" | "cc" | "cpp" | "h"
        | "hpp" | "cs" | "php" | "swift" | "m" | "mm" => "brace",
        _ => "unknown",
    }
}

fn find_python_symbol_span(content: &str, symbol: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start = None;
    let mut base_indent = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let is_target = trimmed.starts_with(&format!("def {}(", symbol))
            || trimmed.starts_with(&format!("class {}(", symbol))
            || trimmed == format!("class {}:", symbol)
            || trimmed == format!("def {}:", symbol);
        if is_target {
            start = Some(idx);
            base_indent = line.len() - trimmed.len();
            break;
        }
    }
    let start = start?;
    let mut end = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= base_indent {
            end = idx;
            break;
        }
    }
    Some((start, end))
}

fn find_brace_symbol_span(content: &str, symbol: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.contains(&format!("fn {}(", symbol))
            || trimmed.contains(&format!("function {}(", symbol))
            || trimmed.contains(&format!("class {}", symbol))
            || trimmed.contains(&format!("struct {}", symbol))
            || trimmed.contains(&format!("impl {}", symbol))
        {
            start = Some(idx);
            break;
        }
    }
    let start = start?;

    let mut seen_open = false;
    let mut depth = 0i32;
    for (idx, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            if ch == '{' {
                seen_open = true;
                depth += 1;
            } else if ch == '}' && seen_open {
                depth -= 1;
            }
        }
        if seen_open && depth == 0 {
            return Some((start, idx + 1));
        }
    }
    None
}

fn find_symbol_span(path: &Path, content: &str, symbol: &str) -> Result<(usize, usize), String> {
    match detect_language(path) {
        "python" => find_python_symbol_span(content, symbol)
            .ok_or_else(|| format!("Could not resolve symbol '{symbol}' in python mode.")),
        "brace" => find_brace_symbol_span(content, symbol)
            .ok_or_else(|| format!("Could not resolve symbol '{symbol}' in brace mode.")),
        _ => Err(
            "Unsupported language for symbol editing. Use apply_search_replace fallback."
                .to_string(),
        ),
    }
}

#[async_trait]
impl Tool for SymbolEditTool {
    fn name(&self) -> &str {
        "symbol_edit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "symbol_edit",
            "Replace a symbol block by name using language-aware span detection. Use as a guarded fallback when search/replace is insufficient.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Target symbol name (function/class/struct/impl token)"
                    },
                    "replacement": {
                        "type": "string",
                        "description": "Full replacement text for the resolved symbol span"
                    },
                    "mode": {
                        "type": "string",
                        "description": "Edit mode. Currently supported: entire_symbol"
                    }
                }),
                &["path", "symbol", "replacement"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        if !self.enabled {
            return ToolResult::error(
                "symbol_edit is disabled by configuration. Use apply_search_replace or enable SYMBOL_EDIT_ENABLED=true.".to_string(),
            )
            .with_error_type("feature_disabled");
        }
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing 'path' parameter".into()),
        };
        let symbol = match input.get("symbol").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim(),
            _ => return ToolResult::error("Missing 'symbol' parameter".into()),
        };
        let replacement = match input.get("replacement").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return ToolResult::error("Missing 'replacement' parameter".into()),
        };
        let mode = input
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("entire_symbol");
        if mode != "entire_symbol" {
            return ToolResult::error(format!(
                "Unsupported mode '{}'. Supported mode: entire_symbol",
                mode
            ));
        }

        let auth = super::auth_context_from_input(&input);
        let working_dir =
            super::resolve_tool_working_dir_for_auth(&self.working_dir, auth.as_ref());
        let resolved_path = super::resolve_tool_path(&self.working_dir, &working_dir, path);
        let resolved_path_str = resolved_path.to_string_lossy().to_string();
        if let Err(msg) = crate::tools::path_guard::check_path(&resolved_path_str) {
            return ToolResult::error(msg);
        }
        if let Err(msg) =
            super::check_shadow_workspace_write(self.working_dir.as_path(), &resolved_path)
        {
            return ToolResult::error(msg);
        }
        if let Err(msg) = super::assert_persona_tool_path_allowed(
            &self.working_dir,
            &resolved_path,
            auth.as_ref(),
            true,
        ) {
            return ToolResult::error(msg);
        }
        info!(
            "Applying symbol edit for symbol '{}' in {}",
            symbol,
            resolved_path.display()
        );
        let content = match tokio::fs::read_to_string(&resolved_path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read file: {e}")),
        };
        let lines: Vec<&str> = content.lines().collect();
        let (start, end) = match find_symbol_span(&resolved_path, &content, symbol) {
            Ok(span) => span,
            Err(msg) => {
                return ToolResult::error(format!(
                    "{} Fallback: use apply_search_replace with a larger search block around the symbol declaration.",
                    msg
                ))
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(&lines[..start]);
        for line in replacement.lines() {
            out.push(line);
        }
        out.extend_from_slice(&lines[end..]);
        let output = out.join("\n");
        match tokio::fs::write(&resolved_path, output).await {
            Ok(()) => ToolResult::success(format!(
                "Successfully replaced symbol '{}' span (lines {}-{}).",
                symbol,
                start + 1,
                end
            )),
            Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_symbol_edit_python_function() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_symbol_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mod.py");
        std::fs::write(
            &file,
            "def foo():\n    return 1\n\ndef bar():\n    return 2\n",
        )
        .unwrap();
        let tool = SymbolEditTool::new(".", true);
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "symbol": "foo",
                "replacement": "def foo():\n    return 99"
            }))
            .await;
        assert!(!result.is_error);
        let updated = std::fs::read_to_string(&file).unwrap();
        assert!(updated.contains("return 99"));
        assert!(updated.contains("def bar():"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
