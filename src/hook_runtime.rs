use std::collections::HashSet;
use std::sync::Arc;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::db::{call_blocking, Database, HookDefinitionRecord};
use crate::error::FinallyAValueBotError;
use crate::hook_executor::{
    execute_command_hook, execute_prompt_hook, HookCommandPayload, HookOutput, HookPromptPayload,
};
use crate::safety_redaction::EnvSecretRedactor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEventName {
    BeforeTurn,
    PreToolUse,
    PostToolUse,
    PostToolBatch,
    PreStop,
    PostDelivery,
    PreDelivery,
}

impl HookEventName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeTurn => "BeforeTurn",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolBatch => "PostToolBatch",
            Self::PreStop => "PreStop",
            Self::PostDelivery => "PostDelivery",
            Self::PreDelivery => "PreDelivery",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookRunInput {
    pub chat_id: i64,
    pub persona_id: i64,
    pub caller_channel: String,
    pub is_scheduled_task: bool,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_output: Option<String>,
    pub tool_is_error: Option<bool>,
    pub stop_reason: Option<String>,
    pub assistant_text: Option<String>,
    pub runtime_signals: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct HookMemoryEffects {
    pub terminal_pz_post_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HookRunResult {
    pub blocked_reason: Option<String>,
    pub additional_contexts: Vec<String>,
    pub matched_hook_ids: Vec<i64>,
    pub updated_tool_input: Option<Value>,
    pub memory_effects: HookMemoryEffects,
    pub run_persona_focus_sync: bool,
    pub updated_assistant_text: Option<String>,
}

fn event_matches(record: &HookDefinitionRecord, event: HookEventName) -> bool {
    record
        .event_name
        .trim()
        .eq_ignore_ascii_case(event.as_str())
}

fn matcher_target(input: &HookRunInput, event: HookEventName) -> String {
    match event {
        HookEventName::PreToolUse | HookEventName::PostToolUse => {
            let mut parts = Vec::new();
            if let Some(name) = input.tool_name.as_deref() {
                parts.push(format!("tool_name={name}"));
            }
            if let Some(v) = input.tool_input.as_ref() {
                parts.push(format!(
                    "tool_input={}",
                    serde_json::to_string(v).unwrap_or_default()
                ));
            }
            if let Some(v) = input.tool_output.as_deref() {
                parts.push(format!("tool_output={v}"));
            }
            if let Some(v) = input.tool_is_error {
                parts.push(format!("tool_is_error={v}"));
            }
            parts.join("\n")
        }
        HookEventName::PreStop | HookEventName::PostDelivery | HookEventName::PreDelivery => input
            .stop_reason
            .clone()
            .or_else(|| input.assistant_text.clone())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn matcher_matches(matcher: Option<&str>, input: &HookRunInput, event: HookEventName) -> bool {
    let Some(matcher) = matcher.map(str::trim).filter(|m| !m.is_empty()) else {
        return true;
    };
    let target = matcher_target(input, event);
    if matcher == "*" {
        return true;
    }
    Regex::new(matcher)
        .map(|re| re.is_match(&target))
        .unwrap_or(false)
}

fn hook_scope_matches_persona(record: &HookDefinitionRecord, persona_id: i64) -> bool {
    let Some(scoped_persona_ids) = record.scoped_persona_ids.as_ref() else {
        return true;
    };
    scoped_persona_ids.contains(&persona_id)
}

fn parse_payload(payload_json: &str) -> Value {
    serde_json::from_str(payload_json).unwrap_or(Value::Null)
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn signal_bool(input: &HookRunInput, key: &str) -> bool {
    input
        .runtime_signals
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn signal_u64(input: &HookRunInput, key: &str) -> u64 {
    input
        .runtime_signals
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

fn build_hook_input_json(
    event: HookEventName,
    hook: &HookDefinitionRecord,
    input: &HookRunInput,
) -> String {
    serde_json::json!({
        "event": event.as_str(),
        "hook": { "id": hook.id, "name": hook.name },
        "chat_id": input.chat_id,
        "persona_id": input.persona_id,
        "channel": input.caller_channel,
        "is_scheduled_task": input.is_scheduled_task,
        "tool_name": input.tool_name,
        "tool_input": input.tool_input,
        "tool_output": input.tool_output,
        "tool_is_error": input.tool_is_error,
        "stop_reason": input.stop_reason,
        "assistant_text": input.assistant_text,
        "runtime_signals": input.runtime_signals,
    })
    .to_string()
}

pub fn run_hooks_for_event(
    _db: &Database,
    _chat_id: i64,
    _persona_id: i64,
    _event: HookEventName,
    _input: &HookRunInput,
) -> Result<HookRunResult, FinallyAValueBotError> {
    Ok(HookRunResult::default())
}

pub async fn run_hooks_for_event_async(
    db: Arc<Database>,
    config: &Config,
    env_redactor: &EnvSecretRedactor,
    event: HookEventName,
    input: &HookRunInput,
) -> Result<HookRunResult, FinallyAValueBotError> {
    let chat_id = input.chat_id;
    let persona_id = input.persona_id;
    let input_cloned = input.clone();
    let hooks = call_blocking(db.clone(), move |db| {
        let hooks = db.list_hook_definitions()?;
        let mut matched = Vec::new();
        for hook in hooks {
            if !hook.enabled || !event_matches(&hook, event) {
                continue;
            }
            if !hook_scope_matches_persona(&hook, persona_id) {
                continue;
            }
            if !db.is_hook_allowed_for_persona(chat_id, persona_id, hook.id)? {
                continue;
            }
            if !matcher_matches(hook.matcher.as_deref(), &input_cloned, event) {
                continue;
            }
            matched.push(hook);
        }
        Ok::<Vec<HookDefinitionRecord>, FinallyAValueBotError>(matched)
    })
    .await?;

    let mut out = HookRunResult::default();
    let mut seen_post_ids = HashSet::new();

    let apply_output = |out: &mut HookRunResult, output: HookOutput, hook_name: &str| {
        let permission = output.permission.unwrap_or_else(|| "allow".to_string());
        let reason = output
            .reason
            .or(output.user_message)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("Blocked by hook '{hook_name}'"));
        if permission.eq_ignore_ascii_case("deny") || permission.eq_ignore_ascii_case("ask") {
            out.blocked_reason = Some(reason);
        }
        if let Some(ctx) = output
            .additional_context
            .or(output.agent_message)
            .filter(|s| !s.trim().is_empty())
        {
            out.additional_contexts.push(ctx);
        }
        if output.updated_tool_input.is_some() {
            out.updated_tool_input = output.updated_tool_input;
        }
        if let Some(text) = output
            .updated_assistant_text
            .filter(|s| !s.trim().is_empty())
        {
            out.updated_assistant_text = Some(text);
        }
        if let Some(effects) = output.effects.and_then(|e| e.memory_tier3_prune) {
            for id in effects.terminal_pz_post_ids {
                out.memory_effects.terminal_pz_post_ids.push(id);
            }
        }
    };

    for hook in hooks {
        out.matched_hook_ids.push(hook.id);
        let payload = parse_payload(&hook.action_payload_json);
        let hook_input_json = build_hook_input_json(event, &hook, input);
        match hook.action_type.trim().to_ascii_lowercase().as_str() {
            "block" => {
                let reason = payload_string(&payload, "reason")
                    .unwrap_or_else(|| format!("Blocked by hook '{}'", hook.name));
                out.blocked_reason = Some(reason);
                break;
            }
            "add_context" => {
                if let Some(text) = payload_string(&payload, "additional_context")
                    .or_else(|| payload_string(&payload, "context"))
                {
                    out.additional_contexts.push(text);
                }
            }
            "command" => {
                let payload: HookCommandPayload = serde_json::from_value(payload).map_err(|e| {
                    FinallyAValueBotError::ToolExecution(format!(
                        "invalid command hook payload for '{}': {e}",
                        hook.name
                    ))
                })?;
                let output = execute_command_hook(config, &payload, &hook_input_json).await?;
                apply_output(&mut out, output, &hook.name);
                if out.blocked_reason.is_some() {
                    break;
                }
            }
            "prompt" => {
                let payload: HookPromptPayload = serde_json::from_value(payload).map_err(|e| {
                    FinallyAValueBotError::ToolExecution(format!(
                        "invalid prompt hook payload for '{}': {e}",
                        hook.name
                    ))
                })?;
                let output =
                    execute_prompt_hook(config, env_redactor, &payload, &hook_input_json).await?;
                apply_output(&mut out, output, &hook.name);
                if out.blocked_reason.is_some() {
                    break;
                }
            }
            "builtin_persona_focus_sync" => {
                out.run_persona_focus_sync = true;
            }
            "builtin_scheduler_policy_context" => {
                if input.is_scheduled_task {
                    out.additional_contexts.push(
                        "Scheduled run policy: send one final assistant reply only; do not call send_message for this chat."
                            .to_string(),
                    );
                }
            }
            "builtin_turn_skill_gate" => {
                if signal_bool(input, "requires_schedule_skill") {
                    out.blocked_reason = Some(
                        "skill_required: schedule_task and update_scheduled_task require activating the `schedule-job` skill first in this turn. Call `activate_skill` with skill_name `schedule-job`, follow its preflight (including timezone handling), then call the scheduling tool.".to_string()
                    );
                    break;
                }
                if signal_bool(input, "requires_modify_skill") {
                    out.blocked_reason = Some(format!(
                        "skill_required: {}",
                        crate::skill_activation_gate::modify_skill_required_error_message()
                    ));
                    break;
                }
            }
            "builtin_deferred_commitment_guard" => {
                if input
                    .stop_reason
                    .as_deref()
                    .map(|s| s == "ask_clarification")
                    .unwrap_or(false)
                {
                    break;
                }
                let should_reject = signal_bool(input, "deferred_commitment_should_reject");
                let nudge_count = signal_u64(input, "deferred_commitment_nudges");
                let nudge_max = signal_u64(input, "deferred_commitment_max_nudges");
                let can_continue = signal_bool(input, "can_continue_iteration");
                if should_reject && can_continue && nudge_count < nudge_max {
                    out.blocked_reason = Some(
                        "deferred_commitment: You ended your turn while promising further work. Either call a tool now (read_agent_history, read_file, glob, list_cursor_agent_runs, etc.) or give a final answer with what you already know—do not say you are checking something without a tool call."
                            .to_string(),
                    );
                    break;
                }
            }
            "builtin_loop_guard" => {
                if signal_bool(input, "force_stall_present") {
                    continue;
                }
                let legacy_count = signal_u64(input, "legacy_edit_without_block_count");
                if legacy_count >= signal_u64(input, "legacy_edit_hint_threshold").max(2) {
                    out.additional_contexts.push("Routing hint: you are repeatedly editing files via write_file/edit_file without using apply_search_replace. For non-trivial code edits, first call read_repo_map, then use apply_search_replace with explicit SEARCH/REPLACE blocks; keep edit_file as fallback only.".to_string());
                }
                let discovery_count = signal_u64(input, "discovery_streak_count");
                let discovery_hint = signal_u64(input, "discovery_streak_hint_threshold").max(1);
                let discovery_stall = signal_u64(input, "discovery_streak_stall_threshold").max(2);
                if discovery_count >= discovery_hint {
                    out.additional_contexts.push("Routing hint: you are in a discovery/search loop (list tasks, list cursor runs, broad grep/find). For job status use `read_tiered_memory` and `list_cursor_agent_runs`; for files use `glob` with a pattern (e.g. PZ-*.png) or `read_file` on a known path — not recursive shell grep over `shared/`.".to_string());
                }
                if discovery_count >= discovery_stall {
                    out.blocked_reason = Some(
                        "stall_response: I stopped because this run kept searching without making progress (repeated status/list/grep steps). Tell me what you need in one line — e.g. show the latest PZ image, check a specific background job id, or retry generation — and I will use a direct path (tiered memory, glob, or a single read) instead of scanning the tree."
                            .to_string(),
                    );
                    break;
                }
            }
            "builtin_secret_vault_scrub" => {
                let data_dir = config.runtime_data_dir();
                let chat_id = input.chat_id;
                let _ = call_blocking(db.clone(), move |db| {
                    crate::secret_vault::drain_pending_scrubs(db, &data_dir, chat_id);
                    Ok(())
                })
                .await;
            }
            "builtin_dense_delivery_guard" => {
                if let Some(text) = input.assistant_text.as_deref() {
                    if let Some(updated) = crate::dense_delivery_guard::maybe_apply_dense_delivery(
                        config,
                        db.as_ref(),
                        input.chat_id,
                        input.persona_id,
                        &input.caller_channel,
                        text,
                        env_redactor,
                    )
                    .await
                    {
                        out.updated_assistant_text = Some(updated);
                    }
                }
            }
            _ => {}
        }
    }
    out.memory_effects
        .terminal_pz_post_ids
        .retain(|id| seen_post_ids.insert(id.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Arc<Database> {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_hook_runtime_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Arc::new(Database::new(root.to_str().unwrap()).unwrap())
    }

    #[test]
    fn default_policy_allows_hook() {
        let db = test_db();
        let chat_id = 9001;
        let persona_id = db
            .create_persona(chat_id, "default", None)
            .expect("create persona");
        let hook_id = db
            .upsert_hook_definition(
                None,
                "block-bash",
                HookEventName::PreToolUse.as_str(),
                // Matcher is tested against the composite target ("tool_name=bash …").
                Some("tool_name=bash"),
                "block",
                r#"{"reason":"blocked"}"#,
                None,
                true,
            )
            .expect("upsert hook");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PreToolUse,
                &HookRunInput {
                    chat_id,
                    persona_id,
                    tool_name: Some("bash".to_string()),
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert_eq!(out.blocked_reason.as_deref(), Some("blocked"));
        assert!(out.matched_hook_ids.contains(&hook_id));
    }

    #[test]
    fn persona_allowlist_blocks_unassigned_hook() {
        let db = test_db();
        let chat_id = 9002;
        let persona_id = db
            .create_persona(chat_id, "default", None)
            .expect("create persona");
        let hook_id = db
            .upsert_hook_definition(
                None,
                "add-note",
                HookEventName::PostToolBatch.as_str(),
                None,
                "add_context",
                r#"{"additional_context":"ctx"}"#,
                None,
                true,
            )
            .expect("upsert hook");
        db.set_persona_hook_skill_policy(chat_id, persona_id, Some(&[]), None)
            .expect("set policy");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PostToolBatch,
                &HookRunInput {
                    chat_id,
                    persona_id,
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert!(out.additional_contexts.is_empty());
        assert!(!out.matched_hook_ids.contains(&hook_id));
    }

    #[test]
    fn persona_scope_blocks_other_personas_even_with_default_policy() {
        let db = test_db();
        let chat_id = 9006;
        let owner_persona_id = db
            .create_persona(chat_id, "owner", None)
            .expect("create owner persona");
        let other_persona_id = db
            .create_persona(chat_id, "other", None)
            .expect("create other persona");
        let hook_id = db
            .upsert_hook_definition(
                None,
                "owner-only-hook",
                HookEventName::PostToolBatch.as_str(),
                None,
                "add_context",
                r#"{"additional_context":"owner-only"}"#,
                Some(&[owner_persona_id]),
                true,
            )
            .expect("upsert hook");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PostToolBatch,
                &HookRunInput {
                    chat_id,
                    persona_id: other_persona_id,
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert!(out.additional_contexts.is_empty());
        assert!(!out.matched_hook_ids.contains(&hook_id));
    }

    #[test]
    fn command_and_prompt_action_types_supported() {
        let db = test_db();
        let chat_id = 9003;
        let persona_id = db
            .create_persona(chat_id, "default", None)
            .expect("create persona");
        db.upsert_hook_definition(
            None,
            "hook-command-demo",
            HookEventName::PreToolUse.as_str(),
            None,
            "command",
            r#"{"command":"missing.sh","fail_closed":false}"#,
            None,
            false,
        )
        .expect("upsert hook");
        db.upsert_hook_definition(
            None,
            "hook-prompt-demo",
            HookEventName::PreToolUse.as_str(),
            None,
            "prompt",
            r#"{"prompt":"Return {\"permission\":\"allow\"}","fail_closed":false}"#,
            None,
            false,
        )
        .expect("upsert hook");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PreToolUse,
                &HookRunInput {
                    chat_id,
                    persona_id,
                    tool_name: Some("bash".to_string()),
                    tool_input: Some(serde_json::json!({"cmd":"echo ok"})),
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert!(out.blocked_reason.is_none());
    }

    #[test]
    fn builtin_persona_focus_sync_sets_flag() {
        let db = test_db();
        let chat_id = 9004;
        let persona_id = db
            .create_persona(chat_id, "default", None)
            .expect("create persona");
        db.upsert_hook_definition(
            None,
            "postdelivery-focus-sync",
            HookEventName::PostDelivery.as_str(),
            None,
            "builtin_persona_focus_sync",
            "{}",
            None,
            true,
        )
        .expect("upsert hook");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PostDelivery,
                &HookRunInput {
                    chat_id,
                    persona_id,
                    stop_reason: Some("end_turn".to_string()),
                    assistant_text: Some("done".to_string()),
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert!(out.run_persona_focus_sync);
    }

    #[test]
    fn builtin_turn_skill_gate_blocks_missing_activation() {
        let db = test_db();
        let chat_id = 9005;
        let persona_id = db
            .create_persona(chat_id, "default", None)
            .expect("create persona");
        db.upsert_hook_definition(
            None,
            "pretool-turn-skill-gate-test",
            HookEventName::PreToolUse.as_str(),
            None,
            "builtin_turn_skill_gate",
            "{}",
            None,
            true,
        )
        .expect("upsert hook");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let out = rt
            .block_on(run_hooks_for_event_async(
                db.clone(),
                &crate::config::test_config(),
                &crate::safety_redaction::EnvSecretRedactor::empty(),
                HookEventName::PreToolUse,
                &HookRunInput {
                    chat_id,
                    persona_id,
                    tool_name: Some("schedule_task".to_string()),
                    runtime_signals: Some(serde_json::json!({
                        "requires_schedule_skill": true
                    })),
                    ..HookRunInput::default()
                },
            ))
            .expect("run hooks");
        assert!(out
            .blocked_reason
            .as_deref()
            .unwrap_or_default()
            .starts_with("skill_required:"));
    }
}
