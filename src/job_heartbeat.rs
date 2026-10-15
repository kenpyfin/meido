use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::warn;

use crate::channel::{deliver_to_contact, DeliveryScope};
use crate::db::call_blocking;
use crate::telegram::{AgentEvent, AppState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobType {
    ManualBackground,
    Scheduled,
    ShellBackground,
    RunOptimize,
}

impl JobType {
    fn as_str(self) -> &'static str {
        match self {
            JobType::ManualBackground => "manual_background",
            JobType::Scheduled => "scheduled",
            JobType::ShellBackground => "shell_background",
            JobType::RunOptimize => "run_optimize",
        }
    }

    fn notify_user_periodically(self) -> bool {
        matches!(
            self,
            JobType::ManualBackground | JobType::ShellBackground | JobType::RunOptimize
        )
    }
}

#[derive(Debug, Clone)]
pub enum HeartbeatSignal {
    Started(String),
    Progress(String),
    ToolStart(String),
    ToolResult { tool: String, is_error: bool },
    Finished(String),
    Failed(String),
}

pub fn signal_from_agent_event(evt: &AgentEvent) -> Option<HeartbeatSignal> {
    match evt {
        AgentEvent::Iteration { iteration } => Some(HeartbeatSignal::Progress(format!(
            "iteration {}",
            iteration
        ))),
        AgentEvent::ToolStart { name, .. } => Some(HeartbeatSignal::ToolStart(name.clone())),
        AgentEvent::ToolResult { name, is_error, .. } => Some(HeartbeatSignal::ToolResult {
            tool: name.clone(),
            is_error: *is_error,
        }),
        AgentEvent::Hook {
            event_name,
            tool_name,
            matched_hook_ids,
            blocked_reason,
            ..
        } => Some(HeartbeatSignal::Progress(format!(
            "hook {} tool={} matched={} blocked={}",
            event_name,
            tool_name.as_deref().unwrap_or("-"),
            matched_hook_ids.len(),
            blocked_reason.as_deref().unwrap_or("-"),
        ))),
        AgentEvent::TextDelta { .. } => None,
        AgentEvent::FinalResponse { .. } => None,
    }
}

pub fn spawn_shared_heartbeat(
    state: Arc<AppState>,
    run_key: String,
    chat_id: i64,
    persona_id: i64,
    job_type: JobType,
    _lease_owner: Option<String>,
    notify_chat_progress: bool,
) -> UnboundedSender<HeartbeatSignal> {
    let (tx, mut rx) = mpsc::unbounded_channel::<HeartbeatSignal>();
    tokio::spawn(async move {
        let heartbeat_period =
            Duration::from_secs(state.config.background_job_lease_fallback_renew_secs.max(1));
        let lease_ttl_secs = state.config.background_job_lease_ttl_secs as i64;
        let mut ticker = tokio::time::interval(heartbeat_period);
        let mut stage = "queued".to_string();
        let mut message = "queued".to_string();
        let mut active = true;
        let mut last_user_notify = Instant::now()
            .checked_sub(heartbeat_period)
            .unwrap_or_else(Instant::now);

        let _ = call_blocking(state.db.clone(), {
            let run_key = run_key.clone();
            let stage = stage.clone();
            let message = message.clone();
            let job_type = job_type.as_str().to_string();
            move |db| {
                db.upsert_job_heartbeat(
                    &run_key, chat_id, persona_id, &job_type, &stage, &message, true,
                )
            }
        })
        .await;
        let _ = call_blocking(state.db.clone(), {
            let run_key = run_key.clone();
            let payload = format!(
                r#"{{"stage":"{}","message":"{}"}}"#,
                stage,
                message.replace('"', "'")
            );
            move |db| {
                db.append_run_timeline_event(
                    &run_key,
                    chat_id,
                    persona_id,
                    "heartbeat",
                    Some(&payload),
                )
            }
        })
        .await;

        loop {
            tokio::select! {
                maybe_sig = rx.recv() => {
                    let Some(sig) = maybe_sig else {
                        if active {
                            warn!(
                                run_key = %run_key,
                                chat_id,
                                "heartbeat channel closed without Finished/Failed; marking inactive"
                            );
                            stage = "aborted".to_string();
                            message = "heartbeat channel closed (worker ended unexpectedly)".to_string();
                            let _ = call_blocking(state.db.clone(), {
                                let run_key = run_key.clone();
                                let stage = stage.clone();
                                let message = message.clone();
                                let job_type = job_type.as_str().to_string();
                                move |db| {
                                    db.upsert_job_heartbeat(
                                        &run_key,
                                        chat_id,
                                        persona_id,
                                        &job_type,
                                        &stage,
                                        &message,
                                        false,
                                    )
                                }
                            })
                            .await;
                            let _ = call_blocking(state.db.clone(), {
                                let run_key = run_key.clone();
                                let stage = stage.clone();
                                let message = message.clone();
                                let payload = format!(
                                    r#"{{"stage":"{}","message":"{}"}}"#,
                                    stage,
                                    message.replace('"', "'")
                                );
                                move |db| {
                                    db.append_run_timeline_event(
                                        &run_key,
                                        chat_id,
                                        persona_id,
                                        "heartbeat",
                                        Some(&payload),
                                    )
                                }
                            })
                            .await;
                        }
                        break;
                    };
                    match sig {
                        HeartbeatSignal::Started(m) => {
                            stage = "running".to_string();
                            message = m;
                        }
                        HeartbeatSignal::Progress(m) => {
                            stage = "running".to_string();
                            message = m;
                        }
                        HeartbeatSignal::ToolStart(tool) => {
                            stage = "running".to_string();
                            message = format!("running tool: {}", tool);
                        }
                        HeartbeatSignal::ToolResult { tool, is_error } => {
                            stage = if is_error { "running_with_errors".to_string() } else { "running".to_string() };
                            message = format!(
                                "tool {} {}",
                                tool,
                                if is_error { "reported an error" } else { "completed" }
                            );
                        }
                        HeartbeatSignal::Finished(m) => {
                            stage = "completed".to_string();
                            message = m;
                            active = false;
                        }
                        HeartbeatSignal::Failed(m) => {
                            stage = "failed".to_string();
                            message = m;
                            active = false;
                        }
                    }

                    let _ = call_blocking(state.db.clone(), {
                        let run_key = run_key.clone();
                        let stage = stage.clone();
                        let message = message.clone();
                        let job_type = job_type.as_str().to_string();
                        move |db| db.upsert_job_heartbeat(&run_key, chat_id, persona_id, &job_type, &stage, &message, active)
                    })
                    .await;
                    if active && job_type == JobType::ManualBackground {
                        let _ = call_blocking(state.db.clone(), {
                            let run_key = run_key.clone();
                            let stage = stage.clone();
                            move |db| {
                                db.renew_background_job_lease(&run_key, lease_ttl_secs, &stage)
                            }
                        })
                        .await;
                    }
                    let _ = call_blocking(state.db.clone(), {
                        let run_key = run_key.clone();
                        let stage = stage.clone();
                        let message = message.clone();
                        let payload = format!(r#"{{"stage":"{}","message":"{}"}}"#, stage, message.replace('"', "'"));
                        move |db| db.append_run_timeline_event(&run_key, chat_id, persona_id, "heartbeat", Some(&payload))
                    })
                    .await;

                    if job_type.notify_user_periodically()
                        && notify_chat_progress
                        && stage != "completed"
                        && stage != "failed"
                    {
                        let _ = deliver_to_contact(
                            state.db.clone(),
                            state.telegram_bots.as_ref(),
                            state.discord_http.as_ref(),
                            state.wecom.as_deref(),
                            &state.config.bot_username,
                            chat_id,
                            persona_id,
                            &format!("Background update: {}", message),
                            Some(state.config.workspace_root_absolute()),
                            DeliveryScope::StoreOnly,
                            None,
                        )
                        .await;
                        last_user_notify = Instant::now();
                    }

                    if !active {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    let _ = call_blocking(state.db.clone(), {
                        let run_key = run_key.clone();
                        let stage = stage.clone();
                        let message = message.clone();
                        let job_type = job_type.as_str().to_string();
                        move |db| db.upsert_job_heartbeat(&run_key, chat_id, persona_id, &job_type, &stage, &message, active)
                    }).await;
                    if active && job_type == JobType::ManualBackground {
                        let _ = call_blocking(state.db.clone(), {
                            let run_key = run_key.clone();
                            let stage = stage.clone();
                            move |db| {
                                db.renew_background_job_lease(&run_key, lease_ttl_secs, &stage)
                            }
                        })
                        .await;
                    }

                    if active
                        && job_type.notify_user_periodically()
                        && notify_chat_progress
                        && last_user_notify.elapsed() >= heartbeat_period
                    {
                        let _ = deliver_to_contact(
                            state.db.clone(),
                            state.telegram_bots.as_ref(),
                            state.discord_http.as_ref(),
                            state.wecom.as_deref(),
                            &state.config.bot_username,
                            chat_id,
                            persona_id,
                            &format!("Background update: {}", message),
                            Some(state.config.workspace_root_absolute()),
                            DeliveryScope::StoreOnly,
                            None,
                        )
                        .await;
                        last_user_notify = Instant::now();
                    }
                }
            }
        }
    });
    tx
}
