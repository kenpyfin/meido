//! Session goal extraction for the current agent turn (`[current_request]`).

use crate::claude::{ContentBlock, Message, MessageContent};
use crate::safety_redaction::EnvSecretRedactor;

/// Primary task for this turn — aligned with task-first context restructuring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGoalContext {
    pub current_request: String,
    pub is_short_reply: bool,
    pub disambiguation_assistant: Option<String>,
}

const SHORT_REPLY_MAX_CHARS: usize = 40;

/// A web/Telegram reply-to block stored at the start of the user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotedMessageRef {
    pub id: String,
    pub role: String,
    pub sender: String,
    pub content: String,
    pub follow_up: String,
}

/// Parse `[quoted_message …]…[/quoted_message]` plus optional follow-up text.
pub fn parse_quoted_message_block(text: &str) -> Option<QuotedMessageRef> {
    let text = text.trim();
    const PREFIX: &str = "[quoted_message ";
    const CLOSE: &str = "[/quoted_message]";
    if !text.starts_with(PREFIX) {
        return None;
    }
    let after_prefix = &text[PREFIX.len()..];
    let tag_end = after_prefix.find(']')?;
    let attrs = &after_prefix[..tag_end];
    let rest = after_prefix[tag_end + 1..].trim_start();
    let close_idx = rest.find(CLOSE)?;
    let content = rest[..close_idx].trim().to_string();
    let follow_up = rest[close_idx + CLOSE.len()..].trim().to_string();
    Some(QuotedMessageRef {
        id: extract_quoted_attr(attrs, "id").unwrap_or_default(),
        role: extract_quoted_attr(attrs, "role").unwrap_or_else(|| "assistant".into()),
        sender: extract_quoted_attr(attrs, "sender").unwrap_or_default(),
        content,
        follow_up,
    })
}

fn extract_quoted_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
            continue;
        }
        if c == '"' {
            break;
        }
        out.push(c);
    }
    Some(out)
}

/// True when history text is scheduler/shell plumbing, not an interactive reply to latch onto.
pub fn is_non_interactive_history_text(text: &str) -> bool {
    let unwrapped = strip_stored_dialogue_markup(text);
    let t = unwrapped.trim();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    t.starts_with("##INTERNAL_SHELL_SUCCESS_FOLLOWUP##")
        || t.starts_with("##INTERNAL_SHELL_FAILURE_RETRY##")
        || t.starts_with("[scheduler]")
        || t.starts_with("[scheduler]:")
        || t.starts_with("Background command started (job `")
        || t.starts_with("Background command cancelled (job `")
        || t.starts_with("Background command could not be started (job `")
        || t.starts_with("Your background command finished.")
        || t.starts_with("The background command failed.")
        || t.starts_with("Background job `")
        || lower.starts_with("scheduled daily")
}

/// Parse body from `[current_request]...[/current_request]`.
pub fn parse_current_request_content(text: &str) -> Option<String> {
    let text = text.trim();
    if !text.starts_with("[current_request") {
        return None;
    }
    let body_start = text.find(']')? + 1;
    let body_end = text.rfind("[/current_request]")?;
    if body_end <= body_start {
        return None;
    }
    Some(text[body_start..body_end].trim().to_string())
}

fn text_from_message_content(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn parse_wrapped_user_message(text: &str) -> Option<(String, Option<String>, String)> {
    let text = text.trim();
    const PREFIX: &str = "<user_message";
    const SUFFIX: &str = "</user_message>";
    if !text.starts_with(PREFIX) || !text.ends_with(SUFFIX) {
        return None;
    }
    let tag_body = text[PREFIX.len()..text.len() - SUFFIX.len()].trim_start();
    let close = tag_body.find('>')?;
    let attrs = &tag_body[..close];
    let content = tag_body[close + 1..].to_string();
    let sender = extract_xml_attr(attrs, "sender")?;
    let at = extract_xml_attr(attrs, "at");
    Some((sender, at, content))
}

fn parse_wrapped_assistant_message(text: &str) -> Option<String> {
    let text = text.trim();
    const PREFIX: &str = "<assistant_message";
    const SUFFIX: &str = "</assistant_message>";
    if !text.starts_with(PREFIX) {
        return None;
    }
    let tag_body = if text.ends_with(SUFFIX) {
        &text[PREFIX.len()..text.len() - SUFFIX.len()]
    } else {
        &text[PREFIX.len()..]
    };
    let tag_body = tag_body.trim_start();
    let close = tag_body.find('>')?;
    let mut content = tag_body[close + 1..].to_string();
    if let Some(suffix_pos) = content.rfind(SUFFIX) {
        content.truncate(suffix_pos);
    }
    Some(unescape_xml_entities(content.trim()))
}

fn unescape_xml_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Remove LLM-history XML wrappers accidentally echoed into user-visible chat text.
pub fn strip_stored_dialogue_markup(text: &str) -> String {
    let mut t = text.trim().to_string();
    for _ in 0..3 {
        if let Some(inner) = parse_wrapped_assistant_message(&t) {
            return inner;
        }
        if let Some((_, _, inner)) = parse_wrapped_user_message(&t) {
            return unescape_xml_entities(inner.trim());
        }
        if let Some(next) = strip_one_leading_persona_bracket(&t) {
            if next == t {
                return t;
            }
            t = next;
            continue;
        }
        return t;
    }
    t
}

fn strip_one_leading_persona_bracket(text: &str) -> Option<String> {
    let rest = text.trim_start();
    if !rest.starts_with('[') {
        return None;
    }
    let close = rest.find(']')?;
    let token = &rest[1..close];
    if token.is_empty() || token.len() > 64 || token.contains('\n') {
        return None;
    }
    Some(rest[close + 1..].trim_start().to_string())
}

fn extract_xml_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn latest_user_text_fallback(messages: &[Message]) -> String {
    for m in messages.iter().rev() {
        if m.role != "user" {
            continue;
        }
        let text = text_from_message_content(&m.content);
        if let Some(inner) = parse_current_request_content(&text) {
            return inner;
        }
        if let Some((_, _, inner)) = parse_wrapped_user_message(&text) {
            return inner;
        }
        if let Some(rest) = text.strip_prefix("[scheduler]: ") {
            return rest.to_string();
        }
        if !text.starts_with('[') {
            return text;
        }
    }
    String::new()
}

fn assistant_text_from_message(msg: &Message) -> String {
    if msg.role != "assistant" {
        return String::new();
    }
    text_from_message_content(&msg.content)
}

/// Heuristic: short follow-up that needs prior assistant context to interpret.
pub fn is_short_reply_request(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.chars().count() <= SHORT_REPLY_MAX_CHARS {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    const PHRASES: &[&str] = &[
        "yes",
        "no",
        "yep",
        "nope",
        "ok",
        "okay",
        "sure",
        "use the",
        "use option",
        "option a",
        "option b",
        "the first",
        "the second",
        "go ahead",
        "do it",
        "retry",
        "wait",
    ];
    PHRASES
        .iter()
        .any(|p| lower == *p || lower.starts_with(&format!("{p} ")))
}

fn truncate_for_eval(text: &str, max_chars: usize, redactor: Option<&EnvSecretRedactor>) -> String {
    let t = if let Some(r) = redactor {
        r.redact(text)
    } else {
        text.to_string()
    };
    if t.chars().count() <= max_chars {
        t
    } else {
        format!("{}...", t.chars().take(max_chars).collect::<String>())
    }
}

/// Extract the session goal from the message list the agent loop used.
pub fn extract_session_goal(
    messages: &[Message],
    redactor: Option<&EnvSecretRedactor>,
) -> SessionGoalContext {
    let mut current_request = String::new();
    let mut current_request_idx = None;

    for (idx, msg) in messages.iter().enumerate().rev() {
        if msg.role != "user" {
            continue;
        }
        let text = text_from_message_content(&msg.content);
        if let Some(body) = parse_current_request_content(&text) {
            current_request = body;
            current_request_idx = Some(idx);
            break;
        }
    }

    if current_request.is_empty() {
        current_request = latest_user_text_fallback(messages);
    }

    let quoted = parse_quoted_message_block(&current_request);
    let is_short_reply = quoted
        .as_ref()
        .map(|q| q.follow_up.trim().is_empty() || is_short_reply_request(&q.follow_up))
        .unwrap_or_else(|| is_short_reply_request(&current_request));

    let disambiguation_assistant =
        if let Some(q) = quoted.as_ref().filter(|q| q.role == "assistant") {
            if q.content.trim().is_empty() {
                None
            } else {
                Some(truncate_for_eval(&q.content, 800, redactor))
            }
        } else if is_short_reply {
            current_request_idx.and_then(|idx| {
                messages[..idx].iter().rev().find_map(|m| {
                    let t = assistant_text_from_message(m);
                    if t.trim().is_empty() || is_non_interactive_history_text(&t) {
                        None
                    } else {
                        Some(truncate_for_eval(&t, 800, redactor))
                    }
                })
            })
        } else {
            None
        };

    SessionGoalContext {
        current_request: truncate_for_eval(&current_request, 2000, redactor),
        is_short_reply,
        disambiguation_assistant,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_current_request() {
        let t = "[current_request sender=\"alice\"]\nfix the bug\n[/current_request]";
        assert_eq!(
            parse_current_request_content(t).as_deref(),
            Some("fix the bug")
        );
    }

    #[test]
    fn extract_session_goal_from_current_request() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\" sender=\"u\">old</user_message>".into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">pick A or B?</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\nB\n[/current_request]".into(),
                ),
            },
        ];
        let goal = extract_session_goal(&messages, None);
        assert_eq!(goal.current_request, "B");
        assert!(goal.is_short_reply);
        assert!(goal
            .disambiguation_assistant
            .as_ref()
            .unwrap()
            .contains("A or B"));
    }

    #[test]
    fn is_short_reply_detects_yes() {
        assert!(is_short_reply_request("yes"));
        assert!(!is_short_reply_request(
            "please regenerate the image with the blue background"
        ));
    }

    #[test]
    fn parse_quoted_message_block_extracts_follow_up() {
        let raw = "[quoted_message id=\"abc\" role=\"assistant\" sender=\"bot\"]\nV5 is not a keep.\n[/quoted_message]\n\nV6";
        let q = parse_quoted_message_block(raw).expect("quote");
        assert_eq!(q.id, "abc");
        assert_eq!(q.role, "assistant");
        assert_eq!(q.content, "V5 is not a keep.");
        assert_eq!(q.follow_up, "V6");
    }

    #[test]
    fn extract_session_goal_prefers_quoted_assistant() {
        let messages = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">Scheduled daily PZ pipeline is the task.</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\n[quoted_message id=\"x\" role=\"assistant\" sender=\"bot\"]\nFort Mason FILL V5\n[/quoted_message]\n\nV6\n[/current_request]"
                        .into(),
                ),
            },
        ];
        let goal = extract_session_goal(&messages, None);
        assert!(goal.is_short_reply);
        assert!(goal
            .disambiguation_assistant
            .as_ref()
            .unwrap()
            .contains("Fort Mason FILL V5"));
        assert!(!goal
            .disambiguation_assistant
            .as_ref()
            .unwrap()
            .contains("Scheduled daily"));
    }

    #[test]
    fn extract_session_goal_skips_scheduled_assistant() {
        let messages = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">FILL V13 keeps the whole-skirt mask.</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">Scheduled daily PZ pipeline is the task.</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\nV6\n[/current_request]".into(),
                ),
            },
        ];
        let goal = extract_session_goal(&messages, None);
        assert!(goal.is_short_reply);
        let d = goal.disambiguation_assistant.unwrap();
        assert!(d.contains("FILL V13"));
        assert!(!d.contains("Scheduled daily"));
    }

    #[test]
    fn strip_stored_dialogue_markup_unwraps_assistant_echo() {
        let raw = "<assistant_message context=\"prior_turn\" at=\"2026-06-17T22:58:05.123456789+00:00\">Hello &amp; welcome</assistant_message>";
        assert_eq!(strip_stored_dialogue_markup(raw), "Hello & welcome");
    }

    #[test]
    fn strip_stored_dialogue_markup_leaves_normal_text() {
        assert_eq!(strip_stored_dialogue_markup("plain reply"), "plain reply");
    }
}
