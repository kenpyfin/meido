//! Post-Tool Evaluator (PTE): evaluates whether a task is complete after tool execution.
//! Called after each tool iteration on the Classic / ClassicCostRouting agent loop only
//! (Cursor and Gemini ADK do not use this path).
//! Decides whether to continue the agent loop, ask the user, or synthesize a final response.
//! Doubt defaults to `ask_user` (clarification), not another tool round.

use crate::agent_turn_context::extract_session_goal;
use crate::claude::{ContentBlock, Message, MessageContent, ResponseContentBlock};
use crate::config::Config;
use crate::error::FinallyAValueBotError;
use crate::llm::{self, EVALUATOR_TIMEOUT_SECS};
use crate::safety_redaction::EnvSecretRedactor;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PteAction {
    Continue,
    Complete,
    AskUser,
    HandoffBackground,
    StopWithSummary,
}

#[derive(Debug, Clone)]
pub struct PteResult {
    pub action: PteAction,
    pub reason: String,
    pub provider_label: Option<String>,
    pub disabled: bool,
    pub provider_skipped: bool,
}

pub fn pte_action_name(action: &PteAction) -> &'static str {
    match action {
        PteAction::Continue => "continue",
        PteAction::Complete => "complete",
        PteAction::AskUser => "ask_user",
        PteAction::HandoffBackground => "handoff_background",
        PteAction::StopWithSummary => "stop_with_summary",
    }
}

/// User-visible reply when PTE chooses `ask_user`.
pub fn format_pte_ask_user_reply(reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        return "I need a clarification before continuing. What should I do next?".into();
    }
    let lower = reason.to_ascii_lowercase();
    if lower.contains("stalled") {
        return format!(
            "I paused because progress is stalled: {reason} Choose: retry now, wait, or adjust the request."
        );
    }
    if reason.ends_with('?') {
        reason.to_string()
    } else {
        format!("I need a clarification before continuing: {reason}")
    }
}

/// Build the PTE system prompt with principles and memory context baked in.
fn build_pte_system_prompt(principles_content: &str, memory_context: &str) -> String {
    let mut prompt = String::from(
        r#"You are a task-completion evaluator. Judge whether the session goal (`current_request`) is fulfilled by tool results so far.

Output JSON only: {"action": "continue" | "complete" | "ask_user", "reason": "brief rationale or a direct question for the user"}

Rules:
- "complete" = the session goal (`current_request`) is fulfilled by tool results
- "continue" = a clear next tool step will make progress toward that goal
- "ask_user" = missing critical information, blocked, ambiguous, or no clear next step without guessing. Put a direct clarification question in reason
- Prior turns and bulletin/memory are background — judge only `current_request`
- Consider principles as constraints only
- If in doubt, say "ask_user" (do not keep calling tools)
- Keep reason concise (one sentence)
"#,
    );

    if !principles_content.trim().is_empty() {
        prompt.push_str("\n# Principles\n\n");
        prompt.push_str(principles_content);
        prompt.push_str("\n");
    }

    if !memory_context.trim().is_empty() {
        prompt.push_str("\n# Memory Context\n\n");
        prompt.push_str(memory_context);
        prompt.push_str("\n");
    }

    prompt
}

/// Build a summary of the most recent tool calls and results.
pub fn build_tool_results_summary(
    env_redactor: &EnvSecretRedactor,
    messages: &[Message],
    max_messages: usize,
) -> String {
    let mut out = String::new();
    let start = messages.len().saturating_sub(max_messages);

    for msg in messages.iter().skip(start) {
        match &msg.content {
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { name, input, .. } => {
                            let input_str =
                                serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                            let input_preview = if input_str.len() > 200 {
                                format!("{}...", &input_str[..200])
                            } else {
                                input_str
                            };
                            out.push_str(&format!(
                                "Tool called: {} with {}\n",
                                name,
                                env_redactor.redact(&input_preview)
                            ));
                        }
                        ContentBlock::ToolResult {
                            content, is_error, ..
                        } => {
                            let status = if is_error.unwrap_or(false) {
                                "ERROR"
                            } else {
                                "OK"
                            };
                            let preview = if content.chars().count() > 300 {
                                format!("{}...", content.chars().take(300).collect::<String>())
                            } else {
                                content.clone()
                            };
                            out.push_str(&format!(
                                "Result ({}): {}\n",
                                status,
                                env_redactor.redact(&preview)
                            ));
                        }
                        _ => {}
                    }
                }
            }
            MessageContent::Text(t) => {
                if msg.role == "assistant" && !t.trim().is_empty() {
                    let preview = if t.chars().count() > 200 {
                        format!("{}...", t.chars().take(200).collect::<String>())
                    } else {
                        t.clone()
                    };
                    out.push_str(&format!("Assistant: {}\n", env_redactor.redact(&preview)));
                }
            }
        }
    }

    out
}

fn has_repeated_stalled_failures(messages: &[Message]) -> bool {
    let mut repeated_error_markers = 0usize;
    let mut repeated_no_output_markers = 0usize;
    for msg in messages.iter().rev().take(8) {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    let lower = content.to_ascii_lowercase();
                    if is_error.unwrap_or(false)
                        && (lower.contains("timed out")
                            || lower.contains("no such file")
                            || lower.contains("no files found"))
                    {
                        repeated_error_markers = repeated_error_markers.saturating_add(1);
                    }
                    if lower.contains("no files found")
                        || lower.contains("still no")
                        || lower.contains("no such file")
                    {
                        repeated_no_output_markers = repeated_no_output_markers.saturating_add(1);
                    }
                }
            }
        }
    }
    repeated_error_markers >= 2 && repeated_no_output_markers >= 2
}

fn has_repeated_no_progress_signatures(messages: &[Message]) -> bool {
    let mut signatures: Vec<String> = Vec::new();
    for msg in messages.iter().rev().take(10) {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    let marker = if *is_error == Some(true) { "err" } else { "ok" };
                    let lowered = content.to_ascii_lowercase();
                    let bucket = if lowered.contains("no files found")
                        || lowered.contains("no such file")
                        || lowered.contains("timed out")
                        || lowered.contains("still no")
                    {
                        "no_progress"
                    } else if lowered.contains("completed")
                        || lowered.contains("saved")
                        || lowered.contains("success")
                    {
                        "progress"
                    } else {
                        "unknown"
                    };
                    signatures.push(format!("{marker}:{bucket}"));
                }
            }
        }
    }
    if signatures.len() < 3 {
        return false;
    }
    let head = signatures[0].clone();
    signatures
        .iter()
        .take(3)
        .all(|s| s == &head && s.contains("no_progress"))
}

/// Build the user message for PTE evaluation.
fn build_pte_user_prompt(
    env_redactor: &EnvSecretRedactor,
    messages: &[Message],
    iteration: usize,
    max_iterations: usize,
) -> String {
    let goal = extract_session_goal(messages, Some(env_redactor));
    let tool_summary = build_tool_results_summary(env_redactor, messages, 6);
    let mut prompt = format!(
        "Session goal (current_request): {}\n\nTools called and their results:\n{}\nCurrent iteration: {} of {}",
        goal.current_request, tool_summary, iteration + 1, max_iterations
    );
    if goal.is_short_reply {
        prompt.push_str("\nShort-reply turn: yes\n");
        if let Some(ref d) = goal.disambiguation_assistant {
            prompt.push_str("Disambiguation (prior assistant excerpt):\n");
            prompt.push_str(d);
            prompt.push('\n');
        }
    }
    prompt
}

/// Evaluate whether the task is complete after tool execution.
/// Returns Continue immediately if PTE is disabled.
pub async fn evaluate_completion(
    enabled: bool,
    config: &Config,
    multimodel: Option<&crate::local_delegate::LocalDelegateConfig>,
    env_redactor: &EnvSecretRedactor,
    principles_content: &str,
    memory_context: &str,
    messages: &[Message],
    iteration: usize,
) -> Result<PteResult, FinallyAValueBotError> {
    if !enabled {
        return Ok(PteResult {
            action: PteAction::Continue,
            reason: String::new(),
            provider_label: None,
            disabled: true,
            provider_skipped: false,
        });
    }

    // Fast-path stall classifier to avoid infinite "continue" loops on repeated
    // identical failure/no-output states.
    if has_repeated_stalled_failures(messages) {
        return Ok(PteResult {
            action: PteAction::AskUser,
            reason: "Repeated stalled failures detected; stop loop and ask user whether to retry or wait.".to_string(),
            provider_label: None,
            disabled: false,
            provider_skipped: false,
        });
    }
    if has_repeated_no_progress_signatures(messages) {
        return Ok(PteResult {
            action: PteAction::StopWithSummary,
            reason: "Repeated no-progress tool outcomes detected; stop and return concise summary."
                .to_string(),
            provider_label: None,
            disabled: false,
            provider_skipped: false,
        });
    }

    let system_prompt = build_pte_system_prompt(principles_content, memory_context);
    let user_prompt = build_pte_user_prompt(
        env_redactor,
        messages,
        iteration,
        config.max_tool_iterations,
    );

    let pte_messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(user_prompt),
    }];

    let provider_bundle = match llm::create_evaluator_provider(config, multimodel) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("PTE skipped: {e}");
            return Ok(PteResult {
                action: PteAction::Continue,
                reason: e.to_string(),
                provider_label: None,
                disabled: false,
                provider_skipped: true,
            });
        }
    };
    let provider_label = provider_bundle.label.clone();
    let response = match tokio::time::timeout(
        std::time::Duration::from_secs(EVALUATOR_TIMEOUT_SECS),
        provider_bundle
            .provider
            .send_message(&system_prompt, pte_messages, None),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!("PTE evaluation failed (fail-open): {e}");
            return Ok(PteResult {
                action: PteAction::Continue,
                reason: e.to_string(),
                provider_label: Some(provider_label),
                disabled: false,
                provider_skipped: true,
            });
        }
        Err(_) => {
            tracing::warn!("PTE evaluation timed out after {EVALUATOR_TIMEOUT_SECS}s (fail-open)");
            return Ok(PteResult {
                action: PteAction::Continue,
                reason: format!("evaluator timed out after {EVALUATOR_TIMEOUT_SECS}s"),
                provider_label: Some(provider_label),
                disabled: false,
                provider_skipped: true,
            });
        }
    };

    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            ResponseContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let mut parsed = parse_pte_response(&text)?;
    parsed.provider_label = Some(provider_label);
    info!(
        "PTE decision: {:?} at iteration {} - {}",
        parsed.action,
        iteration + 1,
        parsed.reason
    );
    Ok(parsed)
}

fn parse_pte_response(text: &str) -> Result<PteResult, FinallyAValueBotError> {
    let trimmed = text.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    #[derive(Deserialize)]
    struct Raw {
        action: String,
        reason: Option<String>,
    }
    let raw: Raw = serde_json::from_str(json_str).map_err(|e| {
        FinallyAValueBotError::Config(format!(
            "PTE failed to parse JSON: {e}. Raw: {}",
            json_str.chars().take(300).collect::<String>()
        ))
    })?;
    let action = match raw.action.to_lowercase().as_str() {
        "complete" => PteAction::Complete,
        "ask_user" => PteAction::AskUser,
        "handoff_background" => PteAction::HandoffBackground,
        "stop_with_summary" => PteAction::StopWithSummary,
        _ => PteAction::Continue,
    };
    Ok(PteResult {
        action,
        reason: raw.reason.unwrap_or_default(),
        provider_label: None,
        disabled: false,
        provider_skipped: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pte_continue() {
        let j = r#"{"action": "continue", "reason": "task not done"}"#;
        let r = parse_pte_response(j).unwrap();
        assert_eq!(r.action, PteAction::Continue);
        assert_eq!(r.reason, "task not done");
    }

    #[test]
    fn test_parse_pte_complete() {
        let j = r#"{"action": "complete", "reason": "all steps done"}"#;
        let r = parse_pte_response(j).unwrap();
        assert_eq!(r.action, PteAction::Complete);
        assert_eq!(r.reason, "all steps done");
    }

    #[test]
    fn test_parse_pte_unknown_defaults_to_continue() {
        let j = r#"{"action": "unknown", "reason": "weird"}"#;
        let r = parse_pte_response(j).unwrap();
        assert_eq!(r.action, PteAction::Continue);
    }

    #[test]
    fn test_parse_pte_ask_user() {
        let j = r#"{"action": "ask_user", "reason": "Which mailbox should I open?"}"#;
        let r = parse_pte_response(j).unwrap();
        assert_eq!(r.action, PteAction::AskUser);
        assert_eq!(r.reason, "Which mailbox should I open?");
    }

    #[test]
    fn test_format_pte_ask_user_reply_question() {
        let text = format_pte_ask_user_reply("Which mailbox should I open?");
        assert_eq!(text, "Which mailbox should I open?");
    }

    #[test]
    fn test_format_pte_ask_user_reply_statement() {
        let text = format_pte_ask_user_reply("The mailcode cannot be fetched with available tools");
        assert!(text.starts_with("I need a clarification before continuing:"));
        assert!(text.contains("mailcode"));
    }

    #[test]
    fn test_format_pte_ask_user_reply_stalled() {
        let text = format_pte_ask_user_reply(
            "Repeated stalled failures detected; stop loop and ask user whether to retry or wait.",
        );
        assert!(text.contains("progress is stalled"));
        assert!(text.contains("retry now"));
    }

    #[test]
    fn test_build_pte_system_prompt_empty() {
        let prompt = build_pte_system_prompt("", "");
        assert!(prompt.contains("task-completion evaluator"));
        assert!(prompt.contains("ask_user"));
        assert!(prompt.contains("If in doubt, say \"ask_user\""));
        assert!(!prompt.contains("If in doubt, say \"continue\""));
        assert!(!prompt.contains("# Principles"));
        assert!(!prompt.contains("# Memory Context"));
    }

    #[test]
    fn test_build_pte_system_prompt_with_content() {
        let prompt = build_pte_system_prompt("Be helpful", "User likes Rust");
        assert!(prompt.contains("# Principles"));
        assert!(prompt.contains("Be helpful"));
        assert!(prompt.contains("# Memory Context"));
        assert!(prompt.contains("User likes Rust"));
    }

    #[test]
    fn test_build_pte_user_prompt_session_goal() {
        use crate::safety_redaction::EnvSecretRedactor;
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text(
                "[current_request]\nFix the login bug\n[/current_request]".into(),
            ),
        }];
        let redactor = EnvSecretRedactor::empty();
        let prompt = build_pte_user_prompt(&redactor, &messages, 0, 5);
        assert!(prompt.contains("Session goal (current_request):"));
        assert!(prompt.contains("Fix the login bug"));
    }

    #[test]
    fn test_has_repeated_stalled_failures_true() {
        let msg = Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "a".into(),
                    content: "Tool timed out after 1500s".into(),
                    is_error: Some(true),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "b".into(),
                    content: "No files found matching pattern.".into(),
                    is_error: Some(true),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "c".into(),
                    content: "ls: No such file or directory".into(),
                    is_error: Some(true),
                },
            ]),
        };
        assert!(has_repeated_stalled_failures(&[msg]));
    }

    #[test]
    fn test_has_repeated_stalled_failures_false() {
        let msg = Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "x".into(),
                content: "Saved swapped image successfully".into(),
                is_error: Some(false),
            }]),
        };
        assert!(!has_repeated_stalled_failures(&[msg]));
    }
}
