//! One extra persona turn inside a host chat. The guest sees a short redacted window
//! and the reply is stored on the host thread, not in the guest's own session.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::channels::telegram::{process_with_agent_with_events, AgentRequestContext, AppState};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::claude::{Message, MessageContent};
use crate::db::{call_blocking, StoredMessage, MESSAGE_ORIGIN_GUEST};
use crate::error::FinallyAValueBotError;

use super::{GUEST_HISTORY_CHARS, GUEST_HISTORY_TURNS};

pub async fn run_guest_turn(
    state: Arc<AppState>,
    chat_id: i64,
    host_persona_id: i64,
    guest_persona_id: i64,
    session_id: Option<String>,
    question: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    if guest_persona_id == host_persona_id {
        return Err("a persona cannot guest in its own chat".into());
    }
    let started = std::time::Instant::now();
    let db = state.db.clone();
    let question_owned = question.to_string();
    let session_for_load = session_id.clone();
    let (guest_name, history) = call_blocking(db, move |db| {
        if !db.persona_exists(chat_id, guest_persona_id)? {
            return Err(FinallyAValueBotError::ToolExecution(
                "guest persona is not in this chat".into(),
            ));
        }
        let guest = db
            .get_persona(guest_persona_id)?
            .filter(|p| p.chat_id == chat_id)
            .ok_or_else(|| {
                FinallyAValueBotError::ToolExecution("guest persona is not in this chat".into())
            })?;
        let stored = if let Some(session_id) = session_for_load.as_deref() {
            db.get_recent_messages_for_session(session_id, GUEST_HISTORY_TURNS)?
                .0
        } else {
            db.get_recent_messages(chat_id, host_persona_id, GUEST_HISTORY_TURNS, true)?
        };
        Ok((guest.name, stored))
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut messages = Vec::new();
    let mut budget = GUEST_HISTORY_CHARS;
    for item in history {
        let role = if item.is_from_bot {
            "assistant"
        } else {
            "user"
        };
        let mut text = item.content;
        if item.origin == MESSAGE_ORIGIN_GUEST {
            text = format!("[Guest: {}]\n{text}", item.sender_name);
        }
        let text = state.env_redactor.redact(&text);
        let take = text.chars().take(budget).collect::<String>();
        budget = budget.saturating_sub(take.chars().count());
        if take.is_empty() {
            continue;
        }
        messages.push(Message {
            role: role.into(),
            content: MessageContent::Text(take),
        });
        if budget == 0 {
            break;
        }
    }
    if messages.is_empty() {
        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Text(
                state
                    .env_redactor
                    .redact(&question_owned)
                    .chars()
                    .take(GUEST_HISTORY_CHARS)
                    .collect(),
            ),
        });
    }

    let (tx, rx) = oneshot::channel();
    let app = state.clone();
    let run_id = format!("guest:{}", uuid::Uuid::new_v4());
    let meta = QueueEnqueueMeta {
        run_id,
        persona_id: guest_persona_id,
        source: QueueSource::Web,
        label: format!("guest:{guest_persona_id}"),
        project_id: None,
        workflow_id: None,
        on_hard_abort: None,
    };
    state
        .chat_queue
        .enqueue_with_meta(chat_id, meta, move |lane_cancel| async move {
            let cancel = cancel.or(Some(lane_cancel));
            let result = process_with_agent_with_events(
                &app,
                AgentRequestContext {
                    caller_channel: "web",
                    chat_id,
                    chat_type: "private",
                    persona_id: guest_persona_id,
                    is_scheduled_task: false,
                    is_background_job: false,
                    run_key: Some(format!("guest:{}", uuid::Uuid::new_v4())),
                    reply_bot_instance_id: None,
                    session_id: None,
                    history_override: Some(messages),
                    tool_iteration_cap: None,
                    skip_recall: true,
                },
                None,
                crate::telegram::InboundImages::new(),
                None,
                cancel,
            )
            .await
            .map(|r| r.response)
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .await;
    let result = rx.await.map_err(|_| "guest turn dropped".to_string())??;
    let reply = state.env_redactor.redact(&result);
    let stored = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id,
        persona_id: host_persona_id,
        session_id,
        sender_name: guest_name.clone(),
        content: reply.clone(),
        is_from_bot: true,
        timestamp: chrono::Utc::now().to_rfc3339(),
        origin: MESSAGE_ORIGIN_GUEST.to_string(),
        speaker_persona_id: Some(guest_persona_id),
    };
    let db = state.db.clone();
    call_blocking(db, move |db| db.store_message(&stored))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        target: "collab",
        kind = "guest",
        chat_id,
        host_persona_id,
        guest_persona_id,
        input_chars = question_owned.chars().count(),
        duration_ms = started.elapsed().as_millis() as u64,
        "guest turn stored"
    );
    Ok(reply)
}
