//! Pre-delivery Quality Evaluator (PDQE): synchronous QC before the user receives a reply.
//! Runs for Classic and Gemini ADK engines — Cursor finish skips PDQE.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tracing::warn;

use crate::agent_history::append_pdqe_step_to_agent_history;
use crate::agent_turn_context::SessionGoalContext;
use crate::channels::telegram::BACKGROUND_JOB_HANDOFF_PREFIX;
use crate::claude::{Message, MessageContent, ResponseContentBlock};
use crate::config::Config;
use crate::db::call_blocking;
use crate::db::Database;
use crate::error::FinallyAValueBotError;
use crate::llm::{self, EVALUATOR_TIMEOUT_SECS};
use crate::post_tool_evaluator::build_tool_results_summary;
use crate::safety_redaction::EnvSecretRedactor;

const EVAL_TIMEOUT_SECS: u64 = EVALUATOR_TIMEOUT_SECS;
const PDQE_TOOL_TRACE_MAX_MESSAGES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityVerdictKind {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone)]
pub struct QualityVerdict {
    pub kind: QualityVerdictKind,
    pub issues: Vec<String>,
    pub feedback_for_agent: String,
    pub confidence: f64,
}

/// Structured JSON persisted in agent history PDQE `eval:` lines (web UI parses this).
pub fn format_pdqe_verdict_detail(verdict: &QualityVerdict, note: Option<&str>) -> String {
    let verdict_label = match verdict.kind {
        QualityVerdictKind::Pass => "pass",
        QualityVerdictKind::Fail => "fail",
        QualityVerdictKind::Skip => "skip",
    };
    let mut obj = serde_json::json!({
        "verdict": verdict_label,
        "confidence": verdict.confidence,
        "issues": verdict.issues,
    });
    if !verdict.feedback_for_agent.trim().is_empty() {
        obj["feedback"] = serde_json::Value::String(verdict.feedback_for_agent.clone());
    }
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        obj["note"] = serde_json::Value::String(note.to_string());
    }
    serde_json::to_string(&obj).unwrap_or_else(|_| "{}".into())
}

/// Context for pre-delivery quality evaluation (in-loop gate).
#[derive(Debug, Clone)]
pub struct PostDeliveryEvalContext {
    pub run_key: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub caller_channel: String,
    pub chat_type: String,
    pub stop_reason: String,
    pub delivered_text: String,
    pub session_goal: SessionGoalContext,
    pub tool_trace_summary: String,
    pub principles_excerpt: String,
    pub is_scheduled_task: bool,
    pub is_conversational: bool,
    pub intent_signature: Option<String>,
    pub tool_names: Vec<String>,
    /// Basename of the run's `agent_history/*.md` file (set when the main loop saved history).
    pub agent_history_basename: Option<String>,
    pub runtime_data_dir: String,
}

pub fn quality_eval_channel_allowed(config: &Config, channel: &str) -> bool {
    let ch = channel.trim().to_lowercase();
    config
        .quality_eval_channels
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .any(|allowed| allowed == ch)
}

pub fn should_skip_pdqe(
    pdqe_enabled: bool,
    config: &Config,
    ctx: &PostDeliveryEvalContext,
) -> Option<&'static str> {
    if !pdqe_enabled {
        return Some("disabled");
    }
    if !quality_eval_channel_allowed(config, &ctx.caller_channel) {
        return Some("channel_not_allowed");
    }
    if ctx.stop_reason == "cancelled"
        || ctx.stop_reason == "cursor_status_recheck"
        || ctx.stop_reason == "cursor_engine_stream_interrupted"
    {
        return Some("stop_reason");
    }
    if ctx.delivered_text.trim().is_empty() {
        return Some("empty_delivery");
    }
    if ctx
        .delivered_text
        .starts_with(BACKGROUND_JOB_HANDOFF_PREFIX)
    {
        return Some("background_handoff");
    }
    if ctx.session_goal.current_request.trim().is_empty() {
        return Some("no_session_goal");
    }
    None
}

/// User-visible notice when PDQE fails but the candidate reply is still delivered.
pub fn format_pdqe_user_delivery_notice(verdict: &QualityVerdict) -> String {
    let mut notice =
        String::from("A pre-delivery quality review flagged potential issues with this response.");
    if !verdict.issues.is_empty() {
        notice.push_str("\n\nIssues noted:");
        for issue in &verdict.issues {
            notice.push_str("\n- ");
            notice.push_str(issue);
        }
    }
    if !verdict.feedback_for_agent.trim().is_empty() {
        notice.push_str("\n\nReview summary: ");
        notice.push_str(verdict.feedback_for_agent.trim());
    }
    let pct = (verdict.confidence.clamp(0.0, 1.0) * 100.0).round() as i32;
    notice.push_str(&format!("\n\n(Review confidence: {pct}%)"));
    notice
}

pub fn append_pdqe_delivery_notice(final_text: &mut String, verdict: &QualityVerdict) {
    let notice = format_pdqe_user_delivery_notice(verdict);
    if final_text.trim().is_empty() {
        *final_text = notice;
        return;
    }
    final_text.push_str("\n\n---\n\n");
    final_text.push_str(&notice);
}

/// User-visible message when a PDQE-triggered revision pass times out before completing.
pub fn format_post_quality_eval_retry_timeout_message() -> String {
    "A quality review flagged issues with my previous answer, but I could not finish revising it in time. Please try again or break your request into smaller steps.".to_string()
}

pub fn format_agent_llm_timeout_message(pending_quality_eval_retry: bool) -> String {
    if pending_quality_eval_retry {
        format_post_quality_eval_retry_timeout_message()
    } else {
        "The request took too long after the last step. Please try again or break your request into smaller steps.".to_string()
    }
}

pub fn build_quality_feedback_message(
    session_goal: &SessionGoalContext,
    verdict: &QualityVerdict,
) -> String {
    format!(
        "[quality_eval_feedback]\nSession goal (current_request): {}\n\nQuality review found: {}\nIssues: {}\n\nRevise or complete the task for the user relative to the session goal above. Do not repeat the same mistakes. If you cannot fix it, say what is blocked.",
        session_goal.current_request,
        verdict.feedback_for_agent,
        if verdict.issues.is_empty() {
            "(none listed)".to_string()
        } else {
            verdict.issues.join("; ")
        }
    )
}

fn build_pdqe_system_prompt(principles_excerpt: &str) -> String {
    let mut prompt = String::from(
        r#"You are a pre-delivery quality evaluator. Judge whether the candidate assistant reply satisfies the session goal (`current_request`) for this turn.

Output JSON only:
{"verdict":"pass"|"fail"|"skip","issues":["..."],"feedback_for_agent":"one paragraph","confidence":0.0}

Rules:
- The session goal is ONLY `current_request`. Prior turns (`prior_turn`) and persona memory/bulletin are background — do not penalize ignoring unrelated older tasks.
- Pass: direct answer to current_request, justified clarification question, or honest blocked state relative to current_request.
- Fail: off-topic vs current_request, clearly incomplete for that ask, contradicts tool evidence, or expands into memory/bulletin topics the user did not raise.
- When disambiguation context is provided for a short reply, judge whether the reply resolves the referent correctly.
- Do not use web search. Judge only the supplied fields.
- confidence is 0.0–1.0 (how sure you are of verdict).
"#,
    );
    if !principles_excerpt.trim().is_empty() {
        prompt.push_str("\n# Principles (constraints only)\n\n");
        prompt.push_str(principles_excerpt);
        prompt.push('\n');
    }
    prompt
}

fn build_pdqe_user_prompt(ctx: &PostDeliveryEvalContext) -> String {
    let mut prompt = format!(
        "Session goal (current_request):\n{}\n\nCandidate reply:\n{}\n\nTool trace:\n{}\nStop reason: {}\n",
        ctx.session_goal.current_request,
        ctx.delivered_text,
        ctx.tool_trace_summary,
        ctx.stop_reason
    );
    if ctx.session_goal.is_short_reply {
        prompt.push_str("\nShort-reply turn: yes\n");
        if let Some(ref d) = ctx.session_goal.disambiguation_assistant {
            prompt.push_str("Disambiguation (prior assistant excerpt):\n");
            prompt.push_str(d);
            prompt.push('\n');
        }
    }
    prompt
}

pub fn parse_evaluator_json_response(text: &str) -> Result<QualityVerdict, FinallyAValueBotError> {
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
        verdict: String,
        #[serde(default)]
        issues: Vec<String>,
        #[serde(default)]
        feedback_for_agent: String,
        #[serde(default)]
        confidence: Option<f64>,
    }
    let raw: Raw = serde_json::from_str(json_str).map_err(|e| {
        FinallyAValueBotError::Config(format!(
            "PDQE failed to parse JSON: {e}. Raw: {}",
            json_str.chars().take(300).collect::<String>()
        ))
    })?;
    let kind = match raw.verdict.to_lowercase().as_str() {
        "pass" => QualityVerdictKind::Pass,
        "fail" => QualityVerdictKind::Fail,
        "skip" => QualityVerdictKind::Skip,
        other => {
            return Err(FinallyAValueBotError::Config(format!(
                "PDQE unknown verdict: {other}"
            )));
        }
    };
    Ok(QualityVerdict {
        kind,
        issues: raw.issues,
        feedback_for_agent: raw.feedback_for_agent,
        confidence: raw.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
    })
}

fn fast_path_verdict(_config: &Config, _ctx: &PostDeliveryEvalContext) -> Option<QualityVerdict> {
    None
}

#[derive(Debug, Clone)]
pub struct QualityEvalOutcome {
    pub verdict: QualityVerdict,
    pub provider_label: String,
}

pub async fn evaluate_delivery_quality(
    pdqe_enabled: bool,
    config: &Config,
    multimodel: Option<&crate::local_delegate::LocalDelegateConfig>,
    env_redactor: &EnvSecretRedactor,
    ctx: &PostDeliveryEvalContext,
) -> Result<QualityEvalOutcome, FinallyAValueBotError> {
    if let Some(_reason) = should_skip_pdqe(pdqe_enabled, config, ctx) {
        return Ok(QualityEvalOutcome {
            verdict: QualityVerdict {
                kind: QualityVerdictKind::Skip,
                issues: vec![],
                feedback_for_agent: String::new(),
                confidence: 0.0,
            },
            provider_label: String::new(),
        });
    }

    if let Some(v) = fast_path_verdict(config, ctx) {
        return Ok(QualityEvalOutcome {
            verdict: v,
            provider_label: String::new(),
        });
    }

    let provider_bundle = llm::create_evaluator_provider(config, multimodel)?;
    let provider_label = provider_bundle.label.clone();
    let system = build_pdqe_system_prompt(&ctx.principles_excerpt);
    let user = build_pdqe_user_prompt(ctx);
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(user),
    }];

    let response = tokio::time::timeout(
        Duration::from_secs(EVAL_TIMEOUT_SECS),
        provider_bundle
            .provider
            .send_message(&system, messages, None),
    )
    .await
    .map_err(|_| FinallyAValueBotError::Config("PDQE evaluator call timed out".into()))??;

    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            ResponseContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let _ = env_redactor;
    let verdict = parse_evaluator_json_response(&text)?;
    Ok(QualityEvalOutcome {
        verdict,
        provider_label,
    })
}

pub fn log_pdqe_to_agent_history(ctx: &PostDeliveryEvalContext, step: &str, detail: &str) {
    let Some(ref basename) = ctx.agent_history_basename else {
        return;
    };
    if let Err(e) = append_pdqe_step_to_agent_history(
        &ctx.runtime_data_dir,
        ctx.chat_id,
        ctx.persona_id,
        basename,
        step,
        detail,
        None,
    ) {
        warn!(
            run_key = %ctx.run_key,
            basename = %basename,
            "Failed to append PDQE step to agent history: {e}"
        );
    }
}

pub async fn append_quality_timeline(
    db: Arc<Database>,
    run_key: &str,
    chat_id: i64,
    persona_id: i64,
    event_type: &str,
    payload: &str,
) {
    let _ = call_blocking(db, {
        let run_key = run_key.to_string();
        let event_type = event_type.to_string();
        let payload = payload.to_string();
        move |db| {
            db.append_run_timeline_event(&run_key, chat_id, persona_id, &event_type, Some(&payload))
        }
    })
    .await;
}

/// Build tool trace summary for PDQE from messages in the current run.
pub fn build_pdqe_tool_trace(
    env_redactor: &EnvSecretRedactor,
    messages: &[Message],
    run_start_index: usize,
) -> String {
    let start = run_start_index.min(messages.len());
    let slice = &messages[start..];
    let window = slice.len().min(PDQE_TOOL_TRACE_MAX_MESSAGES);
    build_tool_results_summary(env_redactor, slice, window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pass_verdict() {
        let j = r#"{"verdict":"pass","issues":[],"feedback_for_agent":"","confidence":0.95}"#;
        let v = parse_evaluator_json_response(j).unwrap();
        assert_eq!(v.kind, QualityVerdictKind::Pass);
        assert!((v.confidence - 0.95).abs() < 0.01);
    }

    #[test]
    fn parse_fail_verdict() {
        let j = r#"{"verdict":"fail","issues":["incomplete"],"feedback_for_agent":"missing steps","confidence":0.8}"#;
        let v = parse_evaluator_json_response(j).unwrap();
        assert_eq!(v.kind, QualityVerdictKind::Fail);
        assert_eq!(v.issues, vec!["incomplete"]);
    }

    #[test]
    fn format_pdqe_verdict_detail_includes_issues_and_feedback() {
        let detail = format_pdqe_verdict_detail(
            &QualityVerdict {
                kind: QualityVerdictKind::Fail,
                issues: vec!["off-topic".into()],
                feedback_for_agent: "Stay on current_request.".into(),
                confidence: 0.88,
            },
            Some("retry 1/1 triggered"),
        );
        assert!(detail.contains("\"issues\""));
        assert!(detail.contains("\"feedback\""));
        assert!(detail.contains("\"note\""));
    }

    #[test]
    fn format_pdqe_user_delivery_notice_includes_issues() {
        let notice = format_pdqe_user_delivery_notice(&QualityVerdict {
            kind: QualityVerdictKind::Fail,
            issues: vec!["incomplete".into(), "off-topic".into()],
            feedback_for_agent: "Answer the user's question directly.".into(),
            confidence: 0.91,
        });
        assert!(notice.contains("quality review flagged"));
        assert!(notice.contains("- incomplete"));
        assert!(notice.contains("Answer the user's question directly."));
        assert!(notice.contains("91%"));
    }

    #[test]
    fn format_agent_llm_timeout_message_after_pdqe_retry() {
        let msg = format_agent_llm_timeout_message(true);
        assert!(msg.contains("quality review flagged"));
        assert!(!msg.contains("after the last step"));
        let generic = format_agent_llm_timeout_message(false);
        assert!(generic.contains("after the last step"));
    }

    #[test]
    fn channel_allowlist() {
        let mut config = crate::config::test_config();
        config.quality_eval_channels = "telegram,web".into();
        assert!(quality_eval_channel_allowed(&config, "telegram"));
        assert!(quality_eval_channel_allowed(&config, "Web"));
        assert!(!quality_eval_channel_allowed(&config, "scheduler"));
    }
}
