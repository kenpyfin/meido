//! A tagged persona answers inside the host chat.
//!
//! The guest uses its own memory, skills, and recall. The only borrowed input
//! is the host's recent window. The reply is stored on the host thread, not in
//! the guest's own session.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::agent::{
    process_with_agent_with_events, AgentRequestContext, AppState, EMPTY_USER_REQUEST,
};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::claude::{Message, MessageContent};
use crate::db::{call_blocking, Database, StoredMessage, MESSAGE_ORIGIN_GUEST};
use crate::error::FinallyAValueBotError;

use super::{
    GUEST_HISTORY_CHARS, GUEST_HISTORY_LINE_CHARS, GUEST_HISTORY_TURNS, GUEST_MAX_PER_TURN,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestSpeaker {
    pub id: i64,
    pub name: String,
}

/// Single legacy id first, then the list. Callers dedupe in `validate_guest_ids`.
pub fn ordered_guest_ids(single: Option<i64>, many: &[i64]) -> Vec<i64> {
    let mut ids = Vec::new();
    if let Some(id) = single.filter(|id| *id > 0) {
        ids.push(id);
    }
    ids.extend(many.iter().copied().filter(|id| *id > 0));
    ids
}

/// Deduped speakers in typed order. The host and unknown personas are rejected.
pub fn validate_guest_ids(
    db: &Database,
    chat_id: i64,
    host_persona_id: i64,
    ids: &[i64],
) -> Result<Vec<GuestSpeaker>, String> {
    let mut speakers = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(*id) {
            continue;
        }
        if *id == host_persona_id {
            return Err("You cannot tag the current persona.".into());
        }
        let exists = db.persona_exists(chat_id, *id).map_err(|e| e.to_string())?;
        let Some(persona) = db.get_persona(*id).map_err(|e| e.to_string())? else {
            return Err("A tagged persona is not in this chat.".into());
        };
        if !exists || persona.chat_id != chat_id {
            return Err(format!("{} is not in this chat.", persona.name));
        }
        speakers.push(GuestSpeaker {
            id: persona.id,
            name: persona.name,
        });
    }
    if speakers.len() > GUEST_MAX_PER_TURN {
        return Err(format!("Tag at most {GUEST_MAX_PER_TURN} personas."));
    }
    Ok(speakers)
}

pub async fn run_guest_turn(
    state: Arc<AppState>,
    chat_id: i64,
    host_persona_id: i64,
    guest_persona_id: i64,
    session_id: Option<String>,
    question: &str,
    position: usize,
    total: usize,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    if guest_persona_id == host_persona_id {
        return Err("a persona cannot guest in its own chat".into());
    }
    let started = std::time::Instant::now();
    let db = state.db.clone();
    let question_owned = question.to_string();
    let session_for_load = session_id.clone();
    let (guest_name, host_name, history) = call_blocking(db, move |db| {
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
        let host_name = db
            .get_persona(host_persona_id)?
            .filter(|p| p.chat_id == chat_id)
            .map(|p| p.name)
            .unwrap_or_else(|| "the current persona".into());
        let stored = if let Some(session_id) = session_for_load.as_deref() {
            db.get_recent_messages_for_session(session_id, GUEST_HISTORY_TURNS)?
                .0
        } else {
            db.get_recent_messages(chat_id, host_persona_id, GUEST_HISTORY_TURNS, true)?
        };
        Ok((guest.name, host_name, stored))
    })
    .await
    .map_err(|e| e.to_string())?;

    let question_redacted = state.env_redactor.redact(&question_owned);
    let framed = frame_guest_question(&host_name, position, total, &question_redacted);
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
    let messages = pack_guest_history(lines, &question_redacted, &framed);
    ensure_guest_window(&messages)?;

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
    if result.trim() == EMPTY_USER_REQUEST {
        return Err(EMPTY_USER_REQUEST.to_string());
    }
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

fn frame_guest_question(host_name: &str, position: usize, total: usize, question: &str) -> String {
    let host = guest_attr(host_name);
    let host = if host.is_empty() {
        "the current persona".to_string()
    } else {
        host
    };
    format!(
        "[guest_turn host=\"{host}\" position=\"{position} of {total}\"]\n\
         You were tagged into {host}'s thread. The lines above are {host}'s recent conversation, not yours. \
         Answer as yourself from your own memory. Use recall for your past conversations.\n\
         {question}"
    )
}

fn guest_attr(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '<' && *c != '>')
        .take(80)
        .collect()
}

/// Pack the host's recent window so the framed question is the final user message.
///
/// History arrives oldest-first. The framed question is reserved from its start.
/// A stored copy of the raw question is removed so a later guest does not see it twice.
/// Each older line is capped at `GUEST_HISTORY_LINE_CHARS` (keeping its tail), then
/// the remaining character budget is filled newest-first. A line that still does not
/// fit contributes its tail and the walk stops.
fn pack_guest_history(
    mut history: Vec<GuestHistoryLine>,
    question: &str,
    framed: &str,
) -> Vec<Message> {
    if !question.is_empty() {
        if let Some(index) = history
            .iter()
            .rposition(|line| line.role == "user" && line.text == question)
        {
            history.remove(index);
        }
    }
    let trigger_text = take_head(framed, GUEST_HISTORY_CHARS);
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
        let capped_len = len.min(GUEST_HISTORY_LINE_CHARS);
        if capped_len <= budget {
            let text = if len <= GUEST_HISTORY_LINE_CHARS {
                line.text
            } else {
                take_tail(&line.text, GUEST_HISTORY_LINE_CHARS)
            };
            budget = budget.saturating_sub(text.chars().count());
            older_newest_first.push(GuestHistoryLine {
                role: line.role,
                text,
            });
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

fn ensure_guest_window(messages: &[Message]) -> Result<(), String> {
    if messages
        .last()
        .is_some_and(|message| message.role == "user")
    {
        Ok(())
    } else {
        Err("guest window has no user question".into())
    }
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
        let question = "@Reading_Pal_Developer Give me the summary";
        let packed = pack_guest_history(
            vec![
                line("assistant", &older),
                line("assistant", &older),
                line("user", question),
            ],
            question,
            question,
        );
        assert_eq!(packed.last().map(|m| m.role.as_str()), Some("user"));
        assert_eq!(text_of(packed.last().unwrap()), question);
        let prior_chars: usize = packed
            .iter()
            .rev()
            .skip(1)
            .map(|m| text_of(m).chars().count())
            .sum();
        assert!(prior_chars < GUEST_HISTORY_CHARS);
        assert!(prior_chars > 0);
        assert!(text_of(&packed[0]).chars().count() <= GUEST_HISTORY_LINE_CHARS);
    }

    #[test]
    fn long_reply_is_capped_per_line() {
        let long = format!("HEAD{}", "x".repeat(GUEST_HISTORY_LINE_CHARS + 40));
        let packed = pack_guest_history(
            vec![
                line("assistant", &long),
                line("assistant", &long),
                line("user", "ASK"),
            ],
            "ASK",
            "ASK",
        );
        assert_eq!(packed.len(), 3);
        assert_eq!(
            text_of(&packed[0]).chars().count(),
            GUEST_HISTORY_LINE_CHARS
        );
        assert_eq!(
            text_of(&packed[1]).chars().count(),
            GUEST_HISTORY_LINE_CHARS
        );
        assert!(!text_of(&packed[0]).contains("HEAD"));
        assert!(text_of(&packed[0]).ends_with('x'));
        assert_eq!(text_of(&packed[2]), "ASK");
    }

    #[test]
    fn partial_older_line_keeps_the_tail_nearest_the_question() {
        let trigger = "ASK";
        let older = format!("OLD_HEAD{}", "y".repeat(GUEST_HISTORY_CHARS));
        let packed = pack_guest_history(
            vec![line("assistant", &older), line("user", trigger)],
            trigger,
            trigger,
        );
        assert_eq!(packed.len(), 2);
        assert!(!text_of(&packed[0]).contains("OLD_HEAD"));
        assert!(text_of(&packed[0]).ends_with('y'));
        assert_eq!(
            text_of(&packed[0]).chars().count(),
            GUEST_HISTORY_LINE_CHARS
        );
        assert_eq!(text_of(&packed[1]), "ASK");
    }

    #[test]
    fn question_is_appended_when_the_slice_ends_on_the_assistant() {
        let packed = pack_guest_history(
            vec![line("assistant", "earlier reply")],
            "the question that was not in the slice",
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
    fn later_guest_keeps_one_framed_question() {
        let question = "look at this";
        let framed = frame_guest_question("pep", 2, 2, question);
        let packed = pack_guest_history(
            vec![
                line("user", question),
                line("assistant", "[Guest: Reading_Pal_Developer]\nfirst reply"),
            ],
            question,
            &framed,
        );
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].role, "assistant");
        assert!(text_of(&packed[0]).contains("first reply"));
        assert_eq!(packed[1].role, "user");
        assert!(text_of(&packed[1]).starts_with("[guest_turn host=\"pep\" position=\"2 of 2\"]"));
        assert!(text_of(&packed[1]).contains(question));
        assert_eq!(
            packed
                .iter()
                .filter(|message| message.role == "user")
                .count(),
            1
        );
        assert!(ensure_guest_window(&packed).is_ok());
    }

    #[test]
    fn empty_window_is_rejected() {
        let packed = pack_guest_history(vec![], "", "");
        assert!(packed.is_empty());
        assert_eq!(
            ensure_guest_window(&packed).unwrap_err(),
            "guest window has no user question"
        );
        let assistant_only = pack_guest_history(vec![line("assistant", "hi")], "", "");
        assert!(ensure_guest_window(&assistant_only).is_err());
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
            "second",
        );
        assert_eq!(
            packed.iter().map(text_of).collect::<Vec<_>>(),
            vec!["first", "reply", "second"]
        );
    }
}
