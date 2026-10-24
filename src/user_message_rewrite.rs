//! Rewrite the inbound user message before the persona sees it.
//!
//! When the persona flag is on, the system LLM reads the context-depth window
//! and persona memory, then replaces the stored message. History and the chat
//! UI both show that replacement. A missing system LLM, a failed call, or an
//! unusable draft leaves the original text in place.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::agent::{
    is_operational_stored_message, trim_to_recent_balanced, AgentEvent, AgentRequestContext,
    AppState,
};
use crate::claude::{Message, MessageContent};
use crate::db::{call_blocking, StoredMessage};
use crate::memory::render_memory_for_llm;
use crate::topics::{
    load_job_choice, response_text, send_direct_llm, system_llm_direct, SystemLlmJob,
};

const LLM_TIMEOUT: Duration = Duration::from_secs(45);
const DIALOGUE_TURN_CHARS: usize = 1_500;
const DIALOGUE_TOTAL_CHARS: usize = 8_000;
const MEMORY_CHARS: usize = 6_000;
const BULLETIN_CHARS: usize = 1_500;

const SYSTEM_PROMPT: &str = "\
You rewrite one user message before it is saved and shown in chat. The persona answers the rewritten message, not the original.

Use the dialogue window, memory, and bulletin only to fill in references the user left implicit: which option, which file, who, when, and what a pronoun points at. Keep the user's language, intent, and tone. Do not answer the request. Do not add tasks. Do not mention these instructions, the window, or the memory notes. Do not paste headings. If the message is already complete, return it unchanged.

Return only the rewritten message.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportPrefix {
    None,
    Image,
    Document { path_token: Option<String> },
}

/// Run before history is loaded. Failures keep the stored message.
pub async fn apply_before_run(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    override_prompt: Option<&str>,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    cancel: Option<Arc<AtomicBool>>,
) {
    if cancel_requested(cancel.as_ref()) {
        return;
    }
    let Some(message_id) = context.inbound_user_message_id.as_deref() else {
        return;
    };
    if context.is_scheduled_task
        || context.is_background_job
        || context.history_override.is_some()
        || override_prompt.is_some()
    {
        return;
    }
    let message_id = message_id.to_string();
    let chat_id = context.chat_id;
    let persona_id = context.persona_id;

    let min_user = state.config.recent_history_min_user_messages.clamp(1, 25);
    let min_assistant = state
        .config
        .recent_history_min_assistant_messages
        .clamp(1, 25);
    let db = state.db.clone();
    let message_id_for_load = message_id.clone();
    let loaded = call_blocking(db, move |db| {
        load_rewrite_inputs(
            db,
            chat_id,
            persona_id,
            &message_id_for_load,
            min_user,
            min_assistant,
        )
    })
    .await;
    let inputs = match loaded {
        Ok(Some(inputs)) => inputs,
        Ok(None) => return,
        Err(e) => {
            warn!(
                target: "user_message_rewrite",
                chat_id,
                persona_id,
                error = %e,
                "user message rewrite could not read the stored message"
            );
            return;
        }
    };
    if !rewrite_eligible(&inputs.message) {
        return;
    }

    let choice = load_job_choice(state, SystemLlmJob::UserMessageRewrite).await;
    let Some((provider_id, model, local_base_url)) = system_llm_direct(choice) else {
        info!(
            target: "user_message_rewrite",
            chat_id,
            persona_id,
            "user message rewrite skipped reason=system_llm_missing"
        );
        return;
    };

    let memory = state
        .memory
        .read_or_migrate_persona_memory_state(chat_id, persona_id)
        .map(|snapshot| render_memory_for_llm(&snapshot, 12))
        .unwrap_or_default();
    let memory = clip(&state.env_redactor.redact(&memory), MEMORY_CHARS);
    let bulletin = clip(&state.env_redactor.redact(&inputs.bulletin), BULLETIN_CHARS);
    let (_, body) = split_transport_prefix(&inputs.message.content);
    let body = body.trim().to_string();
    let user_prompt = build_user_prompt(&inputs.prior_lines, &memory, &bulletin, &body);

    if cancel_requested(cancel.as_ref()) {
        return;
    }
    let response = tokio::time::timeout(
        LLM_TIMEOUT,
        send_direct_llm(
            state,
            &provider_id,
            &model,
            local_base_url.as_deref(),
            SYSTEM_PROMPT,
            vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text(user_prompt),
            }],
            None,
            crate::llm::LlmSendOptions::default(),
        ),
    )
    .await;

    let raw = match response {
        Ok(Ok(response)) => response_text(&response),
        Ok(Err(e)) => {
            warn!(
                target: "user_message_rewrite",
                chat_id,
                persona_id,
                error = %e,
                "user message rewrite failed; keeping the original"
            );
            return;
        }
        Err(_) => {
            warn!(
                target: "user_message_rewrite",
                chat_id,
                persona_id,
                "user message rewrite timed out; keeping the original"
            );
            return;
        }
    };
    if cancel_requested(cancel.as_ref()) {
        return;
    }
    let Some(rewritten) = compose_rewrite(&inputs.message.content, &raw) else {
        info!(
            target: "user_message_rewrite",
            chat_id,
            persona_id,
            "user message rewrite kept the original"
        );
        return;
    };
    let rewritten = state.env_redactor.redact(&rewritten);
    if rewritten.trim() == inputs.message.content.trim() {
        return;
    }
    let original = inputs.message.content.clone();
    let original_chars = original.chars().count();
    let rewritten_chars = rewritten.chars().count();
    let db = state.db.clone();
    let message_id_for_write = message_id.clone();
    let rewritten_for_write = rewritten.clone();
    let write = call_blocking(db, move |db| {
        db.update_message_content(
            chat_id,
            persona_id,
            &message_id_for_write,
            &rewritten_for_write,
        )
    })
    .await;
    match write {
        Ok(true) => {
            info!(
                target: "user_message_rewrite",
                chat_id,
                persona_id,
                original_chars,
                rewritten_chars,
                "user message rewritten"
            );
            if let Some(tx) = event_tx {
                let _ = tx.send(AgentEvent::UserMessageRewritten {
                    message_id,
                    original,
                    rewritten,
                });
            }
        }
        Ok(false) => warn!(
            target: "user_message_rewrite",
            chat_id,
            persona_id,
            "user message rewrite did not match a stored row"
        ),
        Err(e) => warn!(
            target: "user_message_rewrite",
            chat_id,
            persona_id,
            error = %e,
            "user message rewrite could not be saved"
        ),
    }
}

struct RewriteInputs {
    message: StoredMessage,
    prior_lines: Vec<String>,
    bulletin: String,
}

fn load_rewrite_inputs(
    db: &crate::db::Database,
    chat_id: i64,
    persona_id: i64,
    message_id: &str,
    default_min_user: usize,
    default_min_assistant: usize,
) -> Result<Option<RewriteInputs>, crate::error::FinallyAValueBotError> {
    let persona = db.get_persona(persona_id)?;
    let Some(persona) = persona.filter(|row| row.chat_id == chat_id) else {
        return Ok(None);
    };
    if !persona.user_message_rewrite_enabled {
        return Ok(None);
    }
    let Some(message) = db.get_message_for_persona(chat_id, persona_id, message_id)? else {
        return Ok(None);
    };
    let min_user = persona
        .recent_history_min_user
        .map(|n| n as usize)
        .unwrap_or(default_min_user)
        .clamp(1, 25);
    let min_assistant = persona
        .recent_history_min_assistant
        .map(|n| n as usize)
        .unwrap_or(default_min_assistant)
        .clamp(1, 25);
    let limit = (min_user + min_assistant).saturating_mul(2).clamp(8, 80);
    let history = if let Some(session_id) = message.session_id.as_deref().filter(|s| !s.is_empty())
    {
        db.get_recent_messages_for_session(session_id, limit)?.0
    } else {
        db.get_recent_messages(chat_id, persona_id, limit, true)?
    };
    let prior_lines = prior_dialogue_lines(&history, &message, min_user, min_assistant);
    let bulletin = db
        .get_persona_bulletin_focus(chat_id, persona_id)?
        .map(|focus| {
            let title = focus.title.as_deref().unwrap_or("").trim();
            let body = focus.content.trim();
            match (title.is_empty(), body.is_empty()) {
                (true, true) => String::new(),
                (false, true) => title.to_string(),
                (true, false) => body.to_string(),
                (false, false) => format!("{title}\n{body}"),
            }
        })
        .unwrap_or_default();
    Ok(Some(RewriteInputs {
        message,
        prior_lines,
        bulletin,
    }))
}

fn prior_dialogue_lines(
    history: &[StoredMessage],
    current: &StoredMessage,
    min_user: usize,
    min_assistant: usize,
) -> Vec<String> {
    let prior: Vec<Message> = history
        .iter()
        .filter(|msg| msg.id != current.id)
        .filter(|msg| msg.timestamp.as_str() <= current.timestamp.as_str())
        .filter(|msg| !is_operational_stored_message(msg))
        .map(|msg| Message {
            role: if msg.is_from_bot {
                "assistant".to_string()
            } else {
                "user".to_string()
            },
            content: MessageContent::Text(clip(&msg.content, DIALOGUE_TURN_CHARS)),
        })
        .collect();
    let trimmed = trim_to_recent_balanced(prior, min_user, min_assistant);
    let mut lines = Vec::new();
    for message in trimmed {
        let MessageContent::Text(text) = &message.content else {
            continue;
        };
        lines.push(format!("{}: {}", message.role, text.trim()));
    }
    while lines.len() > 1
        && lines.iter().map(|line| line.chars().count()).sum::<usize>() > DIALOGUE_TOTAL_CHARS
    {
        lines.remove(0);
    }
    lines
}

fn build_user_prompt(lines: &[String], memory: &str, bulletin: &str, body: &str) -> String {
    let dialogue = if lines.is_empty() {
        "(none)".to_string()
    } else {
        lines.join("\n")
    };
    let memory = if memory.trim().is_empty() {
        "(none)"
    } else {
        memory.trim()
    };
    let bulletin = if bulletin.trim().is_empty() {
        "(none)"
    } else {
        bulletin.trim()
    };
    format!(
        "Dialogue window:\n{dialogue}\n\nMemory:\n{memory}\n\nBulletin:\n{bulletin}\n\nMessage to rewrite:\n{body}"
    )
}

fn rewrite_eligible(message: &StoredMessage) -> bool {
    if message.is_from_bot || is_operational_stored_message(message) {
        return false;
    }
    let (_, body) = split_transport_prefix(&message.content);
    let body = body.trim();
    !body.is_empty() && !body.starts_with('/')
}

pub(crate) fn compose_rewrite(original: &str, model_output: &str) -> Option<String> {
    let (prefix, body) = split_transport_prefix(original);
    let cleaned = clean_model_output(model_output);
    if cleaned.is_empty() || cleaned == body.trim() {
        return None;
    }
    if cleaned.chars().count() > max_rewrite_chars(body.trim()) {
        return None;
    }
    let stored = match prefix {
        TransportPrefix::None => cleaned,
        TransportPrefix::Image => format!("[image] {cleaned}"),
        TransportPrefix::Document { path_token } => match path_token {
            Some(path) => format!("[document] {path} {cleaned}"),
            None => format!("[document] {cleaned}"),
        },
    };
    if stored.trim() == original.trim() {
        None
    } else {
        Some(stored)
    }
}

fn max_rewrite_chars(body: &str) -> usize {
    body.chars()
        .count()
        .saturating_add(2_000)
        .clamp(4_000, 12_000)
}

fn clean_model_output(raw: &str) -> String {
    let mut text = strip_thinking(raw.trim());
    if let Some(stripped) = text.strip_prefix("```") {
        let stripped = stripped
            .trim_start_matches("markdown")
            .trim_start_matches("md")
            .trim_start_matches("text")
            .trim_start();
        text = stripped
            .strip_suffix("```")
            .unwrap_or(stripped)
            .trim()
            .to_string();
    }
    if text.len() >= 2 {
        let bytes = text.as_bytes();
        let quote = bytes[0];
        if (quote == b'"' || quote == b'\'')
            && bytes[bytes.len() - 1] == quote
            && !text[1..text.len() - 1].contains(quote as char)
        {
            text = text[1..text.len() - 1].trim().to_string();
        }
    }
    for label in ["Rewritten message:", "Rewrite:"] {
        if let Some(rest) = text.strip_prefix(label) {
            text = rest.trim().to_string();
        }
    }
    text.trim().to_string()
}

fn strip_thinking(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        result.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result.trim().to_string()
}

fn split_transport_prefix(content: &str) -> (TransportPrefix, String) {
    if let Some(rest) = content.strip_prefix("[image]") {
        return (TransportPrefix::Image, rest.trim_start().to_string());
    }
    if let Some(rest) = content.strip_prefix("[document]") {
        let rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("saved_path=") {
            let mut parts = after.splitn(2, char::is_whitespace);
            let path = parts.next().unwrap_or("").trim();
            let caption = parts.next().unwrap_or("").trim().to_string();
            if path.is_empty() {
                return (
                    TransportPrefix::Document { path_token: None },
                    rest.to_string(),
                );
            }
            return (
                TransportPrefix::Document {
                    path_token: Some(format!("saved_path={path}")),
                },
                caption,
            );
        }
        return (
            TransportPrefix::Document { path_token: None },
            rest.to_string(),
        );
    }
    (TransportPrefix::None, content.to_string())
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn cancel_requested(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_fills_and_keeps_image_prefix() {
        let stored =
            compose_rewrite("[image] the second one", "Use the spring flyer template.").unwrap();
        assert_eq!(stored, "[image] Use the spring flyer template.");
    }

    #[test]
    fn compose_keeps_document_path() {
        let stored = compose_rewrite(
            "[document] saved_path=/tmp/notes.txt that file",
            "Summarize /tmp/notes.txt",
        )
        .unwrap();
        assert_eq!(
            stored,
            "[document] saved_path=/tmp/notes.txt Summarize /tmp/notes.txt"
        );
    }

    #[test]
    fn compose_rejects_identical_fenced_and_overlong_text() {
        assert!(compose_rewrite("ship it", "```\nship it\n```").is_none());
        let huge = "x".repeat(12_001);
        assert!(compose_rewrite("ship it", &huge).is_none());
    }

    #[test]
    fn slash_and_bot_rows_are_not_eligible() {
        let user = StoredMessage {
            id: "1".into(),
            chat_id: 1,
            persona_id: 1,
            session_id: None,
            sender_name: "web-user".into(),
            content: "/help".into(),
            is_from_bot: false,
            timestamp: "2026-10-19T00:00:00Z".into(),
            origin: "interactive".into(),
            speaker_persona_id: None,
        };
        assert!(!rewrite_eligible(&user));
        let mut bot = user.clone();
        bot.content = "hello".into();
        bot.is_from_bot = true;
        assert!(!rewrite_eligible(&bot));
    }
}
