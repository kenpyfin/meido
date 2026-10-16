//! Register an external long-running id (e.g. ComfyUI `prompt_id`) in `background_jobs` so it
//! appears in cockpit/queue diagnostics. Does not reserve the chat's single shell/handoff slot.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::claude::ToolDefinition;
use crate::db::call_blocking;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};

pub struct RegisterTrackedJobTool {
    db: Arc<crate::db::Database>,
}

impl RegisterTrackedJobTool {
    pub fn new(db: Arc<crate::db::Database>) -> Self {
        Self { db }
    }
}

fn is_plausible_external_id(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 8
        && t.len() <= 128
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[async_trait]
impl Tool for RegisterTrackedJobTool {
    fn name(&self) -> &str {
        "register_tracked_job"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "register_tracked_job",
            "Record an external long-running job id (e.g. ComfyUI prompt_id) in the bot's background_jobs table so it appears in the queue/cockpit. Use immediately after submitting to an external API whose id should match what you tell the user. Does not start tmux or block spawn_background_command.",
            schema_object(
                json!({
                    "external_id": {
                        "type": "string",
                        "description": "Opaque id returned by the external system (often a UUID), e.g. ComfyUI prompt_id"
                    },
                    "label": {
                        "type": "string",
                        "description": "Short label for queue display (e.g. PZ ComfyUI)"
                    },
                    "note": {
                        "type": "string",
                        "description": "Optional detail stored as job prompt text"
                    },
                    "source": {
                        "type": "string",
                        "description": "Origin for trigger_reason (default: comfyui_prompt)"
                    }
                }),
                &["external_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => {
                return ToolResult::error(
                    "Missing auth context (__finally_a_value_bot_auth)".into(),
                )
                .with_error_type("auth_required");
            }
        };

        let external_id = match input.get("external_id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => return ToolResult::error("Missing or empty 'external_id'".into()),
        };

        if !is_plausible_external_id(external_id) {
            return ToolResult::error(
                "external_id must be 8–128 chars (letters, digits, hyphen, underscore)".into(),
            );
        }

        let label = input
            .get("label")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let note = input
            .get("note")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("External queue job");

        let source = input
            .get("source")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("comfyui_prompt");

        let chat_id = auth.caller_chat_id;
        let persona_id = auth.caller_persona_id;
        let ext = external_id.to_string();
        let label_opt = label.clone();
        let note_owned = note.to_string();
        let source_owned = source.to_string();

        let existing = match call_blocking(self.db.clone(), {
            let ext = ext.clone();
            move |db| db.get_background_job(&ext)
        })
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::error(format!("Database error: {e}"));
            }
        };

        if let Some(j) = existing {
            if j.chat_id == chat_id {
                return ToolResult::success(format!(
                    "Job id `{ext}` is already registered (job_kind={}). No duplicate row created.",
                    j.job_kind
                ));
            }
            return ToolResult::error(format!(
                "external_id `{ext}` is already registered for another chat"
            ));
        }

        let ext_clone = ext.clone();
        match call_blocking(self.db.clone(), move |db| {
            db.create_background_tracked_job(
                &ext_clone,
                chat_id,
                persona_id,
                &note_owned,
                label_opt.as_deref(),
                &source_owned,
            )
        })
        .await
        {
            Ok(()) => ToolResult::success(format!(
                "Registered tracked job `{ext}` in the background queue (job_kind=tracked). Use this same id when reporting status to the user."
            )),
            Err(e) => ToolResult::error(format!("Failed to register tracked job: {e}")),
        }
    }
}
