//! Process host: builds AppState and starts web first, then optional channels.

use std::collections::HashMap;
use std::sync::Arc;

use serenity::http::Http as SerenityHttp;
use teloxide::prelude::*;
use teloxide::types::BotCommand;
use tracing::{error, info, warn};

use crate::agent::AppState;
use crate::background_jobs::BackgroundJobControl;
use crate::chat_queue::ChatRunQueue;
use crate::config::Config;
use crate::db::Database;
use crate::memory::MemoryManager;
use crate::safety_redaction::EnvSecretRedactor;
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;

pub async fn run_app(
    config: Config,
    db: Database,
    memory: MemoryManager,
    skills: SkillManager,
    mcp_manager: crate::mcp::McpManager,
) -> anyhow::Result<()> {
    let telegram_enabled = !config.telegram_bot_token.trim().is_empty()
        || db
            .list_channel_bot_instances_by_platform("telegram")
            .map(|rows| rows.iter().any(|r| !r.token.trim().is_empty()))
            .unwrap_or(false);

    let mut telegram_bots_map: HashMap<i64, Bot> = HashMap::new();
    let mut telegram_token_owner: HashMap<String, i64> = HashMap::new();
    let tg_rows = db.list_channel_bot_instances_by_platform("telegram")?;
    for row in tg_rows {
        let token = row.token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(existing_id) = telegram_token_owner.get(token) {
            warn!(
                "Skipping duplicate Telegram bot instance {} (same token already owned by instance {})",
                row.id, existing_id
            );
            continue;
        }
        telegram_token_owner.insert(token.to_string(), row.id);
        let bot = Bot::new(token);
        match bot.get_me().await {
            Ok(me) => info!(
                "Telegram bot instance {} ready (@{})",
                row.id,
                me.user.username.as_deref().unwrap_or("?")
            ),
            Err(e) => warn!(
                "Telegram bot instance {} getMe failed: {} (dispatcher will still start)",
                row.id, e
            ),
        }
        telegram_bots_map.insert(row.id, bot);
    }
    if telegram_bots_map.is_empty() && telegram_enabled {
        telegram_bots_map.insert(
            crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY,
            Bot::new(config.telegram_bot_token.as_str()),
        );
    }

    let db = Arc::new(db);

    let tool_bot = telegram_bots_map
        .get(&crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY)
        .cloned()
        .unwrap_or_else(|| Bot::new("telegram-disabled"));

    if telegram_enabled {
        // Register slash commands so they appear in the Telegram menu (every instance).
        let commands = [
            BotCommand {
                command: "reset".into(),
                description: "Clear conversation (memory unchanged)".into(),
            },
            BotCommand {
                command: "skills".into(),
                description: "List available skills".into(),
            },
            BotCommand {
                command: "persona".into(),
                description: "Manage personas (tap to switch)".into(),
            },
            BotCommand {
                command: "archive".into(),
                description: "Archive conversation to markdown".into(),
            },
            BotCommand {
                command: "schedule".into(),
                description: "List and manage scheduled jobs".into(),
            },
        ];
        for b in telegram_bots_map.values() {
            if let Err(e) = b.set_my_commands(commands.clone()).await {
                error!("Failed to set Telegram bot commands: {}", e);
            }
        }
    } else {
        info!("Telegram channel disabled (no TELEGRAM_BOT_TOKEN configured)");
    }

    let mut config = config;
    config.merge_llm_model_from_app_settings(&db)?;
    let toggle_init = crate::runtime_toggles::RuntimeToggles::merge_from_app_settings(
        crate::runtime_toggles::RuntimeToggleInit {
            tool_output_debug: config.tool_output_debug,
            post_tool_evaluator_enabled: config.post_tool_evaluator_enabled,
            response_quality_evaluator_enabled: config.response_quality_evaluator_enabled,
            agent_engine: crate::runtime_toggles::AgentEngine::Classic,
            max_tool_iterations: config.max_tool_iterations,
        },
        &db,
    )?;
    config.tool_output_debug = toggle_init.tool_output_debug;
    config.post_tool_evaluator_enabled = toggle_init.post_tool_evaluator_enabled;
    config.response_quality_evaluator_enabled = toggle_init.response_quality_evaluator_enabled;
    let runtime_toggles = crate::runtime_toggles::RuntimeToggles::from_init(toggle_init);
    let llm = crate::llm::LlmHandle::new(&config);
    if let Ok(mm_cfg) = crate::local_delegate::load_from_db(&db) {
        if let Err(e) = llm.apply_local_delegate_config(mm_cfg) {
            warn!("Failed to apply local delegate config: {e}");
        }
    }
    if runtime_toggles.agent_engine() == crate::runtime_toggles::AgentEngine::Classic {
        let mm = llm.local_delegate_config();
        if mm.routing_enabled {
            runtime_toggles
                .set_agent_engine(crate::runtime_toggles::AgentEngine::ClassicCostRouting);
            if let Err(e) = crate::runtime_toggles::RuntimeToggles::persist_agent_engine(
                &db,
                crate::runtime_toggles::AgentEngine::ClassicCostRouting,
            ) {
                warn!("Failed to migrate agent engine to classic_cost_routing: {e}");
            } else {
                info!("Migrated agent engine to classic_cost_routing (legacy MULTIMODEL_ENABLED)");
            }
        }
    }
    let cursor_settings = Arc::new(std::sync::RwLock::new(
        crate::cursor_engine_config::load_from_db(&db, &config).unwrap_or_else(|e| {
            warn!("Failed to load Cursor settings from DB: {e}");
            crate::cursor_engine_config::CursorEngineSettings::from_env(&config)
        }),
    ));
    let gemini_adk_settings = Arc::new(std::sync::RwLock::new(
        crate::gemini_adk::config::load_from_db(&db, &config).unwrap_or_else(|e| {
            warn!("Failed to load Gemini ADK settings from DB: {e}");
            crate::gemini_adk::GeminiAdkSettings::from_env(&config)
        }),
    ));
    let gemini_adk_profile = Arc::new(std::sync::RwLock::new(
        crate::gemini_adk::profile::load_from_db(&db).unwrap_or_else(|e| {
            warn!("Failed to load Gemini ADK topology from DB: {e}");
            crate::gemini_adk::AdkTopologyProfile::default_profile()
        }),
    ));
    let cursor_sidecar =
        crate::cursor_sdk_sidecar::bootstrap(&config, &db, cursor_settings.clone()).await;
    let steel_browser = crate::steel_browser_sidecar::bootstrap(&config).await;
    let app_state_slot: Arc<std::sync::OnceLock<Arc<AppState>>> =
        Arc::new(std::sync::OnceLock::new());
    let env_redactor = Arc::new(EnvSecretRedactor::discover(&config));
    let mut tools = ToolRegistry::new(
        &config,
        tool_bot,
        db.clone(),
        runtime_toggles.clone(),
        env_redactor.clone(),
    );
    tools.add_tool(Box::new(
        crate::tools::spawn_background_command::SpawnBackgroundCommandTool::new(
            &config,
            app_state_slot.clone(),
        ),
    ));
    let tool_names: Vec<String> = tools.definitions().iter().map(|d| d.name.clone()).collect();
    info!(
        "Tool registry initialized ({} tools): {}",
        tool_names.len(),
        tool_names.join(", ")
    );
    info!(
        "Framework toggles: allow_fuzzy_search_replace={}, symbol_edit_enabled={}, post_edit_validation_enabled={}, post_edit_validation_commands={}",
        config.allow_fuzzy_search_replace,
        config.symbol_edit_enabled,
        config.post_edit_validation_enabled,
        config
            .post_edit_validation_commands
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("<default>")
    );
    let edit_tools = [
        "read_repo_map",
        "apply_search_replace",
        "symbol_edit",
        "edit_file",
    ];
    let registered_edit_tools: Vec<&str> = edit_tools
        .iter()
        .copied()
        .filter(|name| tool_names.iter().any(|n| n == name))
        .collect();
    info!(
        "Registered edit tool set: {}",
        registered_edit_tools.join(", ")
    );

    // Register MCP tools
    for (server, tool_info) in mcp_manager.all_tools() {
        tools.add_tool(Box::new(crate::tools::mcp::McpTool::new(server, tool_info)));
    }

    let mut discord_http_map: HashMap<i64, Arc<SerenityHttp>> = HashMap::new();
    let mut discord_launches: Vec<(i64, String)> = Vec::new();
    let mut discord_token_owner: HashMap<String, i64> = HashMap::new();
    let dc_rows = db.list_channel_bot_instances_by_platform("discord")?;
    for row in dc_rows {
        let token = row.token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(existing_id) = discord_token_owner.get(token) {
            warn!(
                "Skipping duplicate Discord bot instance {} (same token already owned by instance {})",
                row.id, existing_id
            );
            continue;
        }
        discord_token_owner.insert(token.to_string(), row.id);
        discord_launches.push((row.id, token.to_string()));
        discord_http_map.insert(row.id, Arc::new(SerenityHttp::new(token)));
    }
    if discord_http_map.is_empty() {
        if let Some(ref t) = config.discord_bot_token {
            if !t.trim().is_empty() {
                discord_launches.push((crate::db::BOT_INSTANCE_DISCORD_PRIMARY, t.clone()));
                discord_http_map.insert(
                    crate::db::BOT_INSTANCE_DISCORD_PRIMARY,
                    Arc::new(SerenityHttp::new(t.as_str())),
                );
            }
        }
    }

    let wecom = crate::channels::wecom::WecomGateway::from_config(&config);
    let prepared_listener = if config.web_enabled {
        match crate::web::bind_web_listener(&config.web_host, config.web_port).await {
            Ok(bound) => Some(bound),
            Err(err) => {
                error!(
                    "Failed to bind web server at {}:{}: {}",
                    config.web_host, config.web_port, err
                );
                None
            }
        }
    } else {
        None
    };
    let web_listener = prepared_listener.map(|(listener, bound)| {
        if bound != config.web_port {
            info!(
                "Default web port {} is in use; using {}",
                config.web_port, bound
            );
        }
        config.web_port = bound;
        listener
    });
    if config.web_enabled {
        let runtime_dir = config.runtime_data_dir();
        let recorded = if web_listener.is_some() {
            crate::daemon::write_bound_port(std::path::Path::new(&runtime_dir), config.web_port)
        } else {
            crate::daemon::write_bind_failed(std::path::Path::new(&runtime_dir))
        };
        if let Err(err) = recorded {
            warn!("Failed to record web listen port: {err}");
        }
    }
    let state = Arc::new(AppState {
        config,
        env_redactor,
        runtime_toggles,
        cursor_settings,
        gemini_adk_settings,
        gemini_adk_profile,
        cursor_sidecar,
        steel_browser,
        telegram_bots: Arc::new(telegram_bots_map),
        db,
        memory,
        skills,
        llm,
        tools,
        cursor_mcp: Arc::new(crate::cursor_mcp_bridge::CursorMcpRegistry::new()),
        discord_http: Arc::new(discord_http_map),
        wecom,
        chat_queue: ChatRunQueue::default(),
        background_job_control: BackgroundJobControl::default(),
        persona_external_paths: Arc::new(
            crate::persona_external_paths::PersonaExternalPaths::load(
                Config::resolve_config_path().unwrap_or(None),
            ),
        ),
    });
    let _ = app_state_slot.set(state.clone());

    info!("web UI is the primary interface; messaging channels are optional");
    // Start local web server if enabled
    if let Some(listener) = web_listener {
        let web_state = state.clone();
        info!(
            "Starting Web UI server on {}:{}",
            state.config.web_host, state.config.web_port
        );
        tokio::spawn(async move {
            crate::web::start_web_server(web_state, listener).await;
        });
    }

    // Start scheduler and shell background monitor
    crate::scheduler::spawn_scheduler(state.clone());
    crate::background_shell::spawn_background_shell_monitor(state.clone());

    // Start WhatsApp webhook server if configured
    if let (Some(token), Some(phone_id), Some(verify)) = (
        &state.config.whatsapp_access_token,
        &state.config.whatsapp_phone_number_id,
        &state.config.whatsapp_verify_token,
    ) {
        let wa_state = state.clone();
        let token = token.clone();
        let phone_id = phone_id.clone();
        let verify = verify.clone();
        let port = state.config.whatsapp_webhook_port;
        info!("Starting WhatsApp webhook server on port {port}");
        tokio::spawn(async move {
            crate::whatsapp::start_whatsapp_server(wa_state, token, phone_id, verify, port).await;
        });
    }

    if let Some(wecom) = state.wecom.clone() {
        let wecom_state = state.clone();
        if wecom.is_aibot() {
            if let Some(aibot) = wecom.aibot_client() {
                info!("Starting WeCom AI Bot long connection");
                tokio::spawn(async move {
                    crate::channels::wecom_aibot::start_wecom_aibot(wecom_state, aibot).await;
                });
            }
        } else if let Some(callback) = wecom.callback_client() {
            let port = state.config.wecom_webhook_port;
            info!("Starting WeCom callback server on port {port}");
            tokio::spawn(async move {
                crate::wecom::start_wecom_server(wecom_state, callback, port).await;
            });
        }
    }

    // Start one Discord client per configured Discord bot instance
    for (inst_id, token) in discord_launches {
        let discord_state = state.clone();
        info!("Starting Discord bot instance {inst_id}");
        tokio::spawn(async move {
            crate::discord::start_discord_bot(discord_state, token.as_str(), inst_id).await;
        });
    }

    if telegram_enabled {
        let bots = state.telegram_bots.as_ref().clone();
        crate::telegram::spawn_dispatchers(state.clone(), bots).await?;
    }
    tokio::signal::ctrl_c().await?;

    Ok(())
}
