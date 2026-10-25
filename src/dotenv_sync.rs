//! Two-way sync between the configuration `.env` and the web UI.
//!
//! The file is the operator config. The UI reads it on each settings request and
//! writes allowlisted keys back without dropping comments. A hand edit is picked
//! up when the file's modification time changes. Keys that are bound at process
//! start (workspace, web bind, vault roots) stay on the running process until restart.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::config::Config;

const DEFAULT_ENV: &str = "\
WORKSPACE_DIR=./workspace
TIMEZONE=UTC
WEB_ENABLED=true
WEB_HOST=127.0.0.1
WEB_PORT=10961
";

const SECRET_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "CURSOR_API_KEY",
    "PERPLEXITY_API_KEY",
    "TAVILY_API_KEY",
    "WEB_AUTH_TOKEN",
    "STEEL_API_KEY",
];

const PLAIN_KEYS: &[&str] = &[
    "WORKSPACE_DIR",
    "TIMEZONE",
    "AGENT_DISPLAY_NAME",
    "WEB_HOST",
    "WEB_PORT",
    "WEB_ENABLED",
    "VAULT_ORIGIN_VAULT_PATH",
    "VAULT_VECTOR_DB_PATH",
    "VAULT_VECTOR_DB_COLLECTION",
    "VAULT_PRINCIPLES_PATH",
    "VAULT_EMBEDDING_SERVER_URL",
    "VAULT_ORIGIN_VAULT_REPO",
    "SYSTEM_LLM_PROVIDER",
    "SYSTEM_LLM_MODEL",
    "HOOK_PROMPT_MODEL",
    "MEMPALACE_RECALL_MODE",
    "BROWSER_MANAGED",
    "STEEL_API_PORT",
    "STEEL_CDP_PORT",
    "STEEL_API_URL",
    "STEEL_DOCKER_IMAGE",
    "STEEL_DOCKER_CONTAINER_NAME",
    "SEARXNG_URL",
    "AGENT_ENGINE",
    "TOOL_OUTPUT_DEBUG",
    "POST_TOOL_EVALUATOR_ENABLED",
    "RESPONSE_QUALITY_EVALUATOR_ENABLED",
    "MAX_TOOL_ITERATIONS",
];

const PATH_KEYS: &[&str] = &[
    "VAULT_ORIGIN_VAULT_PATH",
    "VAULT_VECTOR_DB_PATH",
    "VAULT_PRINCIPLES_PATH",
];

#[derive(Debug, Clone)]
struct SeenFile {
    path: PathBuf,
    mtime: Option<SystemTime>,
    keys: HashSet<String>,
}

fn file_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn seen_file() -> &'static Mutex<Option<SeenFile>> {
    static SEEN: OnceLock<Mutex<Option<SeenFile>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(None))
}

pub fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.iter().any(|name| *name == key)
}

fn is_persona_external_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("MEIDO_PERSONA_") else {
        return false;
    };
    let Some(id) = rest.strip_suffix("_EXTERNAL_PATHS") else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

pub fn is_allowlisted(key: &str) -> bool {
    PLAIN_KEYS.iter().any(|name| *name == key)
        || SECRET_KEYS.iter().any(|name| *name == key)
        || is_persona_external_key(key)
}

pub fn config_path() -> Result<PathBuf, String> {
    match Config::resolve_config_path() {
        Ok(Some(path)) => Ok(path),
        Ok(None) => {
            let path = PathBuf::from("./.env");
            seed_env_file(&path)?;
            Ok(path)
        }
        Err(err) => Err(err.to_string()),
    }
}

pub fn seed_env_file(dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
    }
    let example = Path::new("./.env.example");
    if example.is_file() {
        fs::copy(example, dest).map_err(|e| format!("Could not copy .env.example: {e}"))?;
    } else {
        fs::write(dest, DEFAULT_ENV)
            .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn newline_of(content: &str) -> &str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim().strip_prefix("export ").unwrap_or(line.trim());
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn commented_assignment(line: &str, name: &str) -> bool {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('#') else {
        return false;
    };
    let rest = rest.trim();
    let rest = rest.strip_prefix("export ").unwrap_or(rest).trim();
    rest.strip_prefix(name)
        .is_some_and(|tail| tail.starts_with('='))
}

fn quote_env_value(value: &str) -> String {
    if value
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '#' | '\\' | '$' | '`'))
    {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

pub fn upsert_env_line(content: &str, name: &str, value: &str) -> String {
    let newline = newline_of(content);
    let line = format!("{name}={}", quote_env_value(value));
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in content.lines() {
        if assignment_key(raw) == Some(name) {
            if !replaced {
                out.push(line.clone());
                replaced = true;
            }
            continue;
        }
        if !replaced && commented_assignment(raw, name) {
            out.push(line.clone());
            replaced = true;
            continue;
        }
        out.push(raw.to_string());
    }
    if !replaced {
        if content.ends_with('\n') || content.is_empty() {
            out.push(line);
        } else {
            out.push(line);
        }
    }
    let mut joined = out.join(newline);
    if !joined.ends_with(newline) {
        joined.push_str(newline);
    }
    joined
}

pub fn remove_env_line(content: &str, name: &str) -> String {
    let newline = newline_of(content);
    let out: Vec<&str> = content
        .lines()
        .filter(|raw| assignment_key(raw) != Some(name))
        .collect();
    if out.is_empty() {
        return String::new();
    }
    let mut joined = out.join(newline);
    if content.ends_with('\n') || content.ends_with("\r\n") {
        joined.push_str(newline);
    }
    joined
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return trimmed[1..trimmed.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\\\", "\\");
        }
    }
    trimmed.to_string()
}

pub fn parse_assignments(content: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in content.lines() {
        let Some(key) = assignment_key(raw) else {
            continue;
        };
        if !is_allowlisted(key) {
            continue;
        }
        let trimmed = raw.trim().strip_prefix("export ").unwrap_or(raw.trim());
        let Some((_, value)) = trimmed.split_once('=') else {
            continue;
        };
        out.insert(key.to_string(), unquote(value));
    }
    out
}

fn atomic_replace(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.dotenv.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("env")
    ));
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("Could not write .env: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Could not write .env: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Could not write .env: {e}"))?;
    }
    if let Some(permissions) = permissions {
        let _ = fs::set_permissions(&tmp, permissions);
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path).map_err(|e| format!("Could not replace .env: {e}"))?;
    }
    Ok(())
}

/// Apply `edit` to the configuration file under the process-wide `.env` lock.
pub fn mutate_file(
    path: &Path,
    edit: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    let _guard = file_lock().lock().unwrap_or_else(|err| err.into_inner());
    if !path.is_file() {
        seed_env_file(path)?;
    }
    let contents =
        fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let next = edit(&contents)?;
    atomic_replace(path, &next)?;
    if let Ok(meta) = fs::metadata(path) {
        let keys: HashSet<String> = parse_assignments(&next).into_keys().collect();
        let mut seen = seen_file().lock().unwrap_or_else(|err| err.into_inner());
        *seen = Some(SeenFile {
            path: path.to_path_buf(),
            mtime: meta.modified().ok(),
            keys,
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct EnvUpdate {
    pub key: String,
    pub value: Option<String>,
}

pub fn apply_updates(path: &Path, updates: &[EnvUpdate]) -> Result<(), String> {
    for update in updates {
        if !is_allowlisted(&update.key) {
            return Err(format!("Refusing to write {}", update.key));
        }
        if let Some(value) = &update.value {
            validate_value(&update.key, value)?;
        }
    }
    mutate_file(path, |contents| {
        let mut next = contents.to_string();
        for update in updates {
            next = match &update.value {
                Some(value) => upsert_env_line(&next, &update.key, value),
                None => remove_env_line(&next, &update.key),
            };
        }
        Ok(next)
    })
}

fn validate_value(key: &str, value: &str) -> Result<(), String> {
    if value.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err(format!("{key} must be a single line"));
    }
    if value.chars().count() > 8192 {
        return Err(format!("{key} is too long"));
    }
    match key {
        "WEB_PORT" | "STEEL_API_PORT" | "STEEL_CDP_PORT" => {
            let port: u16 = value
                .parse()
                .map_err(|_| format!("{key} must be a port from 1 to 65535"))?;
            if port == 0 {
                return Err(format!("{key} must be a port from 1 to 65535"));
            }
        }
        "WEB_HOST" => {
            if !web_host_ok(value) {
                return Err("WEB_HOST must be 127.0.0.1, 0.0.0.0, localhost, or a DNS name".into());
            }
        }
        "VAULT_PRINCIPLES_PATH"
            if !Path::new(value).is_absolute() && (value.contains("..") || value.contains(':')) =>
        {
            return Err(
                "VAULT_PRINCIPLES_PATH must stay inside the workspace or be an absolute path"
                    .into(),
            );
        }
        _ => {}
    }
    Ok(())
}

fn web_host_ok(host: &str) -> bool {
    matches!(
        host,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "0.0.0.0" | "::" | "[::]"
    ) || host.parse::<std::net::Ipv4Addr>().is_ok()
        || host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
            && host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
}

pub fn is_local_web_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Store a path under the workspace as a workspace-relative path. Absolute paths outside stay absolute.
pub fn store_path(workspace: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let ws = workspace.trim();
    let ws_abs = normalize_lexical(ws);
    let value_abs = if path_text_is_absolute(trimmed) {
        normalize_lexical(trimmed)
    } else {
        normalize_lexical(&format!(
            "{}/{}",
            ws_abs.trim_end_matches('/'),
            trimmed.trim_start_matches("./")
        ))
    };
    let prefix = format!("{}/", ws_abs.trim_end_matches('/'));
    if value_abs.starts_with(&prefix) {
        value_abs[prefix.len()..].replace('\\', "/")
    } else if path_text_is_absolute(trimmed) {
        trimmed.replace('\\', "/")
    } else {
        trimmed.trim_start_matches("./").replace('\\', "/")
    }
}

/// `Path::is_absolute` is false for `/tmp/...` on Windows. A leading slash is still absolute.
fn path_text_is_absolute(value: &str) -> bool {
    Path::new(value).is_absolute() || value.starts_with('/') || value.starts_with('\\')
}

fn normalize_lexical(input: &str) -> String {
    let absolute = Path::new(input).is_absolute() || input.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in input.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if !absolute {
        return parts.join("/");
    }
    if parts.is_empty() {
        return "/".into();
    }
    format!("/{}", parts.join("/"))
}

#[derive(Debug, Clone)]
pub struct DiskSync {
    pub changed: bool,
    pub values: HashMap<String, String>,
    pub removed: Vec<String>,
}

/// Re-read `.env` when it changed on disk and publish allowlisted keys into the process environment.
pub fn refresh_process_env(path: &Path) -> Result<DiskSync, String> {
    let _guard = file_lock().lock().unwrap_or_else(|err| err.into_inner());
    if !path.is_file() {
        return Ok(DiskSync {
            changed: false,
            values: HashMap::new(),
            removed: Vec::new(),
        });
    }
    let meta = fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mtime = meta.modified().ok();
    let contents =
        fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let values = parse_assignments(&contents);
    let mut seen = seen_file().lock().unwrap_or_else(|err| err.into_inner());
    let previous = seen.clone();
    let changed = !matches!(&previous, Some(prev) if prev.path == path && prev.mtime == mtime);
    let mut removed = Vec::new();
    if changed {
        if let Some(prev) = &previous {
            if prev.path == path {
                for key in &prev.keys {
                    if !values.contains_key(key) {
                        std::env::remove_var(key);
                        removed.push(key.clone());
                    }
                }
            }
        }
        for (key, value) in &values {
            std::env::set_var(key, value);
        }
        *seen = Some(SeenFile {
            path: path.to_path_buf(),
            mtime,
            keys: values.keys().cloned().collect(),
        });
    }
    Ok(DiskSync {
        changed,
        values,
        removed,
    })
}

pub fn mark_seen(path: &Path, values: &HashMap<String, String>) {
    let mtime = fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let mut seen = seen_file().lock().unwrap_or_else(|err| err.into_inner());
    *seen = Some(SeenFile {
        path: path.to_path_buf(),
        mtime,
        keys: values.keys().cloned().collect(),
    });
}

fn file_value<'a>(values: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|v| !v.trim().is_empty())
}

fn same(current: &str, file: Option<&str>) -> bool {
    match file {
        None => true,
        Some(value) => current.trim() == value.trim(),
    }
}

pub fn restart_required(config: &Config, values: &HashMap<String, String>) -> bool {
    if !same(&config.workspace_dir, file_value(values, "WORKSPACE_DIR")) {
        return true;
    }
    if !same(&config.timezone, file_value(values, "TIMEZONE")) {
        return true;
    }
    if !same(&config.web_host, file_value(values, "WEB_HOST")) {
        return true;
    }
    if let Some(port) = file_value(values, "WEB_PORT") {
        if port.parse::<u16>().ok() != Some(config.web_port) {
            return true;
        }
    }
    let file_token = file_value(values, "WEB_AUTH_TOKEN").unwrap_or("");
    let running_token = config.web_auth_token.as_deref().unwrap_or("");
    if values.contains_key("WEB_AUTH_TOKEN") && file_token != running_token {
        return true;
    }
    let vault = config.vault.clone().unwrap_or_default();
    for (key, current) in [
        (
            "VAULT_ORIGIN_VAULT_PATH",
            vault.origin_vault_path.as_deref().unwrap_or(""),
        ),
        (
            "VAULT_VECTOR_DB_PATH",
            vault.vector_db_path.as_deref().unwrap_or(""),
        ),
        (
            "VAULT_PRINCIPLES_PATH",
            vault.principles_path.as_deref().unwrap_or(""),
        ),
    ] {
        if values.contains_key(key) {
            let next = values.get(key).map(String::as_str).unwrap_or("");
            if current.trim() != next.trim() {
                return true;
            }
        }
    }
    if let Some(raw) = file_value(values, "BROWSER_MANAGED") {
        let enabled = matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        if enabled != config.browser_managed {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InitializerStatus {
    pub needs_initialization: bool,
    pub workspace_configured: bool,
    pub web_access_configured: bool,
    pub vault_configured: bool,
    pub system_llm_configured: bool,
    pub llm_configured: bool,
    pub cursor_configured: bool,
    pub mempalace_ready: bool,
    pub steel_configured: bool,
}

pub struct ReadinessExtras {
    pub system_llm_saved: bool,
    pub provider_key_saved: bool,
    pub local_model_ready: bool,
    pub mempalace_ready: bool,
    pub steel_installed: bool,
}

pub fn initializer_status(
    values: &HashMap<String, String>,
    extras: &ReadinessExtras,
) -> InitializerStatus {
    let workspace_configured =
        file_value(values, "WORKSPACE_DIR").is_some() && file_value(values, "TIMEZONE").is_some();
    let host = file_value(values, "WEB_HOST").unwrap_or("127.0.0.1");
    let web_access_configured =
        is_local_web_host(host) || file_value(values, "WEB_AUTH_TOKEN").is_some();
    let vault_configured = file_value(values, "VAULT_ORIGIN_VAULT_PATH").is_some()
        && file_value(values, "VAULT_VECTOR_DB_PATH").is_some();
    let system_llm_configured = extras.system_llm_saved
        || (file_value(values, "SYSTEM_LLM_PROVIDER").is_some()
            && file_value(values, "SYSTEM_LLM_MODEL").is_some());
    let llm_configured = extras.provider_key_saved
        || extras.local_model_ready
        || SECRET_KEYS.iter().any(|key| {
            *key != "WEB_AUTH_TOKEN"
                && *key != "TAVILY_API_KEY"
                && *key != "STEEL_API_KEY"
                && file_value(values, key).is_some()
        });
    let cursor_configured = file_value(values, "CURSOR_API_KEY").is_some();
    let steel_configured = extras.steel_installed;
    let needs_initialization = !(workspace_configured
        && vault_configured
        && system_llm_configured
        && llm_configured
        && extras.mempalace_ready
        && extras.steel_installed);
    InitializerStatus {
        needs_initialization,
        workspace_configured,
        web_access_configured,
        vault_configured,
        system_llm_configured,
        llm_configured,
        cursor_configured,
        mempalace_ready: extras.mempalace_ready,
        steel_configured,
    }
}

pub fn command_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn lan_ipv4() -> Option<String> {
    let output = Command::new("ip")
        .args(["-4", "route", "get", "1.1.1.1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "src" {
            return parts.next().map(str::to_string);
        }
    }
    None
}

const DEFAULT_AGENTS: &str = include_str!("../defaults/AGENTS.md");

pub fn seed_principles(workspace: &str, principles: Option<&str>) {
    let rel = principles
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("AGENTS.md");
    let path = Path::new(rel);
    if !path.is_absolute() && (rel.contains(':') || rel.split(['/', '\\']).any(|p| p == "..")) {
        return;
    }
    let dest = crate::config::resolve_workspace_data_path(Path::new(workspace), rel);
    if dest.as_os_str().is_empty() || dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(dest, DEFAULT_AGENTS);
}

pub fn ignore_workspace_paths(workspace: &str, origin: Option<&str>, db: Option<&str>) {
    add_gitignore_dir(workspace);
    if let Some(origin) = origin.filter(|p| !p.trim().is_empty()) {
        add_gitignore_dir(&join_workspace(workspace, origin));
    }
    if let Some(db) = db.filter(|p| !p.trim().is_empty()) {
        add_gitignore_dir(&join_workspace(workspace, db));
    }
}

fn join_workspace(workspace: &str, value: &str) -> String {
    if Path::new(value).is_absolute() {
        value.to_string()
    } else {
        format!(
            "{}/{}",
            workspace.trim_end_matches(['/', '\\']),
            value.trim_start_matches("./")
        )
    }
}

fn add_gitignore_dir(raw: &str) {
    if raw.trim().is_empty() || raw.contains(['#', '!', '*', '?', '[']) {
        return;
    }
    let Ok(root) = std::env::current_dir() else {
        return;
    };
    if !root.join(".git").exists() && !root.join(".gitignore").exists() {
        return;
    }
    let target = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let root_s = root.to_string_lossy().trim_end_matches('/').to_string();
    let target_s = target.to_string_lossy().to_string();
    let Some(rel) = target_s
        .strip_prefix(&root_s)
        .map(|s| s.trim_start_matches('/'))
    else {
        return;
    };
    if rel.is_empty() || rel == ".git" || rel.starts_with(".git/") {
        return;
    }
    let pattern = format!("{rel}/");
    let file = root.join(".gitignore");
    let existing = fs::read_to_string(&file).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == pattern || line.trim() == pattern.trim_end_matches('/'))
    {
        return;
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.contains("# Meido workspace and vault (installer)") {
        next.push_str("# Meido workspace and vault (installer)\n");
    }
    next.push_str(&pattern);
    next.push('\n');
    let _ = fs::write(file, next);
}

pub fn normalize_updates(
    workspace: &str,
    updates: &mut [EnvUpdate],
    auth_token_already_set: bool,
) -> Result<(), String> {
    let mut host = workspace.to_string();
    let mut token_present = auth_token_already_set;
    for update in updates.iter() {
        if update.key == "WORKSPACE_DIR" {
            if let Some(value) = &update.value {
                host = value.clone();
            }
        }
        if update.key == "WEB_AUTH_TOKEN" {
            token_present = update.value.as_ref().is_some_and(|v| !v.trim().is_empty());
        }
    }
    for update in updates.iter_mut() {
        if PATH_KEYS.contains(&update.key.as_str()) {
            if let Some(value) = update.value.clone() {
                update.value = Some(store_path(&host, &value));
            }
        }
    }
    let sets_public_host = updates.iter().any(|update| {
        update.key == "WEB_HOST"
            && update
                .value
                .as_deref()
                .is_some_and(|host| !is_local_web_host(host))
    });
    if sets_public_host && !token_present {
        return Err("A web UI address beyond this machine requires WEB_AUTH_TOKEN".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_active_line_and_uncomments_template() {
        let commented = "# GEMINI_API_KEY=\nOTHER=keep\n";
        let updated = upsert_env_line(commented, "GEMINI_API_KEY", "secret-value");
        assert!(updated.contains("GEMINI_API_KEY=secret-value"));
        assert!(!updated.contains("# GEMINI_API_KEY="));
        assert!(updated.contains("OTHER=keep"));
        let again = upsert_env_line(&updated, "GEMINI_API_KEY", "next");
        assert_eq!(again.matches("GEMINI_API_KEY=").count(), 1);
        assert!(again.contains("OTHER=keep"));
    }

    #[test]
    fn remove_leaves_other_keys() {
        let file = "A=1\nWEB_PORT=10961\nB=2\n";
        let cleared = remove_env_line(file, "WEB_PORT");
        assert!(!cleared.contains("WEB_PORT"));
        assert!(cleared.contains("A=1"));
        assert!(cleared.contains("B=2"));
    }

    #[test]
    fn store_path_keeps_absolute_outside_workspace() {
        let stored = store_path("/tmp/workspace", "/tmp/workspace/shared/ORIGIN");
        assert_eq!(stored, "shared/ORIGIN");
        let outside = store_path("/tmp/workspace", "/var/notes");
        assert_eq!(outside, "/var/notes");
    }

    #[test]
    fn initializer_opens_until_core_items_are_set() {
        let mut values = HashMap::new();
        values.insert("WORKSPACE_DIR".into(), "./workspace".into());
        values.insert("TIMEZONE".into(), "UTC".into());
        let mut extras = ReadinessExtras {
            system_llm_saved: false,
            provider_key_saved: false,
            local_model_ready: false,
            mempalace_ready: false,
            steel_installed: false,
        };
        assert!(initializer_status(&values, &extras).needs_initialization);
        values.insert("VAULT_ORIGIN_VAULT_PATH".into(), "shared/ORIGIN".into());
        values.insert("VAULT_VECTOR_DB_PATH".into(), "shared/vault_db".into());
        values.insert("GEMINI_API_KEY".into(), "key".into());
        values.insert("SYSTEM_LLM_PROVIDER".into(), "google".into());
        values.insert("SYSTEM_LLM_MODEL".into(), "gemini".into());
        assert!(initializer_status(&values, &extras).needs_initialization);
        extras.mempalace_ready = true;
        extras.steel_installed = true;
        assert!(!initializer_status(&values, &extras).needs_initialization);
    }
}
