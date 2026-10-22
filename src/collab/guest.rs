//! One extra persona turn inside a host chat. The guest sees a short redacted window
//! and the reply is stored on the host thread, not in the guest's own session.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::agent::{process_with_agent_with_events, AgentRequestContext, AppState};
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

    let question_redacted = state.env_redactor.redact(&question_owned);
    let mut lines = Vec::with_capacity(history.len());
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
        if text.is_empty() {
            continue;
        }
        lines.push(GuestHistoryLine {
            role: role.into(),
            text,
        });
    }
    let mut messages = pack_guest_history(lines, &question_redacted);
    if messages.is_empty() {
        let fallback = take_head(&question_redacted, GUEST_HISTORY_CHARS);
        if !fallback.is_empty() {
            messages.push(Message {
                role: "user".into(),
                content: MessageContent::Text(fallback),
            });
        }
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
                    inbound_user_message_id: None,
                },
                None,
                crate::agent::InboundImages::new(),
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

struct GuestHistoryLine {
    role: String,
    text: String,
}

/// Pack the guest window so the triggering question is the final user message.
///
/// History arrives oldest-first. The character budget is spent newest-first:
/// the trailing user line (or `question`, when the slice does not end on the
/// user) is reserved from the start of that text, then older lines fill what
/// remains. A line that does not fit contributes its tail, the part nearest
/// the question, and then the walk stops.
fn pack_guest_history(mut history: Vec<GuestHistoryLine>, question: &str) -> Vec<Message> {
    let stored_trigger = if history.last().is_some_and(|line| line.role == "user") {
        history.pop()
    } else {
        None
    };
    let trigger_source = stored_trigger
        .as_ref()
        .map(|line| line.text.as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or(question);
    let trigger_text = take_head(trigger_source, GUEST_HISTORY_CHARS);
    let mut budget = GUEST_HISTORY_CHARS.saturating_sub(trigger_text.chars().count());

    let mut older_newest_first = Vec::new();
    for line in history.into_iter().rev() {
        if budget == 0 {
            break;
        }
        let len = line.text.chars().count();
        if len == 0 {
            continue;
        }
        if len <= budget {
            budget -= len;
            older_newest_first.push(line);
            continue;
        }
        let tail = take_tail(&line.text, budget);
        if !tail.is_empty() {
            older_newest_first.push(GuestHistoryLine {
                role: line.role,
                text: tail,
            });
        }
        break;
    }
    older_newest_first.reverse();

    let mut messages = Vec::with_capacity(older_newest_first.len() + 1);
    for line in older_newest_first {
        messages.push(Message {
            role: line.role,
            content: MessageContent::Text(line.text),
        });
    }
    if !trigger_text.is_empty() {
        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Text(trigger_text),
        });
    }
    messages
}

fn take_head(text: &str, budget: usize) -> String {
    text.chars().take(budget).collect()
}

fn take_tail(text: &str, budget: usize) -> String {
    let len = text.chars().count();
    if len <= budget {
        return text.to_string();
    }
    text.chars().skip(len - budget).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(role: &str, text: &str) -> GuestHistoryLine {
        GuestHistoryLine {
            role: role.into(),
            text: text.into(),
        }
    }

    fn text_of(message: &Message) -> &str {
        match &message.content {
            MessageContent::Text(text) => text,
            MessageContent::Blocks(_) => panic!("expected text"),
        }
    }

    #[test]
    fn budget_keeps_the_triggering_question_when_older_replies_are_long() {
        let older = "A".repeat(GUEST_HISTORY_CHARS);
        let packed = pack_guest_history(
            vec![
                line("assistant", &older),
                line("assistant", &older),
                line("user", "@Reading_Pal_Developer Give me the summary"),
            ],
            "@Reading_Pal_Developer Give me the summary",
        );
        assert_eq!(packed.last().map(|m| m.role.as_str()), Some("user"));
        assert_eq!(
            text_of(packed.last().unwrap()),
            "@Reading_Pal_Developer Give me the summary"
        );
        let prior_chars: usize = packed
            .iter()
            .rev()
            .skip(1)
            .map(|m| text_of(m).chars().count())
            .sum();
        assert!(prior_chars < GUEST_HISTORY_CHARS);
        assert!(prior_chars > 0);
    }

    #[test]
    fn partial_older_line_keeps_the_tail_nearest_the_question() {
        let trigger = "ASK";
        let older = format!("OLD_HEAD{}", "x".repeat(GUEST_HISTORY_CHARS));
        let packed = pack_guest_history(
            vec![line("assistant", &older), line("user", trigger)],
            trigger,
        );
        assert_eq!(packed.len(), 2);
        assert!(!text_of(&packed[0]).contains("OLD_HEAD"));
        assert!(text_of(&packed[0]).ends_with('x'));
        assert_eq!(text_of(&packed[1]), "ASK");
        let total: usize = packed.iter().map(|m| text_of(m).chars().count()).sum();
        assert_eq!(total, GUEST_HISTORY_CHARS);
    }

    #[test]
    fn question_is_appended_when_the_slice_ends_on_the_assistant() {
        let packed = pack_guest_history(
            vec![line("assistant", "earlier reply")],
            "the question that was not in the slice",
        );
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].role, "assistant");
        assert_eq!(packed[1].role, "user");
        assert_eq!(
            text_of(&packed[1]),
            "the question that was not in the slice"
        );
    }

    #[test]
    fn short_history_stays_in_order() {
        let packed = pack_guest_history(
            vec![
                line("user", "first"),
                line("assistant", "reply"),
                line("user", "second"),
            ],
            "second",
        );
        assert_eq!(
            packed.iter().map(|m| text_of(m)).collect::<Vec<_>>(),
            vec!["first", "reply", "second"]
        );
    }
}
