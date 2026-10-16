//! Auto-start the embedded Cursor SDK sidecar when the bot starts.
//!
//! Default runtime is Node (`scripts/cursor-sdk-runner.mjs` + `@cursor/sdk`).
//! Set `CURSOR_SDK_RUNNER_SCRIPT` to `scripts/cursor-sdk-runner.py` to roll back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::Config;
use crate::cursor_engine_config::{
    self, CursorEngineSettings, SidecarHealth, APP_SETTING_CURSOR_SDK_RUNNER_OK,
    APP_SETTING_CURSOR_SDK_RUNNER_URL,
};
use crate::db::Database;
use crate::error::FinallyAValueBotError;

const HEALTH_POLL_INTERVAL_MS: u64 = 250;
const HEALTH_WAIT_MAX_SECS: u64 = 45;
const SUPERVISOR_INTERVAL_SECS: u64 = 30;
/// Cancel individual /run sessions older than this; do not kill the sidecar process.
const STUCK_RUN_CANCEL_SECS: u64 = 1200;
const SUPERVISOR_HEALTH_TIMEOUT_MS: u64 = 2_500;
const SOFT_RECYCLE_WAIT_SECS: u64 = 120;
const SIDECAR_VENV_DIR_NAME: &str = "cursor-sdk-venv";
const SIDECAR_NODE_DIR_NAME: &str = "cursor-sdk-node";
const SIDECAR_PID_FILE: &str = "cursor-sdk-sidecar.pid";
const SIDECAR_STDERR_LOG: &str = "cursor-sdk-sidecar.stderr.log";
const SIDECAR_PYTHON_PACKAGES: &[&str] = &["cursor-sdk", "aiohttp"];
const SIDECAR_NODE_PACKAGE: &str = "@cursor/sdk";
const VENV_CREATE_TIMEOUT_SECS: u64 = 120;
const PIP_INSTALL_TIMEOUT_SECS: u64 = 180;
const NPM_INSTALL_TIMEOUT_SECS: u64 = 180;
const NODE_SCRIPT_NAMES: &[&str] = &["cursor-sdk-runner.mjs", "cursor-sdk-runner.py"];
const SDK_SHIM_SOURCE: &str = "export * from \"@cursor/sdk\";\n";
const NODE_PACKAGE_JSON: &str = r#"{
  "name": "cursor-sdk-sidecar",
  "private": true,
  "type": "module"
}
"#;

#[derive(Clone)]
struct SupervisorCtx {
    port: u16,
    runtime_data_dir: String,
    workspace_root: PathBuf,
    max_uptime_secs: u64,
    auto_install: bool,
    python_override: String,
    node_override: String,
}

/// Keeps the sidecar child process alive and auto-recycles when idle / wedged.
pub struct SidecarHandle {
    child: Mutex<Option<Child>>,
    recycle_lock: Mutex<()>,
    ctx: SupervisorCtx,
    pub managed_locally: bool,
    pub runner_url: String,
}

pub fn default_local_runner_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn find_sidecar_script_in(dir: &Path) -> Option<PathBuf> {
    let scripts = dir.join("scripts");
    for name in NODE_SCRIPT_NAMES {
        let path = scripts.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn resolve_sidecar_script() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("CURSOR_SDK_RUNNER_SCRIPT") {
        let path = PathBuf::from(raw.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(path) = find_sidecar_script_in(&cwd) {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().take(6) {
            if let Some(path) = find_sidecar_script_in(ancestor) {
                return Some(path);
            }
        }
    }
    None
}

fn is_python_script(script: &Path) -> bool {
    script
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
}

fn is_local_runner_url(url: &str, port: u16) -> bool {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed.is_empty()
        || trimmed == default_local_runner_url(port).trim_end_matches('/')
        || trimmed.starts_with("http://127.0.0.1:")
        || trimmed.starts_with("http://localhost:")
}

fn sidecar_venv_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.runtime_data_dir()).join(SIDECAR_VENV_DIR_NAME)
}

fn sidecar_node_prefix_dir_from(runtime_data_dir: &str) -> PathBuf {
    PathBuf::from(runtime_data_dir).join(SIDECAR_NODE_DIR_NAME)
}

fn sidecar_node_prefix_dir(config: &Config) -> PathBuf {
    sidecar_node_prefix_dir_from(&config.runtime_data_dir())
}

fn sidecar_pid_path(config: &Config) -> PathBuf {
    PathBuf::from(config.runtime_data_dir()).join(SIDECAR_PID_FILE)
}

fn sidecar_stderr_log_path(config: &Config) -> PathBuf {
    PathBuf::from(config.runtime_data_dir()).join(SIDECAR_STDERR_LOG)
}

fn venv_python_executable(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn base_python_executable(config: &Config) -> String {
    let python = config.cursor_sdk_python.trim();
    if python.is_empty() {
        "python3".to_string()
    } else {
        python.to_string()
    }
}

fn base_node_executable(config: &Config) -> String {
    let node = config.cursor_sdk_node.trim();
    if node.is_empty() {
        "node".to_string()
    } else {
        node.to_string()
    }
}

fn interpreter_from_ctx(ctx: &SupervisorCtx, python: bool) -> PathBuf {
    if python {
        PathBuf::from(if ctx.python_override.trim().is_empty() {
            "python3"
        } else {
            ctx.python_override.trim()
        })
    } else {
        PathBuf::from(if ctx.node_override.trim().is_empty() {
            "node"
        } else {
            ctx.node_override.trim()
        })
    }
}

async fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<(), String> {
    run_command_with_timeout_in(program, args, timeout_secs, None).await
}

async fn run_command_with_timeout_in(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cwd: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output())
        .await
        .map_err(|_| {
            format!(
                "timed out after {timeout_secs}s: {program} {}",
                args.join(" ")
            )
        })?
        .map_err(|e| format!("failed to run {program}: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "{program} {} failed (status={}): {}{}",
            args.join(" "),
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" | stdout: {}", stdout.trim())
            }
        ))
    }
}

async fn python_can_import(python: &Path, modules: &str) -> bool {
    let python_cmd = python.to_string_lossy().to_string();
    run_command_with_timeout(&python_cmd, &["-c", &format!("import {modules}")], 15)
        .await
        .is_ok()
}

async fn ensure_sidecar_venv(config: &Config) -> Result<PathBuf, String> {
    let venv_dir = sidecar_venv_dir(config);
    let python = venv_python_executable(&venv_dir);
    if python.is_file() && python_can_import(&python, "cursor_sdk, aiohttp").await {
        return Ok(python);
    }

    std::fs::create_dir_all(config.runtime_data_dir())
        .map_err(|e| format!("failed to create runtime dir for Cursor SDK venv: {e}"))?;

    let base_python = base_python_executable(config);
    if !python.is_file() {
        info!(
            "Creating Cursor SDK venv at {} using {base_python}",
            venv_dir.display()
        );
        let venv_path = venv_dir.to_string_lossy().to_string();
        run_command_with_timeout(
            &base_python,
            &["-m", "venv", &venv_path],
            VENV_CREATE_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| format!("Cursor SDK venv creation failed: {e}"))?;
    }

    if !python.is_file() {
        return Err(format!(
            "Cursor SDK venv python not found at {}",
            python.display()
        ));
    }

    info!(
        "Installing Cursor SDK sidecar packages ({}) into {}",
        SIDECAR_PYTHON_PACKAGES.join(", "),
        venv_dir.display()
    );
    let python_cmd = python.to_string_lossy().to_string();
    run_command_with_timeout(
        &python_cmd,
        &["-m", "pip", "install", "--upgrade", "pip"],
        PIP_INSTALL_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| format!("Cursor SDK pip bootstrap failed: {e}"))?;

    let mut pip_args: Vec<&str> = vec!["-m", "pip", "install"];
    pip_args.extend(SIDECAR_PYTHON_PACKAGES);
    run_command_with_timeout(&python_cmd, &pip_args, PIP_INSTALL_TIMEOUT_SECS)
        .await
        .map_err(|e| format!("Cursor SDK dependency install failed: {e}"))?;

    if !python_can_import(&python, "cursor_sdk, aiohttp").await {
        return Err(
            "Cursor SDK dependencies installed but import check failed (cursor_sdk, aiohttp)"
                .into(),
        );
    }

    info!(
        "Cursor SDK sidecar Python environment ready at {}",
        python.display()
    );
    Ok(python)
}

async fn resolve_sidecar_python(config: &Config) -> Result<PathBuf, String> {
    if config.cursor_sdk_auto_install {
        ensure_sidecar_venv(config).await
    } else {
        Ok(PathBuf::from(base_python_executable(config)))
    }
}

fn node_sdk_installed(prefix: &Path) -> bool {
    prefix.join("sdk-shim.mjs").is_file()
        && (prefix
            .join("node_modules")
            .join("@cursor")
            .join("sdk")
            .is_dir()
            || prefix
                .join("node_modules")
                .join("@cursor")
                .join("sdk")
                .is_file())
}

async fn ensure_sidecar_node_prefix(config: &Config) -> Result<PathBuf, String> {
    let prefix = sidecar_node_prefix_dir(config);
    std::fs::create_dir_all(&prefix)
        .map_err(|e| format!("failed to create Cursor SDK node prefix: {e}"))?;

    let package_json = prefix.join("package.json");
    if !package_json.is_file() {
        std::fs::write(&package_json, NODE_PACKAGE_JSON)
            .map_err(|e| format!("failed to write Cursor SDK package.json: {e}"))?;
    }
    std::fs::write(prefix.join("sdk-shim.mjs"), SDK_SHIM_SOURCE)
        .map_err(|e| format!("failed to write Cursor SDK shim: {e}"))?;

    if node_sdk_installed(&prefix) {
        return Ok(prefix);
    }

    let node = base_node_executable(config);
    info!(
        "Installing {SIDECAR_NODE_PACKAGE} into {} using npm (node={node})",
        prefix.display()
    );
    let prefix_str = prefix.to_string_lossy().to_string();
    run_command_with_timeout(
        "npm",
        &["install", "--prefix", &prefix_str, SIDECAR_NODE_PACKAGE],
        NPM_INSTALL_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| format!("Cursor SDK npm install failed: {e}"))?;

    if !node_sdk_installed(&prefix) {
        return Err(format!(
            "{SIDECAR_NODE_PACKAGE} installed but sdk-shim check failed under {}",
            prefix.display()
        ));
    }
    info!(
        "Cursor SDK sidecar Node environment ready at {}",
        prefix.display()
    );
    Ok(prefix)
}

async fn resolve_sidecar_node_prefix(config: &Config) -> Result<PathBuf, String> {
    if config.cursor_sdk_auto_install {
        ensure_sidecar_node_prefix(config).await
    } else {
        Ok(sidecar_node_prefix_dir(config))
    }
}

fn write_sidecar_pid(config: &Config, pid: u32) {
    let path = sidecar_pid_path(config);
    if let Err(e) = std::fs::write(&path, pid.to_string()) {
        warn!(
            "Failed to write Cursor SDK sidecar pid file {}: {e}",
            path.display()
        );
    }
}

fn read_sidecar_pid(config: &Config) -> Option<u32> {
    let raw = std::fs::read_to_string(sidecar_pid_path(config)).ok()?;
    raw.trim().parse().ok()
}

async fn terminate_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = run_command_with_timeout("kill", &["-TERM", &pid.to_string()], 5).await;
    }
    #[cfg(windows)]
    {
        let _ = run_command_with_timeout("taskkill", &["/PID", &pid.to_string(), "/F"], 10).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[cfg(unix)]
async fn terminate_listener_on_port(port: u16) {
    let script = format!(
        "fuser -k {port}/tcp 2>/dev/null || lsof -ti:{port} | xargs -r kill -TERM 2>/dev/null"
    );
    let _ = run_command_with_timeout("sh", &["-c", &script], 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[cfg(not(unix))]
async fn terminate_listener_on_port(_port: u16) {}

async fn stop_recorded_sidecar(config: &Config) {
    if let Some(pid) = read_sidecar_pid(config) {
        info!("Stopping recorded Cursor SDK sidecar process (pid={pid})");
        terminate_pid(pid).await;
    }
    let _ = std::fs::remove_file(sidecar_pid_path(config));
}

async fn wait_for_sidecar_health(url: &str) -> SidecarHealth {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(HEALTH_WAIT_MAX_SECS);
    loop {
        let health = cursor_engine_config::probe_sidecar_health(url).await;
        if health.reachable {
            return health;
        }
        if tokio::time::Instant::now() >= deadline {
            return health;
        }
        tokio::time::sleep(Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
    }
}

fn in_docker() -> bool {
    crate::meido_env::is_flag_one(crate::meido_env::IN_DOCKER) || Path::new("/.dockerenv").exists()
}

fn script_mtime_unix(script: &Path) -> Option<u64> {
    let meta = std::fs::metadata(script).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn health_suggests_stale_attach(
    health: &SidecarHealth,
    ctx: &SupervisorCtx,
    script: &Path,
) -> bool {
    if !health.reachable || !health.cursor_sdk_installed {
        return false;
    }
    if health.uptime_secs >= ctx.max_uptime_secs {
        return true;
    }
    if let (Some(mtime), true) = (script_mtime_unix(script), health.started_at_unix > 0) {
        if mtime > health.started_at_unix {
            return true;
        }
    }
    false
}

#[cfg(unix)]
async fn kill_sdk_bridge_processes(runtime_data_dir: &str) {
    let runtime = runtime_data_dir.trim();
    let script = if runtime.is_empty() {
        "pkill -f 'cursor-sdk-bridge.js' 2>/dev/null || true".to_string()
    } else {
        format!(
            "pgrep -af 'cursor-sdk-bridge.js' 2>/dev/null | while read -r line; do \
               case \"$line\" in *\"{runtime}\"*) \
                 pid=$(echo \"$line\" | awk '{{print $1}}'); \
                 kill -TERM \"$pid\" 2>/dev/null || true ;; \
               esac; \
             done"
        )
    };
    let _ = run_command_with_timeout("sh", &["-c", &script], 15).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[cfg(not(unix))]
async fn kill_sdk_bridge_processes(_runtime_data_dir: &str) {}

async fn request_soft_recycle(runner_url: &str) -> Result<bool, String> {
    let trimmed = runner_url.trim().trim_end_matches('/');
    let url = format!("{trimmed}/admin/request_recycle");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("recycle request failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let accepted = body
        .get("accepted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if status.as_u16() == 202 || !accepted {
        Ok(false)
    } else {
        Ok(true)
    }
}

/// Cancel sidecar `/run` sessions older than `max_age_secs`. Younger sessions stay.
/// Never force-kills the Node process.
async fn request_cancel_old_runs(runner_url: &str, max_age_secs: u64) -> Result<u64, String> {
    let trimmed = runner_url.trim().trim_end_matches('/');
    let url = format!("{trimmed}/admin/cancel_old_runs");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "max_age_secs": max_age_secs }))
        .send()
        .await
        .map_err(|e| format!("cancel_old_runs request failed: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err("cancel_old_runs_unsupported".into());
    }
    if !status.is_success() {
        return Err(format!("cancel_old_runs HTTP {status}"));
    }
    let body = resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let cancelled = body.get("cancelled").and_then(|v| v.as_u64()).unwrap_or(0);
    Ok(cancelled)
}

async fn wait_for_sidecar_gone(runner_url: &str, wait_secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
    while tokio::time::Instant::now() < deadline {
        let health = cursor_engine_config::probe_sidecar_health_with_timeout(
            runner_url,
            Duration::from_millis(SUPERVISOR_HEALTH_TIMEOUT_MS),
        )
        .await;
        if !health.reachable {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

impl SidecarHandle {
    pub fn inactive() -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            recycle_lock: Mutex::new(()),
            ctx: SupervisorCtx {
                port: 3848,
                runtime_data_dir: String::new(),
                workspace_root: PathBuf::from("."),
                max_uptime_secs: 86_400,
                auto_install: true,
                python_override: "python3".into(),
                node_override: "node".into(),
            },
            managed_locally: false,
            runner_url: default_local_runner_url(3848),
        })
    }

    async fn force_recycle_and_respawn(self: &Arc<Self>, reason: &str) {
        let _guard = self.recycle_lock.lock().await;
        info!("Cursor SDK sidecar force recycle ({reason})");
        {
            let mut slot = self.child.lock().await;
            if let Some(mut child) = slot.take() {
                let _ = child.start_kill();
            }
        }
        // Config-less stop via ctx paths
        let pid_path = PathBuf::from(&self.ctx.runtime_data_dir).join(SIDECAR_PID_FILE);
        if let Ok(raw) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = raw.trim().parse::<u32>() {
                terminate_pid(pid).await;
            }
            let _ = std::fs::remove_file(&pid_path);
        }
        terminate_listener_on_port(self.ctx.port).await;
        kill_sdk_bridge_processes(&self.ctx.runtime_data_dir).await;

        match self.spawn_fresh().await {
            Ok(()) => info!(
                "Cursor SDK sidecar respawned at {} after force recycle",
                self.runner_url
            ),
            Err(e) => warn!("Cursor SDK sidecar respawn failed after force recycle: {e}"),
        }
    }

    async fn cancel_old_runs(self: &Arc<Self>, max_age_secs: u64) {
        match request_cancel_old_runs(&self.runner_url, max_age_secs).await {
            Ok(cancelled) => info!(
                cancelled,
                max_age_secs, "Cursor sidecar cancelled old runs; younger sessions kept"
            ),
            Err(e) => warn!(
                error = %e,
                max_age_secs,
                "Cursor sidecar cancel-old-runs failed; not force-recycling (would kill younger sessions)"
            ),
        }
    }

    async fn soft_recycle_then_respawn(self: &Arc<Self>, reason: &str) {
        let _guard = self.recycle_lock.lock().await;
        info!("Cursor SDK sidecar soft recycle requested ({reason})");
        match request_soft_recycle(&self.runner_url).await {
            Ok(true) => {
                info!("Cursor SDK sidecar accepted soft recycle");
            }
            Ok(false) => {
                info!("Cursor SDK sidecar busy; deferring soft recycle ({reason})");
                return;
            }
            Err(e) => {
                warn!("Cursor SDK soft recycle request failed ({e}); will force if still up");
            }
        }
        let gone = wait_for_sidecar_gone(&self.runner_url, SOFT_RECYCLE_WAIT_SECS).await;
        if !gone {
            // Drop lock before nested force? We're holding recycle_lock — force also takes it.
            // Avoid deadlock: do force body inline without re-locking.
            warn!("Cursor SDK sidecar still up after soft wait; forcing ({reason})");
            {
                let mut slot = self.child.lock().await;
                if let Some(mut child) = slot.take() {
                    let _ = child.start_kill();
                }
            }
            let pid_path = PathBuf::from(&self.ctx.runtime_data_dir).join(SIDECAR_PID_FILE);
            if let Ok(raw) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = raw.trim().parse::<u32>() {
                    terminate_pid(pid).await;
                }
                let _ = std::fs::remove_file(&pid_path);
            }
            terminate_listener_on_port(self.ctx.port).await;
            kill_sdk_bridge_processes(&self.ctx.runtime_data_dir).await;
        } else {
            let mut slot = self.child.lock().await;
            *slot = None;
        }
        match self.spawn_fresh().await {
            Ok(()) => info!(
                "Cursor SDK sidecar respawned at {} after soft recycle",
                self.runner_url
            ),
            Err(e) => warn!("Cursor SDK sidecar respawn failed after soft recycle: {e}"),
        }
    }

    async fn spawn_fresh(self: &Arc<Self>) -> Result<(), String> {
        let script = resolve_sidecar_script().ok_or_else(|| {
            "cursor-sdk-runner.mjs not found (set CURSOR_SDK_RUNNER_SCRIPT or run from repo root)"
                .to_string()
        })?;
        let python = is_python_script(&script);
        let interpreter = if python && self.ctx.auto_install {
            let venv_dir = PathBuf::from(&self.ctx.runtime_data_dir).join(SIDECAR_VENV_DIR_NAME);
            let venv_py = venv_python_executable(&venv_dir);
            if venv_py.is_file() {
                venv_py
            } else {
                interpreter_from_ctx(&self.ctx, true)
            }
        } else {
            interpreter_from_ctx(&self.ctx, python)
        };
        let node_prefix = if python {
            None
        } else {
            Some(sidecar_node_prefix_dir_from(&self.ctx.runtime_data_dir))
        };
        let stderr_log = PathBuf::from(&self.ctx.runtime_data_dir).join(SIDECAR_STDERR_LOG);
        let proc = spawn_sidecar_process(
            &script,
            self.ctx.port,
            &interpreter,
            &stderr_log,
            &self.ctx.runtime_data_dir,
            &self.ctx.workspace_root,
            self.ctx.max_uptime_secs,
            node_prefix.as_deref(),
        )
        .await?;
        if let Some(pid) = proc.id() {
            let path = PathBuf::from(&self.ctx.runtime_data_dir).join(SIDECAR_PID_FILE);
            if let Err(e) = std::fs::write(&path, pid.to_string()) {
                warn!(
                    "Failed to write Cursor SDK sidecar pid file {}: {e}",
                    path.display()
                );
            }
        }
        {
            let mut slot = self.child.lock().await;
            *slot = Some(proc);
        }
        let health = wait_for_sidecar_health(&self.runner_url).await;
        if !health.reachable {
            return Err(format!(
                "sidecar did not become healthy at {}",
                self.runner_url
            ));
        }
        Ok(())
    }
}

async fn supervise_sidecar(handle: Arc<SidecarHandle>) {
    let mut wedged_streak: u8 = 0;
    let mut stuck_runs_streak: u8 = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(SUPERVISOR_INTERVAL_SECS)).await;
        let health = cursor_engine_config::probe_sidecar_health_with_timeout(
            &handle.runner_url,
            Duration::from_millis(SUPERVISOR_HEALTH_TIMEOUT_MS),
        )
        .await;

        if !health.reachable {
            wedged_streak = wedged_streak.saturating_add(1);
            stuck_runs_streak = 0;
            if wedged_streak >= 2 {
                wedged_streak = 0;
                handle.force_recycle_and_respawn("wedged_health").await;
            }
            continue;
        }
        wedged_streak = 0;

        if health.runs_in_flight > 0 && health.oldest_run_age_secs >= STUCK_RUN_CANCEL_SECS {
            stuck_runs_streak = stuck_runs_streak.saturating_add(1);
            if stuck_runs_streak >= 3 {
                stuck_runs_streak = 0;
                warn!(
                    runs_in_flight = health.runs_in_flight,
                    oldest_run_age_secs = health.oldest_run_age_secs,
                    max_age_secs = STUCK_RUN_CANCEL_SECS,
                    "Cursor sidecar stuck runs detected; cancelling old sessions only"
                );
                handle.cancel_old_runs(STUCK_RUN_CANCEL_SECS).await;
                continue;
            }
        } else {
            stuck_runs_streak = 0;
        }

        let mut soft_reason: Option<&'static str> = None;
        if health.uptime_secs >= handle.ctx.max_uptime_secs && health.runs_in_flight == 0 {
            soft_reason = Some("max_uptime");
        } else if let Some(script) = resolve_sidecar_script() {
            if let Some(mtime) = script_mtime_unix(&script) {
                if health.started_at_unix > 0
                    && mtime > health.started_at_unix
                    && health.runs_in_flight == 0
                {
                    soft_reason = Some("script_updated");
                }
            }
        }

        if let Some(reason) = soft_reason {
            handle.soft_recycle_then_respawn(reason).await;
        }
    }
}

async fn spawn_sidecar_process(
    script: &Path,
    port: u16,
    interpreter: &Path,
    stderr_log: &Path,
    runtime_data_dir: &str,
    workspace_root: &Path,
    max_uptime_secs: u64,
    node_prefix: Option<&Path>,
) -> Result<Child, String> {
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log)
        .map_err(|e| {
            format!(
                "failed to open Cursor SDK sidecar stderr log {}: {e}",
                stderr_log.display()
            )
        })?;

    let mut cmd = Command::new(interpreter);
    cmd.arg(script)
        .arg(port.to_string())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_file);

    if !runtime_data_dir.trim().is_empty() {
        cmd.env(
            crate::meido_env::RUNTIME_DATA.preferred,
            runtime_data_dir.trim(),
        );
    }

    let cursor_key = std::env::var("CURSOR_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .or_else(|| crate::engine_connections::saved_secret(runtime_data_dir, "CURSOR_API_KEY"));
    if let Some(key) = cursor_key {
        cmd.env("CURSOR_API_KEY", key);
    }
    cmd.env(
        "CURSOR_SIDECAR_MAX_UPTIME_SECS",
        max_uptime_secs.max(300).to_string(),
    );
    if let Some(prefix) = node_prefix {
        cmd.env("CURSOR_SDK_NODE_PREFIX", prefix.as_os_str());
    }

    // Inherited by Cursor SDK local shell tools so git does not bind the bot checkout
    // when cwd is under WORKSPACE_DIR. Tier-1 target repos remain usable via explicit cd.
    crate::self_repo::apply_git_ceiling_env(&mut cmd, workspace_root);

    cmd.spawn().map_err(|e| {
        format!(
            "failed to spawn Cursor SDK sidecar ({}): {e}",
            interpreter.display()
        )
    })
}

fn persist_bootstrap(
    db: &Database,
    runner_url: &str,
    runner_ok: bool,
) -> Result<(), FinallyAValueBotError> {
    db.set_app_setting(APP_SETTING_CURSOR_SDK_RUNNER_URL, runner_url.trim())?;
    db.set_app_setting(
        APP_SETTING_CURSOR_SDK_RUNNER_OK,
        if runner_ok { "true" } else { "false" },
    )?;
    Ok(())
}

fn sidecar_needs_restart(health: &SidecarHealth) -> bool {
    !health.reachable || !health.cursor_sdk_installed
}

/// Start (or attach to) the local sidecar, wait for health, and update `cursor_settings`.
pub async fn bootstrap(
    config: &Config,
    db: &Database,
    cursor_settings: Arc<std::sync::RwLock<CursorEngineSettings>>,
) -> Arc<SidecarHandle> {
    let port = config.cursor_sdk_runner_port;
    let mut settings = cursor_settings
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| CursorEngineSettings::from_env(config));

    if settings.sdk_runner_url.trim().is_empty() {
        settings.sdk_runner_url = default_local_runner_url(port);
    }

    let runner_url = settings.sdk_runner_url.clone();
    let ctx = SupervisorCtx {
        port,
        runtime_data_dir: config.runtime_data_dir(),
        workspace_root: config.workspace_root_absolute(),
        max_uptime_secs: config.cursor_sidecar_max_uptime_secs.max(300),
        auto_install: config.cursor_sdk_auto_install,
        python_override: config.cursor_sdk_python.clone(),
        node_override: config.cursor_sdk_node.clone(),
    };
    let mut managed_locally = false;
    let mut child: Option<Child> = None;
    let supervise =
        config.cursor_sdk_auto_start && !in_docker() && is_local_runner_url(&runner_url, port);

    if supervise {
        let script = resolve_sidecar_script();
        let python_script = script.as_ref().is_some_and(|p| is_python_script(p));
        let interpreter = if python_script {
            match resolve_sidecar_python(config).await {
                Ok(python) => python,
                Err(e) => {
                    warn!("Cursor SDK sidecar Python setup failed: {e}");
                    PathBuf::from(base_python_executable(config))
                }
            }
        } else {
            if let Err(e) = resolve_sidecar_node_prefix(config).await {
                warn!("Cursor SDK sidecar Node setup failed: {e}");
            }
            PathBuf::from(base_node_executable(config))
        };
        let node_prefix = if python_script {
            None
        } else {
            Some(sidecar_node_prefix_dir(config))
        };

        let mut health = cursor_engine_config::probe_sidecar_health(&runner_url).await;
        if let Some(ref script) = script {
            if health_suggests_stale_attach(&health, &ctx, script) {
                info!(
                    "Cursor SDK sidecar at {runner_url} is stale (uptime={}s); recycling before attach",
                    health.uptime_secs
                );
                let _ = request_soft_recycle(&runner_url).await;
                let _ = wait_for_sidecar_gone(&runner_url, 30).await;
                stop_recorded_sidecar(config).await;
                terminate_listener_on_port(port).await;
                kill_sdk_bridge_processes(&ctx.runtime_data_dir).await;
                health = cursor_engine_config::probe_sidecar_health(&runner_url).await;
            }
        }

        if sidecar_needs_restart(&health) {
            if health.reachable && !health.cursor_sdk_installed {
                info!(
                    "Cursor SDK sidecar at {runner_url} is missing @cursor/sdk; restarting with managed runtime"
                );
                stop_recorded_sidecar(config).await;
                terminate_listener_on_port(port).await;
                health = cursor_engine_config::probe_sidecar_health(&runner_url).await;
            }

            if sidecar_needs_restart(&health) {
                if let Some(ref script) = script {
                    let stderr_log = sidecar_stderr_log_path(config);
                    match spawn_sidecar_process(
                        script,
                        port,
                        &interpreter,
                        &stderr_log,
                        &config.runtime_data_dir(),
                        &config.workspace_root_absolute(),
                        ctx.max_uptime_secs,
                        node_prefix.as_deref(),
                    )
                    .await
                    {
                        Ok(proc) => {
                            if let Some(pid) = proc.id() {
                                write_sidecar_pid(config, pid);
                            }
                            info!(
                                "Started Cursor SDK sidecar on {} (script={}, interpreter={})",
                                runner_url,
                                script.display(),
                                interpreter.display()
                            );
                            child = Some(proc);
                            managed_locally = true;
                        }
                        Err(e) => {
                            warn!("Cursor SDK sidecar auto-start failed: {e}");
                        }
                    }
                } else {
                    warn!(
                        "Cursor SDK sidecar script not found (expected scripts/cursor-sdk-runner.mjs); \
                         set CURSOR_SDK_RUNNER_SCRIPT or run from repo root"
                    );
                }
            } else {
                info!("Cursor SDK sidecar already reachable at {runner_url}");
            }
        } else {
            info!("Cursor SDK sidecar already reachable at {runner_url}");
        }
    }

    let health = wait_for_sidecar_health(&runner_url).await;
    let runner_ok = health.reachable && health.api_key_configured && health.cursor_sdk_installed;
    settings.sdk_runner_url = runner_url.clone();
    settings.sdk_runner_ok = runner_ok;

    if let Err(e) = persist_bootstrap(db, &runner_url, runner_ok) {
        warn!("Failed to persist Cursor SDK bootstrap settings: {e}");
    }

    if let Ok(mut guard) = cursor_settings.write() {
        *guard = settings;
    }

    if runner_ok {
        info!("Cursor SDK sidecar ready at {runner_url}");
    } else if health.reachable && !health.cursor_sdk_installed {
        warn!(
            "Cursor SDK sidecar is up at {runner_url} but @cursor/sdk is not installed; \
             restart the bot to auto-install (CURSOR_SDK_AUTO_INSTALL=true)"
        );
    } else if health.reachable {
        warn!(
            "Cursor SDK sidecar is up at {runner_url} but CURSOR_API_KEY is not set; \
             add it to repo-root .env for Cursor engine runs"
        );
    } else if config.cursor_sdk_auto_start {
        warn!(
            "Cursor SDK sidecar not ready at {runner_url}. \
             Ensure node and npm are on PATH or set CURSOR_SDK_NODE."
        );
    }

    let handle = Arc::new(SidecarHandle {
        child: Mutex::new(child),
        recycle_lock: Mutex::new(()),
        ctx,
        managed_locally,
        runner_url,
    });
    if supervise {
        tokio::spawn(supervise_sidecar(handle.clone()));
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_runner_url_detection() {
        assert!(is_local_runner_url("http://127.0.0.1:3848", 3848));
        assert!(is_local_runner_url("", 3848));
        assert!(!is_local_runner_url("http://192.168.1.5:3848", 3848));
    }

    #[test]
    fn default_url_uses_port() {
        assert_eq!(default_local_runner_url(3848), "http://127.0.0.1:3848");
    }

    #[test]
    fn venv_python_path_unix_layout() {
        if cfg!(windows) {
            return;
        }
        let venv = PathBuf::from("/tmp/cursor-sdk-venv");
        assert_eq!(
            venv_python_executable(&venv),
            PathBuf::from("/tmp/cursor-sdk-venv/bin/python")
        );
    }

    #[test]
    fn is_python_script_by_extension() {
        assert!(is_python_script(Path::new("scripts/cursor-sdk-runner.py")));
        assert!(!is_python_script(Path::new(
            "scripts/cursor-sdk-runner.mjs"
        )));
    }

    #[test]
    fn node_prefix_dir_under_runtime() {
        assert_eq!(
            sidecar_node_prefix_dir_from("/tmp/runtime"),
            PathBuf::from("/tmp/runtime/cursor-sdk-node")
        );
    }

    #[test]
    fn sidecar_needs_restart_when_sdk_missing() {
        let health = SidecarHealth {
            reachable: true,
            api_key_configured: true,
            cursor_sdk_installed: false,
            error: None,
            ..Default::default()
        };
        assert!(sidecar_needs_restart(&health));
    }
}
