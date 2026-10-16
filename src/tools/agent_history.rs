//! Tool for reading agent run history so the agent can review its own past
//! behavior and optimize its workflow.

use async_trait::async_trait;
use serde_json::json;

use crate::claude::ToolDefinition;

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};

pub struct ReadAgentHistoryTool {
    data_dir: String,
}

impl ReadAgentHistoryTool {
    pub fn new(data_dir: &str) -> Self {
        ReadAgentHistoryTool {
            data_dir: data_dir.to_string(),
        }
    }
}

#[async_trait]
impl Tool for ReadAgentHistoryTool {
    fn name(&self) -> &str {
        "read_agent_history"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_agent_history",
            "Read this persona's recent agent run history (iterations, tool calls, durations, outcomes). Newer run files may also include a JSON snapshot of the first-turn system prompt and messages after the trace. Use when asked to optimize your workflow, review past behavior, or improve efficiency.",
            schema_object(
                json!({
                    "limit": {
                        "type": "integer",
                        "description": "Max number of recent runs to return (default 10, max 50)"
                    },
                    "since": {
                        "type": "string",
                        "description": "Only return runs on or after this date (YYYY-MM-DD). Optional."
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (defaults from auth context)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Persona ID (defaults from auth context)"
                    }
                }),
                &[],
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

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(50) as usize)
            .unwrap_or(10);

        let since_filter = input
            .get("since")
            .and_then(|v| v.as_str())
            .map(|s| s.replace('-', ""));

        let dir = crate::agent_history::history_dir_path(&self.data_dir, chat_id, persona_id);

        if !dir.exists() {
            return ToolResult::success("No agent history found for this persona.".into());
        }

        let mut files = match crate::agent_history::list_agent_history_md_basenames_sorted(&dir) {
            Ok(f) => f,
            Err(e) => return ToolResult::error(format!("Failed to read agent history dir: {e}")),
        };

        if let Some(ref since) = since_filter {
            files.retain(|name| {
                let stem = name.strip_suffix(".md").unwrap_or(name);
                let date_part: String = stem.chars().take(8).collect();
                date_part >= *since
            });
        }

        // Most recent first
        files.reverse();
        files.truncate(limit);

        if files.is_empty() {
            return ToolResult::success("No matching agent history runs found.".into());
        }

        let mut output = String::with_capacity(65536);
        output.push_str(&format!(
            "Agent run history ({} most recent run(s)):\n\n---\n\n",
            files.len()
        ));

        for basename in &files {
            let full_path = dir.join(basename);
            match std::fs::read_to_string(&full_path) {
                Ok(content) => {
                    output.push_str(&content);
                    output.push_str("\n---\n\n");
                }
                Err(e) => {
                    output.push_str(&format!("[Error reading {}]: {}\n\n---\n\n", basename, e));
                }
            }
        }

        const MAX_OUTPUT: usize = 250_000;
        if output.len() > MAX_OUTPUT {
            let boundary = output.floor_char_boundary(MAX_OUTPUT);
            output.truncate(boundary);
            output.push_str("\n\n[...truncated...]");
        }

        ToolResult::success(output)
    }
}
