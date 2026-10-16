//! Install the official MemPalace MCP into the runtime data directory.
//!
//! Fresh installs leave `MEMPALACE_LIGHT_MCP_COMMAND` empty. Startup and
//! `gateway install` create `runtime/mempalace-venv`, install the `mempalace`
//! package, and point recall at `python -m mempalace.mcp_server` with a palace
//! directory under runtime. An explicit command or endpoint is left unchanged.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tracing::warn;

use crate::config::Config;

const VENV_DIR_NAME: &str = "mempalace-venv";
const PALACE_DIR_NAME: &str = "mempalace";
const PIP_TIMEOUT: Duration = Duration::from_secs(600);
const VENV_TIMEOUT: Duration = Duration::from_secs(120);

pub fn apply_managed_mempalace(config: &mut Config) {
    if !config.mempalace.endpoint.trim().is_empty() || !config.mempalace.command.trim().is_empty() {
        return;
    }
    match ensure_managed_mempalace(config) {
        Ok(()) => {
            if let Some((command, args)) = resolved_stdio(config) {
                config.mempalace.command = command;
                config.mempalace.args = args;
            }
            if config.mempalace.startup_timeout_secs <= 20 {
                config.mempalace.startup_timeout_secs = 180;
            }
            if config.mempalace.query_timeout_secs <= 15 {
                config.mempalace.query_timeout_secs = 60;
            }
        }
        Err(err) => {
            eprintln!("Warning: MemPalace MCP was not installed: {err}");
            warn!("MemPalace MCP was not installed: {err}");
        }
    }
}

/// Command and args recall should use when `.env` does not set a command or endpoint.
pub fn resolved_stdio(config: &Config) -> Option<(String, Vec<String>)> {
    if !config.mempalace.endpoint.trim().is_empty() {
        return None;
    }
    if !config.mempalace.command.trim().is_empty() {
        return Some((
            config.mempalace.command.clone(),
            config.mempalace.args.clone(),
        ));
    }
    let python = venv_python(&venv_dir(config));
    if python.is_file() {
        return Some((
            python.to_string_lossy().to_string(),
            mcp_args(&palace_dir(config)),
        ));
    }
    None
}

pub fn ensure_managed_mempalace(config: &Config) -> Result<(), String> {
    let runtime = PathBuf::from(config.runtime_data_dir());
    std::fs::create_dir_all(&runtime)
        .map_err(|err| format!("failed to create runtime dir: {err}"))?;
    let palace = palace_dir(config);
    std::fs::create_dir_all(&palace)
        .map_err(|err| format!("failed to create MemPalace data dir: {err}"))?;

    let venv = venv_dir(config);
    let python = venv_python(&venv);
    if !python.is_file() {
        let base = base_python();
        tracing::info!("Creating MemPalace venv at {} using {base}", venv.display());
        run_quiet(
            &base,
            &["-m", "venv", &venv.to_string_lossy()],
            VENV_TIMEOUT,
        )?;
    }
    if !python.is_file() {
        return Err(format!(
            "MemPalace venv python not found at {}",
            python.display()
        ));
    }
    if !imports_mcp_server(&python)? {
        tracing::info!("Installing mempalace into {}", venv.display());
        let python_s = python.to_string_lossy().to_string();
        run_quiet(
            &python_s,
            &["-m", "pip", "install", "mempalace"],
            PIP_TIMEOUT,
        )?;
    }
    if !imports_mcp_server(&python)? {
        return Err(
            "mempalace installed but `import mempalace.mcp_server` failed. Recall stays on the existing search until this imports."
                .into(),
        );
    }
    tracing::info!(
        "MemPalace MCP ready: {} -m mempalace.mcp_server --palace {}",
        python.display(),
        palace.display()
    );
    Ok(())
}

fn venv_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.runtime_data_dir()).join(VENV_DIR_NAME)
}

fn palace_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.runtime_data_dir()).join(PALACE_DIR_NAME)
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

fn base_python() -> String {
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

fn mcp_args(palace: &Path) -> Vec<String> {
    vec![
        "-m".to_string(),
        "mempalace.mcp_server".to_string(),
        "--palace".to_string(),
        palace.to_string_lossy().to_string(),
    ]
}

fn imports_mcp_server(python: &Path) -> Result<bool, String> {
    let output = Command::new(python)
        .args(["-c", "import mempalace.mcp_server"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("failed to run {}: {err}", python.display()))?;
    Ok(output.status.success())
}

fn run_quiet(cmd: &str, args: &[&str], timeout: Duration) -> Result<(), String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn {cmd}: {err}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut err);
                }
                let tail = clip_tail(&err, 500);
                return Err(format!("{cmd} exited {status}: {tail}"));
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err(format!("{cmd} timed out after {}s", timeout.as_secs()));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(err) => return Err(format!("{cmd} wait failed: {err}")),
        }
    }
}

fn clip_tail(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - max_chars).collect::<String>()
}
