use meido::config::Config;
use meido::error::FinallyAValueBotError;
use meido::{
    builtin_skills, channel_integration_config, daemon, db, gateway, logging, mcp, memory, runtime,
    skills,
};
use std::path::{Path, PathBuf};
use tracing::info;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        r#"Meido v{VERSION} — Finally a helpful one. Web-first agentic assistant; Telegram, WhatsApp, Discord & WeCom are optional channels

USAGE:
    meido <COMMAND>

COMMANDS:
    start [--foreground]  Start Meido and the web UI, then return
                          (--foreground stays attached to this terminal)
    stop                  Stop the process from `meido start`
    gateway               Optional login service on macOS and Linux.
                          Not required for the web UI. See `meido gateway help`.
    version               Show version information
    help                  Show this help message

WEB UI AND LOGIN SERVICE:
    `meido start` serves the web UI. It returns immediately and stays up until
    `meido stop` or a reboot.
    `meido gateway` is optional. It is a user service (systemd on Linux, launchd
    on macOS) that runs that same start at login and restarts Meido if the
    process exits. Windows has no gateway; use `meido start`.

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
       MEIDO_WORKSPACE_DIR (or WORKSPACE_DIR), WEB_HOST/WEB_PORT, WEB_AUTH_TOKEN when exposing non-local.
    2. Start the app: meido start
    3. Open Web UI: http://127.0.0.1:10961 and finish runtime settings there.

       Runtime channel/LLM settings are now managed from Web UI and persisted in SQLite.

CONFIG FILE (.env):
    Meido reads configuration from .env in the current directory.
    Copy .env.example to .env and fill in values. Override path with MEIDO_CONFIG.

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
    meido start               Start Meido and the web UI, then return
    meido start --foreground  Start attached to this terminal
    meido stop                Stop that process. Also stops the login service if it is running.
    meido gateway install     Optional: install the login service (macOS and Linux)
    meido gateway status      Show whether that service is running
    meido gateway logs 100    Show last 100 lines of its logs
    meido version             Show version
    meido help                Show this message

ABOUT:
    https://github.com/kenpyfin/meido"#
    );
}

fn print_detached_status(config: &Config, runtime_dir: &str, pid: u32, wait_for_bind: bool) {
    if wait_for_bind {
        println!("Meido started in the background (pid {pid}).");
    } else {
        println!("Meido is already running (pid {pid}).");
    }
    if wait_for_bind && config.web_enabled {
        println!("Waiting for the web UI to bind...");
        match daemon::wait_for_bound_port(Path::new(runtime_dir), pid) {
            daemon::BoundWait::Ready(port) => {
                println!("{}", daemon::web_ui_line(&config.web_host, port, true));
            }
            daemon::BoundWait::Failed => {
                println!("The web UI failed to bind. Logs: {runtime_dir}/logs");
            }
            daemon::BoundWait::Exited => {
                println!("Meido exited before the web UI bound. Logs: {runtime_dir}/logs");
            }
            daemon::BoundWait::TimedOut => {
                println!("The web UI has not bound yet. Logs: {runtime_dir}/logs");
            }
        }
    } else if let Some(port) = daemon::bound_port_for_pid(Path::new(runtime_dir), pid) {
        println!(
            "{}",
            daemon::web_ui_line(&config.web_host, port, config.web_enabled)
        );
    } else {
        println!(
            "{}",
            daemon::web_ui_line(&config.web_host, config.web_port, config.web_enabled)
        );
    }
    println!("Logs: {runtime_dir}/logs");
    println!("Stop with: meido stop");
}

fn load_config() -> anyhow::Result<Config> {
    match Config::load() {
        Ok(config) => {
            config.publish_workspace_paths();
            Ok(config)
        }
        Err(FinallyAValueBotError::Config(e)) => {
            eprintln!("Config missing/invalid: {e}");
            eprintln!("Create or fix .env (bootstrap values), then open Web UI to finish setup.");
            eprintln!("Example: cp .env.example .env && meido start (then visit Web UI)");
            Err(anyhow::anyhow!("config required: {e}"))
        }
        Err(e) => Err(e.into()),
    }
}

fn print_version() {
    println!("meido {VERSION}");
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
    let shadow = meido::tools::shadow_workspace_path(data_root);
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

const DEFAULT_AGENTS_MD: &str = include_str!("../defaults/AGENTS.md");

/// `true` when principles stay at `{workspace}/AGENTS.md` (unset or that relative name).
fn principles_at_workspace_root(principles_path: Option<&str>) -> bool {
    match principles_path.map(str::trim).filter(|p| !p.is_empty()) {
        None | Some("AGENTS.md") | Some("./AGENTS.md") => true,
        Some(_) => false,
    }
}

/// Principles file from `VAULT_PRINCIPLES_PATH` when set. Absolute paths are kept.
/// Relative `..` is refused. Unset means `{workspace}/AGENTS.md`.
fn principles_dest(workspace_root: &Path, principles_path: Option<&str>) -> Option<PathBuf> {
    let rel = principles_path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("AGENTS.md");
    let path = Path::new(rel);
    if !path.is_absolute()
        && (rel.contains(':')
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)))
    {
        return None;
    }
    let dest = meido::config::resolve_workspace_data_path(workspace_root, rel);
    if dest.as_os_str().is_empty() {
        None
    } else {
        Some(dest)
    }
}

/// Write the shipped principles file when the configured path has none.
/// An existing file is left unchanged.
fn ensure_default_agents_md(workspace_root: &Path, principles_path: Option<&str>) {
    let Some(dest) = principles_dest(workspace_root, principles_path) else {
        tracing::warn!(
            "Not writing default principles: VAULT_PRINCIPLES_PATH is not a safe relative path"
        );
        return;
    };
    if dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            tracing::warn!(
                "Failed to create directory for default principles: {}",
                parent.display()
            );
            return;
        }
    }
    if let Err(e) = std::fs::write(&dest, DEFAULT_AGENTS_MD) {
        tracing::warn!(
            "Failed to write default principles to '{}': {}",
            dest.display(),
            e
        );
    } else {
        tracing::info!("Wrote default principles to {}", dest.display());
    }
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
    meido::llm_catalog::any_provider_api_key_configured()
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
        Some("stop") => {
            let config = load_config()?;
            let gateway_stopped = match gateway::stop_if_active() {
                Ok(stopped) => stopped,
                Err(e) => {
                    eprintln!("Warning: failed to stop gateway service: {e}");
                    false
                }
            };
            let stopped = daemon::stop(Path::new(&config.runtime_data_dir()), config.web_port)?;
            if stopped.is_empty() && !gateway_stopped {
                println!("Meido is not running.");
            }
            return Ok(());
        }
        Some("gateway") => {
            gateway::handle_gateway_cli(&args[2..])?;
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

    let mut config = load_config()?;
    let foreground = args.iter().any(|arg| arg == "--foreground" || arg == "-f");
    let runtime_data_dir = config.runtime_data_dir();
    match daemon::resolve_start(Path::new(&runtime_data_dir), foreground)? {
        daemon::StartRole::Run => {}
        daemon::StartRole::Background { pid } => {
            print_detached_status(&config, &runtime_data_dir, pid, true);
            return Ok(());
        }
        daemon::StartRole::AlreadyRunning { pid } => {
            print_detached_status(&config, &runtime_data_dir, pid, false);
            return Ok(());
        }
    }
    let _pid_guard = if daemon::is_detached_child() {
        Some(daemon::PidGuard::install(Path::new(&runtime_data_dir))?)
    } else {
        None
    };
    if daemon::is_detached_child() || meido::meido_env::is_present(meido::meido_env::GATEWAY) {
        logging::init_logging(&runtime_data_dir)?;
    } else {
        logging::init_console_logging();
    }
    info!("Starting Meido...");

    let data_root_dir = config.data_root_dir();
    migrate_legacy_runtime_layout(&data_root_dir, Path::new(&runtime_data_dir));
    migrate_repo_shared_into_workspace(Path::new(config.working_dir()));
    let principles_path = config
        .vault
        .as_ref()
        .and_then(|vault| vault.principles_path.as_deref());
    if principles_at_workspace_root(principles_path) {
        migrate_agents_md_to_workspace_root(
            Path::new(config.working_dir()),
            Path::new(&runtime_data_dir),
        );
    }
    ensure_default_agents_md(Path::new(config.working_dir()), principles_path);

    match builtin_skills::resolve_builtin_skills_dir(&config) {
        Some(p) => info!("Built-in skills directory: {}", p.display()),
        None => tracing::warn!(
            "Built-in skills directory not found; set MEIDO_BUILTIN_SKILLS or keep `builtin_skills/` next to the workspace data root. Only skills under the workspace will be available until then."
        ),
    }

    let db = db::Database::new(&runtime_data_dir)?;
    info!("Database initialized");

    config.merge_llm_selection_from_app_settings(&db)?;
    meido::topics::seed_system_llm_from_env(&db)?;
    if let Ok(Some(env_path)) = meido::config::Config::resolve_config_path() {
        if let Ok(content) = std::fs::read_to_string(&env_path) {
            let mut next = content.clone();
            for key in [
                "LLM_PROVIDER",
                "LLM_MODEL",
                "LLM_BASE_URL",
                "TOPICS_PREFER_TIER2",
                "MULTIMODEL_ENABLED",
                "MULTIMODEL_LOCAL_BASE_URL",
                "MULTIMODEL_LOCAL_MODEL",
                "MULTIMODEL_LOCAL_TOOLS_OK",
                "MULTIMODEL_TIER1_BASE_URL",
                "MULTIMODEL_TIER1_MODEL",
                "MULTIMODEL_TIER1_TOOLS_OK",
                "MULTIMODEL_TIER2_BASE_URL",
                "MULTIMODEL_TIER2_MODEL",
                "MULTIMODEL_TIER2_TOOLS_OK",
            ] {
                next = meido::dotenv_sync::remove_env_line(&next, key);
            }
            if next != content {
                let _ = std::fs::write(&env_path, next);
            }
        }
    }
    info!("Chat models are stored on each persona. System LLM is a separate pair.");

    match meido::persona_shared_migrate::maybe_run(&config, &db) {
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

    meido::mempalace_setup::apply_managed_mempalace(&mut config);

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
    )
    .with_origin_vault_root(config.origin_vault_root());
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

    runtime::run_app(config, db, memory_manager, skill_manager, mcp_manager).await?;

    Ok(())
}
