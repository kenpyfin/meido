//! Shared PreToolUse / PostToolUse / PostToolBatch dispatch for Cursor MCP and future Classic reuse.

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::agent_history::{truncate_preview, ToolCallRecord};
use crate::channels::telegram::{
    hook_event_summary, publish_hook_event, AgentEvent, AgentRequestContext, AppState,
};
use crate::hook_actions::apply_hook_memory_effects;
use crate::hook_runtime::{run_hooks_for_event_async, HookEventName, HookRunInput, HookRunResult};
use crate::tools::{self, ToolAuthContext, ToolResult};

pub const REQUIRED_SCHEDULING_SKILL: &str = "schedule-job";
pub const TOOL_EXECUTION_TIMEOUT_SECS: u64 = 3600;
const DISCOVERY_STREAK_HINT_THRESHOLD: usize = 15;
const DISCOVERY_STREAK_STALL_THRESHOLD: usize = 20;

pub struct ToolHookDispatchContext<'a> {
    pub state: &'a AppState,
    pub context: &'a AgentRequestContext<'a>,
    pub run_key: &'a str,
    pub event_tx: Option<&'a UnboundedSender<AgentEvent>>,
    pub tool_auth: &'a ToolAuthContext,
    pub schedule_skill_activated: &'a mut bool,
    pub modify_skill_activated: &'a mut bool,
    pub discovery_streak_count: &'a mut usize,
    pub legacy_edit_without_block_count: &'a mut usize,
    pub executed_tool_names: &'a mut Vec<String>,
    pub executed_tool_inputs: &'a mut Vec<(String, serde_json::Value)>,
    pub force_stall_response: &'a mut Option<String>,
    pub history_hook_events: &'a mut Vec<String>,
}

pub struct ToolHookDispatchOutcome {
    pub result: ToolResult,
    pub blocked: bool,
    pub record: ToolCallRecord,
}

pub struct PostToolBatchOutcome {
    pub hook: HookRunResult,
    pub stall_response: Option<String>,
}

pub async fn dispatch_tool_with_hooks(
    ctx: &mut ToolHookDispatchContext<'_>,
    tool_name: &str,
    mut tool_input: serde_json::Value,
) -> ToolHookDispatchOutcome {
    let chat_id = ctx.context.chat_id;
    let persona_id = ctx.context.persona_id;
    let started = Instant::now();

    let skills_data_dir = ctx.state.config.skills_data_dir_absolute();
    let tool_shared_dir =
        tools::resolve_tool_working_dir(Path::new(ctx.state.config.working_dir()));

    let log_input = crate::secret_vault::mask_tool_input_for_log(tool_name, &tool_input);
    let missing_schedule_skill = (tool_name == "schedule_task"
        || tool_name == "update_scheduled_task")
        && !*ctx.schedule_skill_activated;
    let missing_modify_skill = crate::skill_activation_gate::requires_modify_skill_activation(
        tool_name,
        &tool_input,
        &skills_data_dir,
        &tool_shared_dir,
    ) && !*ctx.modify_skill_activated;

    let pre_tool_hook = run_hooks_for_event_async(
        ctx.state.db.clone(),
        &ctx.state.config,
        ctx.state.env_redactor.as_ref(),
        HookEventName::PreToolUse,
        &HookRunInput {
            chat_id,
            persona_id,
            caller_channel: ctx.context.caller_channel.to_string(),
            is_scheduled_task: ctx.context.is_scheduled_task,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(log_input.clone()),
            runtime_signals: Some(serde_json::json!({
                "requires_schedule_skill": missing_schedule_skill,
                "requires_modify_skill": missing_modify_skill,
            })),
            ..HookRunInput::default()
        },
    )
    .await
    .ok();

    if let Some(hook) = pre_tool_hook.as_ref() {
        publish_hook_event(
            ctx.state,
            ctx.event_tx,
            ctx.run_key,
            chat_id,
            persona_id,
            HookEventName::PreToolUse,
            Some(tool_name),
            hook,
        )
        .await;
        if let Some(summary) = hook_event_summary(HookEventName::PreToolUse, Some(tool_name), hook)
        {
            ctx.history_hook_events.push(summary);
        }
        if let Some(reason) = hook.blocked_reason.as_deref() {
            let result = if let Some(skill_msg) = reason.strip_prefix("skill_required: ") {
                ToolResult::error(skill_msg.to_string()).with_error_type("skill_required")
            } else {
                ToolResult::error(format!("[Tool blocked by hook] {reason}"))
                    .with_error_type("hook_block")
            };
            let record = ToolCallRecord {
                name: tool_name.to_string(),
                input_preview: ctx.state.env_redactor.redact(&truncate_preview(
                    &serde_json::to_string(&log_input).unwrap_or_default(),
                    10000,
                )),
                result_preview: reason.to_string(),
                duration_ms: started.elapsed().as_millis(),
                is_error: true,
            };
            return ToolHookDispatchOutcome {
                result,
                blocked: true,
                record,
            };
        }
        if let Some(updated) = hook.updated_tool_input.clone() {
            if !crate::secret_vault::tool_input_is_sensitive(tool_name) {
                tool_input = updated;
            }
        }
    }

    let requested_skill_name = tool_input
        .get("skill_name")
        .and_then(|v| v.as_str())
        .map(str::trim);
    let activates_required_schedule_skill = tool_name == "activate_skill"
        && requested_skill_name
            .map(|skill| skill.eq_ignore_ascii_case(REQUIRED_SCHEDULING_SKILL))
            .unwrap_or(false);
    let activates_required_modify_skill = tool_name == "activate_skill"
        && requested_skill_name
            .map(|skill| {
                skill.eq_ignore_ascii_case(crate::skill_activation_gate::REQUIRED_MODIFY_SKILL)
            })
            .unwrap_or(false);

    let result = match tokio::time::timeout(
        Duration::from_secs(TOOL_EXECUTION_TIMEOUT_SECS),
        ctx.state
            .tools
            .execute_with_auth(tool_name, tool_input.clone(), ctx.tool_auth),
    )
    .await
    {
        Ok(tool_result) => tool_result,
        Err(_) => ToolResult::error(format!(
            "Tool execution timed out after {TOOL_EXECUTION_TIMEOUT_SECS}s."
        ))
        .with_error_type("timeout"),
    };

    if activates_required_schedule_skill && !result.is_error {
        *ctx.schedule_skill_activated = true;
    }
    if activates_required_modify_skill && !result.is_error {
        *ctx.modify_skill_activated = true;
    }

    let record = ToolCallRecord {
        name: tool_name.to_string(),
        input_preview: ctx.state.env_redactor.redact(&truncate_preview(
            &serde_json::to_string(&log_input).unwrap_or_default(),
            10000,
        )),
        result_preview: ctx
            .state
            .env_redactor
            .redact(&truncate_preview(&result.content, 10000)),
        duration_ms: result
            .duration_ms
            .unwrap_or_else(|| started.elapsed().as_millis()),
        is_error: result.is_error,
    };

    if let Ok(post_tool_hook) = run_hooks_for_event_async(
        ctx.state.db.clone(),
        &ctx.state.config,
        ctx.state.env_redactor.as_ref(),
        HookEventName::PostToolUse,
        &HookRunInput {
            chat_id,
            persona_id,
            caller_channel: ctx.context.caller_channel.to_string(),
            is_scheduled_task: ctx.context.is_scheduled_task,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(log_input.clone()),
            tool_output: Some(result.content.clone()),
            tool_is_error: Some(result.is_error),
            ..HookRunInput::default()
        },
    )
    .await
    {
        publish_hook_event(
            ctx.state,
            ctx.event_tx,
            ctx.run_key,
            chat_id,
            persona_id,
            HookEventName::PostToolUse,
            Some(tool_name),
            &post_tool_hook,
        )
        .await;
        if let Some(summary) =
            hook_event_summary(HookEventName::PostToolUse, Some(tool_name), &post_tool_hook)
        {
            ctx.history_hook_events.push(summary);
        }
        if let Some(reason) = post_tool_hook.blocked_reason.as_deref() {
            *ctx.force_stall_response = Some(format!(
                "I paused due to post-tool hook policy after `{tool_name}`: {reason}"
            ));
        }
        apply_hook_memory_effects(
            &ctx.state.memory,
            chat_id,
            persona_id,
            &post_tool_hook.memory_effects,
        );
    }

    let used_legacy_edit = matches!(tool_name, "write_file" | "edit_file");
    let used_block_edit = tool_name == "apply_search_replace";
    if used_legacy_edit && !used_block_edit {
        *ctx.legacy_edit_without_block_count =
            ctx.legacy_edit_without_block_count.saturating_add(1);
    } else if used_legacy_edit || used_block_edit {
        *ctx.legacy_edit_without_block_count = 0;
    }

    let iteration_had_progress = is_progress_tool_use(tool_name);
    let iteration_had_discovery = is_discovery_tool_use(tool_name, &tool_input);
    if iteration_had_progress {
        *ctx.discovery_streak_count = 0;
    } else if iteration_had_discovery {
        *ctx.discovery_streak_count = ctx.discovery_streak_count.saturating_add(1);
    }

    ctx.executed_tool_names.push(tool_name.to_string());
    ctx.executed_tool_inputs.push((
        tool_name.to_string(),
        if crate::secret_vault::tool_input_is_sensitive(tool_name) {
            log_input
        } else {
            tool_input
        },
    ));

    ToolHookDispatchOutcome {
        result,
        blocked: false,
        record,
    }
}

pub async fn run_post_tool_batch_hooks(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    run_key: &str,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    discovery_streak_count: usize,
    legacy_edit_without_block_count: usize,
    force_stall_present: bool,
    history_hook_events: &mut Vec<String>,
) -> PostToolBatchOutcome {
    let chat_id = context.chat_id;
    let persona_id = context.persona_id;
    let hook = run_hooks_for_event_async(
        state.db.clone(),
        &state.config,
        state.env_redactor.as_ref(),
        HookEventName::PostToolBatch,
        &HookRunInput {
            chat_id,
            persona_id,
            caller_channel: context.caller_channel.to_string(),
            is_scheduled_task: context.is_scheduled_task,
            runtime_signals: Some(serde_json::json!({
                "legacy_edit_without_block_count": legacy_edit_without_block_count,
                "legacy_edit_hint_threshold": 2,
                "discovery_streak_count": discovery_streak_count,
                "discovery_streak_hint_threshold": DISCOVERY_STREAK_HINT_THRESHOLD,
                "discovery_streak_stall_threshold": DISCOVERY_STREAK_STALL_THRESHOLD,
                "force_stall_present": force_stall_present
            })),
            ..HookRunInput::default()
        },
    )
    .await
    .unwrap_or_else(|_| HookRunResult::default());

    publish_hook_event(
        state,
        event_tx,
        run_key,
        chat_id,
        persona_id,
        HookEventName::PostToolBatch,
        None,
        &hook,
    )
    .await;
    if let Some(summary) = hook_event_summary(HookEventName::PostToolBatch, None, &hook) {
        history_hook_events.push(summary);
    }

    let stall_response = hook.blocked_reason.as_deref().map(|reason| {
        if let Some(stall) = reason.strip_prefix("stall_response: ") {
            stall.to_string()
        } else {
            format!("I paused due to post-tool-batch hook policy: {reason}")
        }
    });

    PostToolBatchOutcome {
        hook,
        stall_response,
    }
}

fn is_progress_tool_use(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "spawn_background_command"
            | "write_file"
            | "edit_file"
            | "apply_search_replace"
            | "symbol_edit"
            | "read_tiered_memory"
            | "read_memory"
            | "write_tiered_memory"
            | "write_memory_state"
            | "schedule_task"
            | "register_tracked_job"
            | "update_bulletin_focus"
            | "add_todo"
            | "list_todos"
    )
}

fn is_discovery_tool_use(tool_name: &str, input: &serde_json::Value) -> bool {
    match tool_name {
        "list_scheduled_tasks" | "list_cursor_agent_runs" => true,
        "grep" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .trim();
            let has_glob = input
                .get("glob")
                .and_then(|v| v.as_str())
                .is_some_and(|g| !g.trim().is_empty());
            !has_glob || path == "." || path.ends_with("shared") || path.ends_with("shared/")
        }
        "glob" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .trim();
            path == "." || path.ends_with("shared") || path.ends_with("shared/")
        }
        "bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            crate::tools::bash_safety::is_expensive_shell_search(cmd)
                || cmd.to_ascii_lowercase().contains("find ")
        }
        _ => false,
    }
}
