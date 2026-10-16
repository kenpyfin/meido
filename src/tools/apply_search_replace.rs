use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tracing::info;

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

pub struct ApplySearchReplaceTool {
    working_dir: PathBuf,
    allow_fuzzy_default: bool,
}

impl ApplySearchReplaceTool {
    pub fn new(working_dir: &str, allow_fuzzy_default: bool) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
            allow_fuzzy_default,
        }
    }
}

#[derive(Clone, Debug)]
struct Block {
    search: String,
    replace: String,
    allow_multiple: bool,
    expected_matches: Option<usize>,
    allow_fuzzy: bool,
}

fn extract_blocks(
    input: &serde_json::Value,
    allow_fuzzy_default: bool,
) -> Result<Vec<Block>, String> {
    if let Some(blocks) = input.get("blocks").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for block in blocks {
            let search = block
                .get("search")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Each block requires 'search'".to_string())?
                .to_string();
            let replace = block
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Each block requires 'replace'".to_string())?
                .to_string();
            let allow_multiple = block
                .get("allow_multiple")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let expected_matches = block
                .get("expected_matches")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let allow_fuzzy = block
                .get("allow_fuzzy")
                .and_then(|v| v.as_bool())
                .unwrap_or(allow_fuzzy_default);
            out.push(Block {
                search,
                replace,
                allow_multiple,
                expected_matches,
                allow_fuzzy,
            });
        }
        if out.is_empty() {
            return Err("blocks must not be empty".to_string());
        }
        return Ok(out);
    }

    let search = input
        .get("search")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'search' parameter".to_string())?
        .to_string();
    let replace = input
        .get("replace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'replace' parameter".to_string())?
        .to_string();
    let allow_multiple = input
        .get("allow_multiple")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let expected_matches = input
        .get("expected_matches")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let allow_fuzzy = input
        .get("allow_fuzzy")
        .and_then(|v| v.as_bool())
        .unwrap_or(allow_fuzzy_default);
    Ok(vec![Block {
        search,
        replace,
        allow_multiple,
        expected_matches,
        allow_fuzzy,
    }])
}

fn normalize_for_fuzzy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fuzzy_replace_once(content: &str, search: &str, replace: &str) -> Option<String> {
    let normalized_search = normalize_for_fuzzy(search);
    if normalized_search.is_empty() {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = search.lines().collect();
    if search_lines.is_empty() {
        return None;
    }
    let window = search_lines.len();
    if window == 0 || lines.len() < window {
        return None;
    }

    let mut match_idx: Option<usize> = None;
    for i in 0..=(lines.len() - window) {
        let candidate = lines[i..i + window].join("\n");
        if normalize_for_fuzzy(&candidate) == normalized_search {
            if match_idx.is_some() {
                return None;
            }
            match_idx = Some(i);
        }
    }

    let start = match_idx?;
    let mut updated = Vec::with_capacity(lines.len() + replace.lines().count());
    updated.extend_from_slice(&lines[..start]);
    for line in replace.lines() {
        updated.push(line);
    }
    updated.extend_from_slice(&lines[start + window..]);
    Some(updated.join("\n"))
}

fn nearby_hints(content: &str, search: &str) -> String {
    let first_token = search
        .split_whitespace()
        .find(|t| t.len() >= 4)
        .unwrap_or("")
        .to_ascii_lowercase();
    if first_token.is_empty() {
        return "No useful nearby hints available.".to_string();
    }
    let mut hints = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if line.to_ascii_lowercase().contains(&first_token) {
            hints.push(format!("line {}: {}", idx + 1, line.trim()));
            if hints.len() >= 3 {
                break;
            }
        }
    }
    if hints.is_empty() {
        "No useful nearby hints available.".to_string()
    } else {
        format!("Nearby hints:\n{}", hints.join("\n"))
    }
}

#[async_trait]
impl Tool for ApplySearchReplaceTool {
    fn name(&self) -> &str {
        "apply_search_replace"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "apply_search_replace",
            "Apply deterministic search/replace edits. Exact matching is default; fuzzy is opt-in and conservative.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "search": {
                        "type": "string",
                        "description": "Exact search block for single-block mode"
                    },
                    "replace": {
                        "type": "string",
                        "description": "Replacement block for single-block mode"
                    },
                    "allow_multiple": {
                        "type": "boolean",
                        "description": "When true, replace all exact matches for this block"
                    },
                    "expected_matches": {
                        "type": "integer",
                        "description": "Expected number of exact matches before replacement"
                    },
                    "allow_fuzzy": {
                        "type": "boolean",
                        "description": "Opt-in fuzzy mode (whitespace-normalized, unique multi-line match only)"
                    },
                    "blocks": {
                        "type": "array",
                        "description": "Optional multi-block edit list. Each block supports search/replace/allow_multiple/expected_matches/allow_fuzzy.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "search": {"type": "string"},
                                "replace": {"type": "string"},
                                "allow_multiple": {"type": "boolean"},
                                "expected_matches": {"type": "integer"},
                                "allow_fuzzy": {"type": "boolean"}
                            },
                            "required": ["search", "replace"]
                        }
                    }
                }),
                &["path"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing 'path' parameter".into()),
        };
        let blocks = match extract_blocks(&input, self.allow_fuzzy_default) {
            Ok(v) => v,
            Err(msg) => return ToolResult::error(msg),
        };
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
            "Applying search/replace edits to file: {}",
            resolved_path.display()
        );

        let mut content = match tokio::fs::read_to_string(&resolved_path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read file: {e}")),
        };

        let mut summaries = Vec::new();
        for (idx, block) in blocks.iter().enumerate() {
            let count = content.matches(&block.search).count();
            if let Some(expected) = block.expected_matches {
                if count != expected {
                    return ToolResult::error(format!(
                        "Block {} expected {} exact matches but found {}.",
                        idx + 1,
                        expected,
                        count
                    ));
                }
            }

            if count == 0 {
                if block.allow_fuzzy {
                    if let Some(updated) =
                        fuzzy_replace_once(&content, &block.search, &block.replace)
                    {
                        content = updated;
                        summaries.push(format!(
                            "Block {}: fuzzy unique match replaced (exact match count was 0).",
                            idx + 1
                        ));
                        continue;
                    }
                }
                return ToolResult::error(format!(
                    "Block {}: search text not found. {}\nHint: provide more surrounding context.",
                    idx + 1,
                    nearby_hints(&content, &block.search)
                ));
            }

            if count > 1 && !block.allow_multiple {
                return ToolResult::error(format!(
                    "Block {}: search matched {} times. Set allow_multiple=true or provide more context.",
                    idx + 1,
                    count
                ));
            }

            if block.allow_multiple {
                content = content.replace(&block.search, &block.replace);
                summaries.push(format!(
                    "Block {}: replaced {} exact matches.",
                    idx + 1,
                    count
                ));
            } else {
                content = content.replacen(&block.search, &block.replace, 1);
                summaries.push(format!("Block {}: replaced 1 exact match.", idx + 1));
            }
        }

        match tokio::fs::write(&resolved_path, content).await {
            Ok(()) => ToolResult::success(format!(
                "Successfully edited {}.\n{}",
                resolved_path.display(),
                summaries.join("\n")
            )),
            Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_file(content: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_sr_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("edit_me.txt");
        std::fs::write(&file, content).unwrap();
        (dir, file)
    }

    #[tokio::test]
    async fn test_apply_search_replace_single_exact() {
        let (dir, file) = setup_file("alpha beta");
        let tool = ApplySearchReplaceTool::new(".", false);
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "search": "beta",
                "replace": "gamma"
            }))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha gamma");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_apply_search_replace_multi_match_requires_flag() {
        let (dir, file) = setup_file("x y x");
        let tool = ApplySearchReplaceTool::new(".", false);
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "search": "x",
                "replace": "z"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("allow_multiple=true"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_apply_search_replace_allow_multiple() {
        let (dir, file) = setup_file("x y x");
        let tool = ApplySearchReplaceTool::new(".", false);
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "search": "x",
                "replace": "z",
                "allow_multiple": true
            }))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "z y z");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_apply_search_replace_blocks_mode() {
        let (dir, file) = setup_file("a b c d");
        let tool = ApplySearchReplaceTool::new(".", false);
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "blocks": [
                    {"search":"a b","replace":"w x"},
                    {"search":"c d","replace":"y z"}
                ]
            }))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "w x y z");
        let _ = std::fs::remove_dir_all(dir);
    }
}
