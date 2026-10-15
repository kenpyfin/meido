use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::config::Config;
use crate::db::Database;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};

const MAX_PROMPT_LEN: usize = 50_000;
const MAX_OUTPUT_LEN: usize = 30_000;
const PROMPT_PREVIEW_LEN: usize = 200;
const OUTPUT_PREVIEW_LEN: usize = 500;

pub struct CursorAgentTool {
    config: Config,
    db: Arc<Database>,
}

fn in_docker() -> bool {
    std::env::var("FINALLY_A_VALUE_BOT_IN_DOCKER").as_deref() == Ok("1")
        || std::path::Path::new("/.dockerenv").exists()
}

async fn tmux_session_exists(session: &str) -> Result<bool, String> {
    let output = tokio::process::Command::new("tmux")
        .args(["has-session", "-t", session])
        .output()
        .await
        .map_err(|e| format!("Failed to run tmux has-session: {e}"))?;
    Ok(output.status.success())
}

impl CursorAgentTool {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        Self {
            config: config.clone(),
            db,
        }
    }

    /// Execute cursor-agent via host runner (HTTP POST). Used when CURSOR_AGENT_RUNNER_URL is set (e.g. in Docker).
    async fn execute_via_runner(
        &self,
        url: &str,
        prompt: &str,
        workdir_str: &str,
        model: &str,
        detach: bool,
        auth: Option<&crate::tools::ToolAuthContext>,
    ) -> ToolResult {
        #[derive(Serialize)]
        struct SpawnRequest<'a> {
            prompt: &'a str,
            workdir: &'a str,
            model: &'a str,
            detach: bool,
        }
        #[derive(Deserialize)]
        struct SpawnResponse {
            success: Option<bool>,
            session_name: Option<String>,
            output: Option<String>,
            error: Option<String>,
        }
        let body = SpawnRequest {
            prompt,
            workdir: workdir_str,
            model: if model.is_empty() { "" } else { model },
            detach,
        };
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                self.config.cursor_agent_timeout_secs + 10,
            ))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to create HTTP client: {}", e)),
        };
        let res = match client.post(url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                return ToolResult::error(format!(
                    "Runner request failed: {}. Is the host runner running and reachable?",
                    e
                ));
            }
        };
        let status = res.status();
        let text = match res.text().await {
            Ok(t) => t,
            Err(e) => return ToolResult::error(format!("Failed to read runner response: {}", e)),
        };
        if !status.is_success() {
            return ToolResult::error(format!(
                "Runner returned {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            ));
        }
        let parsed: SpawnResponse = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult::error(format!(
                    "Failed to parse runner response: {}. Response: {}",
                    e,
                    text.chars().take(300).collect::<String>()
                ));
            }
        };
        if parsed.error.as_deref() == Some("") || parsed.success == Some(false) {
            let err = parsed
                .error
                .unwrap_or_else(|| "Unknown runner error".into());
            return ToolResult::error(err);
        }
        if detach {
            if let Some(session_name) = parsed.session_name {
                if let Some(a) = auth {
                    let db = self.db.clone();
                    let chat_id = a.caller_chat_id;
                    let channel = a.caller_channel.clone();
                    let prompt_preview: String = if prompt.len() <= PROMPT_PREVIEW_LEN {
                        prompt.to_string()
                    } else {
                        format!(
                            "{}...",
                            &prompt[..prompt.floor_char_boundary(PROMPT_PREVIEW_LEN)]
                        )
                    };
                    let started_at = chrono::Utc::now().to_rfc3339();
                    let output_preview = format!(
                        "Spawned in tmux session: {}. Attach: tmux attach -t {}",
                        session_name, session_name
                    );
                    let workdir_owned = workdir_str.to_string();
                    let session_name_for_db = session_name.clone();
                    let _ = crate::db::call_blocking(db, move |database| {
                        database.insert_cursor_agent_run(
                            chat_id,
                            &channel,
                            &prompt_preview,
                            Some(workdir_owned.as_str()),
                            &started_at,
                            &started_at,
                            true,
                            None,
                            Some(&output_preview),
                            None::<&str>,
                            Some(session_name_for_db.as_str()),
                        )
                    })
                    .await;
                }
                ToolResult::success(format!(
                    "Spawned cursor-agent in tmux session `{}`. Attach with: tmux attach -t {}\n\
                     Use the cursor_agent_send tool to send keys.",
                    session_name, session_name
                ))
            } else {
                ToolResult::error("Runner did not return session_name for detach=true".into())
            }
        } else {
            let output = parsed
                .output
                .unwrap_or_else(|| "Runner returned no output".into());
            if parsed.success == Some(false) {
                ToolResult::error(output).with_error_type("process_exit")
            } else {
                ToolResult::success(output)
            }
        }
    }

    /// Spawn cursor-agent in a tmux session; return immediately with attach instructions.
    async fn execute_detached(
        &self,
        prompt: &str,
        workdir_str: &str,
        model: &str,
        auth: Option<&crate::tools::ToolAuthContext>,
    ) -> ToolResult {
        if !self.config.cursor_agent_tmux_enabled || in_docker() {
            return ToolResult::error(
                "Tmux spawn is not available in this environment (Docker or tmux disabled). \
                 Run the bot on a host with tmux and cursor-agent, or use detach: false for inline runs."
                    .into(),
            )
            .with_error_type("tmux_unavailable");
        }
        let prefix = self.config.cursor_agent_tmux_session_prefix.trim();
        let prefix = if prefix.is_empty() {
            "finally_a_value_bot-cursor"
        } else {
            prefix
        };
        let session_name = format!("{}-{}", prefix, chrono::Utc::now().timestamp_millis());
        let started_at = chrono::Utc::now().to_rfc3339();
        let prompt_preview: String = if prompt.len() <= PROMPT_PREVIEW_LEN {
            prompt.to_string()
        } else {
            format!(
                "{}...",
                &prompt[..prompt.floor_char_boundary(PROMPT_PREVIEW_LEN)]
            )
        };
        let cli_path = self.config.cursor_agent_cli_path.trim();
        let ceiling = crate::self_repo::git_ceiling_value(Path::new(self.config.working_dir()));
        let mut tmux_cmd = tokio::process::Command::new("tmux");
        tmux_cmd
            .args([
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-c",
                workdir_str,
                "--",
                "env",
                &format!("{}={}", crate::self_repo::ENV_GIT_CEILING, ceiling),
            ])
            .arg(cli_path)
            .arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("text");
        if !model.is_empty() {
            tmux_cmd.arg("--model").arg(model);
        }
        crate::self_repo::apply_git_ceiling_env(
            &mut tmux_cmd,
            Path::new(self.config.working_dir()),
        );
        let spawn_result = tmux_cmd.spawn();
        let (ok, msg) = match spawn_result {
            Ok(_) => {
                if let Some(a) = auth {
                    let db = self.db.clone();
                    let chat_id = a.caller_chat_id;
                    let channel = a.caller_channel.clone();
                    let workdir_owned = workdir_str.to_string();
                    let session_name_for_db = session_name.clone();
                    let output_preview = format!(
                        "Spawned in tmux session: {}. Attach: tmux attach -t {}",
                        session_name, session_name
                    );
                    let _ = crate::db::call_blocking(db, move |database| {
                        database.insert_cursor_agent_run(
                            chat_id,
                            &channel,
                            &prompt_preview,
                            Some(workdir_owned.as_str()),
                            &started_at,
                            &started_at,
                            true,
                            None,
                            Some(&output_preview),
                            None::<&str>,
                            Some(session_name_for_db.as_str()),
                        )
                    })
                    .await;
                }
                let m = format!(
                    "Spawned cursor-agent in tmux session `{}`. Attach with: tmux attach -t {}\n\
                     Use the cursor_agent_send tool to send keys (e.g. redirect the agent mid-task).",
                    session_name, session_name
                );
                (true, m)
            }
            Err(ref e) => {
                let m = format!(
                    "Failed to spawn tmux session for cursor-agent: {}. Ensure tmux is installed and in PATH.",
                    e
                );
                (false, m)
            }
        };
        if ok {
            ToolResult::success(msg)
        } else {
            ToolResult::error(msg).with_error_type("spawn_error")
        }
    }
}

#[async_trait]
impl Tool for CursorAgentTool {
    fn name(&self) -> &str {
        "cursor_agent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "cursor_agent",
            "Run the Cursor CLI agent (cursor-agent) with a prompt. Use for research, code generation, or analysis that benefits from Cursor's native agent. For long tasks, prefer detach: true to run in background via tmux and avoid request timeouts. Optional: timeout_secs, model override. Working directory is the shared tool workspace.",
            schema_object(
                json!({
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to send to cursor-agent"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default from config, typically 1500)"
                    },
                    "model": {
                        "type": "string",
                        "description": "Override model for this run (e.g. gpt-5). Omit to use config default or Cursor auto"
                    },
                    "detach": {
                        "type": "boolean",
                        "description": "If true, spawn cursor-agent in a tmux session and return immediately. Prefer true for long-running tasks. Attach with tmux attach -t <session>. Not available in Docker."
                    }
                }),
                &["prompt"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing 'prompt' parameter".into()),
        };

        if prompt.len() > MAX_PROMPT_LEN {
            return ToolResult::error(format!(
                "Prompt exceeds maximum length of {} characters",
                MAX_PROMPT_LEN
            ));
        }

        let auth = auth_context_from_input(&input);
        let started_at = chrono::Utc::now().to_rfc3339();
        let config = self.config.clone();
        let live = match crate::db::call_blocking(self.db.clone(), move |db| {
            crate::cursor_engine_config::load_from_db(db, &config)
        })
        .await
        {
            Ok(v) => v,
            Err(_) => crate::cursor_engine_config::CursorEngineSettings::from_env(&self.config),
        };
        let workspace_root = PathBuf::from(self.config.working_dir());
        let working_dir = super::resolve_tool_working_dir_for_auth(&workspace_root, auth.as_ref());
        if let Err(e) = tokio::fs::create_dir_all(&working_dir).await {
            return ToolResult::error(format!(
                "Failed to create working directory {}: {e}",
                working_dir.display()
            ));
        }
        let workdir_str_storage = working_dir.to_string_lossy().to_string();
        if let Err(msg) = crate::tools::path_guard::check_path(&workdir_str_storage) {
            return ToolResult::error(msg);
        }
        if let Err(msg) = crate::self_repo::check_agent_cwd_allowed(&workspace_root, &working_dir) {
            return ToolResult::error(msg).with_error_type("blocked_by_policy");
        }

        let timeout_secs = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(live.timeout_secs);
        let model_override = input
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let model = model_override
            .unwrap_or_else(|| live.cli_model.as_str())
            .trim();

        let detach = input
            .get("detach")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // When runner URL is set (e.g. Docker), POST to host instead of running locally
        if !live.cli_runner_url.trim().is_empty() {
            let url = live.cli_runner_url.trim().trim_end_matches('/').to_string() + "/spawn";
            return self
                .execute_via_runner(
                    &url,
                    prompt,
                    &workdir_str_storage,
                    model,
                    detach,
                    auth.as_ref(),
                )
                .await;
        }

        let cli_path = live.cli_path.trim();
        if cli_path.is_empty() {
            return ToolResult::error("cursor_agent_cli_path is not configured".into());
        }

        if detach {
            return self
                .execute_detached(prompt, &workdir_str_storage, model, auth.as_ref())
                .await;
        }

        info!("Running cursor-agent (timeout {}s)", timeout_secs);

        let mut cmd = tokio::process::Command::new(cli_path);
        cmd.arg("-p").arg(prompt);
        if !model.is_empty() {
            cmd.arg("--model").arg(model);
        }
        cmd.arg("--output-format").arg("text");
        cmd.current_dir(&working_dir);
        crate::self_repo::apply_git_ceiling_env(&mut cmd, &workspace_root);

        let result =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

        let finished_at = chrono::Utc::now().to_rfc3339();
        let prompt_preview: String = if prompt.len() <= PROMPT_PREVIEW_LEN {
            prompt.to_string()
        } else {
            format!(
                "{}...",
                &prompt[..prompt.floor_char_boundary(PROMPT_PREVIEW_LEN)]
            )
        };

        let (success, exit_code, result_content) = match &result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);
                (code == 0, code, {
                    let mut result_text = String::new();
                    if !stdout.is_empty() {
                        result_text.push_str(&stdout);
                    }
                    if !stderr.is_empty() {
                        if !result_text.is_empty() {
                            result_text.push('\n');
                        }
                        result_text.push_str("STDERR:\n");
                        result_text.push_str(&stderr);
                    }
                    if result_text.is_empty() {
                        result_text = format!("Command completed with exit code {code}");
                    }
                    if result_text.len() > MAX_OUTPUT_LEN {
                        result_text.truncate(MAX_OUTPUT_LEN);
                        result_text.push_str("\n... (output truncated)");
                    }
                    result_text
                })
            }
            Ok(Err(_)) => (false, 1, "Failed to execute cursor-agent".to_string()),
            Err(_) => (
                false,
                -1,
                format!("Timed out after {} seconds", timeout_secs),
            ),
        };

        if let Some(ref a) = auth {
            let output_preview = if result_content.len() <= OUTPUT_PREVIEW_LEN {
                result_content.clone()
            } else {
                format!(
                    "{}...",
                    &result_content[..result_content.floor_char_boundary(OUTPUT_PREVIEW_LEN)]
                )
            };
            let db = self.db.clone();
            let chat_id = a.caller_chat_id;
            let channel = a.caller_channel.clone();
            let _ = crate::db::call_blocking(db, move |database| {
                database.insert_cursor_agent_run(
                    chat_id,
                    &channel,
                    &prompt_preview,
                    Some(&workdir_str_storage),
                    &started_at,
                    &finished_at,
                    success,
                    Some(exit_code),
                    Some(&output_preview),
                    None::<&str>,
                    None::<&str>,
                )
            })
            .await;
        }

        match result {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                if exit_code == 0 {
                    ToolResult::success(result_content).with_status_code(exit_code)
                } else {
                    ToolResult::error(format!("Exit code {exit_code}\n{result_content}"))
                        .with_status_code(exit_code)
                        .with_error_type("process_exit")
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to execute cursor-agent: {e}"))
                .with_error_type("spawn_error"),
            Err(_) => ToolResult::error(format!(
                "cursor-agent timed out after {} seconds",
                timeout_secs
            ))
            .with_error_type("timeout"),
        }
    }
}

// --- list_cursor_agent_runs ---

pub struct ListCursorAgentRunsTool {
    db: Arc<Database>,
}

impl ListCursorAgentRunsTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ListCursorAgentRunsTool {
    fn name(&self) -> &str {
        "list_cursor_agent_runs"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_cursor_agent_runs",
            "List recent cursor-agent runs to monitor project status. By default returns runs for the current chat; use this to see last run outcome, success/failure, and output preview. For post-mortems on a finished run, use read_agent_history (iterations and tool traces) or read_file on output_path when present; if the tmux session is still alive, tmux capture-pane via bash can show scrollback.",
            schema_object(
                json!({
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of runs to return (default: 10)"
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Optional: list runs for this chat ID (control chats only). Omit to list runs for the current chat."
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = auth_context_from_input(&input);
        let chat_id = input
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| auth.as_ref().map(|a| a.caller_chat_id));
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as usize;

        match crate::db::call_blocking(self.db.clone(), move |db| {
            db.get_cursor_agent_runs(chat_id, limit)
        })
        .await
        {
            Ok(runs) => {
                if runs.is_empty() {
                    return ToolResult::success("No cursor-agent runs found.".into());
                }
                let mut out = String::new();
                let mut tmux_probe_error: Option<String> = None;
                for r in &runs {
                    let session_running = if let Some(ref sess) = r.tmux_session {
                        match tmux_session_exists(sess).await {
                            Ok(v) => Some(v),
                            Err(e) => {
                                if tmux_probe_error.is_none() {
                                    tmux_probe_error = Some(e);
                                }
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let status = if session_running == Some(true) {
                        "running"
                    } else if r.tmux_session.is_some() && r.exit_code.is_none() {
                        "stopped"
                    } else if r.success {
                        "ok"
                    } else {
                        "failed"
                    };
                    let code = r
                        .exit_code
                        .map(|c| format!(" exit_code={}", c))
                        .unwrap_or_default();
                    let preview = r.prompt_preview.chars().take(60).collect::<String>();
                    let suffix = if r.prompt_preview.chars().count() > 60 {
                        "..."
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "#{} {} {} {} | prompt: {}{}\n",
                        r.id, r.finished_at, status, code, preview, suffix
                    ));
                    if let Some(ref sess) = r.tmux_session {
                        if session_running == Some(true) {
                            out.push_str(&format!(
                                "  session: {} | Attach: tmux attach -t {}\n",
                                sess, sess
                            ));
                        } else if session_running == Some(false) {
                            out.push_str(&format!("  session: {} | not currently running\n", sess));
                        } else {
                            out.push_str(&format!("  session: {} | status unknown\n", sess));
                        }
                    }
                    if let Some(ref prev) = r.output_preview {
                        let first_line = prev.lines().next().unwrap_or("");
                        out.push_str(&format!(
                            "  -> {}\n",
                            &first_line[..first_line.len().min(80)]
                        ));
                    }
                }
                if let Some(err) = tmux_probe_error {
                    out.push_str(&format!(
                        "\n(note: could not verify tmux session state for some runs: {})\n",
                        err
                    ));
                }
                out.push_str(
                    "\nPost-mortem: use read_agent_history for full run traces; if a run has output_path, read_file that path; for a still-running tmux session, bash: tmux capture-pane -t <session> -p\n",
                );
                ToolResult::success(out)
            }
            Err(e) => ToolResult::error(format!("Failed to list cursor-agent runs: {e}")),
        }
    }
}

// --- cursor_agent_send ---

pub struct CursorAgentSendTool {
    config: Config,
}

impl CursorAgentSendTool {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }
}

#[async_trait]
impl Tool for CursorAgentSendTool {
    fn name(&self) -> &str {
        "cursor_agent_send"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "cursor_agent_send",
            "Send keys to a running cursor-agent tmux session (from a run with detach: true). Use to redirect the agent mid-task (e.g. 'Focus on the API first').",
            schema_object(
                json!({
                    "tmux_session": {
                        "type": "string",
                        "description": "The tmux session name (e.g. finally_a_value_bot-cursor-1234567890). Use list_cursor_agent_runs to see running sessions."
                    },
                    "keys": {
                        "type": "string",
                        "description": "Text to send; newline = Enter. Restrict to printable characters and newlines."
                    }
                }),
                &["tmux_session", "keys"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let session = input
            .get("tmux_session")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let keys = input.get("keys").and_then(|v| v.as_str()).unwrap_or("");
        if session.is_empty() {
            return ToolResult::error("Missing tmux_session".into());
        }
        let prefix = self.config.cursor_agent_tmux_session_prefix.trim();
        let prefix = if prefix.is_empty() {
            "finally_a_value_bot-cursor"
        } else {
            prefix
        };
        if !session.starts_with(prefix) {
            return ToolResult::error(format!(
                "Session name must start with '{}' (got '{}'). Only cursor-agent sessions are allowed.",
                prefix, session
            ));
        }
        // Sanitize keys: allow printable ASCII and newlines
        let safe_keys: String = keys
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ' || *c == '\n' || *c == '\t')
            .collect();
        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(["send-keys", "-t", session, &safe_keys, "Enter"]);
        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    ToolResult::success(format!("Sent keys to session {}", session))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    ToolResult::error(format!("tmux send-keys failed: {}", stderr))
                }
            }
            Err(e) => ToolResult::error(format!("Failed to run tmux send-keys: {}", e)),
        }
    }
}

// --- build_skill ---

pub struct BuildSkillTool {
    config: Config,
    db: Arc<Database>,
}

impl BuildSkillTool {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        Self {
            config: config.clone(),
            db,
        }
    }
}

#[async_trait]
impl Tool for BuildSkillTool {
    fn name(&self) -> &str {
        "build_skill"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "build_skill",
            "Create or update a FinallyAValueBot skill by running cursor-agent. For new skills activate `create-skill` first; for changes to an existing skill activate `modify-skill` first (enforced when the skill folder already exists). Use this (not write_file under skills/) when the user asks to add or change a skill. Runs in tmux when available so the bot does not block.",
            schema_object(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Skill name (folder name under skills dir)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the skill"
                    },
                    "instructions": {
                        "type": "string",
                        "description": "Full markdown instructions for the skill (when to use, how to invoke, steps)"
                    }
                }),
                &["name", "description", "instructions"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let instructions = input
            .get("instructions")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return ToolResult::error("Missing name".into());
        }
        let skills_dir = self.config.skills_data_dir_absolute();
        let skills_dir_display = skills_dir.to_string_lossy();
        let prompt = format!(
            r#"Create or update a FinallyAValueBot agent skill.

Skills directory: {}
Create (or update) the skill at: {}/{}/

Requirements:
1. Create the folder {}/{} if it does not exist.
2. Create or overwrite SKILL.md with YAML frontmatter (name, description, platforms, deps, source) and a body.
3. Put reusable helper scripts/files for this capability inside the same skill folder (or shared/skills/<skill-name>/ only when truly shared).
4. Do NOT place skill scripts in persona folders or flat shared paths like shared/scripts/.

Description for this skill: {}
Instructions (markdown body): {}

Put any credentials or config (e.g. .env, API keys) inside the skill folder {}/{} so they are available to all personas. Follow the existing skill format (see other skills in the same directory for examples).{}"#,
            skills_dir_display,
            skills_dir_display,
            name,
            skills_dir_display,
            name,
            description,
            instructions,
            skills_dir_display,
            name,
            crate::agent_path_discipline::build_skill_path_discipline_footer(
                &skills_dir_display,
                name,
            ),
        );
        let cursor_tool = CursorAgentTool::new(&self.config, self.db.clone());
        let mut cursor_input = serde_json::json!({ "prompt": prompt });
        cursor_input["detach"] = serde_json::json!(true);
        if let Some(auth) = input.get("__finally_a_value_bot_auth") {
            cursor_input["__finally_a_value_bot_auth"] = auth.clone();
        }
        let result = cursor_tool.execute(cursor_input).await;

        // Fallback: when cursor-agent is unavailable (Docker, runner down, etc.), create skill directly
        if result.is_error && is_cursor_agent_unavailable(&result.content) {
            info!("build_skill: cursor-agent unavailable, falling back to direct file creation");
            return create_skill_via_write(&skills_dir, name, description, instructions).await;
        }

        result
    }
}

fn is_cursor_agent_unavailable(error_content: &str) -> bool {
    let lower = error_content.to_lowercase();
    lower.contains("cursor-agent")
        || lower.contains("cursor_agent")
        || lower.contains("tmux spawn is not available")
        || lower.contains("runner request failed")
        || lower.contains("runner did not return")
        || lower.contains("not found")
        || lower.contains("not configured")
        || lower.contains("no such file")
        || lower.contains("failed to execute")
        || lower.contains("failed to run")
        || lower.contains("command not found")
}

async fn create_skill_via_write(
    skills_dir: &std::path::Path,
    name: &str,
    description: &str,
    instructions: &str,
) -> ToolResult {
    let skill_dir = skills_dir.join(name);
    if let Err(e) = tokio::fs::create_dir_all(&skill_dir).await {
        return ToolResult::error(format!("Failed to create skill directory: {}", e));
    }

    let safe_name = name.replace('"', "'");
    let yaml_desc = if description.is_empty() {
        format!("Skill: {}", name)
    } else {
        description.replace('\n', " ").trim().to_string()
    };
    let body = instructions.trim();
    let content = format!(
        r#"---
name: {}
description: {}
platforms:
  - linux
  - darwin
deps: []
---

# {}

{}

---

Put credentials in `{}/.env` if needed.
"#,
        safe_name,
        yaml_desc,
        name,
        body,
        skill_dir.display()
    );

    let skill_md = skill_dir.join("SKILL.md");
    if let Err(e) = tokio::fs::write(&skill_md, content).await {
        return ToolResult::error(format!("Failed to write SKILL.md: {}", e));
    }

    info!(
        "build_skill fallback: created {} at {}",
        name,
        skill_md.display()
    );
    ToolResult::success(format!(
        "Created skill '{}' at {}/SKILL.md (cursor-agent unavailable; used direct file creation). \
         Add credentials to {}/.env if needed.",
        name,
        skill_dir.display(),
        skill_dir.display()
    ))
}
