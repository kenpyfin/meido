use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::safety_redaction::EnvSecretRedactor;

use super::{schema_object, Tool, ToolResult};

pub struct WriteFileTool {
    working_dir: PathBuf,
    env_redactor: Option<Arc<EnvSecretRedactor>>,
}

impl WriteFileTool {
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
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_file",
            "Write content to a file. Creates the file and any parent directories if they don't exist. Overwrites existing content.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The file path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write"
                    }
                }),
                &["path", "content"],
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

        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error("Missing 'content' parameter".into()),
        };

        info!("Writing file: {}", resolved_path.display());

        if let Some(parent) = resolved_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolResult::error(format!("Failed to create directories: {e}"));
            }
        }

        match tokio::fs::write(&resolved_path, content).await {
            Ok(()) => {
                if let Some(redactor) = &self.env_redactor {
                    redactor.register_skill_env_file(&resolved_path);
                }
                ToolResult::success(format!("Successfully wrote to {}", resolved_path.display()))
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_write_file_success() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_wf_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.txt");

        let tool = WriteFileTool::new(".");
        let result = tool
            .execute(json!({"path": file.to_str().unwrap(), "content": "hello world"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("Successfully wrote"));

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_file_creates_parent_dirs() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_wf2_{}", uuid::Uuid::new_v4()));
        let file = dir.join("sub").join("dir").join("file.txt");

        let tool = WriteFileTool::new(".");
        let result = tool
            .execute(json!({"path": file.to_str().unwrap(), "content": "nested"}))
            .await;
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "nested");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_file_missing_params() {
        let tool = WriteFileTool::new(".");

        let result = tool.execute(json!({"content": "hello"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'path'"));

        let result = tool.execute(json!({"path": "/tmp/x"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'content'"));
    }

    #[tokio::test]
    async fn test_write_file_resolves_relative_to_working_dir() {
        let root =
            std::env::temp_dir().join(format!("finally_a_value_bot_wf3_{}", uuid::Uuid::new_v4()));
        let work = root.join("workspace");
        std::fs::create_dir_all(&work).unwrap();

        let tool = WriteFileTool::new(work.to_str().unwrap());
        let result = tool
            .execute(json!({"path": "nested/out.txt", "content": "ok"}))
            .await;
        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(work.join("shared/nested/out.txt")).unwrap(),
            "ok"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
