//! Cursor SDK engine + cursor-agent CLI settings (persisted in `app_settings`, hot-reloaded).

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::db::Database;
use crate::error::FinallyAValueBotError;

pub const APP_SETTING_CURSOR_SDK_RUNNER_URL: &str = "CURSOR_SDK_RUNNER_URL";
pub const APP_SETTING_CURSOR_SDK_MODEL: &str = "CURSOR_SDK_MODEL";
pub const APP_SETTING_CURSOR_SDK_RUNNER_OK: &str = "CURSOR_SDK_RUNNER_OK";
pub const APP_SETTING_CURSOR_AGENT_CLI_PATH: &str = "CURSOR_AGENT_CLI_PATH";
pub const APP_SETTING_CURSOR_AGENT_MODEL: &str = "CURSOR_AGENT_MODEL";
pub const APP_SETTING_CURSOR_AGENT_RUNNER_URL: &str = "CURSOR_AGENT_RUNNER_URL";
pub const APP_SETTING_CURSOR_AGENT_TIMEOUT_SECS: &str = "CURSOR_AGENT_TIMEOUT_SECS";
pub const APP_SETTING_CURSOR_INTERACTIVE_TIMEOUT_SECS: &str = "CURSOR_INTERACTIVE_TIMEOUT_SECS";
pub const APP_SETTING_CURSOR_AGENT_TMUX_ENABLED: &str = "CURSOR_AGENT_TMUX_ENABLED";
pub const APP_SETTING_CURSOR_SDK_MODEL_PARAMS: &str = "CURSOR_SDK_MODEL_PARAMS";
pub const APP_SETTING_CURSOR_MCP_TOOLS_ENABLED: &str = "CURSOR_MCP_TOOLS_ENABLED";
pub const APP_SETTING_CURSOR_MCP_EXPOSE_SEND_MESSAGE: &str = "CURSOR_MCP_EXPOSE_SEND_MESSAGE";
pub const APP_SETTING_CURSOR_DELEGATION_SLIM_PROMPT: &str = "CURSOR_DELEGATION_SLIM_PROMPT";
pub const APP_SETTING_CURSOR_DELEGATION_RESUME_DELTA: &str = "CURSOR_DELEGATION_RESUME_DELTA";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorModelParam {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorModelParameterValue {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorModelParameterDef {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub values: Vec<CursorModelParameterValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorModelVariant {
    pub params: Vec<CursorModelParam>,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorModelCatalogEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<CursorModelParameterDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<CursorModelVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorEngineSettings {
    pub sdk_runner_url: String,
    pub sdk_model: String,
    pub sdk_model_params: Vec<CursorModelParam>,
    pub sdk_runner_ok: bool,
    pub cli_path: String,
    pub cli_model: String,
    pub cli_runner_url: String,
    pub timeout_secs: u64,
    /// Wall-clock budget for interactive Cursor turns (scheduled/background use `timeout_secs`).
    pub interactive_timeout_secs: u64,
    pub tmux_enabled: bool,
    /// Expose bot ToolRegistry to Cursor via loopback MCP (default on).
    pub mcp_tools_enabled: bool,
    /// Allow `send_message` through Cursor MCP (default off).
    pub mcp_expose_send_message: bool,
    /// Strip tool catalog from Cursor sidecar system prompt when MCP is live (default on).
    pub delegation_slim_prompt: bool,
    /// Deprecated: resume-delta sessions are retired. Always false; ignored by the engine.
    pub delegation_resume_delta: bool,
}

impl Default for CursorEngineSettings {
    fn default() -> Self {
        Self {
            sdk_runner_url: String::new(),
            sdk_model: crate::config::default_cursor_sdk_model(),
            sdk_model_params: Vec::new(),
            sdk_runner_ok: false,
            cli_path: crate::config::default_cursor_agent_cli_path(),
            cli_model: String::new(),
            cli_runner_url: String::new(),
            timeout_secs: 3600,
            interactive_timeout_secs: 900,
            tmux_enabled: true,
            mcp_tools_enabled: true,
            mcp_expose_send_message: false,
            delegation_slim_prompt: true,
            delegation_resume_delta: false,
        }
    }
}

fn default_interactive_timeout_secs() -> u64 {
    900
}

impl CursorEngineSettings {
    pub fn from_env(config: &Config) -> Self {
        let port = config.cursor_sdk_runner_port;
        let sdk_runner_url = config
            .cursor_sdk_runner_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| crate::cursor_sdk_sidecar::default_local_runner_url(port));
        Self {
            sdk_runner_url,
            sdk_model: config.cursor_sdk_model.trim().to_string(),
            sdk_model_params: Vec::new(),
            sdk_runner_ok: false,
            cli_path: config.cursor_agent_cli_path.trim().to_string(),
            cli_model: config.cursor_agent_model.trim().to_string(),
            cli_runner_url: config
                .cursor_agent_runner_url
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string(),
            timeout_secs: config.cursor_agent_timeout_secs,
            interactive_timeout_secs: default_interactive_timeout_secs(),
            tmux_enabled: config.cursor_agent_tmux_enabled,
            mcp_tools_enabled: true,
            mcp_expose_send_message: false,
            delegation_slim_prompt: true,
            delegation_resume_delta: false,
        }
    }

    pub fn sdk_configured(&self) -> bool {
        !self.sdk_runner_url.trim().is_empty() && !self.sdk_model.trim().is_empty()
    }

    pub fn engine_ready(&self, health: &SidecarHealth) -> bool {
        self.sdk_configured()
            && self.sdk_runner_ok
            && health.reachable
            && health.api_key_configured
            && health.cursor_sdk_installed
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarHealth {
    pub reachable: bool,
    pub api_key_configured: bool,
    pub cursor_sdk_installed: bool,
    #[serde(default)]
    pub runs_in_flight: u64,
    #[serde(default)]
    pub persona_bridges_active: u64,
    #[serde(default)]
    pub os_bridge_pids: u64,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub started_at_unix: u64,
    #[serde(default)]
    pub recycle_requested: bool,
    #[serde(default)]
    pub oldest_run_age_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn read_setting(settings: &[(String, String)], key: &str) -> Option<String> {
    settings
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn load_from_db(
    db: &Database,
    env: &Config,
) -> Result<CursorEngineSettings, FinallyAValueBotError> {
    let rows: Vec<(String, String)> = db
        .list_app_settings()?
        .into_iter()
        .map(|s| (s.key, s.value))
        .collect();
    let mut cfg = CursorEngineSettings::from_env(env);

    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_SDK_RUNNER_URL) {
        cfg.sdk_runner_url = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_SDK_MODEL) {
        cfg.sdk_model = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_SDK_MODEL_PARAMS) {
        if let Ok(parsed) = serde_json::from_str::<Vec<CursorModelParam>>(&v) {
            cfg.sdk_model_params = parsed
                .into_iter()
                .filter(|p| !p.id.trim().is_empty() && !p.value.trim().is_empty())
                .collect();
        }
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_SDK_RUNNER_OK) {
        cfg.sdk_runner_ok = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_AGENT_CLI_PATH) {
        cfg.cli_path = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_AGENT_MODEL) {
        cfg.cli_model = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_AGENT_RUNNER_URL) {
        cfg.cli_runner_url = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_AGENT_TIMEOUT_SECS) {
        if let Ok(n) = v.parse::<u64>() {
            cfg.timeout_secs = n.clamp(60, 86_400);
        }
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_INTERACTIVE_TIMEOUT_SECS) {
        if let Ok(n) = v.parse::<u64>() {
            cfg.interactive_timeout_secs = n.clamp(60, 86_400);
        }
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_AGENT_TMUX_ENABLED) {
        cfg.tmux_enabled = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_MCP_TOOLS_ENABLED) {
        cfg.mcp_tools_enabled = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_MCP_EXPOSE_SEND_MESSAGE) {
        cfg.mcp_expose_send_message = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CURSOR_DELEGATION_SLIM_PROMPT) {
        cfg.delegation_slim_prompt = parse_bool(&v);
    }
    // Resume-delta is retired; every message is a fresh Cursor session.
    cfg.delegation_resume_delta = false;

    if cfg.sdk_model.trim().is_empty() {
        cfg.sdk_model = crate::config::default_cursor_sdk_model();
    }
    if cfg.cli_path.trim().is_empty() {
        cfg.cli_path = crate::config::default_cursor_agent_cli_path();
    }
    if cfg.sdk_runner_url.trim().is_empty() {
        cfg.sdk_runner_url =
            crate::cursor_sdk_sidecar::default_local_runner_url(env.cursor_sdk_runner_port);
    }

    Ok(cfg)
}

/// Overlay per-persona Cursor SDK model / params onto global settings.
pub fn apply_persona_cursor_model_override(
    mut settings: CursorEngineSettings,
    cursor_sdk_model: Option<&str>,
    cursor_sdk_model_params_json: Option<&str>,
) -> CursorEngineSettings {
    if let Some(model) = cursor_sdk_model.map(str::trim).filter(|s| !s.is_empty()) {
        settings.sdk_model = model.to_string();
    }
    if let Some(raw) = cursor_sdk_model_params_json
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(parsed) = serde_json::from_str::<Vec<CursorModelParam>>(raw) {
            settings.sdk_model_params = parsed
                .into_iter()
                .filter(|p| !p.id.trim().is_empty())
                .collect();
        }
    }
    if settings.sdk_model.trim().is_empty() {
        settings.sdk_model = crate::config::default_cursor_sdk_model();
    }
    settings
}

pub fn persist_to_db(
    db: &Database,
    cfg: &CursorEngineSettings,
) -> Result<(), FinallyAValueBotError> {
    db.set_app_setting(APP_SETTING_CURSOR_SDK_RUNNER_URL, cfg.sdk_runner_url.trim())?;
    db.set_app_setting(APP_SETTING_CURSOR_SDK_MODEL, cfg.sdk_model.trim())?;
    let params_json = serde_json::to_string(&cfg.sdk_model_params).unwrap_or_else(|_| "[]".into());
    db.set_app_setting(APP_SETTING_CURSOR_SDK_MODEL_PARAMS, &params_json)?;
    db.set_app_setting(
        APP_SETTING_CURSOR_SDK_RUNNER_OK,
        if cfg.sdk_runner_ok { "true" } else { "false" },
    )?;
    db.set_app_setting(APP_SETTING_CURSOR_AGENT_CLI_PATH, cfg.cli_path.trim())?;
    db.set_app_setting(APP_SETTING_CURSOR_AGENT_MODEL, cfg.cli_model.trim())?;
    db.set_app_setting(
        APP_SETTING_CURSOR_AGENT_RUNNER_URL,
        cfg.cli_runner_url.trim(),
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_AGENT_TIMEOUT_SECS,
        &cfg.timeout_secs.to_string(),
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_INTERACTIVE_TIMEOUT_SECS,
        &cfg.interactive_timeout_secs.to_string(),
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_AGENT_TMUX_ENABLED,
        if cfg.tmux_enabled { "true" } else { "false" },
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_MCP_TOOLS_ENABLED,
        if cfg.mcp_tools_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_MCP_EXPOSE_SEND_MESSAGE,
        if cfg.mcp_expose_send_message {
            "true"
        } else {
            "false"
        },
    )?;
    db.set_app_setting(
        APP_SETTING_CURSOR_DELEGATION_SLIM_PROMPT,
        if cfg.delegation_slim_prompt {
            "true"
        } else {
            "false"
        },
    )?;
    db.set_app_setting(APP_SETTING_CURSOR_DELEGATION_RESUME_DELTA, "false")?;
    Ok(())
}

/// Effective HTTP wall-clock for a Cursor sidecar turn.
pub fn cursor_turn_timeout_secs(
    settings: &CursorEngineSettings,
    is_scheduled_task: bool,
    is_background_job: bool,
) -> u64 {
    let max = settings.timeout_secs.max(60);
    if is_scheduled_task || is_background_job {
        max
    } else {
        settings.interactive_timeout_secs.max(60).min(max)
    }
}

pub fn cursor_turn_timeout_notice(timeout_secs: u64) -> String {
    format!(
        "This Cursor turn stopped after {timeout_secs}s without a final reply. \
Please send your request again."
    )
}

/// User-facing copy when the sidecar HTTP/NDJSON stream dies but work may still be running.
pub fn cursor_stream_interrupt_notice(
    timeout_secs: u64,
    timed_out: bool,
    background_still_running: bool,
) -> String {
    if background_still_running {
        if timed_out {
            format!(
                "The live Cursor turn stopped after {timeout_secs}s, but background work for this chat is still running. \
You'll get another message when it finishes. Do not send the same request again unless you want to start a new job."
            )
        } else {
            "The live Cursor stream dropped, but background work for this chat is still running. \
You'll get another message when it finishes. Do not send the same request again unless you want to start a new job."
                .to_string()
        }
    } else if timed_out {
        cursor_turn_timeout_notice(timeout_secs)
    } else {
        "The live Cursor stream dropped before a final reply was ready. \
Please send your request again."
            .to_string()
    }
}

pub fn is_cursor_interrupt_notice(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("live cursor stream dropped")
        || lower.contains("live cursor turn stopped after")
        || lower.contains("this cursor turn stopped after")
}

pub fn validate_runner_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("Runner URL is required".into());
    }
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return Err("Runner URL must start with http:// or https://".into());
    }
    Ok(())
}

pub async fn probe_sidecar_health(base_url: &str) -> SidecarHealth {
    probe_sidecar_health_with_timeout(base_url, Duration::from_secs(10)).await
}

pub async fn probe_sidecar_health_with_timeout(base_url: &str, timeout: Duration) -> SidecarHealth {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return SidecarHealth {
            reachable: false,
            api_key_configured: false,
            cursor_sdk_installed: false,
            error: Some("Runner URL is not configured".into()),
            ..Default::default()
        };
    }

    let url = format!("{trimmed}/health");
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return SidecarHealth {
                reachable: false,
                api_key_configured: false,
                cursor_sdk_installed: false,
                error: Some(format!("HTTP client error: {e}")),
                ..Default::default()
            };
        }
    };

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => SidecarHealth {
                reachable: true,
                api_key_configured: body
                    .get("api_key_configured")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                cursor_sdk_installed: body
                    .get("cursor_sdk_installed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                runs_in_flight: body
                    .get("runs_in_flight")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                persona_bridges_active: body
                    .get("persona_bridges_active")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                os_bridge_pids: body
                    .get("os_bridge_pids")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                uptime_secs: body
                    .get("uptime_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                started_at_unix: body
                    .get("started_at_unix")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                recycle_requested: body
                    .get("recycle_requested")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                oldest_run_age_secs: body
                    .get("oldest_run_age_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                error: None,
            },
            Err(e) => SidecarHealth {
                reachable: true,
                api_key_configured: false,
                cursor_sdk_installed: false,
                error: Some(format!("Invalid health JSON: {e}")),
                ..Default::default()
            },
        },
        Ok(resp) => SidecarHealth {
            reachable: false,
            api_key_configured: false,
            cursor_sdk_installed: false,
            error: Some(format!("Sidecar health returned HTTP {}", resp.status())),
            ..Default::default()
        },
        Err(e) => SidecarHealth {
            reachable: false,
            api_key_configured: false,
            cursor_sdk_installed: false,
            error: Some(format!("Sidecar unreachable: {e}")),
            ..Default::default()
        },
    }
}

pub async fn fetch_sidecar_model_catalog(
    base_url: &str,
) -> Result<Vec<CursorModelCatalogEntry>, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Runner URL is not configured".into());
    }
    let url = format!("{trimmed}/models");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Sidecar models request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let detail = body
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("message").and_then(|v| v.as_str()))
            .unwrap_or("");
        if !detail.is_empty() {
            return Err(detail.to_string());
        }
        return Err(format!("Sidecar models returned HTTP {status}"));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid models JSON: {e}"))?;
    let models = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| serde_json::from_value::<CursorModelCatalogEntry>(m.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if models.is_empty() {
        return Err("No models returned from Cursor API.".into());
    }
    Ok(models)
}

pub async fn fetch_sidecar_models(base_url: &str) -> Result<Vec<String>, String> {
    let catalog = fetch_sidecar_model_catalog(base_url).await?;
    Ok(catalog.into_iter().map(|m| m.id).collect())
}

pub fn cli_on_path(cli_path: &str) -> bool {
    let path = cli_path.trim();
    if path.is_empty() {
        return false;
    }
    if path.contains('/') || path.contains('\\') {
        return std::path::Path::new(path).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(path).exists()))
}

#[derive(Debug, Deserialize)]
pub struct CursorEnginePatchRequest {
    /// When set, `sdk_model` / `sdk_model_params` are stored on this persona.
    #[serde(default)]
    pub persona_id: Option<i64>,
    #[serde(default)]
    pub sdk_runner_url: Option<String>,
    #[serde(default)]
    pub sdk_model: Option<String>,
    #[serde(default)]
    pub sdk_model_params: Option<Vec<CursorModelParam>>,
    #[serde(default)]
    pub cli_path: Option<String>,
    #[serde(default)]
    pub cli_model: Option<String>,
    #[serde(default)]
    pub cli_runner_url: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub interactive_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tmux_enabled: Option<bool>,
    #[serde(default)]
    pub mcp_tools_enabled: Option<bool>,
    #[serde(default)]
    pub mcp_expose_send_message: Option<bool>,
    #[serde(default)]
    pub delegation_slim_prompt: Option<bool>,
    #[serde(default)]
    pub delegation_resume_delta: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_runner_url_requires_http_scheme() {
        assert!(validate_runner_url("http://127.0.0.1:3848").is_ok());
        assert!(validate_runner_url("ftp://bad").is_err());
        assert!(validate_runner_url("").is_err());
    }

    #[test]
    fn engine_ready_requires_health_and_flag() {
        let cfg = CursorEngineSettings {
            sdk_runner_url: "http://127.0.0.1:3848".into(),
            sdk_model: "composer-2.5".into(),
            sdk_runner_ok: true,
            ..Default::default()
        };
        let health = SidecarHealth {
            reachable: true,
            api_key_configured: true,
            cursor_sdk_installed: true,
            error: None,
            ..Default::default()
        };
        assert!(cfg.engine_ready(&health));
    }

    #[test]
    fn cursor_turn_timeout_secs_prefers_interactive_budget() {
        let cfg = CursorEngineSettings {
            timeout_secs: 3600,
            interactive_timeout_secs: 900,
            ..Default::default()
        };
        assert_eq!(cursor_turn_timeout_secs(&cfg, false, false), 900);
        assert_eq!(cursor_turn_timeout_secs(&cfg, true, false), 3600);
        assert_eq!(cursor_turn_timeout_secs(&cfg, false, true), 3600);
    }

    #[test]
    fn cursor_stream_interrupt_notice_mentions_background_and_skips_resend() {
        let with_bg = cursor_stream_interrupt_notice(900, true, true);
        assert!(with_bg.contains("background work"));
        assert!(with_bg.contains("Do not send the same request again"));
        assert!(!with_bg
            .to_lowercase()
            .contains("please send your request again"));

        let decode = cursor_stream_interrupt_notice(900, false, true);
        assert!(decode.contains("stream dropped"));
        assert!(decode.contains("still running"));

        let timeout_only = cursor_stream_interrupt_notice(900, true, false);
        assert!(timeout_only.contains("stopped after 900s"));
        assert!(timeout_only.contains("Please send your request again"));
        assert!(!timeout_only.contains("background work"));
        assert!(!timeout_only.to_lowercase().contains("comfy"));
        assert!(!timeout_only.to_lowercase().contains("check again"));
        assert!(!timeout_only.to_lowercase().contains("narrower request"));

        let decode_idle = cursor_stream_interrupt_notice(900, false, false);
        assert!(decode_idle.contains("stream dropped"));
        assert!(decode_idle.contains("Please send your request again"));
        assert!(!decode_idle.to_lowercase().contains("comfy"));
        assert!(!decode_idle.to_lowercase().contains("check again"));
        assert!(is_cursor_interrupt_notice(&decode_idle));
    }
}
