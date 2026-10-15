use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};
use crate::claude::ToolDefinition;
use crate::db::Database;

pub struct UpdateBulletinFocusTool {
    db: Arc<Database>,
}

impl UpdateBulletinFocusTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for UpdateBulletinFocusTool {
    fn name(&self) -> &str {
        "update_bulletin_focus"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "update_bulletin_focus",
            "Set the per-persona Bulletin focus card. Operator-facing cockpit summary: active goals, in-flight work, blockers, recent outcomes. Replaces the current Bulletin content for this persona. Do not duplicate bulletin text in chat replies — it is injected automatically in persona context.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Target chat ID (default: caller chat)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Target persona ID (default: caller persona)"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short heading for the bulletin card"
                    },
                    "content": {
                        "type": "string",
                        "description": "Multiline bulletin body text. This replaces the current bulletin focus."
                    }
                }),
                &["content"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => return ToolResult::error("Missing auth context".into()),
        };

        let chat_id = input
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_chat_id);
        let persona_id = input
            .get("persona_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_persona_id);

        if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
            return ToolResult::error(e);
        }

        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim(),
            _ => return ToolResult::error("Missing or empty 'content'".into()),
        };
        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());

        let clipped_content = if content.chars().count() > 4000 {
            content.chars().take(4000).collect::<String>()
        } else {
            content.to_string()
        };
        let clipped_title = title.map(|t| {
            if t.chars().count() > 120 {
                t.chars().take(120).collect::<String>()
            } else {
                t.to_string()
            }
        });

        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || {
            db.upsert_persona_bulletin_focus(
                chat_id,
                persona_id,
                clipped_title.as_deref(),
                &clipped_content,
            )
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::success(format!(
                "Bulletin focus updated for chat {} persona {}.",
                chat_id, persona_id
            )),
            Ok(Err(e)) => ToolResult::error(format!("Failed to update bulletin focus: {e}")),
            Err(e) => ToolResult::error(format!("Task error while updating bulletin focus: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn auth_input(chat_id: i64, persona_id: i64, content: &str) -> serde_json::Value {
        json!({
            "content": content,
            "__finally_a_value_bot_auth": {
                "caller_channel": "web",
                "caller_chat_id": chat_id,
                "caller_persona_id": persona_id,
                "control_chat_ids": [],
                "is_scheduled_task": false
            }
        })
    }

    #[tokio::test]
    async fn test_update_bulletin_focus_defaults_to_auth_context() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_test_{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        db.upsert_chat(42, None, "private").unwrap();
        let pid = db.get_or_create_default_persona(42).unwrap();
        let tool = UpdateBulletinFocusTool::new(db.clone());
        let input = auth_input(42, pid, "Current focus");
        let result = tool.execute(input).await;
        assert!(!result.is_error);
        let focus = db.get_persona_bulletin_focus(42, pid).unwrap().unwrap();
        assert_eq!(focus.content, "Current focus");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_update_bulletin_focus_blocks_cross_persona_without_control() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_test_{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        db.upsert_chat(42, None, "private").unwrap();
        let pid = db.get_or_create_default_persona(42).unwrap();
        let other_pid = db.create_persona(42, "other", None).unwrap();
        let tool = UpdateBulletinFocusTool::new(db.clone());
        let mut input = auth_input(42, pid, "Current focus");
        input["persona_id"] = json!(other_pid);
        let result = tool.execute(input).await;
        assert!(result.is_error);
        let _ = std::fs::remove_dir_all(dir);
    }
}
