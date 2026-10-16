use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::claude::ToolDefinition;
use crate::config::Config;
use crate::db::{call_blocking, Database};
use crate::hook_executor::validate_command_payload;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};

pub struct RegisterHookTool {
    db: Arc<Database>,
    config: Config,
}

impl RegisterHookTool {
    pub fn new(db: Arc<Database>, config: &Config) -> Self {
        Self {
            db,
            config: config.clone(),
        }
    }
}

fn normalize_persona_scope_ids(ids: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[async_trait]
impl Tool for RegisterHookTool {
    fn name(&self) -> &str {
        "register_hook"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "register_hook",
            "Create or update a bot lifecycle hook. New hooks default to the current persona only unless global=true is set. Use this instead of calling /api/hooks directly when authoring hooks from an agent run.",
            schema_object(
                json!({
                    "id": {
                        "type": "integer",
                        "description": "Optional existing hook id to update. Omit to create."
                    },
                    "name": {
                        "type": "string",
                        "description": "Unique hook name"
                    },
                    "event_name": {
                        "type": "string",
                        "description": "Lifecycle event: BeforeTurn, PreToolUse, PostToolUse, PostToolBatch, PreStop, PreDelivery, or PostDelivery"
                    },
                    "matcher": {
                        "type": "string",
                        "description": "Optional regex matcher"
                    },
                    "action_type": {
                        "type": "string",
                        "description": "Hook action type (block, add_context, command, prompt, or builtin_*)"
                    },
                    "action_payload_json": {
                        "type": "string",
                        "description": "JSON string payload for the selected action_type"
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Whether the hook is enabled (default true)"
                    },
                    "global": {
                        "type": "boolean",
                        "description": "Set true to make the hook global. Default false for new hooks."
                    },
                    "scoped_persona_ids": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "description": "Optional explicit persona scope allowlist. Overrides default current-persona scope."
                    }
                }),
                &["name", "event_name", "action_type"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(ctx) => ctx,
            None => {
                return ToolResult::error(
                    "Missing auth context (__finally_a_value_bot_auth)".into(),
                )
                .with_error_type("auth_required")
            }
        };

        let id = input.get("id").and_then(|v| v.as_i64());
        let name = match input.get("name").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return ToolResult::error("Missing required parameter: name".into()),
        };
        let event_name = match input.get("event_name").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return ToolResult::error("Missing required parameter: event_name".into()),
        };
        let action_type = match input.get("action_type").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return ToolResult::error("Missing required parameter: action_type".into()),
        };
        let matcher = input
            .get("matcher")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let payload_json = input
            .get("action_payload_json")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("{}")
            .to_string();
        let enabled = input
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let is_global = input
            .get("global")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let scoped_from_input = input
            .get("scoped_persona_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect::<Vec<i64>>());

        if action_type.eq_ignore_ascii_case("command") {
            if let Err(e) = validate_command_payload(&self.config, &payload_json) {
                return ToolResult::error(format!("Invalid command hook payload: {e}"));
            }
        }

        let db = self.db.clone();
        let scope_for_db = if is_global {
            None
        } else if let Some(ids) = scoped_from_input {
            Some(normalize_persona_scope_ids(&ids))
        } else if let Some(existing_id) = id {
            match call_blocking(db.clone(), move |db| db.get_hook_definition(existing_id)).await {
                Ok(Some(existing)) => existing.scoped_persona_ids,
                Ok(None) => {
                    return ToolResult::error(format!("Hook definition {existing_id} not found"))
                }
                Err(e) => {
                    return ToolResult::error(format!("Failed to load existing hook scope: {e}"))
                }
            }
        } else {
            Some(vec![auth.caller_persona_id])
        };

        let name_for_db = name.clone();
        let event_name_for_db = event_name.clone();
        let action_type_for_db = action_type.clone();
        let payload_json_for_db = payload_json.clone();
        let scope_for_db_write = scope_for_db.clone();
        let result = call_blocking(db, move |db| {
            db.upsert_hook_definition(
                id,
                &name_for_db,
                &event_name_for_db,
                matcher.as_deref(),
                &action_type_for_db,
                &payload_json_for_db,
                scope_for_db_write.as_deref(),
                enabled,
            )
        })
        .await;

        match result {
            Ok(hook_id) => {
                let scope_label = match scope_for_db {
                    None => "global".to_string(),
                    Some(ids) if ids.is_empty() => "persona:none".to_string(),
                    Some(ids) => format!(
                        "persona:{}",
                        ids.iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                };
                ToolResult::success(format!(
                    "Hook registered: id={hook_id}, name='{name}', scope={scope_label}, enabled={enabled}"
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to register hook: {e}")),
        }
    }
}
