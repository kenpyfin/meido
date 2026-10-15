use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{authorize_chat_access, default_persona_id_for_chat, schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;
use crate::db::{call_blocking, Database};
use crate::safety_redaction::EnvSecretRedactor;

pub struct ExportChatTool {
    db: Arc<Database>,
    data_dir: String,
    env_redactor: Option<Arc<EnvSecretRedactor>>,
}

impl ExportChatTool {
    pub fn new(db: Arc<Database>, data_dir: &str) -> Self {
        ExportChatTool {
            db,
            data_dir: data_dir.to_string(),
            env_redactor: None,
        }
    }

    pub fn with_redactor(mut self, redactor: Arc<EnvSecretRedactor>) -> Self {
        self.env_redactor = Some(redactor);
        self
    }
}

fn confine_export_path(
    data_dir: &str,
    requested: Option<&str>,
    default_name: &str,
) -> Result<PathBuf, String> {
    let exports = PathBuf::from(data_dir).join("exports");
    let raw = requested.unwrap_or(default_name).trim();
    if raw.is_empty() {
        return Err("Export path is empty".into());
    }
    let path = Path::new(raw);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(
            "Export path must be a file name inside the runtime exports directory. Absolute paths and '..' are not allowed."
                .into(),
        );
    }
    Ok(exports.join(path))
}

#[async_trait]
impl Tool for ExportChatTool {
    fn name(&self) -> &str {
        "export_chat"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "export_chat",
            "Export chat history to a markdown file. Returns the file path.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "The chat ID to export"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional file name under the runtime exports directory. Defaults to {chat_id}_{timestamp}.md. Absolute paths are rejected."
                    }
                }),
                &["chat_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: chat_id".into()),
        };
        if let Err(e) = authorize_chat_access(&input, chat_id) {
            return ToolResult::error(e);
        }

        let cid = chat_id;
        let persona_id = match default_persona_id_for_chat(&input, chat_id) {
            Some(pid) => pid,
            None => {
                match call_blocking(self.db.clone(), move |db| {
                    db.get_or_create_default_persona(cid)
                })
                .await
                {
                    Ok(pid) => pid,
                    Err(e) => return ToolResult::error(format!("Failed to resolve persona: {e}")),
                }
            }
        };
        let cid2 = chat_id;
        let pid = persona_id;
        let messages =
            match call_blocking(self.db.clone(), move |db| db.get_all_messages(cid2, pid)).await {
                Ok(msgs) => msgs,
                Err(e) => return ToolResult::error(format!("Failed to load messages: {e}")),
            };

        if messages.is_empty() {
            return ToolResult::error(format!("No messages found for chat {chat_id}."));
        }

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let default_name = format!("{chat_id}_{timestamp}.md");
        let path = match confine_export_path(
            &self.data_dir,
            input.get("path").and_then(|v| v.as_str()),
            &default_name,
        ) {
            Ok(path) => path,
            Err(e) => return ToolResult::error(e),
        };

        // Build markdown
        let mut md = format!("# Chat Export: {chat_id}\n\n");
        md.push_str(&format!(
            "Exported at: {}\n\n---\n\n",
            chrono::Utc::now().to_rfc3339()
        ));

        for msg in &messages {
            let sender = if msg.is_from_bot {
                "**Bot**"
            } else {
                &msg.sender_name
            };
            md.push_str(&format!(
                "**{}** ({})\n\n{}\n\n---\n\n",
                sender, msg.timestamp, msg.content
            ));
        }

        if let Some(redactor) = &self.env_redactor {
            md = redactor.redact_with_patterns(&md);
        }
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("Failed to create directory: {e}"));
            }
        }
        match std::fs::write(&path, &md) {
            Ok(_) => ToolResult::success(format!(
                "Exported {} messages to {}",
                messages.len(),
                path.display()
            )),
            Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, StoredMessage};

    fn test_db() -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_export_{}",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_export_empty_chat() {
        let (db, dir) = test_db();
        let tool = ExportChatTool::new(db, dir.to_str().unwrap());
        let result = tool.execute(json!({"chat_id": 999})).await;
        assert!(result.is_error);
        assert!(result.content.contains("No messages"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_export_chat_success() {
        let (db, dir) = test_db();
        let pid = db.get_or_create_default_persona(100).unwrap();
        db.store_message(&StoredMessage {
            id: "m1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "hello".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
        db.store_message(&StoredMessage {
            id: "m2".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bot".into(),
            content: "hi there!".into(),
            is_from_bot: true,
            timestamp: "2024-01-01T00:00:02Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        let tool = ExportChatTool::new(db, dir.to_str().unwrap());
        let result = tool
            .execute(json!({"chat_id": 100, "path": "test_export.md"}))
            .await;
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(result.content.contains("2 messages"));

        let content = std::fs::read_to_string(dir.join("exports").join("test_export.md")).unwrap();
        assert!(content.contains("alice"));
        assert!(content.contains("hello"));
        assert!(content.contains("**Bot**"));
        assert!(content.contains("hi there!"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_export_chat_permission_denied() {
        let (db, dir) = test_db();
        let pid = db.get_or_create_default_persona(200).unwrap();
        db.store_message(&StoredMessage {
            id: "m1".into(),
            chat_id: 200,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "hello".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        let tool = ExportChatTool::new(db, dir.to_str().unwrap());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_export_chat_allowed_for_control_chat_cross_chat() {
        let (db, dir) = test_db();
        let pid = db.get_or_create_default_persona(200).unwrap();
        db.store_message(&StoredMessage {
            id: "m1".into(),
            chat_id: 200,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "hello".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
        let tool = ExportChatTool::new(db, dir.to_str().unwrap());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "path": "control_export.md",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let content =
            std::fs::read_to_string(dir.join("exports").join("control_export.md")).unwrap();
        assert!(content.contains("hello"));
        cleanup(&dir);
    }
}
