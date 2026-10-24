use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::agent::{process_with_agent_with_events, AgentRequestContext, AppState};
use crate::background_jobs::{
    await_handoff_startup_ack, is_background_handoff_response, try_enqueue_background_handoff,
    HandoffEnqueueOutcome,
};
use crate::channel::{
    deliver_agent_final_to_contact_with_origin, DeliveryScope, MessageStoreOrigin,
};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::db::{call_blocking, ScheduledTask};
use crate::error::FinallyAValueBotError;
use crate::job_heartbeat::{
    signal_from_agent_event, spawn_shared_heartbeat, HeartbeatSignal, JobType,
};

fn channel_from_chat_type(chat_type: &str) -> &'static str {
    match chat_type {
        "discord" => "discord",
        "whatsapp" => "whatsapp",
        "wecom" => "wecom",
        "web" => "web",
        _ => "telegram",
    }
}

/// Snapshot after DB claim and persona resolution; agent work runs in a spawned task.
struct PreparedScheduledRun {
    task_id: i64,
    chat_id: i64,
    persona_id: i64,
    channel: &'static str,
    prompt: String,
    next_run: Option<String>,
    started_at_str: String,
    output_delivery: String,
}

pub fn spawn_scheduler(state: Arc<AppState>) {
    let permits = state.config.scheduler_max_concurrent_tasks.max(1);
    let semaphore = Arc::new(Semaphore::new(permits));
    let task_timeout_secs = state.config.scheduler_task_timeout_secs;
    let stale_reclaim_secs = state.config.scheduler_stale_running_reclaim_secs as i64;
    let poll_interval_secs = state.config.scheduler_poll_interval_secs.max(1);

    tokio::spawn(async move {
        info!(
            "Scheduler started (poll_interval_secs={poll_interval_secs}, max_concurrent={permits}, task_timeout_secs={task_timeout_secs}, stale_reclaim_secs={stale_reclaim_secs})"
        );
        // Same loop as due tasks. Trash is checked at most once an hour.
        let mut last_trash_expiry = tokio::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or_else(tokio::time::Instant::now);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(poll_interval_secs)).await;
            run_due_tasks(
                &state,
                semaphore.clone(),
                task_timeout_secs,
                stale_reclaim_secs,
            )
            .await;
            if last_trash_expiry.elapsed() >= std::time::Duration::from_secs(3600) {
                last_trash_expiry = tokio::time::Instant::now();
                expire_artifact_trash(&state).await;
            }
            // Topic titling calls an LLM, so it must not block due-task polling.
            let topic_state = state.clone();
            tokio::spawn(async move {
                crate::topics::segment_due_tick(&topic_state).await;
            });
        }
    });
}

async fn expire_artifact_trash(state: &AppState) {
    let root = state.config.workspace_root_absolute();
    for _ in 0..20 {
        let root_for_list = root.clone();
        let due = match tokio::task::spawn_blocking(move || {
            crate::artifacts::Catalog::open(&root_for_list)?.trash_due(chrono::Utc::now())
        })
        .await
        {
            Ok(Ok(due)) => due,
            Ok(Err(e)) => {
                warn!("artifact trash expiry failed: {e}");
                return;
            }
            Err(e) => {
                warn!("artifact trash expiry task failed: {e}");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        let mut progressed = false;
        for artifact in due {
            if persona_has_active_work(state, artifact.chat_id, artifact.persona_id).await {
                continue;
            }
            let id = artifact.id.clone();
            let root_for_delete = root.clone();
            match tokio::task::spawn_blocking(move || {
                crate::artifacts::Catalog::open(&root_for_delete)?.delete(&id, false)
            })
            .await
            {
                Ok(Ok(())) => {
                    progressed = true;
                    info!(artifact_id = %artifact.id, "artifact_trash_expired");
                }
                Ok(Err(e)) => {
                    warn!(artifact_id = %artifact.id, "artifact trash expiry skipped: {e}");
                }
                Err(e) => {
                    warn!(artifact_id = %artifact.id, "artifact trash expiry task failed: {e}");
                }
            }
        }
        if !progressed {
            return;
        }
    }
}

async fn persona_has_active_work(state: &AppState, cid: i64, pid: i64) -> bool {
    if state.chat_queue.diagnostics().await.iter().any(|lane| {
        lane.chat_id == cid
            && lane.persona_id == pid
            && (lane.active_for_ms > 0 || lane.pending > 0)
    }) {
        return true;
    }
    call_blocking(state.db.clone(), move |db| {
        db.has_unfinished_background_jobs_for_chat_persona(cid, pid)
    })
    .await
    .unwrap_or(true)
}

async fn run_due_tasks(
    state: &Arc<AppState>,
    semaphore: Arc<Semaphore>,
    task_timeout_secs: u64,
    stale_reclaim_secs: i64,
) {
    let now = Utc::now().to_rfc3339();

    match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.reclaim_stale_running_tasks(&now, stale_reclaim_secs)
    })
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            info!(
                "Scheduler: reclaimed {} stale running task(s): {:?}",
                ids.len(),
                ids
            );
        }
        Err(e) => error!("Scheduler: failed to reclaim stale running tasks: {e}"),
        _ => {}
    }

    match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.reconcile_stale_active_job_heartbeats(&now, stale_reclaim_secs)
    })
    .await
    {
        Ok(keys) if !keys.is_empty() => {
            info!(
                "Scheduler: reconciled {} stale active job heartbeat(s): {:?}",
                keys.len(),
                keys
            );
        }
        Err(e) => error!("Scheduler: failed to reconcile stale job heartbeats: {e}"),
        _ => {}
    }

    let pending_timeout_secs = state.config.background_job_pending_start_timeout_secs as i64;
    let pending_ids = match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.reconcile_stale_pending_background_jobs(&now, pending_timeout_secs)
    })
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            info!(
                "Scheduler: reconciled {} stale pending background job(s): {:?}",
                ids.len(),
                ids
            );
            ids
        }
        Err(e) => {
            error!("Scheduler: failed to reconcile stale pending background jobs: {e}");
            Vec::new()
        }
        _ => Vec::new(),
    };
    crate::background_shell::notify_shell_jobs_by_ids(state.clone(), &pending_ids).await;

    let expired_ids = match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.reconcile_expired_background_job_leases(&now)
    })
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            info!(
                "Scheduler: reconciled {} expired background job lease(s): {:?}",
                ids.len(),
                ids
            );
            ids
        }
        Err(e) => {
            error!("Scheduler: failed to reconcile expired background leases: {e}");
            Vec::new()
        }
        _ => Vec::new(),
    };
    crate::background_shell::notify_shell_jobs_by_ids(state.clone(), &expired_ids).await;

    let orphan_ids = match call_blocking(state.db.clone(), {
        let now = now.clone();
        move |db| db.reconcile_orphan_stale_background_jobs(&now, stale_reclaim_secs)
    })
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            info!(
                "Scheduler: reconciled {} orphan stale background job(s): {:?}",
                ids.len(),
                ids
            );
            ids
        }
        Err(e) => {
            error!("Scheduler: failed to reconcile orphan background jobs: {e}");
            Vec::new()
        }
        _ => Vec::new(),
    };
    crate::background_shell::notify_shell_jobs_by_ids(state.clone(), &orphan_ids).await;

    let shell_stale =
        crate::background_shell::reconcile_stale_shell_background_jobs(state.clone()).await;
    if !shell_stale.is_empty() {
        info!(
            "Scheduler: reconciled {} stale shell background job(s): {:?}",
            shell_stale.len(),
            shell_stale
        );
    }

    let tasks = match call_blocking(state.db.clone(), {
        let now_for_due = now.clone();
        move |db| db.get_due_tasks(&now_for_due)
    })
    .await
    {
        Ok(t) => t,
        Err(e) => {
            error!("Scheduler: failed to query due tasks: {e}");
            return;
        }
    };

    let due_count = tasks.len();
    let mut claimed = 0usize;
    let mut spawned = 0usize;

    for task in tasks {
        // Claim synchronously before spawning so the next tick cannot re-list the same task
        // while workers are still waiting on the semaphore.
        let (prepared, did_claim) = match claim_and_prepare_scheduled_task(state, task, &now).await
        {
            Ok(pair) => pair,
            Err(e) => {
                error!("Scheduler: failed to prepare task: {e}");
                continue;
            }
        };
        if did_claim {
            claimed += 1;
        }
        let Some(prepared) = prepared else {
            continue;
        };
        spawned += 1;

        let state = state.clone();
        let sem = semaphore.clone();
        tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let chat_id_for_queue = prepared.chat_id;
            let prompt_label = prepared.prompt.chars().take(120).collect::<String>();
            let queue_run_id = uuid::Uuid::new_v4().to_string();
            let on_hard_abort = Some(crate::queue_abort::make_store_only_hard_abort_hook(
                state.db.clone(),
                state.config.bot_username.clone(),
                chat_id_for_queue,
                prepared.persona_id,
                queue_run_id.clone(),
            ));
            let queue_meta = QueueEnqueueMeta {
                run_id: queue_run_id,
                persona_id: prepared.persona_id,
                source: QueueSource::Scheduler,
                label: prompt_label,
                project_id: None,
                workflow_id: None,
                on_hard_abort,
            };
            let chat_queue = state.chat_queue.clone();
            let (queue_position, _) = chat_queue
                .enqueue_with_meta(chat_id_for_queue, queue_meta, |cancel| async move {
                    run_scheduled_agent_and_finalize(
                        state,
                        prepared,
                        task_timeout_secs,
                        Some(cancel),
                    )
                    .await;
                })
                .await;
            info!(
                target: "queue",
                chat_id = chat_id_for_queue,
                queue_position = queue_position,
                "Enqueued scheduled agent run"
            );
        });
    }

    if due_count > 0 {
        info!("Scheduler tick: due_count={due_count}, claimed={claimed}, spawned={spawned}");
    }
}

/// Claim in DB, resolve channel/persona.
/// Second tuple element is true iff the atomic DB claim (`try_mark_task_running`) succeeded.
async fn claim_and_prepare_scheduled_task(
    state: &Arc<AppState>,
    task: ScheduledTask,
    now: &str,
) -> Result<(Option<PreparedScheduledRun>, bool), FinallyAValueBotError> {
    let task_id = task.id;
    let chat_id = task.chat_id;
    let scheduled_persona_id = task.persona_id;
    let prompt = task.prompt.clone();
    let output_delivery = if task.output_delivery == "transcript" {
        "transcript".to_string()
    } else {
        "runs_only".to_string()
    };

    info!(
        "Scheduler: executing task #{} for chat {}",
        task_id, chat_id
    );

    let started_at = Utc::now();
    let started_at_str = started_at.to_rfc3339();

    let tz: chrono_tz::Tz = state.config.timezone.parse().unwrap_or(chrono_tz::Tz::UTC);
    let next_run = if task.schedule_type == "cron" {
        match cron::Schedule::from_str(&task.schedule_value) {
            Ok(schedule) => schedule
                .after(&started_at.with_timezone(&tz))
                .next()
                .map(|t| t.with_timezone(&Utc).to_rfc3339()),
            Err(e) => {
                error!("Scheduler: invalid cron for task #{}: {e}", task_id);
                None
            }
        }
    } else {
        None
    };

    if task.schedule_type == "cron" && next_run.is_none() {
        error!(
            "Scheduler: skipping task #{} — could not compute next cron occurrence",
            task_id
        );
        return Ok((None, false));
    }

    let started_for_claim = started_at_str.clone();
    let next_run_claim = next_run.clone();
    let now_bound = now.to_string();
    let claimed = call_blocking(state.db.clone(), move |db| {
        db.try_mark_task_running(
            task_id,
            &started_for_claim,
            next_run_claim.as_deref(),
            &now_bound,
        )
    })
    .await?;
    if !claimed {
        info!(
            "Scheduler: task #{} already claimed or not due, skipping",
            task_id
        );
        return Ok((None, false));
    }

    let channel = match call_blocking(state.db.clone(), move |db| db.get_chat_type(chat_id)).await {
        Ok(Some(chat_type)) => channel_from_chat_type(&chat_type),
        _ => "telegram",
    };

    let persona_id = call_blocking(state.db.clone(), move |db| {
        if scheduled_persona_id > 0 && db.persona_exists(chat_id, scheduled_persona_id)? {
            return Ok(scheduled_persona_id);
        }
        let fallback = db.get_current_persona_id(chat_id)?;
        if scheduled_persona_id > 0 && fallback != scheduled_persona_id {
            info!(
                "Scheduler: task #{} persona {} missing for chat {}, falling back to persona {}",
                task_id, scheduled_persona_id, chat_id, fallback
            );
        }
        Ok(fallback)
    })
    .await
    .unwrap_or(0);

    if persona_id == 0 {
        error!("Scheduler: could not resolve persona for chat {}", chat_id);
        let next_run_finalize = next_run.clone();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.finalize_task_run(task_id, next_run_finalize.as_deref())?;
            Ok(())
        })
        .await;
        let finished_at_str = Utc::now().to_rfc3339();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.log_task_run(
                task_id,
                chat_id,
                &started_at_str,
                &finished_at_str,
                0,
                false,
                Some("Could not resolve persona for chat"),
            )?;
            Ok(())
        })
        .await;
        return Ok((None, true));
    }

    Ok((
        Some(PreparedScheduledRun {
            task_id,
            chat_id,
            persona_id,
            channel,
            prompt,
            next_run,
            started_at_str,
            output_delivery,
        }),
        true,
    ))
}

async fn record_scheduled_output(
    state: &AppState,
    task_id: i64,
    chat_id: i64,
    persona_id: i64,
    started_at: &str,
    status: &str,
    body: &str,
    error_text: Option<&str>,
) -> Option<i64> {
    let finished = Utc::now().to_rfc3339();
    let runtime = state.config.runtime_data_dir();
    let started = started_at.to_string();
    let status = status.to_string();
    let body = body.to_string();
    let error_text = error_text.map(str::to_string);
    match call_blocking(state.db.clone(), move |db| {
        db.insert_scheduled_run(
            task_id,
            chat_id,
            persona_id,
            &started,
            &finished,
            &status,
            &body,
            error_text.as_deref(),
            Path::new(&runtime),
        )
    })
    .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            error!("Scheduler: failed to store run for task #{task_id}: {e}");
            None
        }
    }
}

async fn maybe_store_scheduled_transcript(
    state: &AppState,
    output_delivery: &str,
    task_id: i64,
    run_id: Option<i64>,
    chat_id: i64,
    persona_id: i64,
    body: &str,
) {
    if output_delivery != "transcript" {
        return;
    }
    let prefix = match run_id {
        Some(id) => format!("[Scheduled task #{task_id} run #{id}]"),
        None => format!("[Scheduled task #{task_id}]"),
    };
    let text = format!("{prefix}\n{body}");
    if let Err(e) = deliver_agent_final_to_contact_with_origin(
        state.db.clone(),
        state.telegram_bots.as_ref(),
        state.discord_http.as_ref(),
        state.wecom.as_deref(),
        &state.config.bot_username,
        chat_id,
        persona_id,
        &text,
        Some(state.config.workspace_root_absolute()),
        DeliveryScope::StoreOnly,
        None,
        MessageStoreOrigin::Scheduled,
    )
    .await
    {
        error!("Scheduler: failed to store transcript for task #{task_id}: {e}");
    }
}

/// Agent run, deliver, finalize, log. Dropping the outer `timeout` future does not necessarily
/// kill child OS processes started by tools (e.g. bash); it only bounds async work.
async fn run_scheduled_agent_and_finalize(
    state: Arc<AppState>,
    p: PreparedScheduledRun,
    task_timeout_secs: u64,
    cancel: Option<Arc<AtomicBool>>,
) {
    let task_id = p.task_id;
    let chat_id = p.chat_id;
    let persona_id = p.persona_id;
    let channel = p.channel;
    let output_delivery = p.output_delivery;
    let mut prompt = p.prompt;
    if crate::collab::coordinator::prompt_is_brief(&prompt) {
        match crate::collab::coordinator::build_digest(&state, chat_id).await {
            Ok(digest) => prompt = format!("{prompt}\n\n{digest}"),
            Err(e) => {
                error!("Scheduler: coordinator digest failed for task #{task_id}: {e}");
            }
        }
    }
    let next_run = p.next_run;
    let started_at_str = p.started_at_str;
    let started_at = chrono::DateTime::parse_from_rfc3339(&started_at_str)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    if crate::recall::is_reconcile_prompt(&prompt) {
        let svc = state.tools.recall();
        let (success, result_summary) = match svc.run_reconcile().await {
            Ok(summary) => {
                info!(task_id, "recall reconcile finished");
                (true, Some(summary))
            }
            Err(class) => {
                error!(task_id, failure_class = %class, "recall reconcile failed");
                (false, Some(format!("recall reconcile failed: {class}")))
            }
        };
        let finished_at = Utc::now();
        let finished_at_str = finished_at.to_rfc3339();
        let duration_ms = (finished_at - started_at).num_milliseconds();
        let next_run_cleanup = next_run.clone();
        let started_for_cleanup = started_at_str.clone();
        let finished_for_cleanup = finished_at_str.clone();
        let summary_for_cleanup = result_summary.clone();
        if let Err(e) = call_blocking(state.db.clone(), move |db| {
            db.finalize_task_run(task_id, next_run_cleanup.as_deref())?;
            db.log_task_run(
                task_id,
                chat_id,
                &started_for_cleanup,
                &finished_for_cleanup,
                duration_ms,
                success,
                summary_for_cleanup.as_deref(),
            )?;
            Ok(())
        })
        .await
        {
            error!("Scheduler: failed to finalize recall reconcile #{task_id}: {e}");
        }
        let body = result_summary.unwrap_or_else(|| {
            if success {
                "recall reconcile finished".to_string()
            } else {
                "recall reconcile failed".to_string()
            }
        });
        let run_status = if success { "ok" } else { "error" };
        let error_text = if success { None } else { Some(body.clone()) };
        let run_id = record_scheduled_output(
            &state,
            task_id,
            chat_id,
            persona_id,
            &started_at_str,
            run_status,
            &body,
            error_text.as_deref(),
        )
        .await;
        maybe_store_scheduled_transcript(
            &state,
            &output_delivery,
            task_id,
            run_id,
            chat_id,
            persona_id,
            &body,
        )
        .await;
        return;
    }

    let hb_key = format!("scheduled:{}:{}", task_id, started_at_str);
    let hb_tx = spawn_shared_heartbeat(
        state.clone(),
        hb_key,
        chat_id,
        persona_id,
        JobType::Scheduled,
        None,
        false,
    );
    let _ = hb_tx.send(HeartbeatSignal::Started(format!(
        "scheduled task #{} started",
        task_id
    )));
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

    let agent_fut = {
        let global_engine = state.runtime_toggles.agent_engine();
        let engine = crate::runtime_toggles::resolve_run_agent_engine_from_persona(
            state.db.as_ref(),
            chat_id,
            persona_id,
            global_engine,
        )
        .as_str();
        info!(
            "Scheduler: task #{} starting agent run (engine={engine}, persona={persona_id})",
            task_id,
        );
        process_with_agent_with_events(
            &state,
            AgentRequestContext {
                caller_channel: channel,
                chat_id,
                chat_type: "private",
                persona_id,
                is_scheduled_task: true,
                is_background_job: false,
                run_key: Some(format!("scheduled:{}:{}", task_id, started_at_str)),
                reply_bot_instance_id: None,
                session_id: None,
                history_override: None,
                tool_iteration_cap: None,
                skip_recall: false,
                inbound_user_message_id: None,
            },
            Some(&prompt),
            crate::agent::InboundImages::new(),
            Some(&evt_tx),
            cancel,
        )
    };

    let (success, result_summary) = match tokio::time::timeout(
        std::time::Duration::from_secs(task_timeout_secs),
        agent_fut,
    )
    .await
    {
        Err(_elapsed) => {
            drop(evt_tx);
            let _ = hb_forward.await;
            error!(
                "Scheduler: task #{} timed out after {}s",
                task_id, task_timeout_secs
            );
            let _ = hb_tx.send(HeartbeatSignal::Failed(format!(
                "scheduled task #{} timed out after {}s",
                task_id, task_timeout_secs
            )));
            let err_text = format!(
                "Scheduled task #{} timed out after {} seconds.",
                task_id, task_timeout_secs
            );
            let run_id = record_scheduled_output(
                &state,
                task_id,
                chat_id,
                persona_id,
                &started_at_str,
                "timeout",
                &err_text,
                Some(&err_text),
            )
            .await;
            maybe_store_scheduled_transcript(
                &state,
                &output_delivery,
                task_id,
                run_id,
                chat_id,
                persona_id,
                &err_text,
            )
            .await;
            (false, Some(format!("Timed out after {task_timeout_secs}s")))
        }
        Ok(Ok(agent_out)) => {
            drop(evt_tx);
            let _ = hb_forward.await;
            let mut response_text = if agent_out.response.trim().is_empty() {
                format!("Scheduled task #{} completed.", task_id)
            } else {
                agent_out.response
            };

            if is_background_handoff_response(&response_text) {
                response_text = match try_enqueue_background_handoff(
                    state.clone(),
                    chat_id,
                    persona_id,
                    prompt.clone(),
                    "scheduler",
                    channel,
                    None,
                )
                .await
                {
                    HandoffEnqueueOutcome::Queued { job_id, start_ack } => {
                        let base = await_handoff_startup_ack(start_ack).await;
                        format!("{base} (scheduled task #{task_id}, background job {job_id})")
                    }
                    HandoffEnqueueOutcome::BlockedAlreadyRunning => {
                        "A background task is already running for this chat. Please wait for it to finish before starting another long-running background task.".into()
                    }
                    HandoffEnqueueOutcome::ActiveLookupFailed(msg) => {
                        format!("Failed to check active background jobs: {msg}")
                    }
                    HandoffEnqueueOutcome::DbCreateFailed(e) => {
                        format!("Failed to queue background task: {e}")
                    }
                };
            }

            let response_text = state.env_redactor.redact(&response_text);
            let run_id = record_scheduled_output(
                &state,
                task_id,
                chat_id,
                persona_id,
                &started_at_str,
                "ok",
                &response_text,
                None,
            )
            .await;
            maybe_store_scheduled_transcript(
                &state,
                &output_delivery,
                task_id,
                run_id,
                chat_id,
                persona_id,
                &response_text,
            )
            .await;
            let _ = hb_tx.send(HeartbeatSignal::Finished(format!(
                "scheduled task #{} completed",
                task_id
            )));
            let summary = if response_text.len() > 200 {
                format!(
                    "{}...",
                    &response_text[..response_text.floor_char_boundary(200)]
                )
            } else {
                response_text
            };
            (true, Some(summary))
        }
        Ok(Err(e)) => {
            drop(evt_tx);
            let _ = hb_forward.await;
            let raw_error = e.to_string();
            let redacted_error_internal = state.env_redactor.redact(&raw_error);
            let redacted_error_user = state.env_redactor.redact(&raw_error);
            error!(
                "Scheduler: task #{} failed: {}",
                task_id, redacted_error_internal
            );
            let _ = hb_tx.send(HeartbeatSignal::Failed(format!(
                "scheduled task #{} failed: {}",
                task_id, redacted_error_internal
            )));
            let err_text = format!(
                "Scheduled task #{} failed: {}",
                task_id, redacted_error_user
            );
            let run_id = record_scheduled_output(
                &state,
                task_id,
                chat_id,
                persona_id,
                &started_at_str,
                "error",
                &err_text,
                Some(&redacted_error_internal),
            )
            .await;
            maybe_store_scheduled_transcript(
                &state,
                &output_delivery,
                task_id,
                run_id,
                chat_id,
                persona_id,
                &err_text,
            )
            .await;
            (false, Some(format!("Error: {}", redacted_error_internal)))
        }
    };

    let finished_at = Utc::now();
    let finished_at_str = finished_at.to_rfc3339();
    let duration_ms = (finished_at - started_at).num_milliseconds();

    let next_run_cleanup = next_run.clone();
    let started_for_cleanup = started_at_str.clone();
    let finished_for_cleanup = finished_at_str.clone();
    let summary_for_cleanup = result_summary.clone();
    if let Err(e) = call_blocking(state.db.clone(), move |db| {
        db.finalize_task_run(task_id, next_run_cleanup.as_deref())?;
        db.log_task_run(
            task_id,
            chat_id,
            &started_for_cleanup,
            &finished_for_cleanup,
            duration_ms,
            success,
            summary_for_cleanup.as_deref(),
        )?;
        Ok(())
    })
    .await
    {
        error!(
            "Scheduler: failed to finalize or log task #{}: {e}",
            task_id
        );
    }
}
