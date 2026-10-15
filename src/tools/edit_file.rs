use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::safety_redaction::EnvSecretRedactor;

use super::{schema_object, Tool, ToolResult};

pub struct EditFileTool {
    working_dir: PathBuf,
    env_redactor: Option<Arc<EnvSecretRedactor>>,
}

impl EditFileTool {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
            env_redactor: None,
        }
    }

    pub fn with_redactor(mut self, redactor: Arc<EnvSecretRedactor>) -> Self {
        self.env_redactor = Some(redactor);
        self
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "edit_file",
            "Edit a file by replacing an exact string match with new content. The old_string must be unique in the file.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact string to find and replace (must be unique in the file)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The string to replace with"
                    }
                }),
                &["path", "old_string", "new_string"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing 'path' parameter".into()),
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

        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'old_string' parameter".into()),
        };
        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'new_string' parameter".into()),
        };

        info!("Editing file: {}", resolved_path.display());

        let content = match tokio::fs::read_to_string(&resolved_path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read file: {e}")),
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            return ToolResult::error(
                "old_string not found in file. Make sure the string matches exactly.".into(),
            );
        }
        if count > 1 {
            return ToolResult::error(format!(
                "old_string found {count} times in file. It must be unique. Provide more context to make it unique."
            ));
        }

        let new_content = content.replacen(old_string, new_string, 1);
        match tokio::fs::write(&resolved_path, new_content).await {
            Ok(()) => {
                if let Some(redactor) = &self.env_redactor {
                    redactor.register_skill_env_file(&resolved_path);
                }
                ToolResult::success(format!("Successfully edited {}", resolved_path.display()))
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup_file(content: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_ef_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("edit_me.txt");
        std::fs::write(&file, content).unwrap();
        (dir, file)
    }

    #[tokio::test]
    async fn test_edit_file_success() {
        let (dir, file) = setup_file("hello world");
        let tool = EditFileTool::new(".");
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "old_string": "world",
                "new_string": "rust"
            }))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello rust");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tool = EditFileTool::new(".");
        let result = tool
            .execute(json!({
                "path": "/nonexistent/file.txt",
                "old_string": "a",
                "new_string": "b"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_edit_file_old_string_not_found() {
        let (dir, file) = setup_file("hello world");
        let tool = EditFileTool::new(".");
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "old_string": "xyz",
                "new_string": "abc"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("not found in file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_file_multiple_matches() {
        let (dir, file) = setup_file("aaa bbb aaa");
        let tool = EditFileTool::new(".");
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "old_string": "aaa",
                "new_string": "ccc"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("2 times"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_file_missing_params() {
        let tool = EditFileTool::new(".");
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'path'"));

        let result = tool.execute(json!({"path": "/tmp/x"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'old_string'"));
    }

    #[tokio::test]
    async fn test_edit_file_resolves_relative_to_working_dir() {
        let root =
            std::env::temp_dir().join(format!("finally_a_value_bot_ef2_{}", uuid::Uuid::new_v4()));
        let work = root.join("workspace");
        let shared = work.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let file = shared.join("edit_me.txt");
        std::fs::write(&file, "aaa bbb").unwrap();

        let tool = EditFileTool::new(work.to_str().unwrap());
        let result = tool
            .execute(json!({
                "path": "edit_me.txt",
                "old_string": "bbb",
                "new_string": "ccc"
            }))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "aaa ccc");

        let _ = std::fs::remove_dir_all(&root);
    }
}
