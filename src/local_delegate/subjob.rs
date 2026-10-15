//! Bounded read-only sub-job loop on the local delegate model.

use std::time::Instant;

use crate::claude::{Message, MessageContent, ResponseContentBlock};
use crate::error::FinallyAValueBotError;
use crate::local_delegate::{tool_choice_for_target, RouteTarget};
use crate::telegram::AppState;
use crate::tools::{ToolAuthContext, ToolRegistry, ToolResult};

const DEFAULT_MAX_ITERATIONS: usize = 3;
const HARD_MAX_ITERATIONS: usize = 5;
const SUBJOB_SYSTEM: &str =
    "You are a read-only discovery assistant. Use only the tools provided. \
Summarize findings clearly for the parent agent. Do not mutate files or run shell commands.";
const SUMMARY_MAX_CHARS: usize = 4000;
const LLM_ROUND_TIMEOUT_SECS: u64 = 180;

pub struct SubjobParams<'a> {
    pub brief: &'a str,
    pub max_iterations: Option<usize>,
    pub current_request: Option<&'a str>,
}

pub async fn run_local_subjob(
    state: &AppState,
    tools: &ToolRegistry,
    tool_auth: &ToolAuthContext,
    params: SubjobParams<'_>,
) -> Result<String, FinallyAValueBotError> {
    let max_it = params
        .max_iterations
        .unwrap_or(DEFAULT_MAX_ITERATIONS)
        .min(HARD_MAX_ITERATIONS)
        .max(1);

    let tool_defs = tools.definitions_filtered(true);
    if tool_defs.is_empty() {
        return Err(FinallyAValueBotError::Config(
            "No read-only tools available for local subjob".into(),
        ));
    }

    let mut user_body = format!("[subjob_brief]\n{}\n[/subjob_brief]", params.brief.trim());
    if let Some(req) = params.current_request.filter(|s| !s.trim().is_empty()) {
        let trimmed: String = req.chars().take(2000).collect();
        user_body.push_str(&format!(
            "\n\n[current_request]\n{trimmed}\n[/current_request]"
        ));
    }

    let mut messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(user_body),
    }];

    let mut log: Vec<String> = Vec::new();

    for _iteration in 0..max_it {
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(LLM_ROUND_TIMEOUT_SECS),
            state.llm.send_message_for_target(
                RouteTarget::LocalReadOnly,
                SUBJOB_SYSTEM,
                messages.clone(),
                Some(tool_defs.clone()),
            ),
        )
        .await
        .map_err(|_| FinallyAValueBotError::LlmApi("local subjob LLM timeout".into()))??;

        let assistant_text: String = response
            .content
            .iter()
            .filter_map(|b| match b {
                ResponseContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if !assistant_text.trim().is_empty() {
            log.push(format!("Assistant: {assistant_text}"));
        }

        let tool_uses: Vec<_> = response
            .content
            .iter()
            .filter_map(|b| match b {
                ResponseContentBlock::ToolUse {
                    id, name, input, ..
                } => Some((id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .collect();

        if tool_uses.is_empty() {
            if !assistant_text.trim().is_empty() {
                return Ok(truncate_summary(&assistant_text));
            }
            break;
        }

        messages.push(Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(
                response
                    .content
                    .iter()
                    .map(|block| match block {
                        ResponseContentBlock::Text { text } => {
                            crate::claude::ContentBlock::Text { text: text.clone() }
                        }
                        ResponseContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            thought_signature,
                        } => crate::claude::ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            thought_signature: thought_signature.clone(),
                        },
                    })
                    .collect(),
            ),
        });

        for (id, name, input) in tool_uses {
            if crate::local_delegate::is_mutation_tool(&name) {
                let err =
                    ToolResult::error(format!("Tool `{name}` is not allowed in read-only subjob"));
                log.push(format!("Blocked mutation tool: {name}"));
                messages.push(tool_result_message(&id, &err));
                continue;
            }
            let start = Instant::now();
            let result = tools.execute_with_auth(&name, input, tool_auth).await;
            log.push(format!(
                "Tool {name} ({}ms): {}",
                start.elapsed().as_millis(),
                truncate_preview(&result.content, 1500)
            ));
            messages.push(tool_result_message(&id, &result));
        }
    }

    Ok(truncate_summary(&log.join("\n\n")))
}

fn tool_result_message(tool_use_id: &str, result: &ToolResult) -> Message {
    Message {
        role: "user".into(),
        content: MessageContent::Blocks(vec![crate::claude::ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: result.content.clone(),
            is_error: Some(result.is_error),
        }]),
    }
}

fn truncate_summary(s: &str) -> String {
    truncate_preview(s, SUMMARY_MAX_CHARS)
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

// Re-export for tests
#[allow(dead_code)]
pub fn subjob_tool_choice(has_tools: bool) -> Option<String> {
    tool_choice_for_target(RouteTarget::LocalReadOnly, has_tools)
}
