use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::json;

use crate::agent::AppState;
use crate::background_shell::{self, ShellEnqueueOutcome};
use crate::claude::ToolDefinition;
use crate::config::Config;

use super::bash_safety::{check_bash_safety, parse_confirmation_prefix};
use super::{auth_context_from_input, schema_object, Tool, ToolResult};

pub struct SpawnBackgroundCommandTool {
    config: Config,
    state: Arc<OnceLock<Arc<AppState>>>,
}

impl SpawnBackgroundCommandTool {
    pub fn new(config: &Config, state: Arc<OnceLock<Arc<AppState>>>) -> Self {
        Self {
            config: config.clone(),
            state,
        }
    }
}

#[async_trait]
impl Tool for SpawnBackgroundCommandTool {
    fn name(&self) -> &str {
        "spawn_background_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "spawn_background_command",
            "Run a long shell command in a detached tmux session. Returns immediately with a job id; the bot sends a separate message when the command finishes. Use for work that may exceed interactive bash timeouts (builds, GPU jobs, long scripts). For downloadable outputs, the command must atomically write {artifacts: [{path: absolute_path, sha256: optional_hash}]} JSON to $ARTIFACT_MANIFEST after files are complete. The server publishes those exact files before the completion notice. Not available in Docker.",
            schema_object(
                json!({
                    "command": {
                        "type": "string",
                        "description": "Shell command to run (same safety rules as bash; use CONFIRM_EXECUTE prefix when required)"
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Working directory (default: tool shared/ cwd)"
                    },
                    "label": {
                        "type": "string",
                        "description": "Short label for queue/ops display"
                    }
                }),
                &["command"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let state = match self.state.get() {
            Some(s) => s.clone(),
            None => {
                return ToolResult::error("Bot runtime not ready for background commands".into())
                    .with_error_type("runtime_unavailable");
            }
        };

        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => {
                return ToolResult::error("Missing auth context (__meido_auth)".into())
                    .with_error_type("auth_required");
            }
        };

        let raw_command = match input.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error("Missing 'command' parameter".into()),
        };
        let (confirmed, command) = parse_confirmation_prefix(raw_command.trim());

        if let Some(blocked) = check_bash_safety(
            &command,
            confirmed,
            &self.config.safety_execution_mode,
            &self.config.safety_risky_categories,
        ) {
            return blocked;
        }
        if let Some(blocked) = super::bash_safety::check_self_repo_git(
            &command,
            Path::new(self.config.working_dir()),
            &auth.external_roots,
        ) {
            return blocked;
        }

        let workspace_root = PathBuf::from(self.config.working_dir());
        let origin_vault = self.config.origin_vault_root();
        let tool_working_dir =
            super::resolve_tool_working_dir_for_auth(&workspace_root, Some(&auth));
        let workdir = input
            .get("workdir")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|p| {
                super::resolve_tool_path_with_origin(
                    &workspace_root,
                    &tool_working_dir,
                    p,
                    &origin_vault,
                    auth.vector_db_root.as_deref(),
                )
            })
            .unwrap_or(tool_working_dir);
        if let Err(msg) = super::assert_persona_tool_path_allowed_with_origin(
            &workspace_root,
            &workdir,
            Some(&auth),
            true,
            &origin_vault,
        ) {
            return ToolResult::error(msg);
        }

        if let Err(e) = tokio::fs::create_dir_all(&workdir).await {
            return ToolResult::error(format!(
                "Failed to create workdir {}: {e}",
                workdir.display()
            ));
        }

        let label = input
            .get("label")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let channel = auth.caller_channel.clone();
        match background_shell::try_enqueue_background_shell(
            state,
            auth.caller_chat_id,
            auth.caller_persona_id,
            command,
            workdir,
            label,
            "tool",
            &channel,
            auth.chat_session_id(),
        )
        .await
        {
            ShellEnqueueOutcome::Started {
                job_id,
                tmux_session,
            } => ToolResult::success(format!(
                "Background command started.\njob_id: {job_id}\ntmux_session: {tmux_session}\n\
                 A separate completion message will be sent when the command finishes."
            )),
            ShellEnqueueOutcome::BlockedAlreadyRunning => ToolResult::error(
                "A background task is already running for this chat. Wait for it to finish before starting another.".into(),
            )
            .with_error_type("background_busy"),
            ShellEnqueueOutcome::ActiveLookupFailed(e) => {
                ToolResult::error(format!("Failed to check active background jobs: {e}"))
            }
            ShellEnqueueOutcome::DbCreateFailed(e) => {
                ToolResult::error(format!("Failed to create background job record: {e}"))
            }
            ShellEnqueueOutcome::TmuxUnavailable(msg) => ToolResult::error(msg)
                .with_error_type("tmux_unavailable"),
            ShellEnqueueOutcome::SpawnFailed(msg) => {
                ToolResult::error(msg).with_error_type("spawn_error")
            }
        }
    }
}
