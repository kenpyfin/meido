use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;
use crate::recall::{RecallRequest, RecallScope, RecallService};

pub struct RecallTool {
    recall: Arc<RecallService>,
}

impl RecallTool {
    pub fn new(recall: Arc<RecallService>) -> Self {
        Self { recall }
    }
}

#[async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "recall",
            "Recall older dialogue or ORIGIN knowledge that is not already in the recent conversation. \
             Use scope auto unless you specifically need only past messages (interactions) or only vault notes (knowledge). \
             Do not call this on an ordinary turn, and do not use it to find a file path; use locate_file for paths.",
            schema_object(
                json!({
                    "query": {
                        "type": "string",
                        "description": "What to recall. A short phrase is enough."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["auto", "interactions", "knowledge"],
                        "description": "auto (normal), interactions (past dialogue), or knowledge (ORIGIN notes)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum hits to return (default 8, max 20)"
                    }
                }),
                &["query"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(query) if !query.trim().is_empty() => query.trim().to_string(),
            _ => return ToolResult::error("Missing or empty 'query' parameter".into()),
        };
        let Some(auth) = auth_context_from_input(&input) else {
            return ToolResult::error(
                "Missing auth context (__meido_auth) with caller_persona_id for this chat".into(),
            )
            .with_error_type("auth_required");
        };
        if auth.caller_persona_id <= 0 {
            return ToolResult::error(
                "Missing auth context (__meido_auth) with caller_persona_id for this chat".into(),
            )
            .with_error_type("auth_required");
        }
        let scope = RecallScope::parse(
            input
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("auto"),
        );
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .min(20) as usize;
        let response = self
            .recall
            .query(RecallRequest {
                query,
                scope,
                chat_id: auth.caller_chat_id,
                persona_id: auth.caller_persona_id,
                limit,
                from_date: None,
                to_date: None,
            })
            .await;
        ToolResult::success(response.to_json().to_string())
    }
}
