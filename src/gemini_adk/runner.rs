//! Native in-process Gemini ADK multi-agent runner.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::agent::{
    pipeline_finish_turn, AgentEvent, AgentProcessResult, AgentRequestContext, AgentRunPrep,
    AppState,
};
use crate::agent_history::{
    EvaluatorStepRecord, IterationRecord, PipelineFinishExtras, PipelineStageRecord, ToolCallRecord,
};
use crate::claude::{
    ContentBlock, Message, MessageContent, MessagesResponse, ResponseContentBlock, ToolDefinition,
};
use crate::config::Config;
use crate::db::call_blocking;
use crate::gemini_adk::agent::{
    exit_loop_tool_definition, is_workflow, resolve_agent_model, transfer_tool_definition,
    EXIT_LOOP_TOOL, TRANSFER_TOOL,
};
use crate::gemini_adk::config::{GeminiAdkSettings, DEFAULT_GEMINI_ADK_MODEL};
use crate::gemini_adk::profile::{AdkAgent, AdkAgentKind, AdkTopologyProfile};
use crate::llm::{create_provider, LlmProvider};
use crate::tool_hook_dispatch::{
    dispatch_tool_with_hooks, run_post_tool_batch_hooks, ToolHookDispatchContext,
};

type ProviderCache = HashMap<String, Box<dyn LlmProvider>>;

#[derive(Default)]
struct RunScratch {
    schedule_skill_activated: bool,
    modify_skill_activated: bool,
    discovery_streak_count: usize,
    legacy_edit_without_block_count: usize,
    force_stall_response: Option<String>,
    executed_tool_names: Vec<String>,
    executed_tool_inputs: Vec<(String, serde_json::Value)>,
    history_hook_events: Vec<String>,
    tool_call_records: Vec<ToolCallRecord>,
    pipeline_stages: Vec<PipelineStageRecord>,
    cloud_calls: u32,
    history_iterations: Vec<IterationRecord>,
    exit_loop_requested: bool,
}

pub async fn run_gemini_adk_engine(
    state: &AppState,
    context: AgentRequestContext<'_>,
    prep: AgentRunPrep,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<AgentProcessResult> {
    let run_start = Instant::now();
    let settings = state
        .gemini_adk_settings
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| GeminiAdkSettings::from_env(&state.config));

    if !GeminiAdkSettings::api_key_configured(&state.config) {
        let notice = "Gemini ADK engine is selected but no Gemini API key is configured. Add GEMINI_API_KEY to .env, or save a key under Settings → Connections, then retry. You can also switch this persona to Single turn.".to_string();
        return finish_adk_turn(
            state,
            &context,
            &prep,
            event_tx,
            notice,
            "end_turn",
            RunScratch::default(),
            run_start,
            "gemini-adk",
        )
        .await;
    }

    let topology = load_topology_for_run(state, context.chat_id, context.persona_id).await?;
    let root_id = topology.root_agent_id.clone();
    let mut session: HashMap<String, serde_json::Value> = HashMap::new();
    session.insert("user_request".into(), json!(prep.latest_user_text.clone()));

    let mut providers: ProviderCache = HashMap::new();
    let mut scratch = RunScratch::default();

    let final_text = match execute_agent(
        state,
        &context,
        &prep,
        event_tx,
        cancel.as_ref(),
        &topology,
        &settings,
        &root_id,
        &mut session,
        &mut providers,
        &mut scratch,
        0,
    )
    .await
    {
        Ok(text) => text,
        Err(e) => {
            warn!(error = %e, "Gemini ADK engine failed");
            format!("Gemini ADK engine error: {e}")
        }
    };

    let delivery = if final_text.trim().is_empty() {
        session
            .get("final")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Done.".to_string())
    } else {
        final_text
    };

    let model_label = topology.default_model.clone();
    finish_adk_turn(
        state,
        &context,
        &prep,
        event_tx,
        delivery,
        "end_turn",
        scratch,
        run_start,
        &model_label,
    )
    .await
}

async fn load_topology_for_run(
    state: &AppState,
    chat_id: i64,
    persona_id: i64,
) -> anyhow::Result<AdkTopologyProfile> {
    let persona_json = call_blocking(state.db.clone(), move |db| {
        Ok(db
            .get_persona(persona_id)?
            .filter(|p| p.chat_id == chat_id)
            .and_then(|p| p.gemini_adk_topology))
    })
    .await
    .unwrap_or(None);

    if let Some(raw) = persona_json.filter(|s| !s.trim().is_empty()) {
        match crate::gemini_adk::profile::parse_topology_json(&raw) {
            Ok(p) => return Ok(p),
            Err(e) => {
                warn!(error = %e, "persona gemini_adk_topology invalid; falling back to global");
            }
        }
    }

    let global = state
        .gemini_adk_profile
        .read()
        .map_err(|_| anyhow::anyhow!("gemini_adk_profile lock poisoned"))?
        .clone();
    Ok(global)
}

fn gemini_provider_for_model(config: &Config, model: &str) -> Box<dyn LlmProvider> {
    let mut cfg = config.clone();
    cfg.llm_provider = "google".into();
    cfg.model = if model.trim().is_empty() {
        DEFAULT_GEMINI_ADK_MODEL.to_string()
    } else {
        model.trim().to_string()
    };
    cfg.api_key =
        crate::llm_catalog::resolve_api_key_for_provider_with_config("google", Some(config));
    create_provider(&cfg)
}

fn ensure_provider(cache: &mut ProviderCache, config: &Config, model: &str) -> String {
    let key = if model.trim().is_empty() {
        DEFAULT_GEMINI_ADK_MODEL.to_string()
    } else {
        model.trim().to_string()
    };
    if !cache.contains_key(&key) {
        cache.insert(key.clone(), gemini_provider_for_model(config, &key));
    }
    key
}

fn filter_tools(all: &[ToolDefinition], allowed: Option<&[String]>) -> Vec<ToolDefinition> {
    match allowed {
        None => all.to_vec(),
        Some(names) => {
            let set: std::collections::HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
            all.iter()
                .filter(|t| set.contains(t.name.as_str()))
                .cloned()
                .collect()
        }
    }
}

fn cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
}

#[async_recursion::async_recursion]
async fn execute_agent(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    prep: &AgentRunPrep,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    cancel: Option<&Arc<AtomicBool>>,
    topology: &AdkTopologyProfile,
    settings: &GeminiAdkSettings,
    agent_id: &str,
    session: &mut HashMap<String, serde_json::Value>,
    providers: &mut ProviderCache,
    scratch: &mut RunScratch,
    depth: usize,
) -> anyhow::Result<String> {
    if depth > 12 {
        return Err(anyhow::anyhow!("ADK agent recursion depth exceeded"));
    }
    if cancelled(cancel) {
        return Ok("Cancelled.".into());
    }

    let agent = topology
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| anyhow::anyhow!("unknown agent id '{agent_id}'"))?
        .clone();

    if !agent.enabled {
        return Ok(String::new());
    }

    let stage_start = Instant::now();
    let text = match agent.kind {
        AdkAgentKind::Llm => {
            run_llm_agent(
                state, context, prep, event_tx, cancel, topology, settings, &agent, session,
                providers, scratch, depth,
            )
            .await?
        }
        AdkAgentKind::Sequential => {
            let mut last = String::new();
            for child in &agent.sub_agents {
                if cancelled(cancel) {
                    break;
                }
                last = execute_agent(
                    state,
                    context,
                    prep,
                    event_tx,
                    cancel,
                    topology,
                    settings,
                    child,
                    session,
                    providers,
                    scratch,
                    depth + 1,
                )
                .await?;
            }
            last
        }
        AdkAgentKind::Parallel => {
            // Sequential join with shared session for safety (tool identity is shared).
            let mut parts = Vec::new();
            for child in &agent.sub_agents {
                if cancelled(cancel) {
                    break;
                }
                let out = execute_agent(
                    state,
                    context,
                    prep,
                    event_tx,
                    cancel,
                    topology,
                    settings,
                    child,
                    session,
                    providers,
                    scratch,
                    depth + 1,
                )
                .await?;
                if !out.trim().is_empty() {
                    parts.push(format!("[{child}] {out}"));
                }
            }
            parts.join("\n\n")
        }
        AdkAgentKind::Loop => {
            let max = agent.max_iterations.unwrap_or(5).clamp(1, 50);
            let mut last = String::new();
            for i in 0..max {
                if cancelled(cancel) {
                    break;
                }
                scratch.exit_loop_requested = false;
                for child in &agent.sub_agents {
                    last = execute_agent(
                        state,
                        context,
                        prep,
                        event_tx,
                        cancel,
                        topology,
                        settings,
                        child,
                        session,
                        providers,
                        scratch,
                        depth + 1,
                    )
                    .await?;
                    if scratch.exit_loop_requested {
                        break;
                    }
                }
                if scratch.exit_loop_requested {
                    info!(agent = %agent.id, iteration = i, "ADK loop exited via exit_loop");
                    break;
                }
            }
            last
        }
    };

    if let Some(key) = agent
        .output_key
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        session.insert(key.to_string(), json!(text.clone()));
    }

    scratch.pipeline_stages.push(PipelineStageRecord {
        stage: format!("agent:{}", agent.id),
        detail: format!("{} ({})", agent.label, agent.kind.as_str()),
        duration_ms: stage_start.elapsed().as_millis(),
    });

    Ok(text)
}

async fn run_llm_agent(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    prep: &AgentRunPrep,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    cancel: Option<&Arc<AtomicBool>>,
    topology: &AdkTopologyProfile,
    settings: &GeminiAdkSettings,
    agent: &AdkAgent,
    session: &mut HashMap<String, serde_json::Value>,
    providers: &mut ProviderCache,
    scratch: &mut RunScratch,
    depth: usize,
) -> anyhow::Result<String> {
    let model = resolve_agent_model(agent, &topology.default_model);
    let max_iters = agent
        .max_iterations
        .unwrap_or(settings.max_iterations)
        .clamp(1, 100);

    let registry_tools = state.tools.definitions();
    let mut tools = filter_tools(&registry_tools, agent.allowed_tools.as_deref());
    let transferable: Vec<String> = agent
        .sub_agents
        .iter()
        .filter(|id| {
            topology
                .agents
                .iter()
                .any(|a| a.id == **id && a.enabled && !is_workflow(a.kind))
        })
        .cloned()
        .collect();
    if !transferable.is_empty() {
        tools.push(transfer_tool_definition(&transferable));
    }
    // Always allow exit_loop so nested loops can terminate from any Llm child.
    tools.push(exit_loop_tool_definition());

    let mut system = prep.system_prompt.clone();
    if !agent.instruction.trim().is_empty() {
        system.push_str("\n\n# ADK agent instruction\n");
        system.push_str(agent.instruction.trim());
    }
    if !session.is_empty() {
        system.push_str("\n\n# ADK session state (JSON)\n");
        system.push_str(&serde_json::to_string_pretty(session).unwrap_or_else(|_| "{}".into()));
    }
    system.push_str(&format!(
        "\n\n# Current ADK agent\nYou are agent `{}` ({}). Respond to the user request. Use tools when needed.",
        agent.id, agent.label
    ));

    let mut messages = prep.messages.clone();
    let mut last_text = String::new();

    for iteration in 0..max_iters {
        if cancelled(cancel) {
            break;
        }
        if let Some(tx) = event_tx {
            let _ = tx.send(AgentEvent::Iteration { iteration });
        }

        let model_key = ensure_provider(providers, &state.config, &model);
        let provider = providers
            .get(&model_key)
            .ok_or_else(|| anyhow::anyhow!("Gemini ADK provider missing for {model_key}"))?;
        scratch.cloud_calls += 1;
        let response: MessagesResponse = provider
            .send_message(&system, messages.clone(), Some(tools.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("Gemini ADK LLM error: {e}"))?;

        let mut tool_uses = Vec::new();
        let mut text_parts = Vec::new();
        for block in &response.content {
            match block {
                ResponseContentBlock::Text { text } => {
                    if !text.trim().is_empty() {
                        text_parts.push(text.clone());
                    }
                }
                ResponseContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    thought_signature,
                } => {
                    tool_uses.push((
                        id.clone(),
                        name.clone(),
                        input.clone(),
                        thought_signature.clone(),
                    ));
                }
            }
        }

        if !text_parts.is_empty() {
            last_text = text_parts.join("\n");
        }

        if tool_uses.is_empty() {
            break;
        }

        let mut assistant_blocks = Vec::new();
        for (id, name, input, thought_signature) in &tool_uses {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                thought_signature: thought_signature.clone(),
            });
        }
        messages.push(Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(assistant_blocks),
        });

        let mut result_blocks = Vec::new();
        let mut transfer_target: Option<(String, String)> = None;

        for (id, name, input, _) in tool_uses {
            if name == EXIT_LOOP_TOOL {
                scratch.exit_loop_requested = true;
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: "Loop exit acknowledged.".into(),
                    is_error: Some(false),
                });
                continue;
            }
            if name == TRANSFER_TOOL {
                let target = input
                    .get("agent_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let reason = input
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if transferable.iter().any(|t| t == &target) {
                    transfer_target = Some((target.clone(), reason.clone()));
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: format!("Transferring to agent `{target}`."),
                        is_error: Some(false),
                    });
                } else {
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: format!(
                            "Unknown or disallowed agent_name `{target}`. Allowed: {}",
                            transferable.join(", ")
                        ),
                        is_error: Some(true),
                    });
                }
                continue;
            }

            let mut dispatch_ctx = ToolHookDispatchContext {
                state,
                context,
                run_key: &prep.run_key,
                event_tx,
                tool_auth: &prep.tool_auth,
                schedule_skill_activated: &mut scratch.schedule_skill_activated,
                modify_skill_activated: &mut scratch.modify_skill_activated,
                discovery_streak_count: &mut scratch.discovery_streak_count,
                legacy_edit_without_block_count: &mut scratch.legacy_edit_without_block_count,
                executed_tool_names: &mut scratch.executed_tool_names,
                executed_tool_inputs: &mut scratch.executed_tool_inputs,
                force_stall_response: &mut scratch.force_stall_response,
                history_hook_events: &mut scratch.history_hook_events,
            };
            let outcome = dispatch_tool_with_hooks(&mut dispatch_ctx, &name, input).await;
            scratch.tool_call_records.push(outcome.record);
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id,
                content: outcome.result.content,
                is_error: Some(outcome.result.is_error),
            });
        }

        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Blocks(result_blocks),
        });

        if let Some((target, reason)) = transfer_target {
            if !reason.trim().is_empty() {
                session.insert(format!("handoff_from_{}", agent.id), json!(reason));
            }
            return execute_agent(
                state,
                context,
                prep,
                event_tx,
                cancel,
                topology,
                settings,
                &target,
                session,
                providers,
                scratch,
                depth + 1,
            )
            .await;
        }

        if scratch.force_stall_response.is_some() {
            break;
        }
    }

    let _ = run_post_tool_batch_hooks(
        state,
        context,
        &prep.run_key,
        event_tx,
        scratch.discovery_streak_count,
        scratch.legacy_edit_without_block_count,
        scratch.force_stall_response.is_some(),
        &mut scratch.history_hook_events,
    )
    .await;

    if let Some(stall) = scratch.force_stall_response.clone() {
        return Ok(stall);
    }

    Ok(last_text)
}

#[allow(clippy::too_many_arguments)]
async fn finish_adk_turn(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    prep: &AgentRunPrep,
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    final_text: String,
    stop_reason: &str,
    scratch: RunScratch,
    run_start: Instant,
    model: &str,
) -> anyhow::Result<AgentProcessResult> {
    let extras = PipelineFinishExtras {
        pipeline_stages: scratch.pipeline_stages,
        cloud_calls: scratch.cloud_calls,
        agent_engine: "gemini_adk".into(),
    };
    let mut messages = prep.messages.clone();
    messages.push(Message {
        role: "assistant".into(),
        content: MessageContent::Text(final_text.clone()),
    });
    let mut pdqe_retries = 0usize;
    let mut pdqe_steps: Vec<EvaluatorStepRecord> = Vec::new();
    let history_iterations = if scratch.tool_call_records.is_empty() {
        scratch.history_iterations
    } else {
        let mut iters = scratch.history_iterations;
        iters.push(IterationRecord {
            iteration: 1,
            stop_reason: "tool_use".into(),
            assistant_text_preview: if final_text.len() <= 200 {
                final_text.clone()
            } else {
                format!("{}...", &final_text[..final_text.floor_char_boundary(200)])
            },
            tool_calls: scratch.tool_call_records,
            hook_events: scratch.history_hook_events,
            pte: None,
            model_tier: "gemini_adk".into(),
            provider: "google".into(),
            model: model.to_string(),
            endpoint: "generativelanguage.googleapis.com".into(),
        });
        iters
    };
    let mut agent_history_basename: Option<String> = None;
    let mut final_text = final_text;
    let run_tool_names = scratch.executed_tool_names;
    loop {
        let mut iterations = history_iterations.clone();
        match pipeline_finish_turn(
            state,
            context,
            event_tx,
            &prep.run_key,
            context.chat_id,
            context.persona_id,
            stop_reason,
            final_text.clone(),
            &prep.system_prompt,
            &mut messages,
            prep.protected_message_count,
            &mut pdqe_retries,
            &mut pdqe_steps,
            &mut iterations,
            &prep.principles_content,
            prep.is_conversational,
            &run_tool_names,
            &mut agent_history_basename,
            !run_tool_names.is_empty(),
            &prep.user_msg_preview,
            run_start,
            &prep.initial_llm_snapshot_json,
            &prep.local_delegate_run_summary,
            Some(&extras),
        )
        .await?
        {
            Some(result) => return Ok(result),
            None => {
                if let Some(Message {
                    content: MessageContent::Text(t),
                    ..
                }) = messages.last()
                {
                    if t != &final_text {
                        final_text = t.clone();
                    }
                }
            }
        }
    }
}
