//! Background "Learn & optimize" worker: analyze a saved agent run with local Tier 2 llama
//! and update persona memory via a restricted tool loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

use serde_json::json;

use crate::agent_history::{split_pdqe_for_optimize, split_trace_for_optimize};
use crate::channel::{deliver_to_contact, DeliveryScope};
use crate::claude::{ContentBlock, Message, MessageContent, ResponseContentBlock, ToolDefinition};
use crate::db::call_blocking;
use crate::error::FinallyAValueBotError;
use crate::job_heartbeat::{spawn_shared_heartbeat, HeartbeatSignal, JobType};
use crate::llm::{create_openai_compatible_provider_with_timeout, LlmProvider, LlmSendOptions};
use crate::local_delegate::ModelTier;
use crate::telegram::AppState;
use crate::tools::ToolAuthContext;

const MAX_OPTIMIZE_HISTORY_BYTES: usize = 80 * 1024;
const MAX_OPTIMIZE_OPERATOR_NOTES_BYTES: usize = 8 * 1024;
const OPTIMIZE_MAX_ITERATIONS: usize = 3;
const OPTIMIZE_LLM_TIMEOUT_SECS: u64 = 1000;

const OPTIMIZE_SYSTEM_PROMPT: &str = "\
You are a memory optimization assistant for an agentic bot operator. \
Analyze the provided agent run trace and update persona memory so future runs are more effective and efficient. \
Use only the tools you are given. \
Bulletin is canonical episodic focus (goals, blockers, outcomes, next step). \
Tier 2 holds durable terminology and preferences — not active status. \
Tier 3 is a short scratchpad (max 15 lines); do not duplicate bulletin lines. \
Add Tier 1 workflow_principles only for clearly reusable cross-run patterns (max 1–2). \
Do not create Tier 2 SOPs without a valid ORIGIN/ vault path. \
If nothing durable should change, read memory and stop without writes.";

/// Scope block appended to the optimizer system prompt so the local model does not
/// confuse path segments (e.g. `personas/997894126/3`) with tool `persona_id` values.
fn build_optimize_system_prompt(
    chat_id: i64,
    persona_id: i64,
    history_filename: &str,
    local_base_url: &str,
    local_model: &str,
) -> String {
    format!(
        "{OPTIMIZE_SYSTEM_PROMPT}\n\n\
         ## Memory scope (required)\n\
         Target persona for all memory tools: chat_id={chat_id}, persona_id={persona_id}.\n\
         Paths in the run trace (e.g. personas/{chat_id}/{persona_id}/...) are filesystem paths — \
         not tool IDs. Never pass a chat id as persona_id.\n\
         Omit chat_id and persona_id on memory tool inputs unless you intentionally use these exact values.\n\n\
         Run file: {history_filename}\n\
         Local endpoint: {local_base_url} model: {local_model}"
    )
}

/// Force memory tool calls onto the job's chat/persona regardless of model-supplied IDs.
fn pin_memory_tool_scope(
    input: serde_json::Value,
    chat_id: i64,
    persona_id: i64,
) -> serde_json::Value {
    let mut obj = match input {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    obj.insert("chat_id".into(), json!(chat_id));
    obj.insert("persona_id".into(), json!(persona_id));
    serde_json::Value::Object(obj)
}

const OPTIMIZE_USER_SUFFIX: &str = "\
\n\n---\n\
Analyze this run for inefficiency: tool loops, wasted iterations, wrong approaches, repeated errors, \
missing vault/SOP usage, and tier routing issues. \
When PDQE feedback is present, treat evaluator issues, confidence, and retry reasons as high-signal guidance. \
When operator guidance is present, prioritize those focus areas while still grounding changes in the trace. \
Update bulletin, Tier 2 preferences/terminology, and Tier 3 only when warranted. \
When finished, reply briefly with what you changed or why no change was needed.";

#[derive(Debug, Clone)]
pub struct RunOptimizeOutcome {
    pub summary: String,
    pub tools_used: Vec<String>,
    pub memory_changed: bool,
}

#[derive(Debug)]
pub enum RunOptimizeEnqueueOutcome {
    Queued { job_id: String },
    BlockedAlreadyRunning,
    ActiveLookupFailed(String),
    DbCreateFailed(String),
}

fn truncate_at_char_boundary(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn normalize_operator_notes(notes: Option<&str>) -> Option<String> {
    let trimmed = notes?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = trimmed.to_string();
    if out.len() > MAX_OPTIMIZE_OPERATOR_NOTES_BYTES {
        truncate_at_char_boundary(&mut out, MAX_OPTIMIZE_OPERATOR_NOTES_BYTES);
        out.push_str("\n…\n(operator notes truncated)");
    }
    Some(out)
}

/// Prepare run markdown for the optimizer LLM (trace + optional PDQE + operator notes, size-capped, redacted).
pub fn build_optimize_user_message(
    history_content: &str,
    operator_notes: Option<&str>,
    redact: impl Fn(&str) -> String,
) -> String {
    let trace = redact(&split_trace_for_optimize(history_content));
    let pdqe_block = split_pdqe_for_optimize(history_content)
        .map(|section| {
            format!(
                "\n\n## Post-delivery quality evaluation (PDQE)\n\n{}\n",
                redact(&section)
            )
        })
        .unwrap_or_default();
    let notes_block = normalize_operator_notes(operator_notes)
        .map(|notes| format!("\n\n## Operator guidance\n\n{notes}\n"))
        .unwrap_or_default();

    let prefix = "Agent run trace to analyze (file content):\n\n";
    let fixed_len =
        prefix.len() + pdqe_block.len() + notes_block.len() + OPTIMIZE_USER_SUFFIX.len();
    let trace_budget = MAX_OPTIMIZE_HISTORY_BYTES.saturating_sub(fixed_len + 80);
    let mut trace_body = trace;
    let mut truncated = false;
    if trace_body.len() > trace_budget {
        truncate_at_char_boundary(&mut trace_body, trace_budget);
        trace_body.push_str("\n\n…\n(trace truncated for analysis)\n");
        truncated = true;
    }

    let mut body = format!("{prefix}{trace_body}{pdqe_block}{notes_block}{OPTIMIZE_USER_SUFFIX}");
    if !truncated && body.len() > MAX_OPTIMIZE_HISTORY_BYTES {
        truncate_at_char_boundary(&mut body, MAX_OPTIMIZE_HISTORY_BYTES.saturating_sub(80));
        body.push_str("\n\n…\n(content truncated for analysis)\n");
        body.push_str(OPTIMIZE_USER_SUFFIX);
    }
    body
}

fn local_tier_config_ready(mm: &crate::local_delegate::LocalDelegateConfig) -> bool {
    mm.local_routable() || mm.tier1_configured() || mm.tier2_configured()
}

fn resolve_optimizer_local_endpoint(
    mm: &crate::local_delegate::LocalDelegateConfig,
) -> (String, String) {
    if !mm.local_base_url.trim().is_empty() {
        (mm.local_base_url.clone(), mm.local_model.clone())
    } else if !mm.tier1_base_url.trim().is_empty() {
        (mm.tier1_base_url.clone(), mm.tier1_model.clone())
    } else {
        (mm.tier2_base_url.clone(), mm.tier2_model.clone())
    }
}

async fn send_local_tier_message(
    state: &AppState,
    system: &str,
    messages: Vec<Message>,
    tools: Option<Vec<ToolDefinition>>,
) -> Result<crate::claude::MessagesResponse, FinallyAValueBotError> {
    let mm = state.llm.local_delegate_config();
    let (base_url, model) = resolve_optimizer_local_endpoint(&mm);
    let request_timeout = std::time::Duration::from_secs(OPTIMIZE_LLM_TIMEOUT_SECS);
    let provider: Arc<dyn LlmProvider> = Arc::from(create_openai_compatible_provider_with_timeout(
        &state.config,
        &base_url,
        &model,
        request_timeout,
    ));
    let has_tools = tools.as_ref().is_some_and(|t| !t.is_empty());
    let options = LlmSendOptions {
        tool_choice: if has_tools {
            crate::local_delegate::tool_choice_for_tier(ModelTier::LocalReadOnly, true)
        } else {
            None
        },
    };
    provider
        .send_message_with_options(system, messages, tools, options)
        .await
}

async fn run_restricted_memory_loop(
    state: &AppState,
    chat_id: i64,
    persona_id: i64,
    system_prompt: &str,
    user_message: String,
    cancel: &AtomicBool,
    hb_tx: Option<&UnboundedSender<HeartbeatSignal>>,
) -> Result<RunOptimizeOutcome, String> {
    let allowed_tools = [
        "read_tiered_memory",
        "write_tiered_memory",
        "update_bulletin_focus",
    ];
    let tool_defs: Vec<ToolDefinition> = state
        .tools
        .definitions()
        .into_iter()
        .filter(|d| allowed_tools.contains(&d.name.as_str()))
        .collect();
    if tool_defs.is_empty() {
        return Err("memory tools unavailable".into());
    }

    let tool_auth = ToolAuthContext {
        run_id: None,
        caller_channel: "web".into(),
        caller_chat_id: chat_id,
        caller_persona_id: persona_id,
        control_chat_ids: state.config.control_chat_ids.clone(),
        is_scheduled_task: false,
        session_id: None,
    };

    let mut messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(user_message),
    }];
    let mut tools_used: Vec<String> = Vec::new();
    let mut memory_changed = false;

    for iter in 0..OPTIMIZE_MAX_ITERATIONS {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if let Some(tx) = hb_tx {
            let _ = tx.send(HeartbeatSignal::Progress(format!(
                "optimizer iteration {}",
                iter + 1
            )));
        }

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(OPTIMIZE_LLM_TIMEOUT_SECS),
            send_local_tier_message(
                state,
                system_prompt,
                messages.clone(),
                Some(tool_defs.clone()),
            ),
        )
        .await
        .map_err(|_| "optimizer LLM timed out".to_string())?
        .map_err(|e| e.to_string())?;

        let stop_reason = response.stop_reason.as_deref().unwrap_or("end_turn");
        let assistant_text: String = response
            .content
            .iter()
            .filter_map(|block| match block {
                ResponseContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        if stop_reason == "end_turn" || stop_reason == "max_tokens" {
            let summary = if assistant_text.trim().is_empty() {
                "Run optimization finished (no assistant summary).".to_string()
            } else {
                assistant_text
            };
            return Ok(RunOptimizeOutcome {
                summary,
                tools_used,
                memory_changed,
            });
        }
        if stop_reason != "tool_use" {
            return Ok(RunOptimizeOutcome {
                summary: format!("Run optimization stopped ({stop_reason})."),
                tools_used,
                memory_changed,
            });
        }

        let assistant_content: Vec<ContentBlock> = response
            .content
            .iter()
            .map(|block| match block {
                ResponseContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                ResponseContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    thought_signature,
                } => ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    thought_signature: thought_signature.clone(),
                },
            })
            .collect();
        messages.push(Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(assistant_content),
        });

        let mut tool_results = Vec::new();
        for block in &response.content {
            if let ResponseContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                tools_used.push(name.clone());
                if let Some(tx) = hb_tx {
                    let _ = tx.send(HeartbeatSignal::ToolStart(name.clone()));
                }
                let result = if !allowed_tools.contains(&name.as_str()) {
                    crate::tools::ToolResult::error(format!(
                        "Tool {name} is not allowed during run optimization."
                    ))
                } else {
                    let pinned = pin_memory_tool_scope(input.clone(), chat_id, persona_id);
                    state
                        .tools
                        .execute_with_auth(name, pinned, &tool_auth)
                        .await
                };
                if result.is_error {
                    warn!(
                        chat_id,
                        persona_id,
                        tool = %name,
                        iteration = iter + 1,
                        preview = %result.content.chars().take(200).collect::<String>(),
                        "Run optimizer tool error"
                    );
                }
                if (name == "write_tiered_memory" || name == "update_bulletin_focus")
                    && !result.is_error
                {
                    memory_changed = true;
                }
                if let Some(tx) = hb_tx {
                    let _ = tx.send(HeartbeatSignal::ToolResult {
                        tool: name.clone(),
                        is_error: result.is_error,
                    });
                }
                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: result.content,
                    is_error: if result.is_error { Some(true) } else { None },
                });
            }
        }
        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Blocks(tool_results),
        });
    }

    Ok(RunOptimizeOutcome {
        summary: "Run optimization reached iteration limit.".to_string(),
        tools_used,
        memory_changed,
    })
}

pub async fn run_optimizer_from_history(
    state: &AppState,
    chat_id: i64,
    persona_id: i64,
    history_content: &str,
    history_filename: &str,
    operator_notes: Option<&str>,
    cancel: &AtomicBool,
    hb_tx: Option<&UnboundedSender<HeartbeatSignal>>,
) -> Result<RunOptimizeOutcome, String> {
    let mm = state.llm.local_delegate_config();
    if !local_tier_config_ready(&mm) {
        return Err(
            "Local model base URL and model must be configured in Settings → Multi-model.".into(),
        );
    }

    let user_message = build_optimize_user_message(history_content, operator_notes, |s| {
        state.env_redactor.redact(s)
    });
    let (local_base_url, local_model) = resolve_optimizer_local_endpoint(&mm);
    let system = build_optimize_system_prompt(
        chat_id,
        persona_id,
        history_filename,
        &local_base_url,
        &local_model,
    );

    if let Some(tx) = hb_tx {
        let _ = tx.send(HeartbeatSignal::Progress(
            "analyzing agent run with Tier 2 model".to_string(),
        ));
    }

    run_restricted_memory_loop(
        state,
        chat_id,
        persona_id,
        &system,
        user_message,
        cancel,
        hb_tx,
    )
    .await
}

/// Enqueue a background run-optimize job for the latest agent history file.
pub async fn try_enqueue_run_optimize(
    state: Arc<AppState>,
    chat_id: i64,
    persona_id: i64,
    history_filename: String,
    history_content: String,
    operator_notes: Option<String>,
) -> RunOptimizeEnqueueOutcome {
    let mm = state.llm.local_delegate_config();
    if !local_tier_config_ready(&mm) {
        return RunOptimizeEnqueueOutcome::DbCreateFailed("Local model is not configured.".into());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state.config.background_job_pending_start_timeout_secs as i64;
    match call_blocking(state.db.clone(), move |db| {
        db.count_active_background_jobs_for_chat(chat_id, &now, pending_timeout_secs)
    })
    .await
    {
        Ok(count) => {
            if count > 0 {
                return RunOptimizeEnqueueOutcome::BlockedAlreadyRunning;
            }
        }
        Err(e) => {
            return RunOptimizeEnqueueOutcome::ActiveLookupFailed(e.to_string());
        }
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let jid = job_id.clone();
    let filename_for_db = history_filename.clone();
    match call_blocking(state.db.clone(), move |db| {
        db.create_background_run_optimize_job(&jid, chat_id, persona_id, &filename_for_db)
    })
    .await
    {
        Ok(()) => {
            spawn_run_optimize_job(
                state,
                job_id.clone(),
                chat_id,
                persona_id,
                history_filename,
                history_content,
                operator_notes,
            );
            RunOptimizeEnqueueOutcome::Queued { job_id }
        }
        Err(e) => RunOptimizeEnqueueOutcome::DbCreateFailed(e.to_string()),
    }
}

/// Spawn optimizer worker (fire-and-forget from web API).
pub fn spawn_run_optimize_job(
    state: Arc<AppState>,
    job_id: String,
    chat_id: i64,
    persona_id: i64,
    history_filename: String,
    history_content: String,
    operator_notes: Option<String>,
) {
    tokio::spawn(async move {
        let cancel = state
            .background_job_control
            .register(job_id.clone(), chat_id)
            .await;
        let lease_owner = uuid::Uuid::new_v4().to_string();
        let lease_ttl_secs = state.config.background_job_lease_ttl_secs as i64;
        info!(
            job_id = %job_id,
            chat_id,
            persona_id,
            filename = %history_filename,
            "Run optimize job starting"
        );

        let jid = job_id.clone();
        let lease_owner_for_claim = lease_owner.clone();
        let claim_res = call_blocking(state.db.clone(), move |db| {
            db.claim_background_job_running(&jid, &lease_owner_for_claim, lease_ttl_secs)
        })
        .await;

        let claimed = match claim_res {
            Ok(true) => true,
            Ok(false) => {
                let msg = "run optimize claim rejected; job is no longer pending".to_string();
                warn!(job_id = %job_id, "{msg}");
                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.fail_background_job(&jid, &msg)
                })
                .await;
                state.background_job_control.finish(&job_id).await;
                false
            }
            Err(e) => {
                let msg = format!("failed to claim run optimize job: {e}");
                error!(job_id = %job_id, "{msg}");
                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.fail_background_job(&jid, &msg)
                })
                .await;
                state.background_job_control.finish(&job_id).await;
                false
            }
        };

        if !claimed {
            return;
        }

        if cancel.load(Ordering::SeqCst) {
            let jid = job_id.clone();
            let _ = call_blocking(state.db.clone(), move |db| {
                db.mark_background_job_cancelled(&jid, "Cancelled by user")
            })
            .await;
            state.background_job_control.finish(&job_id).await;
            return;
        }

        let hb_tx = spawn_shared_heartbeat(
            state.clone(),
            job_id.clone(),
            chat_id,
            persona_id,
            JobType::RunOptimize,
            Some(lease_owner),
            state.config.background_job_notify_chat_progress,
        );
        let _ = hb_tx.send(HeartbeatSignal::Started(
            "learn & optimize started".to_string(),
        ));

        let result = run_optimizer_from_history(
            &state,
            chat_id,
            persona_id,
            &history_content,
            &history_filename,
            operator_notes.as_deref(),
            &cancel,
            Some(&hb_tx),
        )
        .await;

        match result {
            Ok(outcome) => {
                let delivery = if outcome.memory_changed {
                    format!(
                        "Run optimization finished for {}.\n{}",
                        history_filename, outcome.summary
                    )
                } else {
                    format!(
                        "Run optimization finished for {} (no memory writes).\n{}",
                        history_filename, outcome.summary
                    )
                };
                let jid = job_id.clone();
                let delivery_for_db = delivery.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.mark_background_job_completed_raw(&jid, &delivery_for_db)
                })
                .await;
                let jid = job_id.clone();
                let _ = call_blocking(state.db.clone(), move |db| {
                    db.mark_background_job_done(&jid)
                })
                .await;
                let _ = hb_tx.send(HeartbeatSignal::Finished(
                    "learn & optimize completed".to_string(),
                ));
                if let Err(e) = deliver_to_contact(
                    state.db.clone(),
                    state.telegram_bots.as_ref(),
                    state.discord_http.as_ref(),
                    state.wecom.as_deref(),
                    &state.config.bot_username,
                    chat_id,
                    persona_id,
                    &delivery,
                    Some(state.config.workspace_root_absolute()),
                    DeliveryScope::StoreOnly,
                    None,
                )
                .await
                {
                    warn!(job_id = %job_id, "run optimize delivery failed: {e}");
                }
                info!(job_id = %job_id, "Run optimize job completed");
            }
            Err(e) => {
                let msg = if e == "cancelled" {
                    let jid = job_id.clone();
                    let _ = call_blocking(state.db.clone(), move |db| {
                        db.mark_background_job_cancelled(&jid, "Cancelled by user")
                    })
                    .await;
                    "Run optimization cancelled.".to_string()
                } else {
                    let jid = job_id.clone();
                    let err = e.clone();
                    let _ = call_blocking(state.db.clone(), move |db| {
                        db.fail_background_job(&jid, &err)
                    })
                    .await;
                    format!("Run optimization failed: {e}")
                };
                let _ = hb_tx.send(HeartbeatSignal::Failed(msg.clone()));
                let _ = deliver_to_contact(
                    state.db.clone(),
                    state.telegram_bots.as_ref(),
                    state.discord_http.as_ref(),
                    state.wecom.as_deref(),
                    &state.config.bot_username,
                    chat_id,
                    persona_id,
                    &msg,
                    Some(state.config.workspace_root_absolute()),
                    DeliveryScope::StoreOnly,
                    None,
                )
                .await;
                warn!(job_id = %job_id, "{msg}");
            }
        }
        state.background_job_control.finish(&job_id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_optimize_user_message_strips_snapshot_section() {
        use crate::agent_history::SNAPSHOT_SECTION_START;
        let raw = format!(
            "# Run trace\n## Iteration 1\nStop: end_turn{SNAPSHOT_SECTION_START}{{\"schema\":\"x\"}}"
        );
        let msg = build_optimize_user_message(&raw, None, |s| s.to_string());
        assert!(msg.contains("Iteration 1"));
        assert!(!msg.contains("initial_llm_request"));
        assert!(msg.contains("Analyze this run"));
    }

    #[test]
    fn build_optimize_user_message_includes_pdqe_and_operator_notes() {
        let raw = "# Run trace\n## Iteration 1\nStop: end_turn\n\
            ## Post-delivery quality evaluation\n- **2026-06-26** `quality_eval_fail` — confidence=0.91\n\
            ## Initial LLM prompt (debug snapshot)\n{}";
        let msg = build_optimize_user_message(raw, Some("Focus on reducing tool loops."), |s| {
            s.to_string()
        });
        assert!(msg.contains("Iteration 1"));
        assert!(msg.contains("Post-delivery quality evaluation (PDQE)"));
        assert!(msg.contains("quality_eval_fail"));
        assert!(msg.contains("Operator guidance"));
        assert!(msg.contains("Focus on reducing tool loops"));
        assert!(!msg.contains("debug snapshot"));
    }

    #[test]
    fn build_optimize_user_message_truncates_large_trace() {
        let huge = "x".repeat(MAX_OPTIMIZE_HISTORY_BYTES + 5000);
        let msg = build_optimize_user_message(&huge, None, |s| s.to_string());
        assert!(msg.len() <= MAX_OPTIMIZE_HISTORY_BYTES + OPTIMIZE_USER_SUFFIX.len() + 120);
        assert!(msg.contains("truncated for analysis"));
    }

    #[test]
    fn build_optimize_system_prompt_includes_memory_scope() {
        let prompt = build_optimize_system_prompt(
            997894126,
            3,
            "20260626-164700.md",
            "http://10.0.1.217:8080/v1",
            "qwen3-coder:30b-a3b",
        );
        assert!(prompt.contains("chat_id=997894126"));
        assert!(prompt.contains("persona_id=3"));
        assert!(prompt.contains("Never pass a chat id as persona_id"));
    }

    #[test]
    fn pin_memory_tool_scope_overrides_wrong_ids() {
        let input = json!({"chat_id": 997894126, "persona_id": 997894126, "tier": 2});
        let pinned = pin_memory_tool_scope(input, 997894126, 3);
        assert_eq!(pinned["chat_id"], 997894126);
        assert_eq!(pinned["persona_id"], 3);
        assert_eq!(pinned["tier"], 2);
    }
}
