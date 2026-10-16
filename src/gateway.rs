use crate::config::Config;
use crate::db::Database;
use crate::logging;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

const LINUX_SERVICE_NAME: &str = "meido-gateway.service";
const MAC_LABEL: &str = "ai.meido.gateway";
const LOG_STDOUT_FILE: &str = "meido-gateway.log";
const LOG_STDERR_FILE: &str = "meido-gateway.error.log";
const DEFAULT_LOG_LINES: usize = 200;
const STOP_TIMEOUT_SECS: u64 = 10;
/// Vault index at 00:00, 06:00, 12:00, and 18:00 in the configured timezone.
const VAULT_INDEX_CRON: &str = "0 0 */6 * * *";
/// Vault git push at 03:00, 09:00, 15:00, and 21:00 so it does not start with indexing.
const VAULT_PUSH_CRON: &str = "0 0 3,9,15,21 * * *";

#[derive(Debug, Clone)]
struct ServiceContext {
    exe_path: PathBuf,
    working_dir: PathBuf,
    config_path: Option<PathBuf>,
    runtime_logs_dir: PathBuf,
    /// PATH at install time, so the service can find MCP commands, etc.
    path_env: Option<String>,
    /// HOME at install time, so config paths like ~/... resolve correctly.
    home_env: Option<String>,
}

pub fn handle_gateway_cli(args: &[String]) -> Result<()> {
    let Some(action) = args.first().map(|s| s.as_str()) else {
        print_gateway_help();
        return Ok(());
    };

    match action {
        "install" => install(),
        "uninstall" => uninstall(),
        "start" => start(),
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(args.get(1).map(|s| s.as_str())),
        "help" | "--help" | "-h" => {
            print_gateway_help();
            Ok(())
        }
        _ => Err(anyhow!(
            "Unknown gateway action: {}. Use: gateway <install|uninstall|start|stop|status|logs>",
            action
        )),
    }
}

pub fn print_gateway_help() {
    println!(
        r#"Optional login service (macOS and Linux)

`meido start` serves the web UI. It returns immediately and stays up until
`meido stop` or a reboot. You do not need this service to open the web UI.

`meido gateway` installs a user service that runs that same start:
    - starts Meido when you log in
    - restarts it if the process exits
    - keeps PATH and HOME from install time (a service does not inherit your shell)

USAGE:
    meido gateway <ACTION>

ACTIONS:
    install      Install and enable the login service, then start it
    uninstall    Disable and remove the login service
    start        Start the login service
    stop         Stop the login service (it stays stopped until you start it)
    status       Show whether the login service is running
    logs [N]     Show last N lines of its logs (default: 200)
    help         Show this message

NOTE: install captures your current PATH and HOME so tools and MCP commands
work under the service. If you add new executables to PATH later,
run: meido gateway uninstall && meido gateway install
"#
    );
}

/// Returns true when the user-level gateway unit/plist from `gateway install` is present.
pub fn user_gateway_service_installed() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux_unit_path().map(|p| p.exists()).unwrap_or(false)
    }
    #[cfg(target_os = "macos")]
    {
        mac_plist_path().map(|p| p.exists()).unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn restart_user_gateway_now() -> Result<()> {
    let output = run_command("systemctl", &["--user", "restart", LINUX_SERVICE_NAME])?;
    ensure_success(
        output,
        "systemctl",
        &["--user", "restart", LINUX_SERVICE_NAME],
    )
}

#[cfg(target_os = "macos")]
pub(crate) fn restart_user_gateway_now() -> Result<()> {
    let target = mac_target_label()?;
    let output = run_command("launchctl", &["kickstart", "-k", &target])?;
    ensure_success(output, "launchctl", &["kickstart", "-k", &target])
}

fn install() -> Result<()> {
    let ctx = build_context()?;
    if cfg!(target_os = "macos") {
        install_macos(&ctx)?;
    } else if cfg!(target_os = "linux") {
        install_linux(&ctx)?;
    } else {
        return Err(anyhow!(
            "The login service is only supported on macOS and Linux. Use `meido start` to serve the web UI."
        ));
    }
    match load_config_for_gateway(&ctx) {
        Ok(mut cfg) => {
            crate::mempalace_setup::apply_managed_mempalace(&mut cfg);
            if let Err(e) = seed_default_vault_maintenance_tasks(&cfg) {
                eprintln!("Warning: failed to seed recurring vault tasks: {e}");
            }
        }
        Err(e) => eprintln!("Warning: failed to seed recurring vault tasks: {e}"),
    }
    Ok(())
}

fn uninstall() -> Result<()> {
    if cfg!(target_os = "macos") {
        uninstall_macos()
    } else if cfg!(target_os = "linux") {
        uninstall_linux()
    } else {
        Err(anyhow!(
            "The login service is only supported on macOS and Linux. Use `meido start` to serve the web UI."
        ))
    }
}

fn start() -> Result<()> {
    if cfg!(target_os = "macos") {
        start_macos()
    } else if cfg!(target_os = "linux") {
        start_linux()
    } else {
        Err(anyhow!(
            "The login service is only supported on macOS and Linux. Use `meido start` to serve the web UI."
        ))
    }
}

/// Stop the user gateway when it is installed and running.
/// `systemctl stop` does not restart a unit with `Restart=always`.
pub fn stop_if_active() -> Result<bool> {
    if !user_gateway_service_installed() || !gateway_is_active() {
        return Ok(false);
    }
    stop()?;
    Ok(true)
}

fn gateway_is_active() -> bool {
    #[cfg(target_os = "linux")]
    {
        match run_command("systemctl", &["--user", "is-active", LINUX_SERVICE_NAME]) {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout);
                matches!(state.trim(), "active" | "activating" | "reloading")
            }
            Err(_) => false,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(target) = mac_target_label() else {
            return false;
        };
        match run_command("launchctl", &["print", &target]) {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

fn stop() -> Result<()> {
    if cfg!(target_os = "macos") {
        stop_macos()
    } else if cfg!(target_os = "linux") {
        stop_linux()
    } else {
        Err(anyhow!(
            "The login service is only supported on macOS and Linux. Use `meido start` to serve the web UI."
        ))
    }
}

fn status() -> Result<()> {
    if cfg!(target_os = "macos") {
        status_macos()
    } else if cfg!(target_os = "linux") {
        status_linux()
    } else {
        Err(anyhow!(
            "The login service is only supported on macOS and Linux. Use `meido start` to serve the web UI."
        ))
    }
}

fn logs(lines_arg: Option<&str>) -> Result<()> {
    let lines = parse_log_lines(lines_arg)?;
    let ctx = build_context()?;
    println!("== gateway logs: {} ==", ctx.runtime_logs_dir.display());
    let tailed = logging::read_last_lines_from_logs(&ctx.runtime_logs_dir, lines)?;
    if tailed.is_empty() {
        println!("(no log lines found)");
    } else {
        println!("{}", tailed.join("\n"));
    }
    Ok(())
}

fn parse_log_lines(lines_arg: Option<&str>) -> Result<usize> {
    match lines_arg {
        None => Ok(DEFAULT_LOG_LINES),
        Some(raw) => {
            let parsed = raw
                .parse::<usize>()
                .with_context(|| format!("Invalid log line count: {}", raw))?;
            if parsed == 0 {
                return Err(anyhow!("Log line count must be greater than 0"));
            }
            Ok(parsed)
        }
    }
}

fn build_context() -> Result<ServiceContext> {
    let exe_path = std::env::current_exe().context("Failed to resolve current binary path")?;
    let working_dir = std::env::current_dir().context("Failed to resolve current directory")?;
    let config_path = resolve_config_path(&working_dir);
    let runtime_logs_dir = resolve_runtime_logs_dir(&working_dir);
    let path_env = std::env::var("PATH").ok().filter(|s| !s.trim().is_empty());
    let home_env = std::env::var("HOME").ok().filter(|s| !s.trim().is_empty());

    Ok(ServiceContext {
        exe_path,
        working_dir,
        config_path,
        runtime_logs_dir,
        path_env,
        home_env,
    })
}

fn resolve_config_path(cwd: &Path) -> Option<PathBuf> {
    if let Some(from_env) = crate::meido_env::get(crate::meido_env::CONFIG) {
        let path = PathBuf::from(from_env);
        return Some(if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        });
    }

    if cwd.join(".env").exists() {
        return Some(cwd.join(".env"));
    }
    None
}

fn resolve_runtime_logs_dir(cwd: &Path) -> PathBuf {
    match Config::load() {
        Ok(cfg) => PathBuf::from(cfg.runtime_data_dir()).join("logs"),
        Err(_) => cwd.join("runtime").join("logs"),
    }
}

fn load_config_for_gateway(ctx: &ServiceContext) -> Result<Config> {
    let config = if let Some(path) = &ctx.config_path {
        Config::load_from_path(path)?
    } else {
        Config::load()?
    };
    config.publish_workspace_paths();
    Ok(config)
}

/// Create or retime the built-in vault index and git-push jobs so they do not share a start time.
pub fn seed_default_vault_maintenance_tasks(cfg: &Config) -> Result<()> {
    let runtime_data_dir = cfg.runtime_data_dir();
    let db = Database::new(&runtime_data_dir)?;
    let chat_id = cfg.universal_chat_id.unwrap_or(997894126);
    let persona_id = db.ensure_named_persona(chat_id, "default")?;
    let timezone = cfg.timezone.trim();

    let workspace_root = cfg.workspace_root_absolute();
    let ws_index = workspace_root
        .join("skills")
        .join("index-vault")
        .join("index_vault.py");
    let index_script = if ws_index.exists() {
        ws_index
    } else if let Some(p) = crate::builtin_skills::resolve_builtin_skills_dir(cfg)
        .map(|b| b.join("index-vault").join("index_vault.py"))
        .filter(|p| p.exists())
    {
        p
    } else {
        ws_index
    };
    let venv_python = workspace_root
        .join("shared")
        .join(".venv-vault")
        .join("bin")
        .join("python");
    let python_bin = if venv_python.exists() {
        venv_python.to_string_lossy().to_string()
    } else {
        "python3".to_string()
    };

    let index_prompt = format!(
        "Run the vault indexing script: {} {}. When finished, send a message to the user confirming the indexing status.",
        python_bin,
        index_script.display()
    );
    db.ensure_indexing_task(
        chat_id,
        persona_id,
        &index_prompt,
        VAULT_INDEX_CRON,
        timezone,
    )?;

    let push_prompt = format!(
        "Sync ORIGIN vault to git remote: run `cd {} && git add -A && git commit -m \"auto: vault sync\" || true && git push origin HEAD`. If there is nothing to commit, report that no changes were pushed.",
        workspace_root.join("shared").join("ORIGIN").display()
    );
    db.ensure_vault_push_task(chat_id, persona_id, &push_prompt, VAULT_PUSH_CRON, timezone)?;
    sync_recall_maintenance(&db, cfg, chat_id, persona_id, timezone)?;
    Ok(())
}

fn sync_recall_maintenance(
    db: &Database,
    cfg: &Config,
    chat_id: i64,
    persona_id: i64,
    timezone: &str,
) -> Result<()> {
    let mode = crate::recall::RecallMode::parse(&cfg.mempalace.recall_mode);
    let recall_enabled = mode != crate::recall::RecallMode::Disabled;
    let reconcile_prompt = format!(
        "{} Incrementally reconcile the MemPalace recall index from canonical messages, ORIGIN files, and memory snapshots. This task is executed by the gateway, not the model.",
        crate::recall::RECALL_RECONCILE_PREFIX
    );
    if recall_enabled {
        db.ensure_recall_reconcile_task(
            chat_id,
            persona_id,
            &reconcile_prompt,
            crate::recall::RECALL_RECONCILE_CRON,
            timezone,
        )?;
        if db
            .get_app_setting(crate::recall::RECALL_RECONCILE_PAUSE_KEY)?
            .as_deref()
            == Some("1")
        {
            for task_id in db.list_task_ids_by_prompt_prefix(
                chat_id,
                crate::recall::RECALL_RECONCILE_PREFIX,
                "paused",
            )? {
                db.update_task_status(task_id, "active")?;
            }
            db.set_app_setting(crate::recall::RECALL_RECONCILE_PAUSE_KEY, "0")?;
        }
    } else {
        let paused = db.list_task_ids_by_prompt_prefix(
            chat_id,
            crate::recall::RECALL_RECONCILE_PREFIX,
            "active",
        )?;
        if !paused.is_empty() {
            for task_id in paused {
                db.update_task_status(task_id, "paused")?;
            }
            db.set_app_setting(crate::recall::RECALL_RECONCILE_PAUSE_KEY, "1")?;
        }
    }

    if mode == crate::recall::RecallMode::Default && cfg.mempalace.configured() {
        if db
            .recall_checkpoint("last_success")?
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            db.schedule_active_tasks_with_prefix_now(
                chat_id,
                crate::recall::RECALL_RECONCILE_PREFIX,
            )?;
        }
        let active =
            db.list_task_ids_by_prompt_prefix(chat_id, "Run the vault indexing script:", "active")?;
        if !active.is_empty() {
            for task_id in &active {
                db.update_task_status(*task_id, "paused")?;
            }
            let ids = active
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            db.set_app_setting(crate::recall::RECALL_INDEX_PAUSE_KEY, &ids)?;
        }
    } else if let Some(raw) = db.get_app_setting(crate::recall::RECALL_INDEX_PAUSE_KEY)? {
        for id in raw
            .split(',')
            .filter_map(|part| part.trim().parse::<i64>().ok())
        {
            if db.task_status(id)?.as_deref() == Some("paused") {
                db.update_task_status(id, "active")?;
            }
        }
        db.set_app_setting(crate::recall::RECALL_INDEX_PAUSE_KEY, "")?;
    }
    Ok(())
}

fn run_command(cmd: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute command: {} {}", cmd, args.join(" ")))?;
    Ok(output)
}

/// Run a command with a timeout. If it doesn't finish in time, returns an error (command may still be running).
fn run_command_with_timeout(
    cmd: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output> {
    let cmd_str = cmd.to_string();
    let args_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let (tx, rx) = mpsc::channel();
    let cmd_spawn = cmd_str.clone();
    let args_spawn = args_vec.clone();
    std::thread::spawn(move || {
        let output = Command::new(&cmd_spawn).args(&args_spawn).output();
        let _ = tx.send(output);
    });
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(anyhow!(
            "Failed to run {} {}: {}",
            cmd_str,
            args_vec.join(" "),
            e
        )),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!(
            "Stop command did not complete within {} seconds (gateway process may be stuck). \
            Force stop: run `pkill -9 -x meido` then `meido gateway uninstall`.",
            timeout_secs
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow!("Stop command thread exited unexpectedly"))
        }
    }
}

fn ensure_success(output: std::process::Output, cmd: &str, args: &[&str]) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "Command failed: {} {}\nstdout: {}\nstderr: {}",
        cmd,
        args.join(" "),
        stdout.trim(),
        stderr.trim()
    ))
}

fn linux_unit_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join(LINUX_SERVICE_NAME))
}

fn render_linux_unit(ctx: &ServiceContext) -> String {
    let mut unit = String::new();
    unit.push_str("[Unit]\n");
    unit.push_str("Description=Meido Gateway Service\n");
    unit.push_str("After=network.target\n\n");
    unit.push_str("[Service]\n");
    unit.push_str("Type=simple\n");
    unit.push_str(&format!("WorkingDirectory={}\n", ctx.working_dir.display()));
    unit.push_str(&format!("ExecStart={} start\n", ctx.exe_path.display()));
    unit.push_str(&format!(
        "Environment={}=1\n",
        crate::meido_env::GATEWAY.preferred
    ));
    if let Some(config_path) = &ctx.config_path {
        unit.push_str(&format!(
            "Environment={}={}\n",
            crate::meido_env::CONFIG.preferred,
            config_path.display()
        ));
    }
    // Pass install-time PATH/HOME so tools (browser, bash, MCP) find executables
    if let Some(ref path) = ctx.path_env {
        let safe = path.replace('\n', "").replace('"', "\\\"");
        unit.push_str(&format!("Environment=PATH={}\n", safe));
    }
    if let Some(ref home) = ctx.home_env {
        let safe = home.replace('\n', "").replace('"', "\\\"");
        unit.push_str(&format!("Environment=HOME={}\n", safe));
    }
    unit.push_str("Restart=always\n");
    unit.push_str("RestartSec=5\n\n");
    unit.push_str("[Install]\n");
    unit.push_str("WantedBy=default.target\n");
    unit
}

fn install_linux(ctx: &ServiceContext) -> Result<()> {
    let unit_path = linux_unit_path()?;
    let unit_dir = unit_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid unit path"))?;
    std::fs::create_dir_all(unit_dir)
        .with_context(|| format!("Failed to create {}", unit_dir.display()))?;
    std::fs::write(&unit_path, render_linux_unit(ctx))
        .with_context(|| format!("Failed to write {}", unit_path.display()))?;

    ensure_success(
        run_command("systemctl", &["--user", "daemon-reload"])?,
        "systemctl",
        &["--user", "daemon-reload"],
    )?;
    ensure_success(
        run_command(
            "systemctl",
            &["--user", "enable", "--now", LINUX_SERVICE_NAME],
        )?,
        "systemctl",
        &["--user", "enable", "--now", LINUX_SERVICE_NAME],
    )?;

    println!(
        "Installed and started the login service: {}",
        unit_path.display()
    );
    Ok(())
}

fn uninstall_linux() -> Result<()> {
    let _ = run_command(
        "systemctl",
        &["--user", "disable", "--now", LINUX_SERVICE_NAME],
    );
    let _ = run_command("systemctl", &["--user", "daemon-reload"]);

    let unit_path = linux_unit_path()?;
    if unit_path.exists() {
        std::fs::remove_file(&unit_path)
            .with_context(|| format!("Failed to remove {}", unit_path.display()))?;
    }
    let _ = run_command("systemctl", &["--user", "daemon-reload"]);
    println!("Uninstalled the login service");
    Ok(())
}

fn start_linux() -> Result<()> {
    ensure_success(
        run_command("systemctl", &["--user", "start", LINUX_SERVICE_NAME])?,
        "systemctl",
        &["--user", "start", LINUX_SERVICE_NAME],
    )?;
    println!("Login service started");
    Ok(())
}

fn stop_linux() -> Result<()> {
    let output = run_command_with_timeout(
        "systemctl",
        &["--user", "stop", LINUX_SERVICE_NAME],
        STOP_TIMEOUT_SECS,
    )?;
    ensure_success(output, "systemctl", &["--user", "stop", LINUX_SERVICE_NAME])?;
    println!("Login service stopped");
    Ok(())
}

fn status_linux() -> Result<()> {
    let output = run_command(
        "systemctl",
        &["--user", "status", LINUX_SERVICE_NAME, "--no-pager"],
    )?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("Login service is not running"))
    }
}

fn mac_plist_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{MAC_LABEL}.plist")))
}

fn current_uid() -> Result<String> {
    if let Ok(uid) = std::env::var("UID") {
        if !uid.trim().is_empty() {
            return Ok(uid);
        }
    }
    let output = run_command("id", &["-u"])?;
    if !output.status.success() {
        return Err(anyhow!("Failed to determine user id"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_macos_plist(ctx: &ServiceContext) -> String {
    let mut items = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".to_string(),
        "<plist version=\"1.0\">".to_string(),
        "<dict>".to_string(),
        "  <key>Label</key>".to_string(),
        format!("  <string>{MAC_LABEL}</string>"),
        "  <key>ProgramArguments</key>".to_string(),
        "  <array>".to_string(),
        format!("    <string>{}</string>", xml_escape(&ctx.exe_path.to_string_lossy())),
        "    <string>start</string>".to_string(),
        "  </array>".to_string(),
        "  <key>WorkingDirectory</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.to_string_lossy())
        ),
        "  <key>RunAtLoad</key>".to_string(),
        "  <true/>".to_string(),
        "  <key>KeepAlive</key>".to_string(),
        "  <true/>".to_string(),
        "  <key>StandardOutPath</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.join(LOG_STDOUT_FILE).to_string_lossy())
        ),
        "  <key>StandardErrorPath</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.join(LOG_STDERR_FILE).to_string_lossy())
        ),
    ];

    items.push("  <key>EnvironmentVariables</key>".to_string());
    items.push("  <dict>".to_string());
    items.push(format!(
        "    <key>{}</key>",
        crate::meido_env::GATEWAY.preferred
    ));
    items.push("    <string>1</string>".to_string());
    if let Some(config_path) = &ctx.config_path {
        items.push(format!(
            "    <key>{}</key>",
            crate::meido_env::CONFIG.preferred
        ));
        items.push(format!(
            "    <string>{}</string>",
            xml_escape(&config_path.to_string_lossy())
        ));
    }
    // Pass install-time PATH/HOME so tools (browser, bash, MCP) find executables
    if let Some(ref path) = ctx.path_env {
        items.push("    <key>PATH</key>".to_string());
        items.push(format!("    <string>{}</string>", xml_escape(path)));
    }
    if let Some(ref home) = ctx.home_env {
        items.push("    <key>HOME</key>".to_string());
        items.push(format!("    <string>{}</string>", xml_escape(home)));
    }
    items.push("  </dict>".to_string());

    items.push("</dict>".to_string());
    items.push("</plist>".to_string());
    items.join("\n")
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

fn mac_target_label() -> Result<String> {
    let uid = current_uid()?;
    Ok(format!("gui/{uid}/{MAC_LABEL}"))
}

fn install_macos(ctx: &ServiceContext) -> Result<()> {
    let plist_path = mac_plist_path()?;
    let launch_agents = plist_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid plist path"))?;
    std::fs::create_dir_all(launch_agents)
        .with_context(|| format!("Failed to create {}", launch_agents.display()))?;
    std::fs::write(&plist_path, render_macos_plist(ctx))
        .with_context(|| format!("Failed to write {}", plist_path.display()))?;

    let _ = stop_macos();
    start_macos()?;
    println!(
        "Installed and started the login service: {}",
        plist_path.display()
    );
    Ok(())
}

fn uninstall_macos() -> Result<()> {
    let _ = stop_macos();
    let plist_path = mac_plist_path()?;
    if plist_path.exists() {
        std::fs::remove_file(&plist_path)
            .with_context(|| format!("Failed to remove {}", plist_path.display()))?;
    }
    println!("Uninstalled the login service");
    Ok(())
}

fn start_macos() -> Result<()> {
    let target = mac_target_label()?;
    let plist_path = mac_plist_path()?;
    if !plist_path.exists() {
        return Err(anyhow!("Service not installed. Run: meido gateway install"));
    }
    let gui_target = format!("gui/{}", current_uid()?);
    let plist_path_str = plist_path.to_string_lossy().to_string();
    let bootstrap = run_command("launchctl", &["bootstrap", &gui_target, &plist_path_str])?;
    if !bootstrap.status.success() {
        let stderr = String::from_utf8_lossy(&bootstrap.stderr);
        if !(stderr.contains("already loaded") || stderr.contains("already exists")) {
            return Err(anyhow!(
                "Command failed: launchctl bootstrap {} {}\nstderr: {}",
                gui_target,
                plist_path_str,
                stderr.trim()
            ));
        }
    }

    ensure_success(
        run_command("launchctl", &["kickstart", "-k", &target])?,
        "launchctl",
        &["kickstart", "-k", &target],
    )?;
    println!("Login service started");
    Ok(())
}

fn stop_macos() -> Result<()> {
    let target = mac_target_label()?;
    let output =
        match run_command_with_timeout("launchctl", &["bootout", &target], STOP_TIMEOUT_SECS) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{}", e);
                return Err(e);
            }
        };
    if output.status.success() {
        println!("Login service stopped");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such process")
        || stderr.contains("Could not find specified service")
        || stderr.contains("not found")
    {
        return Ok(());
    }

    Err(anyhow!("Failed to stop service: {}", stderr.trim()))
}

fn status_macos() -> Result<()> {
    let target = mac_target_label()?;
    let output = run_command("launchctl", &["print", &target])?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("Login service is not running"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        let input = "a&b<c>d\"e'f";
        let escaped = xml_escape(input);
        assert_eq!(escaped, "a&amp;b&lt;c&gt;d&quot;e&apos;f");
    }

    #[test]
    fn test_render_linux_unit_contains_start_and_restart() {
        let ctx = ServiceContext {
            exe_path: PathBuf::from("/usr/local/bin/meido"),
            working_dir: PathBuf::from("/tmp/meido"),
            config_path: Some(PathBuf::from("/tmp/meido/meido.config.yaml")),
            runtime_logs_dir: PathBuf::from("/tmp/meido/runtime/logs"),
            path_env: None,
            home_env: None,
        };

        let unit = render_linux_unit(&ctx);
        assert!(unit.contains("ExecStart=/usr/local/bin/meido start"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("MEIDO_GATEWAY=1"));
        assert!(unit.contains("MEIDO_CONFIG=/tmp/meido/meido.config.yaml"));
    }

    #[test]
    fn test_render_macos_plist_contains_required_fields() {
        let ctx = ServiceContext {
            exe_path: PathBuf::from("/usr/local/bin/meido"),
            working_dir: PathBuf::from("/tmp/meido"),
            config_path: Some(PathBuf::from("/tmp/meido/meido.config.yaml")),
            runtime_logs_dir: PathBuf::from("/tmp/meido/runtime/logs"),
            path_env: None,
            home_env: None,
        };

        let plist = render_macos_plist(&ctx);
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains(MAC_LABEL));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("MEIDO_GATEWAY"));
        assert!(plist.contains("MEIDO_CONFIG"));
    }

    #[test]
    fn test_parse_log_lines_default_and_custom() {
        assert_eq!(parse_log_lines(None).unwrap(), DEFAULT_LOG_LINES);
        assert_eq!(parse_log_lines(Some("20")).unwrap(), 20);
        assert!(parse_log_lines(Some("0")).is_err());
        assert!(parse_log_lines(Some("abc")).is_err());
    }

    #[test]
    fn test_resolve_runtime_logs_dir_fallback() {
        let dir = resolve_runtime_logs_dir(Path::new("/tmp/meido"));
        assert!(
            dir.ends_with("runtime/logs") || dir.ends_with("meido.data/runtime/logs"),
            "unexpected logs dir: {}",
            dir.display()
        );
    }
}
