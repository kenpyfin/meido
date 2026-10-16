//! Shared agent run bootstrap for classic, Gemini ADK, and Cursor engines.

use std::collections::HashSet;
use std::path::Path;

use crate::agent_history::{format_initial_llm_snapshot_json, truncate_preview};
use crate::channels::inbound_images::InboundImages;
use crate::claude::{ContentBlock, Message, MessageContent};
use crate::db::call_blocking;
use crate::local_delegate::LocalDelegateRunSummary;
use crate::memory::{
    enrich_persona_memory_for_prompt, render_identity_and_tier1_for_system, render_memory_for_llm,
    render_persona_context_memory_with_options, MemoryPromptBuildOptions, PersonaMemoryState,
};
use crate::tools::ToolAuthContext;

use super::{
    build_current_request_from_message, build_persona_context_message, build_system_prompt,
    ensure_persona_memory_file_exists, format_bookmarks_section, format_bulletin_focus_section,
    history_to_claude_messages, latest_user_text, latest_user_text_from_message,
    load_messages_from_db, sops_prompt_sections, split_trailing_user_request,
    trim_to_recent_balanced, trim_to_token_budget, user_request_is_conversational,
    workspace_data_path_display, AgentRequestContext, AppState,
};

/// Session block shared by the classic loop and `prepare_agent_run`.
/// A topic seed is labelled as prior-topic context and is not overwritten by bootstrap JSON.
pub(crate) fn format_session_context_block(session: &crate::db::ChatSession) -> String {
    let mut block = format!(
        "[session_context id=\"{}\" intent=\"{}\" created=\"{}\"]\n",
        session.id, session.intent, session.created_at
    );
    if let Some(bootstrap_json) = session.bootstrap_context_json.as_deref() {
        let trimmed = bootstrap_json.trim();
        if !trimmed.is_empty() {
            block.push_str(trimmed);
            block.push('\n');
        }
    }
    if let Some(seed) = session.seed_context.as_deref() {
        let trimmed = seed.trim();
        if !trimmed.is_empty() {
            block.push_str("[prior_topic_context]\n");
            block.push_str(trimmed);
            block.push_str("\n[/prior_topic_context]\n");
        }
    }
    block.push_str("[/session_context]");
    block
}

/// Bootstrap output shared by classic, Gemini ADK, and Cursor agent engines.
pub struct AgentRunPrep {
    pub principles_content: String,
    pub pte_memory_prose: String,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub protected_message_count: usize,
    pub latest_user_text: String,
    pub run_key: String,
    pub has_image_input: bool,
    pub user_msg_preview: String,
    pub initial_llm_snapshot_json: String,
    pub local_delegate_run_summary: LocalDelegateRunSummary,
    pub is_conversational: bool,
    pub tool_auth: ToolAuthContext,
    pub persona_memory_state: Option<PersonaMemoryState>,
    pub min_user_suffix: usize,
    pub min_asst_suffix: usize,
}

pub async fn prepare_agent_run(
    state: &AppState,
    context: &AgentRequestContext<'_>,
    override_prompt: Option<&str>,
    images: InboundImages,
) -> anyhow::Result<AgentRunPrep> {
    let chat_id = context.chat_id;
    let persona_id = context.persona_id;
    ensure_persona_memory_file_exists(state, chat_id, persona_id);
    if let Err(e) = crate::tools::ensure_persona_shared_dir(
        Path::new(state.config.working_dir()),
        chat_id,
        persona_id,
    ) {
        tracing::warn!(
            "Failed to ensure persona shared dir for chat {chat_id} persona {persona_id}: {e}"
        );
    }

    let persona_row = call_blocking(state.db.clone(), move |db| db.get_persona(persona_id))
        .await?
        .filter(|p| p.chat_id == chat_id);

    let principles_content = state.memory.read_groups_root_memory().unwrap_or_default();
    let memory_prompt_opts = MemoryPromptBuildOptions::from_env();
    let legacy_memory_md =
        std::fs::read_to_string(state.memory.persona_memory_path(chat_id, persona_id))
            .unwrap_or_default();
    let persona_memory_state = state
        .memory
        .read_or_migrate_persona_memory_state(chat_id, persona_id);
    let persona_name_for_prompt = persona_row.as_ref().map(|p| p.name.as_str());
    let identity_tier1_system = persona_memory_state
        .as_ref()
        .map(|s| {
            let enriched = enrich_persona_memory_for_prompt(
                s.clone(),
                persona_name_for_prompt,
                Some(&legacy_memory_md),
            );
            render_identity_and_tier1_for_system(
                &enriched,
                memory_prompt_opts.workflow_principles_prompt_max,
            )
        })
        .unwrap_or_default();
    let pte_memory_prose = persona_memory_state
        .as_ref()
        .map(|s| render_memory_for_llm(s, memory_prompt_opts.workflow_principles_prompt_max))
        .unwrap_or_default();

    let allowed_skill_names = call_blocking(state.db.clone(), move |db| {
        Ok(db
            .get_persona_hook_skill_policy(chat_id, persona_id)?
            .and_then(|p| p.allowed_skill_names)
            .map(|v| v.into_iter().collect::<HashSet<String>>()))
    })
    .await?;
    let skills_catalog = state
        .skills
        .build_skills_catalog_for_allowed(allowed_skill_names.as_ref());
    let workspace_dir = crate::tools::persona_shared_dir(
        Path::new(state.config.working_dir()),
        chat_id,
        persona_id,
    );
    let workspace_path = workspace_dir.to_string_lossy();
    let agents_md_path = state.memory.groups_root_memory_path_display();
    let skills_dir_for_prompt = state
        .config
        .skills_data_dir_absolute()
        .to_string_lossy()
        .to_string();

    let vault_paths_section = build_vault_paths_section(state, &skills_dir_for_prompt);
    let workspace_data_root_display = state
        .config
        .workspace_root_absolute()
        .to_string_lossy()
        .to_string();
    let config_env_summary = build_config_env_summary(state);

    let min_user_suffix = persona_row
        .as_ref()
        .and_then(|p| p.recent_history_min_user)
        .map(|n| (n as usize).clamp(1, 25))
        .unwrap_or_else(|| state.config.recent_history_min_user_messages.clamp(1, 25));
    let min_asst_suffix = persona_row
        .as_ref()
        .and_then(|p| p.recent_history_min_assistant)
        .map(|n| (n as usize).clamp(1, 25))
        .unwrap_or_else(|| {
            state
                .config
                .recent_history_min_assistant_messages
                .clamp(1, 25)
        });

    let operator_memo_redacted: Option<String> = persona_row
        .as_ref()
        .and_then(|p| p.operator_memo.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| {
            let capped: String = if raw.chars().count() > crate::db::OPERATOR_MEMO_MAX_CHARS {
                raw.chars()
                    .take(crate::db::OPERATOR_MEMO_MAX_CHARS)
                    .collect()
            } else {
                raw.to_string()
            };
            state.env_redactor.redact(&capped)
        })
        .filter(|s| !s.trim().is_empty());

    let tz: chrono_tz::Tz = state.config.timezone.parse().unwrap_or(chrono_tz::Tz::UTC);
    let current_time_in_tz = chrono::Utc::now()
        .with_timezone(&tz)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string();
    let (sops_caps_line, sops_body) = sops_prompt_sections();
    let locate_auth = ToolAuthContext {
        run_id: context.run_key.clone(),
        caller_channel: context.caller_channel.to_string(),
        caller_chat_id: chat_id,
        caller_persona_id: persona_id,
        control_chat_ids: state.config.control_chat_ids.clone(),
        is_scheduled_task: context.is_scheduled_task,
        session_id: context.session_id.clone(),
        vector_db_root: state.config.vector_db_root(),
        external_roots: state.external_roots_for_persona(persona_id),
    };
    let origin_vault = state.config.origin_vault_root();
    let locate_registry = crate::tools::locate::build_registry_with_origin(
        &state.config.workspace_root_absolute(),
        Some(&locate_auth),
        persona_memory_state.as_ref(),
        &origin_vault,
    );
    let locate_roots_section = crate::tools::locate::format_registry_for_prompt(&locate_registry);
    let system_prompt = build_system_prompt(
        &state.config.bot_username,
        &principles_content,
        &agents_md_path,
        chat_id,
        persona_id,
        &skills_catalog,
        &workspace_path,
        &skills_dir_for_prompt,
        vault_paths_section.as_deref(),
        &state.config.timezone,
        &workspace_data_root_display,
        &config_env_summary,
        identity_tier1_system.as_str(),
        &sops_caps_line,
        &sops_body,
        &locate_roots_section,
    );

    let mut messages = if context.is_background_job {
        Vec::new()
    } else if let Some(ref override_msgs) = context.history_override {
        override_msgs.clone()
    } else if let Some(ref sid) = context.session_id {
        let session_id = sid.clone();
        let mut history = call_blocking(state.db.clone(), move |db| {
            db.get_all_messages_for_session(&session_id)
        })
        .await?;
        let db = state.db.clone();
        let history = call_blocking(db, move |db| {
            db.expand_trailing_attachments(&mut history);
            Ok(history)
        })
        .await?;
        history_to_claude_messages(&history, &state.config.bot_username, false)
    } else {
        load_messages_from_db(
            state,
            chat_id,
            persona_id,
            context.chat_type,
            context.is_scheduled_task,
        )
        .await?
    };

    messages.retain(|m| matches!(&m.content, MessageContent::Text(_)));

    if let Some(prompt) = override_prompt {
        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Text(format!("[scheduler]: {prompt}")),
        });
    }

    let has_image_input = !images.is_empty();
    images.attach_to_last_user_message(&mut messages);

    if context.session_id.is_none() && context.history_override.is_none() {
        messages = trim_to_recent_balanced(messages, min_user_suffix, min_asst_suffix);
    }

    let (history_messages, current_request) = split_trailing_user_request(messages);
    let Some(current_request) = current_request else {
        return Err(anyhow::anyhow!("I didn't receive any message to process."));
    };

    let bulletin_focus_for_prompt = call_blocking(state.db.clone(), {
        move |db| db.get_persona_bulletin_focus(chat_id, persona_id)
    })
    .await
    .ok()
    .flatten();
    let bulletin_focus_section = format_bulletin_focus_section(bulletin_focus_for_prompt.as_ref());

    let persona_context_memory = persona_memory_state
        .as_ref()
        .map(|s| render_persona_context_memory_with_options(s, bulletin_focus_section.is_some()))
        .unwrap_or_default();

    let bookmarks_for_prompt = call_blocking(state.db.clone(), {
        move |db| db.list_persona_message_bookmarks(chat_id, persona_id, 8)
    })
    .await
    .unwrap_or_default();
    let bookmarks_section = format_bookmarks_section(&bookmarks_for_prompt);

    let runtime_context = format!(
        "[system_runtime_context timezone=\"{}\"]Current date and time: {}[/system_runtime_context]",
        state.config.timezone, current_time_in_tz
    );
    let mut prepended = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(runtime_context),
    }];
    if let Some(ctx) = build_persona_context_message(
        &persona_context_memory,
        !identity_tier1_system.trim().is_empty(),
        bulletin_focus_section.as_deref(),
        operator_memo_redacted.as_deref(),
        bookmarks_section.as_deref(),
    ) {
        prepended.push(Message {
            role: "user".into(),
            content: MessageContent::Text(ctx),
        });
    }
    if let Some(ref sid) = context.session_id {
        let sid_for_touch = sid.clone();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.update_chat_session_last_active(&sid_for_touch)
        })
        .await;
        let session_ctx = call_blocking(state.db.clone(), {
            let sid = sid.clone();
            move |db| db.get_chat_session(&sid)
        })
        .await?;
        if let Some(session) = session_ctx {
            prepended.push(Message {
                role: "user".into(),
                content: MessageContent::Text(format_session_context_block(&session)),
            });
        }
    }
    prepended.extend(history_messages);
    prepended.push(build_current_request_from_message(current_request.clone()));
    if let Some(steer) =
        crate::sop_context_gate::sop_request_steer(&latest_user_text_from_message(&current_request))
    {
        prepended.push(Message {
            role: "user".into(),
            content: MessageContent::Text(steer),
        });
    }
    messages = prepended;
    let protected_message_count = messages.len();
    let latest_user_text = latest_user_text(&messages);
    let run_key = context
        .run_key
        .clone()
        .unwrap_or_else(|| format!("run:{}", uuid::Uuid::new_v4()));

    let tool_auth = ToolAuthContext {
        run_id: context.run_key.clone(),
        caller_channel: context.caller_channel.to_string(),
        caller_chat_id: chat_id,
        caller_persona_id: persona_id,
        control_chat_ids: state.config.control_chat_ids.clone(),
        is_scheduled_task: context.is_scheduled_task,
        session_id: context.session_id.clone(),
        vector_db_root: state.config.vector_db_root(),
        external_roots: state.external_roots_for_persona(persona_id),
    };

    let mut tool_defs = state.tools.definitions();
    let persona_name = persona_row.as_ref().map(|p| p.name.as_str()).unwrap_or("");
    crate::collab::retain_tools_for_persona(&mut tool_defs, persona_name);
    if crate::collab::is_coordinator_name(persona_name)
        && !context.is_scheduled_task
        && context.history_override.is_none()
    {
        if let Ok(digest) = crate::collab::coordinator::build_digest(state, chat_id).await {
            messages.insert(
                0,
                Message {
                    role: "user".into(),
                    content: MessageContent::Text(format!(
                        "Current digest. Answer from this plus today's session, not from older days.\n{digest}"
                    )),
                },
            );
        }
    }
    let token_budget = if context.session_id.is_some() {
        40_000
    } else {
        12_000
    };
    trim_to_token_budget(
        &mut messages,
        &system_prompt,
        &tool_defs,
        token_budget,
        min_user_suffix,
        min_asst_suffix,
        protected_message_count,
    );

    let _ = call_blocking(state.db.clone(), {
        let run_key = run_key.clone();
        move |db| {
            db.append_run_timeline_event(
                &run_key,
                chat_id,
                persona_id,
                "run_started",
                Some("{\"status\":\"started\"}"),
            )
        }
    })
    .await;

    let tool_names_list: Vec<String> = tool_defs.iter().map(|d| d.name.clone()).collect();
    let global_engine = state.runtime_toggles.agent_engine();
    let agent_engine = crate::runtime_toggles::resolve_run_agent_engine_from_persona(
        state.db.as_ref(),
        chat_id,
        persona_id,
        global_engine,
    );
    let mm_cfg = state.llm.local_delegate_config();
    let cost_routing = crate::local_delegate::cost_routing_active(agent_engine, &mm_cfg);
    let is_conversational = user_request_is_conversational(&latest_user_text, has_image_input);
    let local_delegate_run_summary = if agent_engine == crate::runtime_toggles::AgentEngine::Cursor
    {
        let (model, url) = state
            .cursor_settings
            .read()
            .ok()
            .map(|cfg| (cfg.sdk_model.clone(), cfg.sdk_runner_url.clone()))
            .unwrap_or_default();
        LocalDelegateRunSummary::for_cursor_sidecar(&model, &url)
    } else {
        state.llm.local_delegate_run_summary(cost_routing)
    };
    let iter0_tier = crate::local_delegate::RouteTarget::Strategy;
    let iter0_tier_snap = state.llm.tier_endpoint_snapshot(iter0_tier);
    let routing_v1 = local_delegate_run_summary.routing_v1_json(&iter0_tier_snap);
    let initial_llm_snapshot_json = format_initial_llm_snapshot_json(
        &system_prompt,
        &messages,
        &tool_names_list,
        Some(&routing_v1),
    );

    let user_msg_preview = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| match &m.content {
            MessageContent::Text(t) => truncate_preview(t, 120),
            MessageContent::Blocks(blocks) => {
                let text: String = blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                truncate_preview(&text, 120)
            }
        })
        .unwrap_or_default();

    Ok(AgentRunPrep {
        principles_content,
        pte_memory_prose,
        system_prompt,
        messages,
        protected_message_count,
        latest_user_text,
        run_key,
        has_image_input,
        user_msg_preview,
        initial_llm_snapshot_json,
        local_delegate_run_summary,
        is_conversational,
        tool_auth,
        persona_memory_state,
        min_user_suffix,
        min_asst_suffix,
    })
}

fn build_config_env_summary(state: &AppState) -> String {
    let mut config_env_summary = match crate::config::Config::resolve_config_path() {
        Ok(Some(ref p)) => {
            let parent = p
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "(unknown)".into());
            format!("{} — bot loads `{}`", parent, p.display())
        }
        Ok(None) => std::env::current_dir()
            .map(|d| {
                format!(
                    "{} — no resolved config file path (expect `./.env` relative to process cwd)",
                    d.display()
                )
            })
            .unwrap_or_else(|_| "(unknown) — could not resolve config path or cwd".into()),
        Err(_) => std::env::current_dir()
            .map(|d| {
                format!(
                    "{} — config path resolution failed; check MEIDO_CONFIG",
                    d.display()
                )
            })
            .unwrap_or_else(|_| "(unknown)".into()),
    };
    match state.config.tavily_api_key.as_deref() {
        Some(key) if !key.trim().is_empty() => {
            config_env_summary.push_str("; TAVILY_API_KEY configured (web_search uses Tavily)");
        }
        _ => match state.config.web_search_searxng_url.as_deref() {
            Some(url) if !url.trim().is_empty() => {
                config_env_summary.push_str(&format!("; SEARXNG_URL configured ({})", url.trim()));
            }
            _ => config_env_summary.push_str(
                "; web_search: no Tavily/SearXNG (uses DuckDuckGo HTML fallback unless configured)",
            ),
        },
    }
    config_env_summary
}

fn build_vault_paths_section(state: &AppState, skills_dir_for_prompt: &str) -> Option<String> {
    state.config.vault.as_ref().and_then(|v| {
        let ws_root = state.config.workspace_root_absolute();
        let mut parts = Vec::new();
        if let Some(ref p) = v.origin_vault_path {
            if !p.trim().is_empty() {
                let disp = workspace_data_path_display(&ws_root, p);
                if !disp.is_empty() {
                    parts.push(format!("- ORIGIN vault: {disp}"));
                }
            }
        }
        if let Some(ref p) = v.vector_db_path {
            if !p.trim().is_empty() {
                let disp = workspace_data_path_display(&ws_root, p);
                if !disp.is_empty() {
                    parts.push(format!("- Vector DB (ChromaDB local path): {disp}"));
                }
            }
        }
        let use_native = v
            .embedding_server_url
            .as_ref()
            .is_some_and(|u| !u.trim().is_empty())
            && v.vector_db_url
                .as_ref()
                .is_some_and(|u| !u.trim().is_empty());
        let use_command = v
            .vault_search_command
            .as_ref()
            .is_some_and(|c| !c.trim().is_empty());

        if use_native {
            let embed_url = v.embedding_server_url.as_ref().unwrap();
            let db_url = v.vector_db_url.as_ref().unwrap();
            let collection = v.vector_db_collection.as_deref().unwrap_or("vault");
            parts.push(format!(
                "- Knowledge recall: use `recall` (embedding: {}, ChromaDB: {}, collection: {})",
                embed_url.trim(),
                db_url.trim(),
                collection
            ));
        } else if use_command {
            parts.push(
                "- Knowledge recall: use `recall` (command-based vault search is the fallback)"
                    .to_string(),
            );
        } else {
            let ws_script = state
                .config
                .workspace_root_absolute()
                .join("skills")
                .join("search-vault")
                .join("query_vault.py");
            let auto_script = if ws_script.exists() {
                Some(ws_script)
            } else {
                crate::builtin_skills::resolve_builtin_skills_dir(&state.config)
                    .map(|b| b.join("search-vault").join("query_vault.py"))
            };
            if auto_script.as_ref().is_some_and(|p| p.exists()) {
                parts.push(
                    "- Knowledge recall: use `recall` (vault search skill is the fallback)"
                        .to_string(),
                );
            } else if let Some(ref u) = v.embedding_server_url {
                if !u.trim().is_empty() {
                    parts.push(format!("- Embedding server: {}", u.trim()));
                }
            }
        }
        if let Some(ref c) = v.vault_index_command {
            if !c.trim().is_empty() {
                parts.push(format!("- Index: {}", c.trim()));
            }
        }
        let ws_index = state
            .config
            .workspace_root_absolute()
            .join("skills")
            .join("index-vault")
            .join("index_vault.py");
        let index_script = if ws_index.exists() {
            ws_index
        } else if let Some(p) = crate::builtin_skills::resolve_builtin_skills_dir(&state.config)
            .map(|b| b.join("index-vault").join("index_vault.py"))
            .filter(|p| p.exists())
        {
            p
        } else {
            ws_index
        };
        if index_script.exists() {
            parts.push(format!(
                "- Index vault: activate the `index-vault` skill or run `{}`",
                index_script.display()
            ));
        }
        if parts.is_empty() {
            None
        } else {
            parts.push(format!("- Skills directory: {skills_dir_for_prompt}"));
            Some(format!(
                "\n# Vault and Vector DB Paths\n\n{}\n\n",
                parts.join("\n")
            ))
        }
    })
}
