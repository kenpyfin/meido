use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tracing::info;

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

fn split_workspace_prefixed_pattern(
    workspace_root: &std::path::Path,
    working_dir: &std::path::Path,
    pattern: &str,
) -> Option<(PathBuf, String)> {
    let rel = PathBuf::from(pattern);
    let mut comps = rel.components();
    let first = comps.next()?.as_os_str().to_string_lossy().to_string();
    if first != "skills" && first != "runtime" {
        return None;
    }
    let wildcard_idx = pattern
        .char_indices()
        .find_map(|(idx, ch)| (ch == '*' || ch == '?' || ch == '[').then_some(idx))
        .unwrap_or(pattern.len());
    let (base_part, tail_part) = pattern.split_at(wildcard_idx);
    let base_part = base_part.trim_end_matches('/');
    let resolved_base = super::resolve_tool_path(workspace_root, working_dir, base_part);
    let tail = tail_part.trim_start_matches('/').to_string();
    Some((resolved_base, tail))
}

pub struct GlobTool {
    working_dir: PathBuf,
}

impl GlobTool {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
        }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "glob",
            "Find files matching a glob pattern. Returns matching file paths.",
            schema_object(
                json!({
                    "pattern": {
                        "type": "string",
                        "description": "The glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Base directory to search from (default: current directory)"
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
        let base = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let auth = super::auth_context_from_input(&input);
        let working_dir =
            super::resolve_tool_working_dir_for_auth(&self.working_dir, auth.as_ref());
        let resolved_base = super::resolve_tool_path(&self.working_dir, &working_dir, base);
        let resolved_base_str = resolved_base.to_string_lossy().to_string();

        if let Err(msg) = crate::tools::path_guard::check_path(&resolved_base_str) {
            return ToolResult::error(msg);
        }
        if let Err(msg) = super::assert_persona_tool_path_allowed(
            &self.working_dir,
            &resolved_base,
            auth.as_ref(),
            false,
        ) {
            return ToolResult::error(msg);
        }

        let (full_pattern, log_base) = if pattern.starts_with('/') {
            (pattern.to_string(), pattern.to_string())
        } else if let Some((prefix_base, tail)) =
            split_workspace_prefixed_pattern(&self.working_dir, &working_dir, pattern)
        {
            let full = if tail.is_empty() {
                prefix_base.display().to_string()
            } else {
                format!("{}/{}", prefix_base.display(), tail)
            };
            (full, prefix_base.display().to_string())
        } else {
            (
                format!("{}/{}", resolved_base.display(), pattern),
                resolved_base.display().to_string(),
            )
        };
        info!("Glob: {} in {}", pattern, log_base);

        match glob::glob(&full_pattern) {
            Ok(paths) => {
                let mut matches: Vec<String> = paths
                    .filter_map(|p| p.ok())
                    .map(|p| p.display().to_string())
                    .collect();
                matches = crate::tools::path_guard::filter_paths(matches);
                matches.sort();

                if matches.is_empty() {
                    ToolResult::success("No files found matching pattern.".into())
                } else {
                    let count = matches.len();
                    if count > 500 {
                        matches.truncate(500);
                        matches.push(format!("... and {} more files", count - 500));
                    }
                    ToolResult::success(matches.join("\n"))
                }
            }
            Err(e) => ToolResult::error(format!("Invalid glob pattern: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_glob_finds_files() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_glob_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        std::fs::write(dir.join("c.rs"), "").unwrap();

        let tool = GlobTool::new(".");
        let result = tool
            .execute(json!({"pattern": "*.txt", "path": dir.to_str().unwrap()}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("b.txt"));
        assert!(!result.content.contains("c.rs"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_glob2_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let tool = GlobTool::new(".");
        let result = tool
            .execute(json!({"pattern": "*.xyz", "path": dir.to_str().unwrap()}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("No files found"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_glob_missing_pattern() {
        let tool = GlobTool::new(".");
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'pattern'"));
    }

    #[tokio::test]
    async fn test_glob_defaults_to_working_dir() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_glob3_{}",
            uuid::Uuid::new_v4()
        ));
        let work = root.join("workspace");
        let shared = work.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("x.txt"), "").unwrap();

        let tool = GlobTool::new(work.to_str().unwrap());
        let result = tool.execute(json!({"pattern":"*.txt"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("x.txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_glob_resolves_skills_prefix_from_persona_dir() {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_glob4_{}",
            uuid::Uuid::new_v4()
        ));
        let work = root.join("workspace");
        let skills_dir = work.join("skills").join("demo");
        let persona = work.join("shared").join("personas").join("1").join("1");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(&persona).unwrap();
        std::fs::write(skills_dir.join("tool.py"), "").unwrap();

        let tool = GlobTool::new(work.to_str().unwrap());
        let result = tool
            .execute(json!({"pattern":"skills/demo/*.py","path":persona.to_str().unwrap()}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("tool.py"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
