use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use super::{
    authorize_chat_access, authorize_chat_persona_access, default_persona_id_for_chat,
    schema_object, Tool, ToolResult,
};
use crate::claude::ToolDefinition;
use crate::db::Database;

pub struct SearchHistoryTool {
    db: Arc<Database>,
    recall: Option<Arc<crate::recall::RecallService>>,
}

impl SearchHistoryTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db, recall: None }
    }

    pub fn with_recall(mut self, recall: Arc<crate::recall::RecallService>) -> Self {
        self.recall = Some(recall);
        self
    }
}

#[async_trait]
impl Tool for SearchHistoryTool {
    fn name(&self) -> &str {
        "search_chat_history"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "search_chat_history",
            "Search past messages in this chat using full-text search. Use this to recall past conversations, facts, or context the user mentioned previously. Always use this before saying \"I don't remember\" or asking the user to repeat something.",
            schema_object(
                json!({
                    "query": {
                        "type": "string",
                        "description": "Keyword or phrase to search for in past messages"
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "The chat ID to search in (use the current chat_id from the system prompt)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 20, max: 100)"
                    },
                    "from_date": {
                        "type": "string",
                        "description": "Optional start date filter in YYYY-MM-DD format"
                    },
                    "to_date": {
                        "type": "string",
                        "description": "Optional end date filter in YYYY-MM-DD format"
                    }
                }),
                &["query", "chat_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.to_string(),
            _ => return ToolResult::error("Missing or empty 'query' parameter".into()),
        };

        let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing 'chat_id' parameter".into()),
        };

        if let Err(e) = authorize_chat_access(&input, chat_id) {
            return ToolResult::error(e);
        }

        let persona_id = match default_persona_id_for_chat(&input, chat_id) {
            Some(pid) => pid,
            None => {
                return ToolResult::error(
                    "Missing auth context (__meido_auth) with caller_persona_id for this chat"
                        .into(),
                )
                .with_error_type("auth_required");
            }
        };
        if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
            return ToolResult::error(e).with_error_type("auth_required");
        }

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(100) as usize;

        let from_date = input
            .get("from_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let to_date = input
            .get("to_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(recall) = &self.recall {
            if recall.mode() != crate::recall::RecallMode::Disabled {
                let response = recall
                    .query(crate::recall::RecallRequest {
                        query,
                        scope: crate::recall::RecallScope::Interactions,
                        chat_id,
                        persona_id,
                        limit,
                        from_date,
                        to_date,
                    })
                    .await;
                return ToolResult::success(response.to_json().to_string());
            }
        }

        let fts = crate::recall::sanitize_fts_query(&query);
        if fts.is_empty() {
            return ToolResult::success(format!("No messages found matching '{query}'"));
        }
        let db = self.db.clone();
        let from_ref = from_date.clone();
        let to_ref = to_date.clone();

        let result = tokio::task::spawn_blocking(move || {
            db.search_messages(
                chat_id,
                persona_id,
                &fts,
                limit,
                from_ref.as_deref(),
                to_ref.as_deref(),
            )
        })
        .await;

        match result {
            Ok(Ok(messages)) => {
                if messages.is_empty() {
                    return ToolResult::success(format!("No messages found matching '{query}'"));
                }
                let results: Vec<serde_json::Value> = messages
                    .iter()
                    .map(|m| {
                        let excerpt: String = m.content.chars().take(200).collect();
                        let excerpt = if m.content.chars().count() > 200 {
                            format!("{excerpt}...")
                        } else {
                            excerpt
                        };
                        json!({
                            "timestamp": m.timestamp,
                            "sender": m.sender_name,
                            "is_bot": m.is_from_bot,
                            "excerpt": excerpt
                        })
                    })
                    .collect();
                ToolResult::success(serde_json::to_string_pretty(&results).unwrap_or_default())
            }
            Ok(Err(e)) => ToolResult::error(format!(
                "Search failed: {e}. Try simpler keywords or check the query syntax."
            )),
            Err(e) => ToolResult::error(format!("Search task error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, StoredMessage};
    use serde_json::json;

    fn test_db() -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("meido_search_hist_{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_search_chat_history_persona_scoped() {
        let (db, dir) = test_db();
        let pid_a = db.get_or_create_default_persona(100).unwrap();
        let pid_b = db.create_persona(100, "B", None).unwrap();
        db.store_message(&StoredMessage {
            id: "a1".into(),
            chat_id: 100,
            persona_id: pid_a,
            session_id: None,
            sender_name: "user".into(),
            content: "alpha secret keyword".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:00Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
        db.store_message(&StoredMessage {
            id: "b1".into(),
            chat_id: 100,
            persona_id: pid_b,
            session_id: None,
            sender_name: "user".into(),
            content: "beta secret keyword".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        let tool = SearchHistoryTool::new(db);
        let result = tool
            .execute(json!({
                "query": "secret",
                "chat_id": 100,
                "__meido_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("alpha"));
        assert!(!result.content.contains("beta"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_search_chat_history_requires_auth_persona() {
        let (db, dir) = test_db();
        let tool = SearchHistoryTool::new(db);
        let result = tool
            .execute(json!({
                "query": "anything",
                "chat_id": 100
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("auth"));
        cleanup(&dir);
    }
}
