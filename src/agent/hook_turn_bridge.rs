//! Shared bot-native hook dispatch at turn boundaries (BeforeTurn, PreStop).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::claude::{ContentBlock, Message, MessageContent};
use crate::db::call_blocking;
use crate::hook_runtime::{run_hooks_for_event_async, HookEventName, HookRunInput, HookRunResult};

use super::{AgentEvent, AgentRequestContext, AppState};

pub const DEFERRED_COMMITMENT_MAX_NUDGES: usize = 2;

#[derive(Debug, Clone)]
pub struct TurnHookOutcome {
    pub result: HookRunResult,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreStopFollowUp {
    Proceed,
    Nudge { prompt: String },
    BlockFinal { message: String },
}

pub fn hook_event_summary(
    event_name: HookEventName,
    tool_name: Option<&str>,
    result: &HookRunResult,
) -> Option<String> {
    if result.matched_hook_ids.is_empty()
        && result.blocked_reason.is_none()
        && result.additional_contexts.is_empty()
        && result.updated_assistant_text.is_none()
    {
        return None;
    }
    let tool = tool_name.unwrap_or("-");
    let blocked = result.blocked_reason.as_deref().unwrap_or("-");
    Some(format!(
        "{} tool={} matched={:?} blocked={} add_ctx={}",
        event_name.as_str(),
        tool,
        result.matched_hook_ids,
        blocked,
        result.additional_contexts.len()
    ))
}

pub async fn publish_hook_event(
    state: &AppState,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    run_key: &str,
    chat_id: i64,
    persona_id: i64,
    event_name: HookEventName,
    tool_name: Option<&str>,
    result: &HookRunResult,
) {
    if let Some(tx) = event_tx {
        let _ = tx.send(AgentEvent::Hook {
            event_name: event_name.as_str().to_string(),
            tool_name: tool_name.map(|s| s.to_string()),
            matched_hook_ids: result.matched_hook_ids.clone(),
            blocked_reason: result.blocked_reason.clone(),
            additional_context_count: result.additional_contexts.len(),
        });
    }
    let payload = json!({
        "event_name": event_name.as_str(),
        "tool_name": tool_name,
        "matched_hook_ids": result.matched_hook_ids,
        "blocked_reason": result.blocked_reason,
        "additional_context_count": result.additional_contexts.len(),
    })
    .to_string();
    let run_key = run_key.to_string();
    let _ = call_blocking(state.db.clone(), move |db| {
        db.append_run_timeline_event(&run_key, chat_id, persona_id, "hook", Some(&payload))
    })
    .await;
}

pub async fn run_before_turn_hooks(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    run_key: &str,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
) -> anyhow::Result<TurnHookOutcome> {
    let chat_id = context.chat_id;
    let persona_id = context.persona_id;
    let result = run_hooks_for_event_async(
        state.db.clone(),
        &state.config,
        state.env_redactor.as_ref(),
        HookEventName::BeforeTurn,
        &HookRunInput {
            chat_id,
            persona_id,
            caller_channel: context.caller_channel.to_string(),
            is_scheduled_task: context.is_scheduled_task,
            ..HookRunInput::default()
        },
    )
    .await?;
    publish_hook_event(
        state,
        event_tx,
        run_key,
        chat_id,
        persona_id,
        HookEventName::BeforeTurn,
        None,
        &result,
    )
    .await;
    let summary = hook_event_summary(HookEventName::BeforeTurn, None, &result);
    Ok(TurnHookOutcome { result, summary })
}

pub fn build_pre_stop_runtime_signals(
    stop_reason: &str,
    assistant_text: &str,
    messages: &[Message],
    nudge_count: usize,
    can_continue: bool,
) -> Value {
    json!({
        "deferred_commitment_should_reject": stop_reason != "ask_clarification"
            && !assistant_text.trim().is_empty()
            && should_reject_premature_end_turn(assistant_text, messages),
        "deferred_commitment_nudges": nudge_count,
        "deferred_commitment_max_nudges": DEFERRED_COMMITMENT_MAX_NUDGES,
        "can_continue_iteration": can_continue,
    })
}

pub async fn run_pre_stop_hooks(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    run_key: &str,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    stop_reason: &str,
    assistant_text: &str,
    messages: &[Message],
    nudge_count: usize,
    can_continue: bool,
) -> anyhow::Result<TurnHookOutcome> {
    let chat_id = context.chat_id;
    let persona_id = context.persona_id;
    let result = run_hooks_for_event_async(
        state.db.clone(),
        &state.config,
        state.env_redactor.as_ref(),
        HookEventName::PreStop,
        &HookRunInput {
            chat_id,
            persona_id,
            caller_channel: context.caller_channel.to_string(),
            is_scheduled_task: context.is_scheduled_task,
            stop_reason: Some(stop_reason.to_string()),
            assistant_text: Some(assistant_text.to_string()),
            runtime_signals: Some(build_pre_stop_runtime_signals(
                stop_reason,
                assistant_text,
                messages,
                nudge_count,
                can_continue,
            )),
            ..HookRunInput::default()
        },
    )
    .await?;
    publish_hook_event(
        state,
        event_tx,
        run_key,
        chat_id,
        persona_id,
        HookEventName::PreStop,
        None,
        &result,
    )
    .await;
    let summary = hook_event_summary(HookEventName::PreStop, None, &result);
    Ok(TurnHookOutcome { result, summary })
}

pub fn append_hook_context_messages(messages: &mut Vec<Message>, contexts: &[String]) {
    if contexts.is_empty() {
        return;
    }
    messages.push(Message {
        role: "user".into(),
        content: MessageContent::Text(format!("[hook_context]\n{}", contexts.join("\n"))),
    });
}

pub fn pre_stop_follow_up(result: &HookRunResult, nudge_count: usize) -> PreStopFollowUp {
    if let Some(reason) = &result.blocked_reason {
        let reason_for_prompt = reason
            .strip_prefix("deferred_commitment: ")
            .unwrap_or(reason.as_str());
        if reason.starts_with("deferred_commitment:")
            && nudge_count < DEFERRED_COMMITMENT_MAX_NUDGES
        {
            return PreStopFollowUp::Nudge {
                prompt: format!("[hook_pre_stop_blocked]\n{reason_for_prompt}"),
            };
        }
        return PreStopFollowUp::BlockFinal {
            message: format!("I cannot finish this run: {reason_for_prompt}"),
        };
    }
    if !result.additional_contexts.is_empty() && nudge_count < DEFERRED_COMMITMENT_MAX_NUDGES {
        return PreStopFollowUp::Nudge {
            prompt: format!("[hook_context]\n{}", result.additional_contexts.join("\n")),
        };
    }
    PreStopFollowUp::Proceed
}

pub fn should_reject_premature_end_turn(assistant_text: &str, messages: &[Message]) -> bool {
    if !assistant_text_defers_work(assistant_text) {
        return false;
    }
    most_recent_tool_result_is_error(messages)
        || assistant_text_references_incomplete_work(assistant_text)
}

pub(crate) fn assistant_text_defers_work(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const PHRASES: &[&str] = &[
        "checking the log",
        "checking run log",
        "check the log",
        "check run log",
        "i will ",
        "i'll ",
        "i am going to ",
        "i'm going to ",
        "im going to ",
        "let me ",
        "one moment",
        "(checking",
        "right now",
        "immediately",
        "in a moment",
        "give me a moment",
        "hold on",
    ];
    PHRASES.iter().any(|p| lower.contains(p))
}

fn most_recent_tool_result_is_error(messages: &[Message]) -> bool {
    for msg in messages.iter().rev() {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks.iter().rev() {
                if let ContentBlock::ToolResult { is_error, .. } = block {
                    return *is_error == Some(true);
                }
            }
        }
    }
    false
}

fn assistant_text_references_incomplete_work(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("background task")
        || lower.contains("agent run")
        || lower.contains("cursor-agent")
        || lower.contains("cursor agent")
        || lower.contains("run #")
        || lower.contains("run logs")
        || lower.contains("the log")
        || lower.contains("not successfully")
        || lower.contains("no such file")
        || lower.contains("was not created")
        || lower.contains("was not found")
    {
        return true;
    }
    static RUN_ID_RE: OnceLock<Regex> = OnceLock::new();
    let re = RUN_ID_RE.get_or_init(|| Regex::new(r"#\d{1,6}\b").expect("run id regex"));
    re.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_stop_follow_up_nudges_on_deferred_commitment() {
        let result = HookRunResult {
            blocked_reason: Some(
                "deferred_commitment: You ended your turn while promising further work.".into(),
            ),
            ..HookRunResult::default()
        };
        let follow_up = pre_stop_follow_up(&result, 0);
        assert!(matches!(follow_up, PreStopFollowUp::Nudge { .. }));
    }

    #[test]
    fn pre_stop_follow_up_blocks_when_nudges_exhausted() {
        let result = HookRunResult {
            blocked_reason: Some("deferred_commitment: still deferring".into()),
            ..HookRunResult::default()
        };
        let follow_up = pre_stop_follow_up(&result, DEFERRED_COMMITMENT_MAX_NUDGES);
        assert!(matches!(follow_up, PreStopFollowUp::BlockFinal { .. }));
    }

    #[test]
    fn should_reject_premature_end_turn_detects_deferred_work() {
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text("status?".into()),
        }];
        assert!(should_reject_premature_end_turn(
            "Let me check the logs one moment.",
            &messages
        ));
    }
}
