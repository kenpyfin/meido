use finally_a_value_bot::claude::{Message, MessageContent};
use finally_a_value_bot::config::Config;
use finally_a_value_bot::error::FinallyAValueBotError;
use finally_a_value_bot::{
    builtin_skills, channel_integration_config, db, doctor, gateway, logging, mcp, memory, skills,
    telegram,
};
use std::path::Path;
use tracing::info;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        r#"FinallyAValueBot v{VERSION} — Agentic AI assistant for Telegram, WhatsApp, Discord & WeCom

USAGE:
    finally_a_value_bot <COMMAND>

COMMANDS:
    start       Start the bot (Telegram + optional WhatsApp/Discord/WeCom)
    gateway     Manage gateway service (install/uninstall/start/stop/status/logs)
    config      (retired) Use Web UI Settings instead
    doctor      Run preflight diagnostics (cross-platform)
    test-llm [--with-tools]   Test LLM connection (use --with-tools to send tools like Telegram)
    setup       (retired) Use Web UI onboarding instead
    version     Show version information
    help        Show this help message

FEATURES:
    - Agentic tool use (bash, files, search, memory)
    - Web search and page fetching
    - Image/photo understanding (Claude Vision)
    - Voice message transcription (OpenAI Whisper)
    - Scheduled/recurring tasks with timezone support
    - Task execution history/run logs
    - Chat export to markdown
    - Mid-conversation message sending
    - Group chat catch-up (reads all messages since last reply)
    - Group allowlist (restrict which groups can use the bot)
    - Continuous typing indicator
    - MCP (Model Context Protocol) server integration
    - WhatsApp Cloud API support
    - Discord bot support
    - WeCom (企业微信) AI Bot long connection or self-built app callback
    - Sensitive path blacklisting for file tools

SETUP:
    1. Copy .env.example to .env and set bootstrap values:
       FINALLY_A_VALUE_BOT_WORKSPACE_DIR (or WORKSPACE_DIR), WEB_HOST/WEB_PORT, WEB_AUTH_TOKEN when exposing non-local.
    2. Start the app: finally_a_value_bot start
    3. Open Web UI: http://127.0.0.1:10961 and finish runtime settings there.

       Runtime channel/LLM settings are now managed from Web UI and persisted in SQLite.

CONFIG FILE (.env):
    FinallyAValueBot reads configuration from .env in the current directory.
    Copy .env.example to .env and fill in values. Override path with FINALLY_A_VALUE_BOT_CONFIG.

    Core fields:
      llm_provider           Provider preset (default: anthropic)
      api_key                LLM API key (optional when llm_provider=ollama|llama|llamacpp)
      model                  Model name (auto-detected from provider if empty)
      llm_base_url           Custom base URL (optional)

    Runtime:
      workspace_dir          Workspace root (default: ./workspace). Layout: runtime/, skills/, shared/ under this path. Copy to migrate.
      max_tokens             Max tokens per response (default: 8192)
      max_tool_iterations    Max tool loop iterations (default: 100)
      max_history_messages   Chat history context size (default: 50)
      openai_api_key         OpenAI key for voice transcription (optional)
      timezone               IANA timezone for scheduling (default: UTC)

    Channels (Web UI → Settings → Integrations; stored in SQLite):
      Telegram / Discord / WhatsApp / WeCom tokens and platform options
      (bot username, allowed groups/channels, WhatsApp/WeCom webhook fields).
      Legacy channel env vars are one-time-imported on first boot after upgrade.

MCP (optional):
    Place a mcp.json file in workspace_dir to connect MCP servers.
    See https://modelcontextprotocol.io for details.

EXAMPLES:
    finally_a_value_bot start               Start the bot
    finally_a_value_bot gateway install     Install and enable gateway service
    finally_a_value_bot gateway status      Show gateway service status
    finally_a_value_bot gateway logs 100    Show last 100 lines of gateway logs
    finally_a_value_bot config              Retired; use Web UI Settings
    finally_a_value_bot doctor              Run preflight diagnostics
    finally_a_value_bot doctor --json       Output diagnostics as JSON
    finally_a_value_bot test-llm            Test LLM API connection (no tools)
    finally_a_value_bot test-llm --with-tools   Test LLM with full tool list (like Telegram)
    finally_a_value_bot setup               Retired; use Web UI onboarding
    finally_a_value_bot version             Show version
    finally_a_value_bot help                Show this message

ABOUT:
    https://finally_a_value_bot.ai"#
    );
}

fn print_version() {
    println!("finally_a_value_bot {VERSION}");
}

async fn run_test_llm(with_tools: bool) -> anyhow::Result<()> {
    let mut config = match Config::load() {
        Ok(c) => c,
        Err(FinallyAValueBotError::Config(e)) => {
            eprintln!("Config error: {e}");
            eprintln!("Set FINALLY_A_VALUE_BOT_CONFIG or create .env (copy from .env.example)");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to load config: {e}");
            std::process::exit(1);
        }
    };
    let runtime_data_dir = config.runtime_data_dir();
    let db = match db::Database::new(&runtime_data_dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Database init failed: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = config.merge_llm_selection_from_app_settings(&db) {
        eprintln!("LLM config error: {e}");
        std::process::exit(1);
    }
    let provider = finally_a_value_bot::llm::create_provider(&config);
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text("Reply with exactly: OK".into()),
    }];
    let tools_arg = if with_tools {
        let db = std::sync::Arc::new(db);
        let token = if config.telegram_bot_token.is_empty() {
            "dummy"
        } else {
            &config.telegram_bot_token
        };
        let bot = teloxide::Bot::new(token);
        let runtime_toggles =
            finally_a_value_bot::runtime_toggles::RuntimeToggles::new(config.tool_output_debug);
        let env_redactor = std::sync::Arc::new(
            finally_a_value_bot::safety_redaction::EnvSecretRedactor::discover(&config),
        );
        let tools = finally_a_value_bot::tools::ToolRegistry::new(
            &config,
            bot,
            db.clone(),
            runtime_toggles,
            env_redactor,
        );
        let defs = tools.definitions();
        println!("Testing with {} tools (same as Telegram).", defs.len());
        Some(defs)
    } else {
        None
    };
    println!(
        "Testing LLM: provider={} model={} base={}{}",
        config.llm_provider,
        config.model,
        config.llm_base_url.as_deref().unwrap_or("(default)"),
        if with_tools { " (with tools)" } else { "" }
    );
    match provider
        .send_message("You are a test assistant.", messages, tools_arg)
        .await
    {
        Ok(resp) => {
            let text = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    finally_a_value_bot::claude::ResponseContentBlock::Text { text } => {
                        Some(text.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let usage = resp
                .usage
                .as_ref()
                .map(|u| {
                    format!(
                        " (input: {} output: {} tokens)",
                        u.input_tokens, u.output_tokens
                    )
                })
                .unwrap_or_default();
            println!("LLM OK. Response: {}{}", text.trim(), usage);
        }
        Err(e) => {
            eprintln!("LLM error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }

    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            move_path(&child_src, &child_dst)?;
        }
        std::fs::remove_dir_all(src)?;
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        std::fs::remove_file(src)?;
    }

    Ok(())
}

/// Ensure workspace shared directory exists under the data root (for unified layout).
fn ensure_workspace_shared_dir(data_root: &Path) {
    let shared = data_root.join("shared");
    if std::fs::create_dir_all(&shared).is_err() {
        tracing::warn!(
            "Failed to create workspace shared dir: {}",
            shared.display()
        );
    }
    warn_shadow_workspace_if_present(data_root);
}

/// Log when a mistaken nested workspace exists under `shared/workspace/`.
fn warn_shadow_workspace_if_present(data_root: &Path) {
    let shadow = finally_a_value_bot::tools::shadow_workspace_path(data_root);
    if shadow.is_dir() {
        tracing::warn!(
            shadow = %shadow.display(),
            data_root = %data_root.display(),
            "Shadow workspace detected at shared/workspace/ (not WORKSPACE_DIR). \
             Migrate unique data to the canonical layout and remove this directory; \
             file writes into it are blocked."
        );
    }
}

/// If repo-root shared/ exists, copy its contents into workspace shared dir so the canonical workspace has all shared content. Does not overwrite existing files.
fn migrate_repo_shared_into_workspace(working_dir: &Path) {
    let workspace_shared = working_dir.join("shared");
    if std::fs::create_dir_all(&workspace_shared).is_err() {
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let repo_shared = cwd.join("shared");
    if !repo_shared.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(&repo_shared) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let src = entry.path();
        let dst = workspace_shared.join(name_str);
        if dst.exists() {
            continue;
        }
        if src.is_dir() {
            if copy_dir_all(&src, &dst).is_err() {
                tracing::warn!(
                    "Failed to copy repo shared dir '{}' -> '{}'",
                    src.display(),
                    dst.display()
                );
            } else {
                tracing::info!(
                    "Migrated repo shared '{}' -> '{}'",
                    src.display(),
                    dst.display()
                );
            }
        } else if std::fs::copy(&src, &dst).is_err() {
            tracing::warn!(
                "Failed to copy repo shared file '{}' -> '{}'",
                src.display(),
                dst.display()
            );
        } else {
            tracing::info!(
                "Migrated repo shared '{}' -> '{}'",
                src.display(),
                dst.display()
            );
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let child_src = entry.path();
        let child_dst = dst.join(entry.file_name());
        if child_src.is_dir() {
            copy_dir_all(&child_src, &child_dst)?;
        } else {
            std::fs::copy(&child_src, &child_dst)?;
        }
    }
    Ok(())
}

/// Ensure AGENTS.md lives at workspace_root/AGENTS.md (canonical). Move from runtime/groups/ or
/// runtime/ if needed. If both old and new exist, workspace root wins; remove stale copies.
fn migrate_agents_md_to_workspace_root(workspace_root: &Path, runtime_dir: &Path) {
    let new_path = workspace_root.join("AGENTS.md");
    let legacy_locations = [
        runtime_dir.join("groups").join("AGENTS.md"),
        runtime_dir.join("AGENTS.md"),
    ];
    for old_path in &legacy_locations {
        if !old_path.exists() {
            continue;
        }
        if new_path.exists() {
            // Root already has canonical copy; remove stale
            if let Err(e) = std::fs::remove_file(old_path) {
                tracing::warn!(
                    "Failed to remove stale AGENTS.md at '{}': {}",
                    old_path.display(),
                    e
                );
            } else {
                tracing::info!(
                    "Removed stale AGENTS.md at {} (canonical is workspace root)",
                    old_path.display()
                );
            }
        } else {
            if let Err(e) = std::fs::rename(old_path, &new_path) {
                tracing::warn!(
                    "Failed to migrate AGENTS.md '{}' -> '{}': {}",
                    old_path.display(),
                    new_path.display(),
                    e
                );
            } else {
                tracing::info!(
                    "Migrated AGENTS.md to workspace root: {}",
                    new_path.display()
                );
            }
            break; // moved, done
        }
    }
}

fn migrate_legacy_runtime_layout(data_root: &Path, runtime_dir: &Path) {
    if std::fs::create_dir_all(runtime_dir).is_err() {
        return;
    }
    ensure_workspace_shared_dir(data_root);

    let entries = match std::fs::read_dir(data_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == "skills"
            || name_str == "runtime"
            || name_str == "shared"
            || name_str == "mcp.json"
        {
            continue;
        }
        let src = entry.path();
        let dst = runtime_dir.join(name_str);
        if dst.exists() {
            continue;
        }
        if let Err(e) = move_path(&src, &dst) {
            tracing::warn!(
                "Failed to migrate legacy data '{}' -> '{}': {}",
                src.display(),
                dst.display(),
                e
            );
        } else {
            tracing::info!(
                "Migrated legacy runtime data '{}' -> '{}'",
                src.display(),
                dst.display()
            );
        }
    }
}

fn is_llm_ready(config: &Config) -> bool {
    finally_a_value_bot::llm_catalog::any_provider_api_key_configured()
        || matches!(
            config.llm_provider.as_str(),
            "ollama" | "llama" | "llamacpp"
        )
}

fn has_any_realtime_channel(config: &Config, db: &db::Database) -> bool {
    channel_integration_config::has_any_messaging_bot_token(db).unwrap_or_else(|_| {
        !config.telegram_bot_token.trim().is_empty()
            || config.discord_bot_token.is_some()
            || config
                .whatsapp_access_token
                .as_deref()
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false)
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str());

    match command {
        Some("start") => {}
        Some("gateway") => {
            gateway::handle_gateway_cli(&args[2..])?;
            return Ok(());
        }
        Some("setup") => {
            println!("`setup` is retired. Use Web UI onboarding instead.");
            println!("Start the app, then open http://127.0.0.1:10961 and configure Settings.");
            return Ok(());
        }
        Some("config") => {
            println!("`config` is retired. Use Web UI onboarding instead.");
            println!("Start the app, then open http://127.0.0.1:10961 and configure Settings.");
            return Ok(());
        }
        Some("doctor") => {
            doctor::run_cli(&args[2..])?;
            return Ok(());
        }
        Some("test-llm") => {
            let with_tools = args.get(2).map(|s| s.as_str()) == Some("--with-tools");
            run_test_llm(with_tools).await?;
            return Ok(());
        }
        Some("version" | "--version" | "-V") => {
            print_version();
            return Ok(());
        }
        Some("help" | "--help" | "-h") | None => {
            print_help();
            return Ok(());
        }
        Some(unknown) => {
            eprintln!("Unknown command: {unknown}\n");
            print_help();
            std::process::exit(1);
        }
    }

    let mut config = match Config::load() {
        Ok(c) => c,
        Err(FinallyAValueBotError::Config(e)) => {
            eprintln!("Config missing/invalid: {e}");
            eprintln!("Create or fix .env (bootstrap values), then open Web UI to finish setup.");
            eprintln!(
                "Example: cp .env.example .env && finally_a_value_bot start (then visit Web UI)"
            );
            return Err(anyhow::anyhow!("config required: {e}"));
        }
        Err(e) => return Err(e.into()),
    };
    info!("Starting FinallyAValueBot bot...");

    let data_root_dir = config.data_root_dir();
    let runtime_data_dir = config.runtime_data_dir();
    migrate_legacy_runtime_layout(&data_root_dir, Path::new(&runtime_data_dir));
    migrate_repo_shared_into_workspace(Path::new(config.working_dir()));
    migrate_agents_md_to_workspace_root(
        Path::new(config.working_dir()),
        Path::new(&runtime_data_dir),
    );

    match builtin_skills::resolve_builtin_skills_dir(&config) {
        Some(p) => info!("Built-in skills directory: {}", p.display()),
        None => tracing::warn!(
            "Built-in skills directory not found; set FINALLY_A_VALUE_BOT_BUILTIN_SKILLS or keep `builtin_skills/` next to the workspace data root. Only skills under the workspace will be available until then."
        ),
    }

    if std::env::var("FINALLY_A_VALUE_BOT_GATEWAY").is_ok() {
        logging::init_logging(&runtime_data_dir)?;
    } else {
        logging::init_console_logging();
    }

    let db = db::Database::new(&runtime_data_dir)?;
    info!("Database initialized");

    config.merge_llm_selection_from_app_settings(&db)?;
    info!(
        "LLM selection: provider={} model={}",
        config.llm_provider, config.model
    );

    match finally_a_value_bot::persona_shared_migrate::maybe_run(&config, &db) {
        Ok(stats) => {
            if stats.moved_to_persona + stats.moved_to_skills + stats.moved_to_unmigrated > 0 {
                info!(
                    "Persona shared migration: moved_to_persona={}, moved_to_skills={}, moved_to_unmigrated={}",
                    stats.moved_to_persona, stats.moved_to_skills, stats.moved_to_unmigrated
                );
            }
        }
        Err(e) => tracing::warn!("Persona shared migration skipped: {}", e),
    }

    channel_integration_config::migrate_from_env_if_empty(&db, &config)?;
    config.merge_channel_integration_from_db(&db)?;
    channel_integration_config::validate_runtime_channel_or_web(&config, &db)?;
    match db.provision_all_missing_sibling_bindings() {
        Ok(n) if n > 0 => {
            info!("Auto-provisioned {n} sibling channel binding(s) for bot instances")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Sibling channel binding provisioning skipped: {}", e),
    }

    finally_a_value_bot::mempalace_setup::apply_managed_mempalace(&mut config);

    if let Err(e) = gateway::seed_default_vault_maintenance_tasks(&config) {
        tracing::warn!("Failed to seed recurring vault tasks: {}", e);
    }

    // Seed onboarding task for fresh installations
    if is_llm_ready(&config) && has_any_realtime_channel(&config, &db) {
        let seed_chat_id = config.universal_chat_id.unwrap_or(997894126);
        let seed_persona_id = db.get_current_persona_id(seed_chat_id)?;
        let intro_name = if config.agent_display_name.trim().is_empty() {
            "your assistant".to_string()
        } else {
            config.agent_display_name.trim().to_string()
        };
        if let Err(e) = db.ensure_onboarding_task(
            seed_chat_id,
            seed_persona_id,
            &format!(
                "Hello! I am {}, your agentic assistant. I see this is a fresh installation. How can I help you get started? Please tell me about your projects or what you'd like me to track.",
                intro_name
            )
        ) {
            tracing::warn!("Failed to seed onboarding task: {}", e);
        }
    }

    let principles_path = config
        .vault
        .as_ref()
        .and_then(|v| v.principles_path.clone());
    let memory_manager = memory::MemoryManager::with_principles_path(
        &runtime_data_dir,
        config.working_dir(),
        principles_path,
    );
    info!("Memory manager initialized");

    // Workspace + shared skills first (override names); then repository `builtin_skills/` when resolved.
    let skill_manager = skills::SkillManager::from_skills_dirs(config.skill_discovery_dirs());
    let discovered = skill_manager.discover_skills();
    info!(
        "Skill manager initialized ({} skills discovered)",
        discovered.len()
    );

    // Initialize MCP servers (optional, configured via <data_root>/mcp.json)
    let mcp_config_path = data_root_dir.join("mcp.json").to_string_lossy().to_string();
    let mcp_manager = mcp::McpManager::from_config_file(&mcp_config_path).await;
    let mcp_tool_count: usize = mcp_manager.all_tools().len();
    if mcp_tool_count > 0 {
        info!("MCP initialized: {} tools available", mcp_tool_count);
    }

    telegram::run_bot(config, db, memory_manager, skill_manager, mcp_manager).await?;

    Ok(())
}
