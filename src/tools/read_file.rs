use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tracing::info;

use crate::claude::ToolDefinition;

use super::{schema_object, Tool, ToolResult};

pub struct ReadFileTool {
    working_dir: PathBuf,
}

impl ReadFileTool {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_file",
            "Read the contents of a file at the given path. Returns the file content with line numbers.",
            schema_object(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The file path to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    },
                    "center_line": {
                        "type": "integer",
                        "description": "Optional 1-based center line for adaptive window reads"
                    },
                    "context_before": {
                        "type": "integer",
                        "description": "When center_line is set, lines to include before the center"
                    },
                    "context_after": {
                        "type": "integer",
                        "description": "When center_line is set, lines to include after the center"
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
        let auth = super::auth_context_from_input(&input);
        let working_dir =
            super::resolve_tool_working_dir_for_auth(&self.working_dir, auth.as_ref());
        let resolved_path = super::resolve_tool_path(&self.working_dir, &working_dir, path);
        let resolved_path_str = resolved_path.to_string_lossy().to_string();

        if let Err(msg) = crate::tools::path_guard::check_path(&resolved_path_str) {
            return ToolResult::error(msg);
        }
        if let Err(msg) = super::assert_persona_tool_path_allowed(
            &self.working_dir,
            &resolved_path,
            auth.as_ref(),
            false,
        ) {
            return ToolResult::error(msg);
        }

        if is_likely_binary_media_path(&resolved_path) {
            return ToolResult::error(format!(
                "Cannot read binary/image file as text: {path}. To show this file to the user, put it in your final assistant message as a markdown image link with the absolute local path (e.g. ![caption]({resolved_path_str})) — do not use read_file on images."
            ));
        }

        info!("Reading file: {}", resolved_path.display());

        let content = match tokio::fs::read_to_string(&resolved_path).await {
            Ok(c) => c,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("UTF-8") || msg.contains("utf-8") {
                    return ToolResult::error(format!(
                        "Failed to read file: {msg}. This path is likely a binary/image file — use a markdown image link with the absolute path in your final message instead of read_file."
                    ));
                }
                if e.kind() == std::io::ErrorKind::NotFound {
                    return ToolResult::error(format!(
                        "Not found: '{path}' resolved to '{}' but the file does not exist under declared roots. \
                         Call locate_file with this path, or ask the user for the correct location — \
                         do not run find over large trees.",
                        resolved_path.display()
                    ))
                    .with_error_type("not_found");
                }
                return ToolResult::error(format!("Failed to read file: {msg}"));
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|o| (o as usize).saturating_sub(1))
            .unwrap_or(0);
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|l| l as usize)
            .unwrap_or(800);

        let center_line = input
            .get("center_line")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let context_before = input
            .get("context_before")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(25);
        let context_after = input
            .get("context_after")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(25);

        let (offset, limit) = if let Some(center) = center_line {
            let center_idx = center.saturating_sub(1);
            let start = center_idx.saturating_sub(context_before);
            let adaptive_limit = context_before + context_after + 1;
            (start, adaptive_limit.max(1))
        } else {
            (offset, limit.max(1))
        };

        let end = (offset + limit).min(lines.len());
        let selected: Vec<String> = lines[offset..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
            .collect();

        ToolResult::success(selected.join("\n"))
    }
}

fn is_likely_binary_media_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "ico"
                    | "pdf"
                    | "zip"
                    | "gz"
                    | "tar"
                    | "mp4"
                    | "mov"
                    | "mp3"
                    | "wav"
                    | "woff"
                    | "woff2"
                    | "ttf"
                    | "otf"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_read_file_success() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_rf_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\nline5").unwrap();

        let tool = ReadFileTool::new(".");
        let result = tool.execute(json!({"path": file.to_str().unwrap()})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line5"));
        // Should have line numbers
        assert!(result.content.contains("1\t"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_file_with_offset_and_limit() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_rf2_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne").unwrap();

        let tool = ReadFileTool::new(".");
        // offset=2 (1-based, becomes index 1), limit=2 -> lines 2 and 3
        let result = tool
            .execute(json!({"path": file.to_str().unwrap(), "offset": 2, "limit": 2}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("b"));
        assert!(result.content.contains("c"));
        assert!(!result.content.contains("\ta\n") && !result.content.contains("\td"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_file_with_center_line_window() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_rf4_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "1\n2\n3\n4\n5\n6\n7").unwrap();

        let tool = ReadFileTool::new(".");
        let result = tool
            .execute(json!({
                "path": file.to_str().unwrap(),
                "center_line": 4,
                "context_before": 1,
                "context_after": 2
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("3"));
        assert!(result.content.contains("4"));
        assert!(result.content.contains("6"));
        assert!(!result.content.contains("\t1"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_file_rejects_png() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_rf_png_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("image.png");
        std::fs::write(&file, [137u8, 80, 78, 71]).unwrap();

        let tool = ReadFileTool::new(dir.to_str().unwrap());
        let result = tool.execute(json!({"path": "image.png"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("binary/image"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tool = ReadFileTool::new(".");
        let result = tool.execute(json!({"path": "/nonexistent/file.txt"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_read_file_missing_path() {
        let tool = ReadFileTool::new(".");
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'path'"));
    }

    #[tokio::test]
    async fn test_read_file_resolves_relative_to_working_dir() {
        let root =
            std::env::temp_dir().join(format!("finally_a_value_bot_rf3_{}", uuid::Uuid::new_v4()));
        let work = root.join("workspace");
        let shared = work.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("test.txt"), "inside").unwrap();

        let tool = ReadFileTool::new(work.to_str().unwrap());
        let result = tool.execute(json!({"path": "test.txt"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("inside"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
