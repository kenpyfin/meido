//! Tmux-backed shell background jobs: spawn, monitor, finalize, and deliver results.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

use crate::agent::AppState;
use crate::background_jobs::{try_enqueue_background_handoff, HandoffEnqueueOutcome};
use crate::channel::{deliver_agent_final_to_contact, deliver_to_contact, DeliveryScope};
use crate::config::Config;
use crate::db::{call_blocking, BackgroundJob};
use crate::job_heartbeat::{spawn_shared_heartbeat, HeartbeatSignal, JobType};

const MAX_DELIVERY_OUTPUT_LEN: usize = 30_000;
const EXIT_CODE_FILE: &str = "exit_code";
const STDOUT_LOG: &str = "stdout.log";
const COMMAND_SCRIPT: &str = "command.sh";
const WRAPPER_SCRIPT: &str = "wrapper.sh";

pub fn in_docker() -> bool {
    crate::meido_env::is_flag_one(crate::meido_env::IN_DOCKER) || Path::new("/.dockerenv").exists()
}

pub fn tmux_available(config: &Config) -> bool {
    config.background_shell_tmux_enabled && !in_docker()
}

pub async fn tmux_session_exists(session: &str) -> Result<bool, String> {
    let output = tokio::process::Command::new("tmux")
        .args(["has-session", "-t", session])
        .output()
        .await
        .map_err(|e| format!("Failed to run tmux has-session: {e}"))?;
    Ok(output.status.success())
}

pub fn shell_job_dir(config: &Config, job_id: &str) -> PathBuf {
    config
        .workspace_root_absolute()
        .join("runtime")
        .join("background_jobs")
        .join(job_id)
}

/// Join `workspace_root` with a relative working-directory path for shell jobs.
///
/// Repeatedly drops a leading path segment when it duplicates `workspace_root`'s final segment
/// (same idea as `workspace_data_path_display` in `telegram.rs`). Avoids `./workspace` plus
/// `workspace/shared` resolving to `workspace/workspace/shared`.
fn join_workspace_relative_dir(workspace_root: &Path, relative: &Path) -> PathBuf {
    use std::path::Component;

    let trimmed = relative.to_string_lossy();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return workspace_root.to_path_buf();
    }
    let rel = trimmed.trim_start_matches("./");
    if rel.is_empty() {
        return workspace_root.to_path_buf();
    }

    let mut tail = PathBuf::from(rel);
    while let (Some(root_leaf), Some(Component::Normal(first_seg))) =
        (workspace_root.file_name(), tail.components().next())
    {
        if root_leaf != first_seg {
            break;
        }
        tail = tail.components().skip(1).collect();
    }

    workspace_root.join(tail)
}

/// Resolve workdir to an absolute path under the workspace root.
pub fn resolve_shell_workdir(config: &Config, workdir: &Path) -> PathBuf {
    let p = if workdir.is_absolute() {
        workdir.to_path_buf()
    } else {
        join_workspace_relative_dir(&config.workspace_root_absolute(), workdir)
    };
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn abs_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn session_name(prefix: &str, job_id: &str) -> String {
    let short: String = job_id.chars().take(8).collect();
    let prefix = prefix.trim();
    let prefix = if prefix.is_empty() {
        "meido-bg"
    } else {
        prefix
    };
    format!("{prefix}-{short}")
}

fn truncate_label(command: &str, label: Option<&str>) -> String {
    if let Some(l) = label.filter(|s| !s.trim().is_empty()) {
        let t = l.trim();
        if t.len() <= 120 {
            return t.to_string();
        }
        return format!("{}...", &t[..t.floor_char_boundary(120)]);
    }
    let c = command.trim();
    if c.len() <= 120 {
        return format!("shell: {c}");
    }
    format!("shell: {}...", &c[..c.floor_char_boundary(117)])
}

#[derive(Debug)]
pub enum ShellEnqueueOutcome {
    Started {
        job_id: String,
        tmux_session: String,
    },
    BlockedAlreadyRunning,
    ActiveLookupFailed(String),
    DbCreateFailed(String),
    TmuxUnavailable(String),
    SpawnFailed(String),
}

/// Enqueue a shell command as a tmux background job.
pub async fn try_enqueue_background_shell(
    state: Arc<AppState>,
    chat_id: i64,
    persona_id: i64,
    command: String,
    workdir: PathBuf,
    label: Option<String>,
    trigger_reason: &str,
    channel: &str,
    chat_session_id: Option<String>,
) -> ShellEnqueueOutcome {
    if !tmux_available(&state.config) {
        return ShellEnqueueOutcome::TmuxUnavailable(
            "Tmux shell background jobs are not available in this environment (Docker or tmux disabled). \
Run the bot on a host with tmux, or use inline bash for short commands."
                .into(),
        );
    }

    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state.config.background_job_pending_start_timeout_secs as i64;
    match call_blocking(state.db.clone(), move |db| {
        db.count_active_background_jobs_for_chat(chat_id, &now, pending_timeout_secs)
    })
    .await
    {
        Ok(count) => {
            if count > 0 {
                return ShellEnqueueOutcome::BlockedAlreadyRunning;
            }
        }
        Err(e) => return ShellEnqueueOutcome::ActiveLookupFailed(e.to_string()),
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let display_label = truncate_label(&command, label.as_deref());
    let job_dir = shell_job_dir(&state.config, &job_id);
    if let Err(e) = tokio::fs::create_dir_all(&job_dir).await {
        return ShellEnqueueOutcome::SpawnFailed(format!(
            "Failed to create job directory {}: {e}",
            job_dir.display()
        ));
    }

    let workdir_abs = resolve_shell_workdir(&state.config, &workdir);
    let job_dir_abs = abs_path(&job_dir);
    let stdout_path = job_dir_abs.join(STDOUT_LOG);
    let exit_path = job_dir_abs.join(EXIT_CODE_FILE);
    let command_script = job_dir_abs.join(COMMAND_SCRIPT);
    let wrapper_script = job_dir_abs.join(WRAPPER_SCRIPT);
    let tmux_cwd = state.config.workspace_root_absolute();

    let debug_export = if state.runtime_toggles.tool_output_debug() {
        "export TOOL_OUTPUT_DEBUG=1\n"
    } else {
        ""
    };
    let ceiling = crate::self_repo::git_ceiling_value(&state.config.workspace_root_absolute());
    let git_ceiling_export = if ceiling.is_empty() {
        String::new()
    } else {
        format!(
            "export {}={}\n",
            crate::self_repo::ENV_GIT_CEILING,
            shell_escape_single(&ceiling)
        )
    };
    let vault_source =
        crate::secret_vault::vault_source_lines(&state.config.runtime_data_dir(), chat_id);
    let artifact_export = format!(
        "export ARTIFACT_MANIFEST={}\n",
        shell_escape_single(&job_dir_abs.join("artifacts.json").to_string_lossy())
    );
    let command_body = format!(
        "#!/usr/bin/env bash\nset -uo pipefail\n{debug_export}{git_ceiling_export}{vault_source}{artifact_export}cd {}\n{}\n",
        shell_escape_single(&workdir_abs.to_string_lossy()),
        command.trim()
    );
    if let Err(e) = tokio::fs::write(&command_script, command_body).await {
        return ShellEnqueueOutcome::SpawnFailed(format!("Failed to write command script: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(&command_script, std::fs::Permissions::from_mode(0o700))
                .await
        {
            return ShellEnqueueOutcome::SpawnFailed(format!(
                "Failed to chmod command script: {e}"
            ));
        }
    }

    let wrapper_body = format!(
        "#!/usr/bin/env bash\nset -o pipefail\nbash \"{}\" >\"{}\" 2>&1\necho $? >\"{}\"\n",
        command_script.display(),
        stdout_path.display(),
        exit_path.display(),
    );
    if let Err(e) = tokio::fs::write(&wrapper_script, wrapper_body).await {
        return ShellEnqueueOutcome::SpawnFailed(format!("Failed to write wrapper script: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(&wrapper_script, std::fs::Permissions::from_mode(0o700))
                .await
        {
            return ShellEnqueueOutcome::SpawnFailed(format!(
                "Failed to chmod wrapper script: {e}"
            ));
        }
    }

    let prefix = state.config.background_shell_tmux_session_prefix.trim();
    let tmux_session = session_name(prefix, &job_id);
    let workdir_str = workdir_abs.to_string_lossy().to_string();
    let workdir_for_tmux = tmux_cwd.to_string_lossy().to_string();
    let output_path_str = stdout_path.to_string_lossy().to_string();
    let reason = trigger_reason.to_string();
    let jid = job_id.clone();
    let label_for_db = display_label.clone();
    let cmd_for_db = command.clone();
    let session_for_db = tmux_session.clone();
    let chat_session_for_db = chat_session_id.clone();

    match call_blocking(state.db.clone(), move |db| {
        db.create_background_shell_job(
            &jid,
            chat_id,
            persona_id,
            &label_for_db,
            &cmd_for_db,
            &workdir_str,
            &session_for_db,
            &output_path_str,
            &reason,
            chat_session_for_db.as_deref(),
        )
    })
    .await
    {
        Ok(()) => {}
        Err(e) => return ShellEnqueueOutcome::DbCreateFailed(e.to_string()),
    }

    let wrapper_arg = wrapper_script.to_string_lossy().into_owned();
    let spawn_result = tokio::process::Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &tmux_session,
            "-c",
            &workdir_for_tmux,
            "--",
            "bash",
            &wrapper_arg,
        ])
        .spawn();

    match spawn_result {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("Failed to spawn tmux session: {e}");
            let jid = job_id.clone();
            let msg_db = msg.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.fail_background_job(&jid, &msg_db)
            })
            .await;
            notify_shell_job_enqueue_failure(
                &state,
                &job_id,
                chat_id,
                persona_id,
                chat_session_id.clone(),
                &display_label,
                &msg,
            )
            .await;
            return ShellEnqueueOutcome::SpawnFailed(msg);
        }
    }

    let lease_owner = uuid::Uuid::new_v4().to_string();
    let lease_ttl_secs = state.config.background_job_lease_ttl_secs as i64;
    let jid = job_id.clone();
    let lease_owner_claim = lease_owner.clone();
    match call_blocking(state.db.clone(), move |db| {
        db.claim_background_job_running(&jid, &lease_owner_claim, lease_ttl_secs)
    })
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            let msg = "background shell job claim rejected".to_string();
            let _ = kill_tmux_session(&tmux_session).await;
            let jid = job_id.clone();
            let msg_db = msg.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.fail_background_job(&jid, &msg_db)
            })
            .await;
            notify_shell_job_enqueue_failure(
                &state,
                &job_id,
                chat_id,
                persona_id,
                chat_session_id.clone(),
                &display_label,
                &msg,
            )
            .await;
            return ShellEnqueueOutcome::SpawnFailed(msg);
        }
        Err(e) => {
            let msg = format!("failed to claim background shell job: {e}");
            let _ = kill_tmux_session(&tmux_session).await;
            let jid = job_id.clone();
            let msg_db = msg.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.fail_background_job(&jid, &msg_db)
            })
            .await;
            notify_shell_job_enqueue_failure(
                &state,
                &job_id,
                chat_id,
                persona_id,
                chat_session_id.clone(),
                &display_label,
                &msg,
            )
            .await;
            return ShellEnqueueOutcome::SpawnFailed(msg);
        }
    }

    let _ = state
        .background_job_control
        .register(job_id.clone(), chat_id)
        .await;

    let hb_tx = spawn_shared_heartbeat(
        state.clone(),
        job_id.clone(),
        chat_id,
        persona_id,
        JobType::ShellBackground,
        Some(lease_owner),
        state.config.background_job_notify_chat_progress,
    );
    let _ = hb_tx.send(HeartbeatSignal::Started(
        "shell background job started".to_string(),
    ));

    info!(
        job_id = %job_id,
        chat_id,
        channel,
        session = %tmux_session,
        "Background shell job started in tmux"
    );

    let ack = format!(
        "Background command started (job `{}`). You'll receive another message when it finishes.",
        job_id
    );
    if let Err(e) = deliver_to_contact(
        state.db.clone(),
        state.telegram_bots.as_ref(),
        state.discord_http.as_ref(),
        state.wecom.as_deref(),
        &state.config.bot_username,
        chat_id,
        persona_id,
        &ack,
        Some(state.config.workspace_root_absolute()),
        DeliveryScope::StoreOnly,
        chat_session_id,
    )
    .await
    {
        warn!(job_id = %job_id, "Failed to deliver shell job startup ack: {e}");
    }

    spawn_tmux_completion_watcher(state.clone(), job_id.clone(), tmux_session.clone());

    ShellEnqueueOutcome::Started {
        job_id,
        tmux_session,
    }
}

/// Blocks until the tmux session ends, then finalizes (primary completion path; poll is backup).
fn spawn_tmux_completion_watcher(state: Arc<AppState>, job_id: String, session: String) {
    tokio::spawn(async move {
        let wait_out = tokio::process::Command::new("tmux")
            .args(["wait-session", "-t", &session])
            .output()
            .await;

        match &wait_out {
            Ok(out) => {
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                if !out.status.success() && combined.contains("unknown command") {
                    warn!(
                        job_id = %job_id,
                        session = %session,
                        "tmux wait-session is not supported on this host; using poll monitor only"
                    );
                    return;
                }
                info!(
                    job_id = %job_id,
                    session = %session,
                    exit = ?out.status,
                    "tmux wait-session finished"
                );
            }
            Err(e) => warn!(job_id = %job_id, session = %session, "tmux wait-session error: {e}"),
        }

        let jid = job_id.clone();
        let job = match call_blocking(state.db.clone(), move |db| db.get_background_job(&jid)).await
        {
            Ok(Some(j)) => j,
            _ => return,
        };
        if job.status == "running" || shell_job_needs_user_notification(&job) {
            finalize_shell_job(state, job, None).await;
        }
    });
}

fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn kill_tmux_session(session: &str) -> Result<(), String> {
    let output = tokio::process::Command::new("tmux")
        .args(["kill-session", "-t", session])
        .output()
        .await
        .map_err(|e| format!("tmux kill-session failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("tmux kill-session: {stderr}"))
    }
}

pub async fn cancel_background_shell_job(
    state: &Arc<AppState>,
    job: &BackgroundJob,
    reason: &str,
) -> Result<(), String> {
    if let Some(session) = job.tmux_session.as_deref() {
        let _ = kill_tmux_session(session).await;
    }
    state
        .background_job_control
        .request_cancel(&job.id, job.chat_id)
        .await;
    let jid = job.id.clone();
    let reason_owned = reason.to_string();
    let reason_for_db = reason_owned.clone();
    call_blocking(state.db.clone(), move |db| {
        db.mark_background_job_cancelled(&jid, &reason_for_db)
    })
    .await
    .map_err(|e| e.to_string())?;

    let label = job.label.as_deref().unwrap_or(job.prompt.as_str());
    let notice = format!(
        "Background command cancelled (job `{}`).\nTask: {label}\nReason: {reason_owned}",
        job.id
    );
    if let Err(e) = deliver_shell_notification(
        state,
        job.chat_id,
        job.persona_id,
        job.session_id.clone(),
        &notice,
    )
    .await
    {
        warn!(job_id = %job.id, "Failed to deliver shell job cancel notice: {e}");
    } else {
        let jid = job.id.clone();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.record_background_shell_user_notification(&jid, &notice)
        })
        .await;
    }

    state.background_job_control.finish(&job.id).await;
    Ok(())
}

async fn read_exit_code(job_dir: &Path) -> Option<i32> {
    let path = job_dir.join(EXIT_CODE_FILE);
    let text = tokio::fs::read_to_string(&path).await.ok()?;
    text.trim().parse().ok()
}

async fn read_log_output(job: &BackgroundJob, job_dir: &Path) -> String {
    let path = job
        .output_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| job_dir.join(STDOUT_LOG));
    let path = abs_path(&path);
    let from_log = match tokio::fs::read_to_string(&path).await {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => String::new(),
        Err(e) => format!(
            "(could not read log at {}: {e})\n\
             Hint: if exit code is -1, the wrapper may not have run; check tmux and job scripts under {})",
            path.display(),
            job_dir.display()
        ),
    };
    if !from_log.is_empty() {
        return from_log;
    }
    if let Some(err) = job.error_text.as_deref().filter(|s| !s.trim().is_empty()) {
        return format!("{err}\n\n(no command output captured)");
    }
    "Command produced no output.".into()
}

/// Terminal shell job row that never had completion/failure text delivered to the user.
pub fn shell_job_needs_user_notification(job: &BackgroundJob) -> bool {
    if job.job_kind != "shell" {
        return false;
    }
    match job.status.as_str() {
        "failed" => job
            .result_text
            .as_ref()
            .map(|t| t.trim().is_empty())
            .unwrap_or(true),
        "cancelled" => job.last_stage.as_deref() != Some("user_notified"),
        _ => false,
    }
}

fn format_delivery_message(
    job: &BackgroundJob,
    exit_code: i32,
    output: &str,
    agent_followup_scheduled: bool,
    show_shell_output_to_user: bool,
) -> String {
    let label = job.label.as_deref().unwrap_or(job.prompt.as_str());
    let (headline, hint) = if exit_code == 0 {
        if agent_followup_scheduled {
            (
                format!("completed successfully (exit {exit_code})"),
                "Your background command finished. I'm preparing a summary reply with the key outputs.",
            )
        } else {
            (
                format!("completed successfully (exit {exit_code})"),
                "Your background command finished.",
            )
        }
    } else if agent_followup_scheduled {
        (
            format!("FAILED (exit {exit_code})"),
            "The background command failed. I'm starting an agent run now to read this output, fix the issue, and retry the command.",
        )
    } else if show_shell_output_to_user {
        (
            format!("FAILED (exit {exit_code})"),
            "The background command failed. Review the output below; reply if you want me to retry or debug.",
        )
    } else {
        (
            format!("FAILED (exit {exit_code})"),
            "The background command failed. Reply if you want me to investigate or retry.",
        )
    };
    if show_shell_output_to_user {
        format!(
            "{hint}\n\nBackground job `{job_id}` — {headline}\nTask: {label}\n\n{output}",
            hint = hint,
            job_id = job.id,
            headline = headline,
            label = label,
            output = output
        )
    } else {
        format!(
            "{hint}\n\nBackground job `{job_id}` — {headline}\nTask: {label}",
            hint = hint,
            job_id = job.id,
            headline = headline,
            label = label,
        )
    }
}

async fn notify_shell_job_enqueue_failure(
    state: &Arc<AppState>,
    job_id: &str,
    chat_id: i64,
    persona_id: i64,
    session_id: Option<String>,
    label: &str,
    reason: &str,
) {
    let text = format!(
        "Background command could not be started (job `{job_id}`).\nTask: {label}\n\n{reason}"
    );
    if let Err(e) = deliver_shell_notification(state, chat_id, persona_id, session_id, &text).await
    {
        warn!(job_id, "Failed to deliver shell job enqueue failure: {e}");
    } else {
        let jid = job_id.to_string();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.record_background_shell_user_notification(&jid, &text)
        })
        .await;
    }
}

async fn deliver_shell_notification(
    state: &Arc<AppState>,
    chat_id: i64,
    persona_id: i64,
    session_id: Option<String>,
    text: &str,
) -> Result<(), String> {
    deliver_to_contact(
        state.db.clone(),
        state.telegram_bots.as_ref(),
        state.discord_http.as_ref(),
        state.wecom.as_deref(),
        &state.config.bot_username,
        chat_id,
        persona_id,
        text,
        Some(state.config.workspace_root_absolute()),
        DeliveryScope::StoreOnly,
        session_id,
    )
    .await
}

/// Notify users for shell jobs marked failed by reconcile without going through finalize.
pub async fn notify_shell_jobs_by_ids(state: Arc<AppState>, job_ids: &[String]) {
    for job_id in job_ids {
        let jid = job_id.clone();
        let job = match call_blocking(state.db.clone(), move |db| db.get_background_job(&jid)).await
        {
            Ok(Some(j)) => j,
            _ => continue,
        };
        if shell_job_needs_user_notification(&job) {
            finalize_shell_job(state.clone(), job, None).await;
        }
    }
}

pub async fn finalize_shell_job(
    state: Arc<AppState>,
    job: BackgroundJob,
    hb_tx: Option<UnboundedSender<HeartbeatSignal>>,
) {
    let job_id = job.id.clone();
    let chat_id = job.chat_id;
    let persona_id = job.persona_id;

    let already_terminal = matches!(job.status.as_str(), "failed" | "cancelled" | "done");

    if let Ok(Some(current)) = call_blocking(state.db.clone(), {
        let job_id = job_id.clone();
        move |db| db.get_background_job(&job_id)
    })
    .await
    {
        if current.status == "done" {
            return;
        }
        if matches!(current.status.as_str(), "failed" | "cancelled")
            && !shell_job_needs_user_notification(&current)
        {
            return;
        }
    }

    let cancel_flag = state
        .background_job_control
        .cancel_flag(&job_id, chat_id)
        .await;
    if let Some(flag) = cancel_flag {
        if flag.load(Ordering::SeqCst) {
            let _ = call_blocking(state.db.clone(), {
                let job_id = job_id.clone();
                move |db| db.mark_background_job_cancelled(&job_id, "Cancelled by user")
            })
            .await;
            if let Some(tx) = hb_tx.as_ref() {
                let _ = tx.send(HeartbeatSignal::Failed(
                    "shell background job cancelled".to_string(),
                ));
            }
            state.background_job_control.finish(&job_id).await;
            return;
        }
    }

    let job_dir = shell_job_dir(&state.config, &job_id);
    let exit_code = read_exit_code(&job_dir).await.unwrap_or(-1);
    let mut output = read_log_output(&job, &job_dir).await;
    if output.len() > MAX_DELIVERY_OUTPUT_LEN {
        output.truncate(MAX_DELIVERY_OUTPUT_LEN);
        output.push_str("\n... (output truncated)");
    }
    let output = state.env_redactor.redact(&output);
    let success = exit_code == 0;

    // Persist terminal state before agent handoff enqueue: `try_enqueue_background_handoff`
    // counts active jobs and would block while this shell row is still `running`.
    let mut marked_early = false;
    if !already_terminal {
        let jid = job_id.clone();
        let out = output.clone();
        let ec = exit_code;
        let _ = call_blocking(state.db.clone(), move |db| {
            db.mark_background_shell_finished(&jid, ec, &out, success)
        })
        .await;
        marked_early = true;
    }

    let mut artifact_links = String::new();
    let manifest = job_dir.join("artifacts.json");
    if success && manifest.is_file() {
        let root = state.config.workspace_root_absolute();
        let jid = job.id.clone();
        let session = job.session_id.clone();
        match tokio::task::spawn_blocking(move || {
            crate::artifacts::ingest_manifest(
                &root,
                chat_id,
                persona_id,
                &jid,
                session.as_deref(),
                &manifest,
            )
        })
        .await
        {
            Ok(Ok(items)) => {
                for a in items {
                    artifact_links.push_str(&format!(
                        "\n\n[{}]({})",
                        a.name.replace(['[', ']'], "_"),
                        a.url
                    ));
                }
            }
            failure => {
                warn!(?failure, "background_artifact_registration_failed");
                artifact_links.push_str("\n\nOutput registration failed. The command finished, but its deliverables are not yet available; retry publication from the recorded manifest.");
            }
        }
    }
    let agent_followup_scheduled = if success {
        maybe_enqueue_shell_success_agent_followup(state.clone(), &job, exit_code, &output).await
    } else {
        maybe_enqueue_shell_failure_agent_retry(state.clone(), &job, exit_code, &output).await
    };
    let show_shell_output_to_user = state.runtime_toggles.tool_output_debug();
    let mut delivery_text = format_delivery_message(
        &job,
        exit_code,
        &output,
        agent_followup_scheduled,
        show_shell_output_to_user,
    );

    delivery_text.push_str(&artifact_links);
    if let Err(e) = deliver_agent_final_to_contact(
        state.db.clone(),
        state.telegram_bots.as_ref(),
        state.discord_http.as_ref(),
        state.wecom.as_deref(),
        &state.config.bot_username,
        chat_id,
        persona_id,
        &delivery_text,
        Some(state.config.workspace_root_absolute()),
        DeliveryScope::StoreOnly,
        job.session_id.clone(),
    )
    .await
    {
        error!(job_id = %job_id, "Failed to deliver shell job result: {e}");
        let jid = job_id.clone();
        let err = format!("Delivery failed: {e}");
        let err_db = err.clone();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.fail_background_job(&jid, &err_db)
        })
        .await;
        if let Some(tx) = hb_tx.as_ref() {
            let _ = tx.send(HeartbeatSignal::Failed(err));
        }
    } else {
        let jid = job_id.clone();
        let out = output.clone();
        let terminal = already_terminal;
        let _ = call_blocking(state.db.clone(), move |db| {
            if terminal {
                db.record_background_shell_user_notification(&jid, &out)
            } else if marked_early {
                // Row already written before agent handoff enqueue; nothing to update.
                Ok(())
            } else {
                db.mark_background_shell_finished(&jid, exit_code, &out, success)
            }
        })
        .await;
        if let Some(tx) = hb_tx.as_ref() {
            let sig = if success {
                HeartbeatSignal::Finished("shell background job completed".to_string())
            } else {
                HeartbeatSignal::Failed(format!("shell exited with code {exit_code}"))
            };
            let _ = tx.send(sig);
        }
    }

    state.background_job_control.finish(&job_id).await;
    info!(job_id = %job_id, chat_id, exit_code, "Background shell job finalized");
}

/// Renew leases for long-running shell jobs; do not mark failed while tmux is still alive.
pub async fn reconcile_shell_background_job_leases(state: Arc<AppState>) {
    let now = chrono::Utc::now().to_rfc3339();
    let jobs = match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.list_shell_jobs_with_expired_lease(&now)
    })
    .await
    {
        Ok(j) => j,
        Err(e) => {
            warn!("reconcile_shell_background_job_leases: {e}");
            return;
        }
    };
    let lease_ttl = state.config.background_job_lease_ttl_secs as i64;
    for job in jobs {
        let session = job.tmux_session.as_deref().unwrap_or("");
        let alive = tmux_session_exists(session).await.unwrap_or(false);
        if alive {
            let id = job.id.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.renew_background_job_lease(&id, lease_ttl, "running")
            })
            .await;
            continue;
        }
        let jid = job.id.clone();
        let msg = "tmux session ended (lease expired while reconciling)";
        let _ = call_blocking(state.db.clone(), {
            let jid = jid.clone();
            move |db| db.fail_background_job(&jid, msg)
        })
        .await;
        state.background_job_control.finish(&jid).await;
        finalize_shell_job(state.clone(), job, None).await;
    }
}

/// Poll shell jobs (backup if `tmux wait-session` watcher missed); finalize when done.
pub async fn monitor_shell_background_jobs_tick(state: Arc<AppState>) {
    reconcile_shell_background_job_leases(state.clone()).await;

    let jobs = match call_blocking(state.db.clone(), |db| db.list_shell_jobs_for_monitor()).await {
        Ok(j) => j,
        Err(e) => {
            warn!("monitor_shell_background_jobs: list failed: {e}");
            return;
        }
    };

    for job in jobs {
        let Some(session) = job.tmux_session.clone() else {
            continue;
        };
        let job_dir = shell_job_dir(&state.config, &job.id);
        let exit_ready = tokio::fs::try_exists(job_dir.join(EXIT_CODE_FILE))
            .await
            .unwrap_or(false);
        let session_alive = match tmux_session_exists(&session).await {
            Ok(v) => v,
            Err(e) => {
                warn!(job_id = %job.id, "{e}");
                continue;
            }
        };

        if job.status == "running" && session_alive && !exit_ready {
            let lease_ttl = state.config.background_job_lease_ttl_secs as i64;
            let id = job.id.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.renew_background_job_lease(&id, lease_ttl, "running")
            })
            .await;
            continue;
        }

        if exit_ready || !session_alive {
            finalize_shell_job(state.clone(), job, None).await;
        }
    }
}

/// Reconcile shell jobs whose tmux session disappeared while still marked running.
pub async fn reconcile_stale_shell_background_jobs(state: Arc<AppState>) -> Vec<String> {
    let jobs = match call_blocking(state.db.clone(), |db| {
        db.list_running_shell_background_jobs()
    })
    .await
    {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    let mut reconciled = Vec::new();
    for job in jobs {
        let session = job.tmux_session.as_deref().unwrap_or("");
        let alive = tmux_session_exists(session).await.unwrap_or(false);
        if alive {
            continue;
        }
        let jid = job.id.clone();
        let msg = "tmux session ended before monitor finalized";
        let _ = call_blocking(state.db.clone(), {
            let jid = jid.clone();
            move |db| db.fail_background_job(&jid, msg)
        })
        .await;
        state.background_job_control.finish(&jid).await;
        finalize_shell_job(state.clone(), job, None).await;
        reconciled.push(jid);
    }
    reconciled
}

async fn caller_channel_for_chat(state: &AppState, chat_id: i64) -> &'static str {
    match call_blocking(state.db.clone(), move |db| db.get_chat_type(chat_id)).await {
        Ok(Some(t)) if t == "web" => "web",
        Ok(Some(t)) if t == "discord" => "discord",
        Ok(Some(t)) if t == "whatsapp" => "whatsapp",
        Ok(Some(t)) if t == "wecom" => "wecom",
        _ => "telegram",
    }
}

/// Enqueue an agent background job to summarize a successful shell job for the user.
/// Returns true when a follow-up agent run was queued.
async fn maybe_enqueue_shell_success_agent_followup(
    state: Arc<AppState>,
    job: &BackgroundJob,
    exit_code: i32,
    output: &str,
) -> bool {
    if !state.config.background_shell_auto_agent_on_success {
        return false;
    }
    if job.last_stage.as_deref() == Some("agent_success_followup_enqueued") {
        return false;
    }
    let parent_id = job.id.clone();
    let prior = match call_blocking(state.db.clone(), move |db| {
        db.count_shell_success_agent_followups(&parent_id)
    })
    .await
    {
        Ok(n) => n,
        Err(e) => {
            warn!(job_id = %job.id, "shell success follow-up count failed: {e}");
            return false;
        }
    };
    if prior >= 1 {
        return false;
    }
    let attempt = prior + 1;
    let label = job.label.as_deref().unwrap_or(job.prompt.as_str());
    let shell_cmd = job.shell_command.as_deref().unwrap_or("(unknown)");
    let workdir = job.workdir.as_deref().unwrap_or("(unknown)");
    let prompt = format!(
        "##INTERNAL_SHELL_SUCCESS_FOLLOWUP##\n\
         A background shell command completed successfully. Read the output (paths, warnings, inventory), \
         verify key artifacts exist when paths are given, and send the user a concise summary with next steps. \
         Use `send_message` with attachments when images were produced. Do not re-run the same command unless an artifact is missing.\n\n\
         Completed shell job id: `{shell_job_id}`\n\
         Task label: {label}\n\
         Exit code: {exit_code}\n\
         Workdir: {workdir}\n\
         Command:\n```bash\n{shell_cmd}\n```\n\n\
         Output:\n```\n{output}\n```\n",
        shell_job_id = job.id,
    );
    let trigger = format!("shell_success_followup:{}:{attempt}", job.id);
    let jid = job.id.clone();
    let _ = call_blocking(state.db.clone(), move |db| {
        db.mark_background_shell_agent_success_followup_enqueued(&jid)
    })
    .await;
    let channel = caller_channel_for_chat(&state, job.chat_id).await;
    match try_enqueue_background_handoff(
        state,
        job.chat_id,
        job.persona_id,
        prompt,
        &trigger,
        channel,
        job.session_id.clone(),
    )
    .await
    {
        HandoffEnqueueOutcome::Queued { job_id, .. } => {
            info!(
                shell_job_id = %job.id,
                agent_job_id = %job_id,
                attempt,
                "Enqueued agent follow-up after shell success"
            );
            true
        }
        HandoffEnqueueOutcome::BlockedAlreadyRunning => {
            warn!(
                shell_job_id = %job.id,
                "Shell success agent follow-up blocked: another background job is active"
            );
            false
        }
        other => {
            warn!(
                shell_job_id = %job.id,
                ?other,
                "Shell success agent follow-up could not be enqueued"
            );
            false
        }
    }
}

/// Enqueue an agent background job to diagnose a failed shell job and retry via tools.
/// Returns true when a retry agent run was queued.
async fn maybe_enqueue_shell_failure_agent_retry(
    state: Arc<AppState>,
    job: &BackgroundJob,
    exit_code: i32,
    output: &str,
) -> bool {
    if !state.config.background_shell_auto_retry_on_failure {
        return false;
    }
    if job.last_stage.as_deref() == Some("agent_retry_enqueued") {
        return false;
    }
    let max = state.config.background_shell_auto_retry_max.max(1);
    let parent_id = job.id.clone();
    let prior = match call_blocking(state.db.clone(), move |db| {
        db.count_shell_failure_agent_retries(&parent_id)
    })
    .await
    {
        Ok(n) => n,
        Err(e) => {
            warn!(job_id = %job.id, "shell failure retry count failed: {e}");
            return false;
        }
    };
    if prior >= i64::from(max) {
        return false;
    }
    let attempt = prior + 1;
    let label = job.label.as_deref().unwrap_or(job.prompt.as_str());
    let shell_cmd = job.shell_command.as_deref().unwrap_or("(unknown)");
    let workdir = job.workdir.as_deref().unwrap_or("(unknown)");
    let prompt = format!(
        "##INTERNAL_SHELL_FAILURE_RETRY##\n\
         A background shell command failed. Diagnose the output, fix the command (flags, paths, placeholders, missing files), \
         then retry with `spawn_background_command`. Do not use placeholder paths or invalid CLI flags.\n\n\
         Failed shell job id: `{shell_job_id}`\n\
         Task label: {label}\n\
         Exit code: {exit_code}\n\
         Workdir: {workdir}\n\
         Command:\n```bash\n{shell_cmd}\n```\n\n\
         Output:\n```\n{output}\n```\n",
        shell_job_id = job.id,
    );
    let trigger = format!("shell_failure_retry:{}:{attempt}", job.id);
    let jid = job.id.clone();
    let _ = call_blocking(state.db.clone(), move |db| {
        db.mark_background_shell_agent_retry_enqueued(&jid)
    })
    .await;
    let channel = caller_channel_for_chat(&state, job.chat_id).await;
    match try_enqueue_background_handoff(
        state,
        job.chat_id,
        job.persona_id,
        prompt,
        &trigger,
        channel,
        job.session_id.clone(),
    )
    .await
    {
        HandoffEnqueueOutcome::Queued { job_id, .. } => {
            info!(
                shell_job_id = %job.id,
                agent_job_id = %job_id,
                attempt,
                "Enqueued agent retry after shell failure"
            );
            true
        }
        HandoffEnqueueOutcome::BlockedAlreadyRunning => {
            warn!(
                shell_job_id = %job.id,
                "Shell failure agent retry blocked: another background job is active"
            );
            false
        }
        other => {
            warn!(
                shell_job_id = %job.id,
                ?other,
                "Shell failure agent retry could not be enqueued"
            );
            false
        }
    }
}

pub async fn notify_stale_shell_failures_on_startup(state: Arc<AppState>) {
    let jobs = match call_blocking(state.db.clone(), |db| {
        db.list_shell_jobs_needing_notification()
    })
    .await
    {
        Ok(j) => j,
        Err(e) => {
            warn!("startup shell notification sweep failed: {e}");
            return;
        }
    };
    if jobs.is_empty() {
        return;
    }
    info!(
        count = jobs.len(),
        "Notifying users about shell jobs that failed without delivery"
    );
    for job in jobs {
        finalize_shell_job(state.clone(), job, None).await;
    }
}

pub fn spawn_background_shell_monitor(state: Arc<AppState>) {
    if !tmux_available(&state.config) {
        info!("Background shell monitor disabled (tmux unavailable)");
        return;
    }
    let poll_secs = state.config.background_shell_monitor_poll_secs.max(1);
    tokio::spawn(async move {
        notify_stale_shell_failures_on_startup(state.clone()).await;
        info!(poll_secs, "Background shell monitor started");
        let mut interval = tokio::time::interval(Duration::from_secs(poll_secs));
        loop {
            interval.tick().await;
            monitor_shell_background_jobs_tick(state.clone()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn shell_job_dir_under_workspace_runtime() {
        let mut config = Config::load_from_env();
        config.workspace_dir = "/tmp/favb-workspace".into();
        let dir = shell_job_dir(&config, "abc-123");
        assert!(dir.is_absolute());
        assert!(dir.ends_with(Path::new("runtime").join("background_jobs").join("abc-123")));
    }

    #[test]
    fn resolve_shell_workdir_makes_relative_paths_absolute() {
        let mut config = Config::load_from_env();
        let root = std::env::temp_dir().join("favb-shell-workdir-test");
        let _ = std::fs::create_dir_all(root.join("shared"));
        config.workspace_dir = root.to_string_lossy().into();
        let resolved = resolve_shell_workdir(&config, Path::new("shared"));
        assert!(resolved.is_absolute());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_shell_workdir_drops_redundant_workspace_prefix() {
        let mut config = Config::load_from_env();
        let root = std::env::temp_dir()
            .join("favb-shell-workspace-prefix-test")
            .join("workspace");
        let shared = root.join("shared");
        let _ = std::fs::create_dir_all(&shared);
        config.workspace_dir = root.to_string_lossy().into();

        for rel in [
            "./workspace/shared",
            "workspace/shared",
            "workspace/workspace/shared",
        ] {
            let resolved = resolve_shell_workdir(&config, Path::new(rel));
            assert!(
                resolved.ends_with(Path::new("shared")),
                "unexpected path for {rel:?}: {}",
                resolved.display()
            );
            assert!(
                !resolved
                    .components()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|w| w[0].as_os_str() == "workspace" && w[1].as_os_str() == "workspace"),
                "doubled workspace segment in {}",
                resolved.display()
            );
        }
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    fn sample_job() -> BackgroundJob {
        BackgroundJob {
            id: "job-1".into(),
            chat_id: 1,
            persona_id: 1,
            prompt: "prompt".into(),
            status: "running".into(),
            trigger_reason: "manual".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            result_text: None,
            error_text: None,
            lease_owner: None,
            lease_expires_at: None,
            last_progress_at: None,
            last_stage: None,
            job_kind: "shell".into(),
            shell_command: None,
            workdir: None,
            tmux_session: None,
            output_path: None,
            exit_code: None,
            label: Some("my-task".into()),
            session_id: None,
        }
    }

    #[test]
    fn format_delivery_message_omits_output_when_debug_off() {
        let job = sample_job();
        let msg = format_delivery_message(&job, 0, "line1\nline2", true, false);
        assert!(msg.contains("preparing a summary"));
        assert!(msg.contains("my-task"));
        assert!(!msg.contains("line1"));
    }

    #[test]
    fn format_delivery_message_includes_output_when_debug_on() {
        let job = sample_job();
        let msg = format_delivery_message(&job, 0, "line1\nline2", true, true);
        assert!(msg.contains("line1"));
    }
}
