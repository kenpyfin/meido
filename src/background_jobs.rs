use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{error, info, warn};

use crate::agent::{
    process_with_agent_with_events, AgentRequestContext, AppState, BACKGROUND_JOB_HANDOFF_PREFIX,
};
use crate::channel::{deliver_agent_final_to_contact, DeliveryScope};
use crate::claude::{Message, MessageContent, ResponseContentBlock};
use crate::db::call_blocking;
use crate::job_heartbeat::{
    signal_from_agent_event, spawn_shared_heartbeat, HeartbeatSignal, JobType,
};
use crate::llm::LlmProvider;

type JobCancel = Arc<AtomicBool>;
type JobRegistryValue = (i64, JobCancel);
type JobRegistry = HashMap<String, JobRegistryValue>;

#[derive(Debug, Clone)]
pub enum BackgroundStartAck {
    Running,
    Failed(String),
}

#[derive(Clone, Default)]
pub struct BackgroundJobControl {
    jobs: Arc<Mutex<JobRegistry>>,
}

impl BackgroundJobControl {
    pub async fn register(&self, job_id: String, chat_id: i64) -> JobCancel {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut guard = self.jobs.lock().await;
        guard.insert(job_id, (chat_id, cancel.clone()));
        cancel
    }

    pub async fn request_cancel(&self, job_id: &str, chat_id: i64) -> bool {
        let cancel = {
            let guard = self.jobs.lock().await;
            guard.get(job_id).and_then(|(cid, c)| {
                if *cid == chat_id {
                    Some(c.clone())
                } else {
                    None
                }
            })
        };
        if let Some(c) = cancel {
            c.store(true, Ordering::SeqCst);
            info!(chat_id, job_id, "background job cancel requested");
            return true;
        }
        warn!(
            chat_id,
            job_id, "background job cancel requested for unknown job"
        );
        false
    }

    pub async fn finish(&self, job_id: &str) {
        let mut guard = self.jobs.lock().await;
        guard.remove(job_id);
    }

    pub async fn is_registered(&self, job_id: &str) -> bool {
        let guard = self.jobs.lock().await;
        guard.contains_key(job_id)
    }

    pub async fn cancel_flag(&self, job_id: &str, chat_id: i64) -> Option<JobCancel> {
        let guard = self.jobs.lock().await;
        guard.get(job_id).and_then(|(cid, c)| {
            if *cid == chat_id {
                Some(c.clone())
            } else {
                None
            }
        })
    }
}

/// True if the main agent returned a web background handoff sentinel.
pub fn is_background_handoff_response(s: &str) -> bool {
    s.starts_with(BACKGROUND_JOB_HANDOFF_PREFIX)
}

/// Maps agent handoff payload to a `background_jobs.trigger_reason` value.
/// Legacy payloads (`PREFIX` + preview only) map to `timeout`.
pub fn handoff_trigger_for_db(agent_response: &str) -> Option<&'static str> {
    let rest = agent_response.strip_prefix(BACKGROUND_JOB_HANDOFF_PREFIX)?;
    if rest.is_empty() {
        return Some("timeout");
    }
    if let Some(body) = rest.strip_prefix('\n') {
        let (tag, _) = body.split_once('\n').unwrap_or((body, ""));
        return Some(match tag.trim() {
            "pte_handoff" => "pte_handoff",
            _ => "timeout",
        });
    }
    Some("timeout")
}

#[derive(Debug)]
pub enum HandoffEnqueueOutcome {
    Queued {
        job_id: String,
        start_ack: oneshot::Receiver<BackgroundStartAck>,
    },
    BlockedAlreadyRunning,
    ActiveLookupFailed(String),
    DbCreateFailed(String),
}

/// Count active jobs, insert `background_jobs` row, spawn worker. Used by web and scheduler.
pub async fn try_enqueue_background_handoff(
    state: Arc<AppState>,
    chat_id: i64,
    persona_id: i64,
    full_prompt: String,
    trigger_reason_db: &str,
    caller_channel: &str,
    session_id: Option<String>,
) -> HandoffEnqueueOutcome {
    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state.config.background_job_pending_start_timeout_secs as i64;
    match call_blocking(state.db.clone(), move |db| {
        db.count_active_background_jobs_for_chat(chat_id, &now, pending_timeout_secs)
    })
    .await
    {
        Ok(count) => {
            if count > 0 {
                return HandoffEnqueueOutcome::BlockedAlreadyRunning;
            }
        }
        Err(e) => {
            return HandoffEnqueueOutcome::ActiveLookupFailed(e.to_string());
        }
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let jid = job_id.clone();
    let prompt_for_db = full_prompt.clone();
    let reason = trigger_reason_db.to_string();
    let session_for_db = session_id.clone();
    match call_blocking(state.db.clone(), move |db| {
        db.create_background_job(
            &jid,
            chat_id,
            persona_id,
            &prompt_for_db,
            &reason,
            session_for_db.as_deref(),
        )
    })
    .await
    {
        Ok(()) => {
            let start_ack = spawn_background_job(
                state,
                job_id.clone(),
                chat_id,
                persona_id,
                full_prompt,
                caller_channel,
                session_id,
            );
            HandoffEnqueueOutcome::Queued { job_id, start_ack }
        }
        Err(e) => HandoffEnqueueOutcome::DbCreateFailed(e.to_string()),
    }
}

/// User-facing text after waiting (up to 8s) for background worker startup ack.
pub fn user_message_after_handoff_startup_ack(
    ack: Result<Result<BackgroundStartAck, oneshot::error::RecvError>, tokio::time::error::Elapsed>,
) -> String {
    match ack {
        Ok(Ok(BackgroundStartAck::Running)) => "This task is now running as a background subagent. You can continue chatting; a separate reply will arrive when it finishes.".into(),
        Ok(Ok(BackgroundStartAck::Failed(reason))) => {
            format!("Background task failed to start: {reason}")
        }
        Ok(Err(_)) => "Background task was queued, but startup confirmation channel closed early. Please check background jobs panel.".into(),
        Err(_) => "Background task was queued. Startup confirmation is delayed; check the background jobs panel for live status.".into(),
    }
}

/// Await startup ack with the same wall-clock bound as the web stream path.
pub async fn await_handoff_startup_ack(start_ack: oneshot::Receiver<BackgroundStartAck>) -> String {
    let ack_result = timeout(Duration::from_secs(8), start_ack).await;
    user_message_after_handoff_startup_ack(ack_result)
}

/// Spawn a background job and deliver the final result asynchronously.
pub fn spawn_background_job(
    state: Arc<AppState>,
    job_id: String,
    chat_id: i64,
    persona_id: i64,
    prompt: String,
    caller_channel: &str,
    session_id: Option<String>,
) -> oneshot::Receiver<BackgroundStartAck> {
    let caller_channel = caller_channel.to_string();
    let (start_tx, start_rx) = oneshot::channel::<BackgroundStartAck>();
    tokio::spawn(async move {
        let cancel = state
            .background_job_control
            .register(job_id.clone(), chat_id)
            .await;
        let mut start_tx = Some(start_tx);
        let lease_owner = uuid::Uuid::new_v4().to_string();
        let lease_ttl_secs = state.config.background_job_lease_ttl_secs as i64;
        info!(
            job_id = %job_id,
            chat_id = chat_id,
            "Background job starting"
        );

        // Atomically claim and mark running.
        let jid = job_id.clone();
        let lease_owner_for_claim = lease_owner.clone();
        let claim_res = call_blocking(state.db.clone(), move |db| {
            db.claim_background_job_running(&jid, &lease_owner_for_claim, lease_ttl_secs)
        })
        .await;
        match claim_res {
            Ok(true) => {
                if let Some(tx) = start_tx.take() {
                    let _ = tx.send(BackgroundStartAck::Running);
                }
            }
            Ok(false) => {
                let msg = "background job claim rejected; job is no longer pending".to_string();
                warn!(job_id = %job_id, "{msg}");
                if let Some(tx) = start_tx.take() {
                    let _ = tx.send(BackgroundStartAck::Failed(msg.clone()));
                }
                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.fail_background_job(&jid, &msg)
                })
                .await;
                state.background_job_control.finish(&job_id).await;
                return;
            }
            Err(e) => {
                let msg = format!("failed to claim background job: {e}");
                error!(job_id = %job_id, "{msg}");
                if let Some(tx) = start_tx.take() {
                    let _ = tx.send(BackgroundStartAck::Failed(msg.clone()));
                }
                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.fail_background_job(&jid, &msg)
                })
                .await;
                state.background_job_control.finish(&job_id).await;
                return;
            }
        }

        if cancel.load(Ordering::SeqCst) {
            let jid = job_id.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.mark_background_job_cancelled(&jid, "Cancelled by user")
            })
            .await;
            info!(job_id = %job_id, chat_id = chat_id, "Background job cancelled before start");
            state.background_job_control.finish(&job_id).await;
            return;
        }

        let hb_tx = spawn_shared_heartbeat(
            state.clone(),
            job_id.clone(),
            chat_id,
            persona_id,
            JobType::ManualBackground,
            Some(lease_owner),
            state.config.background_job_notify_chat_progress,
        );
        let _ = hb_tx.send(HeartbeatSignal::Started(
            "background job started".to_string(),
        ));
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let hb_forward = {
            let hb_tx = hb_tx.clone();
            tokio::spawn(async move {
                while let Some(evt) = evt_rx.recv().await {
                    if let Some(sig) = signal_from_agent_event(&evt) {
                        let _ = hb_tx.send(sig);
                    }
                }
            })
        };

        // Run the agent with is_background_job=true (disables further handoff)
        let bg_result = process_with_agent_with_events(
            &state,
            AgentRequestContext {
                caller_channel: caller_channel.as_str(),
                chat_id,
                chat_type: "private",
                persona_id,
                is_scheduled_task: false,
                is_background_job: true,
                run_key: Some(job_id.clone()),
                reply_bot_instance_id: None,
                session_id: session_id.clone(),
                history_override: None,
                tool_iteration_cap: None,
                skip_recall: false,
            },
            Some(&prompt),
            crate::agent::InboundImages::new(),
            Some(&evt_tx),
            Some(cancel.clone()),
        )
        .await;
        drop(evt_tx);
        let _ = hb_forward.await;

        let (raw_output, raw_success) = match bg_result {
            Ok(agent_out) => (agent_out.response, true),
            Err(e) => (format!("Background job error: {e}"), false),
        };
        let _ = if raw_success {
            hb_tx.send(HeartbeatSignal::Progress(
                "main agent finished, preparing final response".to_string(),
            ))
        } else {
            hb_tx.send(HeartbeatSignal::Failed(raw_output.clone()))
        };

        let cancelled = cancel.load(Ordering::SeqCst) || raw_output.trim() == "Run cancelled.";
        if cancelled {
            let _ = hb_tx.send(HeartbeatSignal::Failed(
                "background job cancelled".to_string(),
            ));
            let jid = job_id.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.mark_background_job_cancelled(&jid, "Cancelled by user")
            })
            .await;
            info!(job_id = %job_id, chat_id = chat_id, "Background job cancelled");
            state.background_job_control.finish(&job_id).await;
            return;
        }

        // Persist raw background result/error.
        let jid = job_id.clone();
        let output_for_db = raw_output.clone();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.mark_background_job_completed_raw(&jid, &output_for_db)
        })
        .await;

        // Mark continuation lane while we generate user-facing output.
        let jid = job_id.clone();
        let lease_ttl_for_processing = state.config.background_job_lease_ttl_secs as i64;
        let _ = call_blocking(state.db.clone(), move |db| {
            db.mark_background_job_main_agent_processing(&jid, lease_ttl_for_processing)
        })
        .await;

        let format_prompt = if raw_success {
            format!(
                "The user's original request was: \"{}\".\n\nBackground job result:\n{}\n\nRespond to the user with a concise final answer.",
                prompt, raw_output
            )
        } else {
            format!(
                "The user's original request was: \"{}\".\n\nBackground job error:\n{}\n\nInform the user about this failure and suggest next steps.",
                prompt, raw_output
            )
        };

        let final_result = state
            .llm
            .send_message(
                "You are a concise assistant writing final user-facing replies.",
                vec![Message {
                    role: "user".into(),
                    content: MessageContent::Text(format_prompt),
                }],
                None,
            )
            .await
            .map(|resp| {
                resp.content
                    .iter()
                    .filter_map(|block| match block {
                        ResponseContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("")
            });

        match final_result {
            Ok(final_text) => {
                info!(
                    job_id = %job_id,
                    chat_id = chat_id,
                    response_len = final_text.len(),
                    "Background job: main agent produced final response"
                );
                if let Err(e) = deliver_agent_final_to_contact(
                    state.db.clone(),
                    state.telegram_bots.as_ref(),
                    state.discord_http.as_ref(),
                    state.wecom.as_deref(),
                    &state.config.bot_username,
                    chat_id,
                    persona_id,
                    &final_text,
                    Some(state.config.workspace_root_absolute()),
                    DeliveryScope::StoreOnly,
                    session_id.clone(),
                )
                .await
                {
                    error!(
                        job_id = %job_id,
                        "Background job: failed to deliver final response: {e}"
                    );
                    let jid = job_id.clone();
                    let err_text = format!("Delivery failed after continuation: {e}");
                    let _ = call_blocking(state.db.clone(), move |db| {
                        db.fail_background_job(&jid, &err_text)
                    })
                    .await;
                    info!(job_id = %job_id, chat_id = chat_id, "Background job finished");
                    state.background_job_control.finish(&job_id).await;
                    return;
                }

                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.mark_background_job_done(&jid)
                })
                .await;
                let _ = hb_tx.send(HeartbeatSignal::Finished(
                    "background job completed".to_string(),
                ));
            }
            Err(e) => {
                error!(
                    job_id = %job_id,
                    "Background job: format pass failed: {e}"
                );
                let fallback = if raw_success {
                    format!("Your background task completed, but I had trouble generating a summary. Here is the raw result:\n\n{}", raw_output)
                } else {
                    format!("Your background task failed: {}", raw_output)
                };
                let _ = deliver_agent_final_to_contact(
                    state.db.clone(),
                    state.telegram_bots.as_ref(),
                    state.discord_http.as_ref(),
                    state.wecom.as_deref(),
                    &state.config.bot_username,
                    chat_id,
                    persona_id,
                    &fallback,
                    Some(state.config.workspace_root_absolute()),
                    DeliveryScope::StoreOnly,
                    session_id.clone(),
                )
                .await;

                let jid = job_id.clone();
                let err_text = format!("Background formatting failed: {e}");
                let err_text_for_db = err_text.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.fail_background_job(&jid, &err_text_for_db)
                })
                .await;
                let _ = hb_tx.send(HeartbeatSignal::Failed(err_text));
            }
        }

        info!(job_id = %job_id, chat_id = chat_id, "Background job finished");
        state.background_job_control.finish(&job_id).await;
    });
    start_rx
}
