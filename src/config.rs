use crate::error::FinallyAValueBotError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Join `workspace_root` with a vault path.
///
/// Absolute `configured` paths are returned unchanged. Relative paths are joined under
/// `workspace_root`. If the first path segment equals the last segment of `workspace_root`
/// (for example `.../workspace` + `workspace/shared/...`), that leading segment is dropped
/// so the result does not contain `workspace/workspace`.
pub fn resolve_workspace_data_path(workspace_root: &Path, configured: &str) -> PathBuf {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let rel = trimmed.trim_start_matches("./");
    let rel_path = Path::new(rel);
    let tail: PathBuf = if let (Some(root_leaf), Some(first)) =
        (workspace_root.file_name(), rel_path.components().next())
    {
        use std::path::Component;
        if let (Some(r), Component::Normal(f)) = (root_leaf.to_str(), first) {
            if r == f {
                rel_path.components().skip(1).collect()
            } else {
                rel_path.to_path_buf()
            }
        } else {
            rel_path.to_path_buf()
        }
    } else {
        rel_path.to_path_buf()
    };
    workspace_root.join(tail)
}

/// ORIGIN vault root for file tools, locate, and memory snapshots.
///
/// `configured` is `VAULT_ORIGIN_VAULT_PATH`. Absolute values are used as given.
/// Relative values are joined onto `workspace_root`. An empty value is
/// `{workspace_root}/shared/ORIGIN`.
pub fn resolve_origin_vault_root(workspace_root: &Path, configured: Option<&str>) -> PathBuf {
    match configured.map(str::trim).filter(|p| !p.is_empty()) {
        Some(configured) => resolve_workspace_data_path(workspace_root, configured),
        None => workspace_root.join("shared").join("ORIGIN"),
    }
}

fn default_telegram_bot_token() -> String {
    String::new()
}
fn default_bot_username() -> String {
    String::new()
}
fn default_agent_display_name() -> String {
    String::new()
}
fn default_llm_provider() -> String {
    String::new()
}
fn default_api_key() -> String {
    String::new()
}
fn default_model() -> String {
    String::new()
}
fn default_max_tokens() -> u32 {
    8192
}
fn default_max_tool_iterations() -> usize {
    100
}
fn default_max_history_messages() -> usize {
    50
}
fn default_recent_history_min_user_messages() -> usize {
    2
}
fn default_recent_history_min_assistant_messages() -> usize {
    2
}
fn default_max_document_size_mb() -> u64 {
    100
}
fn default_workspace_dir() -> String {
    "./workspace".into()
}
fn default_timezone() -> String {
    "UTC".into()
}
fn default_whatsapp_webhook_port() -> u16 {
    8080
}
fn default_wecom_webhook_port() -> u16 {
    8081
}

/// Web UI and channel inbox when `UNIVERSAL_CHAT_ID` is unset.
pub const DEFAULT_UNIVERSAL_CHAT_ID: i64 = 997894126;
fn default_control_chat_ids() -> Vec<i64> {
    Vec::new()
}
fn default_web_enabled() -> bool {
    true
}
fn default_web_host() -> String {
    "127.0.0.1".into()
}
pub fn default_web_port() -> u16 {
    10961
}

/// How many ports past the default to try when that port is already taken.
pub const WEB_PORT_FALLBACK_SPAN: u16 = 50;
fn default_web_max_inflight_per_session() -> usize {
    2
}
fn default_web_max_requests_per_window() -> usize {
    8
}
fn default_web_rate_window_seconds() -> u64 {
    10
}
fn default_web_run_history_limit() -> usize {
    512
}
fn default_web_session_idle_ttl_seconds() -> u64 {
    300
}
fn default_web_terminal_max_sessions() -> usize {
    2
}
fn default_web_terminal_idle_timeout_secs() -> u64 {
    1800
}
fn default_browser_managed() -> bool {
    false
}
pub fn default_steel_api_port() -> u16 {
    13_920
}
pub fn default_steel_cdp_port() -> u16 {
    13_923
}
pub fn default_steel_docker_image() -> String {
    "ghcr.io/steel-dev/steel-browser:latest".into()
}
pub fn default_steel_docker_container_name() -> String {
    "meido-steel".into()
}
fn default_browser_cdp_port_base() -> u16 {
    9222
}
fn default_browser_headless() -> bool {
    false
}
fn default_safety_output_guard_mode() -> String {
    "moderate".into()
}
fn default_safety_max_emojis_per_response() -> usize {
    12
}
fn default_safety_tail_repeat_limit() -> usize {
    8
}
fn default_safety_execution_mode() -> String {
    "warn_confirm".into()
}
fn default_safety_risky_categories() -> Vec<String> {
    vec![
        "destructive".into(),
        "system".into(),
        "network".into(),
        "package".into(),
    ]
}

#[cfg(target_os = "windows")]
pub(crate) fn default_cursor_agent_cli_path() -> String {
    "cursor-agent.cmd".into()
}
#[cfg(not(target_os = "windows"))]
pub(crate) fn default_cursor_agent_cli_path() -> String {
    "cursor-agent".into()
}

fn default_cursor_agent_model() -> String {
    String::new()
}

fn default_cursor_agent_timeout_secs() -> u64 {
    3600
}

pub(crate) fn default_cursor_sdk_model() -> String {
    "composer-2.5".into()
}

fn default_cursor_sdk_runner_port() -> u16 {
    3848
}

fn default_cursor_sdk_python() -> String {
    "python3".into()
}

fn default_cursor_sdk_node() -> String {
    "node".into()
}

fn default_cursor_sdk_auto_start() -> bool {
    true
}

fn default_cursor_sdk_auto_install() -> bool {
    true
}

fn default_cursor_sidecar_max_uptime_secs() -> u64 {
    86_400
}

fn default_scheduler_task_timeout_secs() -> u64 {
    3600
}

fn default_scheduler_stale_running_reclaim_secs() -> u64 {
    7200
}

fn default_scheduler_max_concurrent_tasks() -> usize {
    2
}

fn default_scheduler_poll_interval_secs() -> u64 {
    60
}

fn default_topics_enabled() -> bool {
    true
}

fn default_topics_min_unsegmented() -> u64 {
    6
}

fn default_topics_idle_minutes() -> u64 {
    20
}

fn default_topics_gap_hours() -> u64 {
    6
}

fn default_topics_prefer_tier2() -> bool {
    true
}

fn default_topics_debounce_minutes() -> u64 {
    2
}

fn default_background_job_lease_ttl_secs() -> u64 {
    // Must exceed longest expected gap between lease renewals (e.g. one bash/ComfyUI tool call).
    1800
}

fn default_background_job_lease_fallback_renew_secs() -> u64 {
    // Renew periodically during long tool calls; must be well below lease TTL.
    60
}

fn default_background_job_pending_start_timeout_secs() -> u64 {
    300
}

fn default_background_job_notify_chat_progress() -> bool {
    false
}

fn default_tool_output_debug() -> bool {
    false
}

/// Parses truthy strings for bool settings (`1`, `true`, `yes`, `on`; case-insensitive).
pub fn parse_bool_setting(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn default_background_shell_tmux_session_prefix() -> String {
    "meido-bg".into()
}

fn default_background_shell_tmux_enabled() -> bool {
    true
}

fn default_background_shell_monitor_poll_secs() -> u64 {
    8
}

fn default_background_shell_auto_retry_on_failure() -> bool {
    true
}

fn default_background_shell_auto_retry_max() -> u32 {
    1
}

fn default_background_shell_auto_agent_on_success() -> bool {
    true
}

fn default_runtime_reliability_profile() -> String {
    "balanced".into()
}

fn default_project_auto_association_strictness() -> String {
    "balanced".into()
}

fn default_post_tool_evaluator_enabled() -> bool {
    false
}

fn default_post_tool_evaluator_model() -> String {
    String::new()
}

fn default_response_quality_evaluator_enabled() -> bool {
    false
}

fn default_evaluator_model() -> String {
    "sonar".into()
}

fn default_evaluator_base_url() -> String {
    "https://api.perplexity.ai".into()
}

fn default_quality_eval_max_nudges_per_run() -> usize {
    1
}

fn default_quality_eval_min_confidence() -> f64 {
    0.7
}

fn default_quality_eval_channels() -> String {
    "telegram,web".into()
}

fn default_hook_command_timeout_secs() -> u64 {
    10
}
fn default_hook_prompt_timeout_secs() -> u64 {
    15
}
fn default_hook_prompt_model() -> String {
    String::new()
}

fn default_allow_fuzzy_search_replace() -> bool {
    false
}

fn default_symbol_edit_enabled() -> bool {
    false
}

fn default_post_edit_validation_enabled() -> bool {
    true
}

fn default_cursor_agent_tmux_session_prefix() -> String {
    "meido-cursor".into()
}

fn default_cursor_agent_tmux_enabled() -> bool {
    true
}

fn is_local_web_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    h == "127.0.0.1" || h == "localhost" || h == "::1"
}

/// Address the socket binds. An IP or loopback name is used as-is. A DNS name such as
/// `pp.local` stays the URL people open, while the socket listens on every interface.
pub(crate) fn web_bind_host(host: &str) -> String {
    let host = host.trim();
    if host.parse::<std::net::IpAddr>().is_ok() || is_local_web_host(host) {
        host.to_string()
    } else {
        "0.0.0.0".to_string()
    }
}

/// Keys that configure LLM providers, models, and related limits. These must come from repo-root
/// `.env` / process environment only — not from `app_settings` (Web UI persistence).
///
/// Exception: [`crate::llm_catalog::APP_SETTING_LLM_MODEL`], [`crate::llm_catalog::APP_SETTING_LLM_PROVIDER`],
/// and [`crate::llm_catalog::APP_SETTING_LLM_BASE_URL`] (local servers only) may be stored in `app_settings`.
pub fn is_llm_related_runtime_setting_key(key: &str) -> bool {
    let u = key.trim().to_ascii_uppercase();
    if u == crate::llm_catalog::APP_SETTING_LLM_MODEL
        || u == crate::llm_catalog::APP_SETTING_LLM_PROVIDER
        || u == crate::llm_catalog::APP_SETTING_LLM_BASE_URL
        || u == crate::llm_catalog::APP_SETTING_LLM_THINKING_ENABLED
        || u == crate::llm_catalog::APP_SETTING_SHOW_THINKING
    {
        return false;
    }
    if u.starts_with("LLM_") {
        return true;
    }
    matches!(
        u.as_str(),
        "OPENAI_API_KEY"
            | "GEMINI_API_KEY"
            | "GOOGLE_API_KEY"
            | "ANTHROPIC_API_KEY"
            | "XAI_API_KEY"
            | "MAX_TOKENS"
            | "MAX_TOOL_ITERATIONS"
            | "MAX_HISTORY_MESSAGES"
            | "RECENT_HISTORY_MIN_USER_MESSAGES"
            | "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES"
            | "MAX_DOCUMENT_SIZE_MB"
            | "POST_TOOL_EVALUATOR_MODEL"
            | "POST_TOOL_EVALUATOR_ENABLED"
            | "SHOW_THINKING"
    )
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SocialPlatformConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SocialConfig {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub tiktok: SocialPlatformConfig,
    #[serde(default)]
    pub instagram: SocialPlatformConfig,
    #[serde(default)]
    pub linkedin: SocialPlatformConfig,
}

/// Optional vault/vector DB config for ORIGIN Obsidian vault integration.
/// Paths are relative to workspace_dir unless absolute.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VaultConfig {
    /// ORIGIN vault path. Relative values are joined onto the workspace (`shared/ORIGIN`).
    /// Absolute values are the file-tool, locate, and memory-snapshot root.
    #[serde(default)]
    pub origin_vault_path: Option<String>,
    /// ChromaDB persistence dir relative to workspace_dir (e.g. "shared/vault_db").
    #[serde(default)]
    pub vector_db_path: Option<String>,
    /// Git repo URL to clone/pull vault (for sync service). Env: VAULT_ORIGIN_VAULT_REPO or VAULT_GIT_URL.
    #[serde(default)]
    pub origin_vault_repo: Option<String>,
    /// Embedding server URL (e.g. "http://10.0.1.211:8080" for llama.cpp).
    #[serde(default)]
    pub embedding_server_url: Option<String>,
    /// Search command; use "{query}" as placeholder for the query.
    #[serde(default)]
    pub vault_search_command: Option<String>,
    /// Index command to run after vault updates.
    #[serde(default)]
    pub vault_index_command: Option<String>,
    /// Override principles file path relative to workspace_dir (e.g. "shared/ORIGIN/AGENTS.md"). Default: "AGENTS.md" at workspace root.
    #[serde(default)]
    pub principles_path: Option<String>,
    /// ChromaDB HTTP server URL (e.g. "http://localhost:8000"). Required for the native search_vault tool.
    #[serde(default)]
    pub vector_db_url: Option<String>,
    /// ChromaDB collection name (default: "vault").
    #[serde(default)]
    pub vector_db_collection: Option<String>,
}

impl SocialConfig {
    pub fn is_platform_enabled(&self, platform: &str) -> bool {
        let (id, secret) = match platform {
            "tiktok" => (
                self.tiktok.client_id.as_deref().unwrap_or(""),
                self.tiktok.client_secret.as_deref().unwrap_or(""),
            ),
            "instagram" => (
                self.instagram.client_id.as_deref().unwrap_or(""),
                self.instagram.client_secret.as_deref().unwrap_or(""),
            ),
            "linkedin" => (
                self.linkedin.client_id.as_deref().unwrap_or(""),
                self.linkedin.client_secret.as_deref().unwrap_or(""),
            ),
            _ => return false,
        };
        !id.trim().is_empty() && !secret.trim().is_empty()
    }
}

fn default_mempalace_startup_timeout_secs() -> u64 {
    20
}

fn default_mempalace_query_timeout_secs() -> u64 {
    15
}

fn default_mempalace_recall_mode() -> String {
    "default".to_string()
}

pub fn default_mempalace_config() -> MemPalaceConfig {
    MemPalaceConfig {
        command: String::new(),
        args: Vec::new(),
        endpoint: String::new(),
        startup_timeout_secs: default_mempalace_startup_timeout_secs(),
        query_timeout_secs: default_mempalace_query_timeout_secs(),
        recall_mode: default_mempalace_recall_mode(),
        selected_persona_ids: Vec::new(),
    }
}

/// MemPalace MCP process. Default mode is `default`. Startup installs the
/// official MCP when command and endpoint are empty. `disabled` keeps the
/// existing history and vault searches.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemPalaceConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_mempalace_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    #[serde(default = "default_mempalace_query_timeout_secs")]
    pub query_timeout_secs: u64,
    /// `disabled` | `shadow` | `selected` | `default`.
    #[serde(default = "default_mempalace_recall_mode")]
    pub recall_mode: String,
    #[serde(default)]
    pub selected_persona_ids: Vec<i64>,
}

impl MemPalaceConfig {
    pub fn configured(&self) -> bool {
        !self.command.trim().is_empty() || !self.endpoint.trim().is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_telegram_bot_token")]
    pub telegram_bot_token: String,
    #[serde(default = "default_bot_username")]
    pub bot_username: String,
    /// Optional friendly identity label used in canonical persona memory identity seeding.
    /// This is not hardcoded into static system prompt text.
    #[serde(default = "default_agent_display_name")]
    pub agent_display_name: String,
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
    #[serde(default = "default_api_key")]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub llm_base_url: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,
    #[serde(default = "default_max_history_messages")]
    pub max_history_messages: usize,
    /// Default minimum user messages kept in the trimmed chat suffix when persona has no override.
    #[serde(default = "default_recent_history_min_user_messages")]
    pub recent_history_min_user_messages: usize,
    /// Default minimum assistant messages in that suffix when persona has no override.
    #[serde(default = "default_recent_history_min_assistant_messages")]
    pub recent_history_min_assistant_messages: usize,
    #[serde(default = "default_max_document_size_mb")]
    pub max_document_size_mb: u64,
    /// Single root for runtime, skills, and tool workspace (shared). Layout: workspace_dir/runtime, workspace_dir/skills, workspace_dir/shared. Copy this folder to migrate.
    #[serde(default = "default_workspace_dir")]
    pub workspace_dir: String,
    #[serde(default)]
    pub openai_api_key: Option<String>,
    /// Google Gemini API key (`GEMINI_API_KEY` or `GOOGLE_API_KEY`). Used when `llm_provider` is google/gemini.
    #[serde(default)]
    pub gemini_api_key: Option<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub allowed_groups: Vec<i64>,
    #[serde(default = "default_control_chat_ids")]
    pub control_chat_ids: Vec<i64>,
    #[serde(default)]
    pub whatsapp_access_token: Option<String>,
    #[serde(default)]
    pub whatsapp_phone_number_id: Option<String>,
    #[serde(default)]
    pub whatsapp_verify_token: Option<String>,
    #[serde(default = "default_whatsapp_webhook_port")]
    pub whatsapp_webhook_port: u16,
    #[serde(default)]
    pub wecom_corp_id: Option<String>,
    #[serde(default)]
    pub wecom_corp_secret: Option<String>,
    #[serde(default)]
    pub wecom_agent_id: i64,
    #[serde(default)]
    pub wecom_callback_token: Option<String>,
    #[serde(default)]
    pub wecom_encoding_aes_key: Option<String>,
    #[serde(default = "default_wecom_webhook_port")]
    pub wecom_webhook_port: u16,
    #[serde(default)]
    pub wecom_allowed_chats: Vec<String>,
    #[serde(default)]
    pub wecom_aibot_id: Option<String>,
    #[serde(default)]
    pub wecom_mode: String,
    #[serde(default)]
    pub discord_bot_token: Option<String>,
    #[serde(default)]
    pub discord_allowed_channels: Vec<u64>,
    #[serde(default)]
    pub show_thinking: bool,
    /// When true, request extended thinking from providers that support it (e.g. Gemini thinkingConfig).
    #[serde(default)]
    pub llm_thinking_enabled: bool,
    #[serde(default = "default_web_enabled")]
    pub web_enabled: bool,
    #[serde(default = "default_web_host")]
    pub web_host: String,
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    #[serde(default)]
    pub web_auth_token: Option<String>,
    #[serde(default = "default_web_max_inflight_per_session")]
    pub web_max_inflight_per_session: usize,
    #[serde(default = "default_web_max_requests_per_window")]
    pub web_max_requests_per_window: usize,
    #[serde(default = "default_web_rate_window_seconds")]
    pub web_rate_window_seconds: u64,
    #[serde(default = "default_web_run_history_limit")]
    pub web_run_history_limit: usize,
    #[serde(default = "default_web_session_idle_ttl_seconds")]
    pub web_session_idle_ttl_seconds: u64,
    /// When true, web UI may open an interactive PTY terminal (requires WEB_AUTH_TOKEN). Env: WEB_TERMINAL_ENABLED.
    #[serde(default)]
    pub web_terminal_enabled: bool,
    #[serde(default = "default_web_terminal_max_sessions")]
    pub web_terminal_max_sessions: usize,
    #[serde(default = "default_web_terminal_idle_timeout_secs")]
    pub web_terminal_idle_timeout_secs: u64,
    /// Allow web terminal inside Docker (default false). Env: WEB_TERMINAL_ALLOW_IN_DOCKER.
    #[serde(default)]
    pub web_terminal_allow_in_docker: bool,
    /// When set, web UI uses this chat_id for all requests (single universal contact across channels). Env: UNIVERSAL_CHAT_ID.
    #[serde(default)]
    pub universal_chat_id: Option<i64>,
    #[serde(default = "default_browser_managed")]
    pub browser_managed: bool,
    /// Local Steel API port when `BROWSER_MANAGED=true`. Env: `STEEL_API_PORT`.
    #[serde(default = "default_steel_api_port")]
    pub steel_api_port: u16,
    /// Host-mapped Steel CDP/debug port. Env: `STEEL_CDP_PORT`.
    #[serde(default = "default_steel_cdp_port")]
    pub steel_cdp_port: u16,
    /// Docker image for managed Steel browser. Env: `STEEL_DOCKER_IMAGE`.
    #[serde(default = "default_steel_docker_image")]
    pub steel_docker_image: String,
    /// Docker container name for managed Steel browser. Env: `STEEL_DOCKER_CONTAINER_NAME`.
    #[serde(default = "default_steel_docker_container_name")]
    pub steel_docker_container_name: String,
    #[serde(default)]
    pub browser_executable_path: Option<String>,
    #[serde(default = "default_browser_cdp_port_base")]
    pub browser_cdp_port_base: u16,
    /// Optional idle timeout (seconds) for managed browser processes. 0 or None = no idle shutdown.
    #[serde(default)]
    pub browser_idle_timeout_secs: Option<u64>,
    #[serde(default = "default_browser_headless")]
    pub browser_headless: bool,
    /// Output repetition guard mode: off | moderate | strict.
    #[serde(default = "default_safety_output_guard_mode")]
    pub safety_output_guard_mode: String,
    /// Max emoji-like characters allowed in one assistant response before trimming.
    #[serde(default = "default_safety_max_emojis_per_response")]
    pub safety_max_emojis_per_response: usize,
    /// Max repeated tail-pattern count allowed before trimming repetitive suffixes.
    #[serde(default = "default_safety_tail_repeat_limit")]
    pub safety_tail_repeat_limit: usize,
    /// Execution safety mode: off | warn_confirm | strict.
    #[serde(default = "default_safety_execution_mode")]
    pub safety_execution_mode: String,
    /// Risky command categories monitored by execution safety.
    #[serde(default = "default_safety_risky_categories")]
    pub safety_risky_categories: Vec<String>,
    /// Optional Tavily API key for web_search. When set, web_search uses Tavily (https://api.tavily.com/search) instead of SearXNG or DuckDuckGo. Env: TAVILY_API_KEY.
    #[serde(default)]
    pub tavily_api_key: Option<String>,
    /// Optional SearXNG instance URL for web_search (e.g. https://search.example.org). Used when Tavily is not configured. Env: SEARXNG_URL.
    #[serde(default)]
    pub web_search_searxng_url: Option<String>,
    /// Path to the cursor-agent CLI. Default: "cursor-agent" (or "cursor-agent.cmd" on Windows). Use when the process PATH doesn't include cursor-agent.
    #[serde(default = "default_cursor_agent_cli_path")]
    pub cursor_agent_cli_path: String,
    /// Model for cursor-agent (e.g. "gpt-5"). Leave empty to omit --model (cursor-agent uses its default / "auto").
    #[serde(default = "default_cursor_agent_model")]
    pub cursor_agent_model: String,
    /// Timeout in seconds for cursor-agent runs. Default: 3600.
    #[serde(default = "default_cursor_agent_timeout_secs")]
    pub cursor_agent_timeout_secs: u64,
    #[serde(default)]
    pub social: Option<SocialConfig>,
    /// Optional vault/vector DB config for ORIGIN Obsidian vault integration.
    #[serde(default)]
    pub vault: Option<VaultConfig>,
    /// Post-Tool Evaluator (PTE): evaluate task completion after each tool iteration. Default false.
    #[serde(default = "default_post_tool_evaluator_enabled")]
    pub post_tool_evaluator_enabled: bool,
    /// Optional model for PTE. If empty, use `EVALUATOR_MODEL`.
    #[serde(default = "default_post_tool_evaluator_model")]
    pub post_tool_evaluator_model: String,
    /// Post-delivery quality evaluator (PDQE): async QC after user receives the reply.
    #[serde(default = "default_response_quality_evaluator_enabled")]
    pub response_quality_evaluator_enabled: bool,
    /// Perplexity API key for PTE/PDQE sidecar (never falls back to LLM_API_KEY).
    #[serde(default)]
    pub perplexity_api_key: Option<String>,
    /// Perplexity model for evaluators (`sonar`, `sonar-pro`, …).
    #[serde(default = "default_evaluator_model")]
    pub evaluator_model: String,
    /// OpenAI-compatible base URL for evaluators.
    #[serde(default = "default_evaluator_base_url")]
    pub evaluator_base_url: String,
    /// Legacy alias: maps to evaluator_model when set.
    #[serde(default = "default_post_tool_evaluator_model")]
    pub response_quality_evaluator_model: String,
    /// Max corrective agent runs per parent run_key after PDQE fail.
    #[serde(default = "default_quality_eval_max_nudges_per_run")]
    pub quality_eval_max_nudges_per_run: usize,
    /// Minimum LLM confidence (0–1) to enqueue a corrective run on PDQE fail.
    #[serde(default = "default_quality_eval_min_confidence")]
    pub quality_eval_min_confidence: f64,
    /// Comma-separated channels where PDQE runs (`telegram`, `web`, …).
    #[serde(default = "default_quality_eval_channels")]
    pub quality_eval_channels: String,
    /// Default timeout in seconds for command hooks.
    #[serde(default = "default_hook_command_timeout_secs")]
    pub hook_command_timeout_secs: u64,
    /// Default timeout in seconds for prompt hooks.
    #[serde(default = "default_hook_prompt_timeout_secs")]
    pub hook_prompt_timeout_secs: u64,
    /// Optional model override for prompt hooks.
    #[serde(default = "default_hook_prompt_model")]
    pub hook_prompt_model: String,
    /// Allow fuzzy fallback in apply_search_replace when input requests allow_fuzzy.
    #[serde(default = "default_allow_fuzzy_search_replace")]
    pub allow_fuzzy_search_replace: bool,
    /// Enable symbol_edit tool for language-aware symbol anchoring.
    #[serde(default = "default_symbol_edit_enabled")]
    pub symbol_edit_enabled: bool,
    /// Run post-edit validation automatically after successful code edits.
    #[serde(default = "default_post_edit_validation_enabled")]
    pub post_edit_validation_enabled: bool,
    /// Optional override commands for post-edit validation, separated by `;;`.
    #[serde(default)]
    pub post_edit_validation_commands: Option<String>,
    /// Tmux session name prefix for cursor_agent when detach=true (e.g. meido-cursor).
    #[serde(default = "default_cursor_agent_tmux_session_prefix")]
    pub cursor_agent_tmux_session_prefix: String,
    /// Allow spawning cursor_agent in tmux when detach=true. Set false in Docker or when tmux unavailable.
    #[serde(default = "default_cursor_agent_tmux_enabled")]
    pub cursor_agent_tmux_enabled: bool,
    /// URL of a host runner that executes cursor-agent (e.g. http://host.docker.internal:3847). When set, the bot POSTs spawn requests instead of running cursor-agent locally.
    #[serde(default)]
    pub cursor_agent_runner_url: Option<String>,
    /// URL of the Cursor SDK sidecar for the Cursor agent engine (e.g. http://127.0.0.1:3848).
    #[serde(default)]
    pub cursor_sdk_runner_url: Option<String>,
    /// Model id for Cursor SDK engine runs (local runtime). Default: composer-2.5.
    #[serde(default = "default_cursor_sdk_model")]
    pub cursor_sdk_model: String,
    /// When true (default), bot starts the local Cursor SDK sidecar on startup.
    #[serde(default = "default_cursor_sdk_auto_start")]
    pub cursor_sdk_auto_start: bool,
    /// When true (default), bot installs @cursor/sdk into a runtime Node prefix (Python rollback uses a venv).
    #[serde(default = "default_cursor_sdk_auto_install")]
    pub cursor_sdk_auto_install: bool,
    /// Soft-recycle Cursor sidecar after this many seconds when idle (default 24h).
    #[serde(default = "default_cursor_sidecar_max_uptime_secs")]
    pub cursor_sidecar_max_uptime_secs: u64,
    /// Local port for the auto-started Cursor SDK sidecar. Default: 3848.
    #[serde(default = "default_cursor_sdk_runner_port")]
    pub cursor_sdk_runner_port: u16,
    /// Python executable used only when CURSOR_SDK_RUNNER_SCRIPT points at the .py rollback runner.
    #[serde(default = "default_cursor_sdk_python")]
    pub cursor_sdk_python: String,
    /// Node executable used to launch the Cursor SDK sidecar. Default: node.
    #[serde(default = "default_cursor_sdk_node")]
    pub cursor_sdk_node: String,
    /// Max wall-clock time (seconds) for a single scheduled-agent run. Default 3600.
    #[serde(default = "default_scheduler_task_timeout_secs")]
    pub scheduler_task_timeout_secs: u64,
    /// Reclaim `scheduled_tasks` stuck in `running` if the claim timestamp is older than this (seconds). Default 7200.
    #[serde(default = "default_scheduler_stale_running_reclaim_secs")]
    pub scheduler_stale_running_reclaim_secs: u64,
    /// Max concurrent scheduled task runs (semaphore). Default 2.
    #[serde(default = "default_scheduler_max_concurrent_tasks")]
    pub scheduler_max_concurrent_tasks: usize,
    /// Seconds between scheduler ticks (reclaim + due-task scan). Default 60.
    #[serde(default = "default_scheduler_poll_interval_secs")]
    pub scheduler_poll_interval_secs: u64,
    /// When true (default), title conversation topics with an LLM. When false, split on time gaps only.
    #[serde(default = "default_topics_enabled")]
    pub topics_enabled: bool,
    /// New messages after the open topic before a scheduled segmentation run. Default 6.
    #[serde(default = "default_topics_min_unsegmented")]
    pub topics_min_unsegmented: u64,
    /// Segment a shorter tail once the latest message is at least this idle. Default 20.
    #[serde(default = "default_topics_idle_minutes")]
    pub topics_idle_minutes: u64,
    /// Heuristic topic split when two messages are at least this many hours apart. Default 6.
    #[serde(default = "default_topics_gap_hours")]
    pub topics_gap_hours: u64,
    /// Prefer a verified Tier 2 local model for segmentation, otherwise the strategy model.
    #[serde(default = "default_topics_prefer_tier2")]
    pub topics_prefer_tier2: bool,
    /// Minimum minutes between automatic segmentation attempts for one session. Default 2.
    #[serde(default = "default_topics_debounce_minutes")]
    pub topics_debounce_minutes: u64,
    /// Lease TTL for active manual background jobs. Worker heartbeat/event flow renews this lease. Default 1800.
    #[serde(default = "default_background_job_lease_ttl_secs")]
    pub background_job_lease_ttl_secs: u64,
    /// Fallback heartbeat lease renewal cadence when no events are emitted. Default 60 (must be < lease TTL).
    #[serde(default = "default_background_job_lease_fallback_renew_secs")]
    pub background_job_lease_fallback_renew_secs: u64,
    /// Maximum age for a pending background job before stale reconciliation fails it. Default 300.
    #[serde(default = "default_background_job_pending_start_timeout_secs")]
    pub background_job_pending_start_timeout_secs: u64,
    /// Post "Background update: …" chat messages during manual background jobs (very noisy). Default false.
    #[serde(default = "default_background_job_notify_chat_progress")]
    pub background_job_notify_chat_progress: bool,
    /// When true, bash/background shell sets `TOOL_OUTPUT_DEBUG=1` so PZ/ComfyUI scripts emit WebSocket poll noise. Default false.
    #[serde(default = "default_tool_output_debug")]
    pub tool_output_debug: bool,
    /// Allow spawning shell commands in tmux for `spawn_background_command`. Default true (false in Docker).
    #[serde(default = "default_background_shell_tmux_enabled")]
    pub background_shell_tmux_enabled: bool,
    #[serde(default = "default_background_shell_tmux_session_prefix")]
    pub background_shell_tmux_session_prefix: String,
    /// Poll interval for tmux shell background job monitor. Default 8.
    #[serde(default = "default_background_shell_monitor_poll_secs")]
    pub background_shell_monitor_poll_secs: u64,
    /// After a failed shell job, enqueue an agent run to diagnose and retry via `spawn_background_command`.
    #[serde(default = "default_background_shell_auto_retry_on_failure")]
    pub background_shell_auto_retry_on_failure: bool,
    /// Max automatic agent retries per failed shell job. Default 1.
    #[serde(default = "default_background_shell_auto_retry_max")]
    pub background_shell_auto_retry_max: u32,
    /// After a successful shell job, enqueue an agent run to summarize outputs for the user.
    #[serde(default = "default_background_shell_auto_agent_on_success")]
    pub background_shell_auto_agent_on_success: bool,
    /// Runtime reliability profile: balanced | aggressive_completion | safe_conservative.
    #[serde(default = "default_runtime_reliability_profile")]
    pub runtime_reliability_profile: String,
    /// Project auto-linking mode: strict | balanced | loose.
    #[serde(default = "default_project_auto_association_strictness")]
    pub project_auto_association_strictness: String,
    /// MemPalace light MCP recall. Disabled unless a mode and command/endpoint are set.
    #[serde(default = "default_mempalace_config")]
    pub mempalace: MemPalaceConfig,
}

impl Config {
    /// Data root directory (workspace root). Layout: runtime/, skills/, shared/ under this path.
    pub fn data_root_dir(&self) -> PathBuf {
        PathBuf::from(&self.workspace_dir)
    }

    /// Working directory for tools (same as workspace root; tools use workspace_dir/shared).
    pub fn working_dir(&self) -> &str {
        &self.workspace_dir
    }

    /// Runtime data directory (db, memory, exports, etc.).
    pub fn runtime_data_dir(&self) -> String {
        self.data_root_dir()
            .join("runtime")
            .to_string_lossy()
            .to_string()
    }

    /// Skills directory under data root.
    pub fn skills_data_dir(&self) -> String {
        self.data_root_dir()
            .join("skills")
            .to_string_lossy()
            .to_string()
    }

    /// Absolute path to the skills directory. Use this in the system prompt so the bot writes skill files to the real skills dir (file tools resolve relative paths from workspace_dir/shared).
    pub fn skills_data_dir_absolute(&self) -> std::path::PathBuf {
        self.workspace_root_absolute().join("skills")
    }

    /// Directories scanned for `SKILL.md` and skill resources, in merge order. The first path wins
    /// when two skills share a name. Workspace and shared skills override built-ins with the same name.
    pub fn skill_discovery_dirs(&self) -> Vec<PathBuf> {
        let root = self.workspace_root_absolute();
        let mut dirs = vec![root.join("skills"), root.join("shared").join("skills")];
        if let Some(builtin) = crate::builtin_skills::resolve_builtin_skills_dir(self) {
            dirs.push(builtin);
        }
        dirs
    }

    /// Put workspace-absolute vault paths into the process environment.
    ///
    /// `.env` keeps `VAULT_VECTOR_DB_PATH` and `VAULT_ORIGIN_VAULT_PATH` relative to the
    /// workspace (`shared/vault_db`). Skill scripts inherit this environment and run with
    /// the skill directory as cwd, so a relative value would be created outside the
    /// workspace. Absolute values are left as set.
    pub fn publish_workspace_paths(&self) {
        let root = self.workspace_root_absolute();
        let root_text = root.to_string_lossy().to_string();
        std::env::set_var("WORKSPACE_DIR", &root_text);
        std::env::set_var(crate::meido_env::WORKSPACE_DIR.preferred, &root_text);
        let Some(vault) = &self.vault else {
            return;
        };
        for (key, configured) in [
            ("VAULT_VECTOR_DB_PATH", vault.vector_db_path.as_deref()),
            (
                "VAULT_ORIGIN_VAULT_PATH",
                vault.origin_vault_path.as_deref(),
            ),
        ] {
            let Some(configured) = configured.map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            if Path::new(configured).is_absolute() {
                continue;
            }
            std::env::set_var(key, resolve_workspace_data_path(&root, configured));
        }
    }

    /// Chroma directory from `VAULT_VECTOR_DB_PATH` when that variable is set.
    ///
    /// Absolute values are kept. Relative values are joined onto the workspace.
    /// `None` when the variable is unset, so callers keep `{workspace}/shared/vault_db`.
    pub fn vector_db_root(&self) -> Option<PathBuf> {
        let configured = self
            .vault
            .as_ref()
            .and_then(|v| v.vector_db_path.as_deref())
            .map(str::trim)
            .filter(|p| !p.is_empty())?;
        let resolved = resolve_workspace_data_path(&self.workspace_root_absolute(), configured);
        if resolved.as_os_str().is_empty() {
            None
        } else {
            Some(resolved)
        }
    }

    /// Absolute ORIGIN vault. Honors `VAULT_ORIGIN_VAULT_PATH` when set.
    pub fn origin_vault_root(&self) -> PathBuf {
        let configured = self
            .vault
            .as_ref()
            .and_then(|v| v.origin_vault_path.as_deref());
        resolve_origin_vault_root(&self.workspace_root_absolute(), configured)
    }

    /// Absolute path to the workspace root (workspace_dir resolved to absolute).
    pub fn workspace_root_absolute(&self) -> std::path::PathBuf {
        let root = PathBuf::from(&self.workspace_dir);
        if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| root.clone())
                .join(&self.workspace_dir)
        }
    }

    /// Whether web terminal is enabled and allowed in the current runtime environment.
    pub fn web_terminal_effective(&self) -> bool {
        self.web_terminal_enabled
            && (self.web_terminal_allow_in_docker || !crate::background_shell::in_docker())
    }

    /// Whether operators can open a web terminal (enabled, environment OK, auth token configured).
    pub fn web_terminal_available(&self) -> bool {
        self.web_terminal_effective()
            && self
                .web_auth_token
                .as_ref()
                .is_some_and(|t| !t.trim().is_empty())
    }

    /// Resolved Steel API URL (`STEEL_API_URL` env override, else local managed port).
    pub fn steel_api_url(&self) -> String {
        Self::env("STEEL_API_URL").unwrap_or_else(|| {
            crate::steel_browser_sidecar::default_local_steel_api_url(self.steel_api_port)
        })
    }

    /// Resolve path to .env file. `MEIDO_CONFIG` (or legacy `FINALLY_A_VALUE_BOT_CONFIG`) can override.
    pub fn resolve_config_path() -> Result<Option<PathBuf>, FinallyAValueBotError> {
        if let Some(custom) = crate::meido_env::get(crate::meido_env::CONFIG) {
            let p = std::path::Path::new(&custom);
            if p.exists() {
                return Ok(Some(PathBuf::from(custom)));
            }
            return Err(FinallyAValueBotError::Config(format!(
                "MEIDO_CONFIG points to non-existent file: {custom}"
            )));
        }
        if std::path::Path::new("./.env").exists() {
            return Ok(Some(PathBuf::from("./.env")));
        }
        Ok(None)
    }

    /// Load `.env` over the process environment. The file is the operator config; a stale exported key must not hide it.
    fn load_dotenv_file(path: &std::path::Path) -> Result<(), FinallyAValueBotError> {
        dotenvy::from_path_override(path)
            .map_err(|e| FinallyAValueBotError::Config(format!("Failed to load .env: {e}")))?;
        Ok(())
    }

    fn env(key: &str) -> Option<String> {
        std::env::var(key).ok().and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
    }

    fn env_u32(key: &str, default: u32) -> u32 {
        Self::env(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    fn env_u64(key: &str, default: u64) -> u64 {
        Self::env(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    fn env_usize(key: &str, default: usize) -> usize {
        Self::env(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    fn env_u16(key: &str, default: u16) -> u16 {
        Self::env(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    fn env_bool(key: &str, default: bool) -> bool {
        match Self::env(key).as_deref().map(|s| s.to_lowercase()) {
            Some(s) => match s.as_str() {
                "1" | "true" | "yes" => true,
                "0" | "false" | "no" => false,
                _ => default,
            },
            None => default,
        }
    }

    fn env_vec_i64(key: &str) -> Vec<i64> {
        Self::env(key)
            .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    fn env_vec_u64(key: &str) -> Vec<u64> {
        Self::env(key)
            .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_default()
    }
    fn env_vec_string(key: &str) -> Vec<String> {
        Self::env(key)
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_ascii_lowercase())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load config from environment (.env file + process env). Load .env from `MEIDO_CONFIG` path or `./`.
    pub fn load() -> Result<Self, FinallyAValueBotError> {
        if Self::resolve_config_path()?.is_none() {
            crate::dotenv_sync::seed_env_file(std::path::Path::new("./.env")).map_err(|e| {
                FinallyAValueBotError::Config(format!("Could not create .env: {e}"))
            })?;
        }
        let env_path = Self::resolve_config_path()?;
        let load_path = env_path
            .as_deref()
            .unwrap_or(std::path::Path::new("./.env"));
        if load_path.exists() {
            Self::load_dotenv_file(load_path)?;
        } else if env_path.is_none() {
            return Err(FinallyAValueBotError::Config(
                "No .env found. Copy .env.example to .env in the current directory, or set MEIDO_CONFIG to its path.".into(),
            ));
        }

        let mut config = Self::load_from_env();
        config.post_deserialize()?;
        Ok(config)
    }

    /// Load config from a specific .env file path (e.g. for config wizard).
    pub fn load_from_path(path: &std::path::Path) -> Result<Self, FinallyAValueBotError> {
        if path.exists() {
            Self::load_dotenv_file(path)?;
        }
        let mut config = Self::load_from_env();
        config.post_deserialize()?;
        Ok(config)
    }

    /// Build Config from current environment (after dotenvy has loaded .env).
    pub fn load_from_env() -> Self {
        let vault = {
            let has_vault = Self::env("VAULT_ORIGIN_VAULT_PATH").is_some()
                || Self::env("VAULT_VECTOR_DB_PATH").is_some()
                || Self::env("VAULT_ORIGIN_VAULT_REPO").is_some()
                || Self::env("VAULT_GIT_URL").is_some()
                || Self::env("VAULT_EMBEDDING_SERVER_URL").is_some()
                || Self::env("VAULT_VECTOR_DB_URL").is_some();
            if has_vault {
                Some(VaultConfig {
                    origin_vault_path: Self::env("VAULT_ORIGIN_VAULT_PATH"),
                    vector_db_path: Self::env("VAULT_VECTOR_DB_PATH"),
                    origin_vault_repo: Self::env("VAULT_ORIGIN_VAULT_REPO")
                        .or_else(|| Self::env("VAULT_GIT_URL")),
                    embedding_server_url: Self::env("VAULT_EMBEDDING_SERVER_URL"),
                    vault_search_command: Self::env("VAULT_SEARCH_COMMAND"),
                    vault_index_command: Self::env("VAULT_INDEX_COMMAND"),
                    principles_path: Self::env("VAULT_PRINCIPLES_PATH"),
                    vector_db_url: Self::env("VAULT_VECTOR_DB_URL"),
                    vector_db_collection: Self::env("VAULT_VECTOR_DB_COLLECTION"),
                })
            } else {
                None
            }
        };

        let social = {
            let has_social = Self::env("SOCIAL_BASE_URL").is_some()
                || Self::env("SOCIAL_TIKTOK_CLIENT_ID").is_some()
                || Self::env("SOCIAL_INSTAGRAM_CLIENT_ID").is_some()
                || Self::env("SOCIAL_LINKEDIN_CLIENT_ID").is_some();
            if has_social {
                Some(SocialConfig {
                    base_url: Self::env("SOCIAL_BASE_URL"),
                    tiktok: SocialPlatformConfig {
                        client_id: Self::env("SOCIAL_TIKTOK_CLIENT_ID"),
                        client_secret: Self::env("SOCIAL_TIKTOK_CLIENT_SECRET"),
                    },
                    instagram: SocialPlatformConfig {
                        client_id: Self::env("SOCIAL_INSTAGRAM_CLIENT_ID"),
                        client_secret: Self::env("SOCIAL_INSTAGRAM_CLIENT_SECRET"),
                    },
                    linkedin: SocialPlatformConfig {
                        client_id: Self::env("SOCIAL_LINKEDIN_CLIENT_ID"),
                        client_secret: Self::env("SOCIAL_LINKEDIN_CLIENT_SECRET"),
                    },
                })
            } else {
                None
            }
        };

        Config {
            telegram_bot_token: Self::env("TELEGRAM_BOT_TOKEN").unwrap_or_default(),
            bot_username: Self::env("BOT_USERNAME").unwrap_or_default(),
            agent_display_name: Self::env("AGENT_DISPLAY_NAME").unwrap_or_default(),
            llm_provider: default_llm_provider(),
            api_key: Self::env("LLM_API_KEY").unwrap_or_else(default_api_key),
            model: default_model(),
            llm_base_url: Self::env("LLM_BASE_URL"),
            max_tokens: Self::env_u32("MAX_TOKENS", default_max_tokens()),
            max_tool_iterations: Self::env_usize(
                "MAX_TOOL_ITERATIONS",
                default_max_tool_iterations(),
            ),
            max_history_messages: Self::env_usize(
                "MAX_HISTORY_MESSAGES",
                default_max_history_messages(),
            ),
            recent_history_min_user_messages: Self::env_usize(
                "RECENT_HISTORY_MIN_USER_MESSAGES",
                default_recent_history_min_user_messages(),
            ),
            recent_history_min_assistant_messages: Self::env_usize(
                "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES",
                default_recent_history_min_assistant_messages(),
            ),
            max_document_size_mb: Self::env_u64(
                "MAX_DOCUMENT_SIZE_MB",
                default_max_document_size_mb(),
            ),
            workspace_dir: Self::env("WORKSPACE_DIR").unwrap_or_else(default_workspace_dir),
            openai_api_key: Self::env("OPENAI_API_KEY"),
            gemini_api_key: Self::env("GEMINI_API_KEY").or_else(|| Self::env("GOOGLE_API_KEY")),
            timezone: Self::env("TIMEZONE").unwrap_or_else(default_timezone),
            allowed_groups: Self::env_vec_i64("ALLOWED_GROUPS"),
            control_chat_ids: Self::env_vec_i64("CONTROL_CHAT_IDS"),
            whatsapp_access_token: Self::env("WHATSAPP_ACCESS_TOKEN"),
            whatsapp_phone_number_id: Self::env("WHATSAPP_PHONE_NUMBER_ID"),
            whatsapp_verify_token: Self::env("WHATSAPP_VERIFY_TOKEN"),
            whatsapp_webhook_port: Self::env_u16(
                "WHATSAPP_WEBHOOK_PORT",
                default_whatsapp_webhook_port(),
            ),
            wecom_corp_id: Self::env("WECOM_CORP_ID"),
            wecom_corp_secret: Self::env("WECOM_BOT_SECRET").or_else(|| Self::env("WECOM_SECRET")),
            wecom_agent_id: Self::env("WECOM_AGENT_ID")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            wecom_callback_token: Self::env("WECOM_CALLBACK_TOKEN"),
            wecom_encoding_aes_key: Self::env("WECOM_ENCODING_AES_KEY"),
            wecom_webhook_port: Self::env_u16("WECOM_WEBHOOK_PORT", default_wecom_webhook_port()),
            wecom_allowed_chats: Self::env("WECOM_ALLOWED_CHATS")
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            wecom_aibot_id: Self::env("WECOM_BOT_ID"),
            wecom_mode: Self::env("WECOM_MODE").unwrap_or_default(),
            discord_bot_token: Self::env("DISCORD_BOT_TOKEN"),
            discord_allowed_channels: Self::env_vec_u64("DISCORD_ALLOWED_CHANNELS"),
            show_thinking: Self::env_bool("SHOW_THINKING", false),
            llm_thinking_enabled: false,
            web_enabled: Self::env_bool("WEB_ENABLED", default_web_enabled()),
            web_host: Self::env("WEB_HOST").unwrap_or_else(default_web_host),
            web_port: Self::env_u16("WEB_PORT", default_web_port()),
            web_auth_token: Self::env("WEB_AUTH_TOKEN"),
            web_max_inflight_per_session: Self::env_usize(
                "WEB_MAX_INFLIGHT_PER_SESSION",
                default_web_max_inflight_per_session(),
            ),
            web_max_requests_per_window: Self::env_usize(
                "WEB_MAX_REQUESTS_PER_WINDOW",
                default_web_max_requests_per_window(),
            ),
            web_rate_window_seconds: Self::env_u64(
                "WEB_RATE_WINDOW_SECONDS",
                default_web_rate_window_seconds(),
            ),
            web_run_history_limit: Self::env_usize(
                "WEB_RUN_HISTORY_LIMIT",
                default_web_run_history_limit(),
            ),
            web_session_idle_ttl_seconds: Self::env_u64(
                "WEB_SESSION_IDLE_TTL_SECONDS",
                default_web_session_idle_ttl_seconds(),
            ),
            web_terminal_enabled: Self::env_bool("WEB_TERMINAL_ENABLED", false),
            web_terminal_max_sessions: Self::env_usize(
                "WEB_TERMINAL_MAX_SESSIONS",
                default_web_terminal_max_sessions(),
            ),
            web_terminal_idle_timeout_secs: Self::env_u64(
                "WEB_TERMINAL_IDLE_TIMEOUT_SECS",
                default_web_terminal_idle_timeout_secs(),
            ),
            web_terminal_allow_in_docker: Self::env_bool("WEB_TERMINAL_ALLOW_IN_DOCKER", false),
            universal_chat_id: Self::env("UNIVERSAL_CHAT_ID").and_then(|s| s.parse().ok()),
            browser_managed: Self::env_bool("BROWSER_MANAGED", default_browser_managed()),
            steel_api_port: Self::env_u16("STEEL_API_PORT", default_steel_api_port()),
            steel_cdp_port: Self::env_u16("STEEL_CDP_PORT", default_steel_cdp_port()),
            steel_docker_image: Self::env("STEEL_DOCKER_IMAGE")
                .unwrap_or_else(default_steel_docker_image),
            steel_docker_container_name: Self::env("STEEL_DOCKER_CONTAINER_NAME")
                .unwrap_or_else(default_steel_docker_container_name),
            browser_executable_path: Self::env("BROWSER_EXECUTABLE_PATH"),
            browser_cdp_port_base: Self::env_u16(
                "BROWSER_CDP_PORT_BASE",
                default_browser_cdp_port_base(),
            ),
            browser_idle_timeout_secs: Self::env("BROWSER_IDLE_TIMEOUT_SECS")
                .and_then(|s| s.parse().ok()),
            browser_headless: Self::env_bool("BROWSER_HEADLESS", default_browser_headless()),
            safety_output_guard_mode: Self::env("SAFETY_OUTPUT_GUARD_MODE")
                .unwrap_or_else(default_safety_output_guard_mode),
            safety_max_emojis_per_response: Self::env_usize(
                "SAFETY_MAX_EMOJIS_PER_RESPONSE",
                default_safety_max_emojis_per_response(),
            ),
            safety_tail_repeat_limit: Self::env_usize(
                "SAFETY_TAIL_REPEAT_LIMIT",
                default_safety_tail_repeat_limit(),
            ),
            safety_execution_mode: Self::env("SAFETY_EXECUTION_MODE")
                .unwrap_or_else(default_safety_execution_mode),
            safety_risky_categories: {
                let parsed = Self::env_vec_string("SAFETY_RISKY_CATEGORIES");
                if parsed.is_empty() {
                    default_safety_risky_categories()
                } else {
                    parsed
                }
            },
            tavily_api_key: Self::env("TAVILY_API_KEY"),
            web_search_searxng_url: Self::env("SEARXNG_URL"),
            cursor_agent_cli_path: Self::env("CURSOR_AGENT_CLI_PATH")
                .unwrap_or_else(default_cursor_agent_cli_path),
            cursor_agent_model: Self::env("CURSOR_AGENT_MODEL").unwrap_or_default(),
            cursor_agent_timeout_secs: Self::env_u64(
                "CURSOR_AGENT_TIMEOUT_SECS",
                default_cursor_agent_timeout_secs(),
            ),
            social,
            vault,
            post_tool_evaluator_enabled: Self::env_bool(
                "POST_TOOL_EVALUATOR_ENABLED",
                default_post_tool_evaluator_enabled(),
            ),
            post_tool_evaluator_model: Self::env("POST_TOOL_EVALUATOR_MODEL").unwrap_or_default(),
            response_quality_evaluator_enabled: Self::env_bool(
                "RESPONSE_QUALITY_EVALUATOR_ENABLED",
                default_response_quality_evaluator_enabled(),
            ),
            perplexity_api_key: Self::env("PERPLEXITY_API_KEY"),
            evaluator_model: Self::env("EVALUATOR_MODEL").unwrap_or_else(default_evaluator_model),
            evaluator_base_url: Self::env("EVALUATOR_BASE_URL")
                .unwrap_or_else(default_evaluator_base_url),
            response_quality_evaluator_model: Self::env("RESPONSE_QUALITY_EVALUATOR_MODEL")
                .unwrap_or_default(),
            quality_eval_max_nudges_per_run: Self::env_usize(
                "QUALITY_EVAL_MAX_NUDGES_PER_RUN",
                default_quality_eval_max_nudges_per_run(),
            ),
            quality_eval_min_confidence: Self::env("QUALITY_EVAL_MIN_CONFIDENCE")
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_quality_eval_min_confidence),
            quality_eval_channels: Self::env("QUALITY_EVAL_CHANNELS")
                .unwrap_or_else(default_quality_eval_channels),
            hook_command_timeout_secs: Self::env_u64(
                "HOOK_COMMAND_TIMEOUT_SECS",
                default_hook_command_timeout_secs(),
            ),
            hook_prompt_timeout_secs: Self::env_u64(
                "HOOK_PROMPT_TIMEOUT_SECS",
                default_hook_prompt_timeout_secs(),
            ),
            hook_prompt_model: Self::env("HOOK_PROMPT_MODEL").unwrap_or_default(),
            allow_fuzzy_search_replace: Self::env_bool(
                "ALLOW_FUZZY_SEARCH_REPLACE",
                default_allow_fuzzy_search_replace(),
            ),
            symbol_edit_enabled: Self::env_bool(
                "SYMBOL_EDIT_ENABLED",
                default_symbol_edit_enabled(),
            ),
            post_edit_validation_enabled: Self::env_bool(
                "POST_EDIT_VALIDATION_ENABLED",
                default_post_edit_validation_enabled(),
            ),
            post_edit_validation_commands: Self::env("POST_EDIT_VALIDATION_COMMANDS"),
            cursor_agent_tmux_session_prefix: Self::env("CURSOR_AGENT_TMUX_SESSION_PREFIX")
                .unwrap_or_else(default_cursor_agent_tmux_session_prefix),
            cursor_agent_tmux_enabled: Self::env_bool(
                "CURSOR_AGENT_TMUX_ENABLED",
                default_cursor_agent_tmux_enabled(),
            ),
            cursor_agent_runner_url: Self::env("CURSOR_AGENT_RUNNER_URL")
                .filter(|s| !s.trim().is_empty()),
            cursor_sdk_runner_url: Self::env("CURSOR_SDK_RUNNER_URL")
                .filter(|s| !s.trim().is_empty()),
            cursor_sdk_model: Self::env("CURSOR_SDK_MODEL")
                .unwrap_or_else(default_cursor_sdk_model),
            cursor_sdk_auto_start: Self::env_bool(
                "CURSOR_SDK_AUTO_START",
                default_cursor_sdk_auto_start(),
            ),
            cursor_sdk_auto_install: Self::env_bool(
                "CURSOR_SDK_AUTO_INSTALL",
                default_cursor_sdk_auto_install(),
            ),
            cursor_sidecar_max_uptime_secs: Self::env_u64(
                "CURSOR_SIDECAR_MAX_UPTIME_SECS",
                default_cursor_sidecar_max_uptime_secs(),
            )
            .max(300),
            cursor_sdk_runner_port: Self::env("CURSOR_SDK_RUNNER_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_cursor_sdk_runner_port),
            cursor_sdk_python: Self::env("CURSOR_SDK_PYTHON")
                .unwrap_or_else(default_cursor_sdk_python),
            cursor_sdk_node: Self::env("CURSOR_SDK_NODE").unwrap_or_else(default_cursor_sdk_node),
            scheduler_task_timeout_secs: Self::env_u64(
                "SCHEDULER_TASK_TIMEOUT_SECS",
                default_scheduler_task_timeout_secs(),
            ),
            scheduler_stale_running_reclaim_secs: Self::env_u64(
                "SCHEDULER_STALE_RUNNING_RECLAIM_SECS",
                default_scheduler_stale_running_reclaim_secs(),
            ),
            scheduler_max_concurrent_tasks: Self::env_usize(
                "SCHEDULER_MAX_CONCURRENT_TASKS",
                default_scheduler_max_concurrent_tasks(),
            ),
            scheduler_poll_interval_secs: Self::env_u64(
                "SCHEDULER_POLL_INTERVAL_SECS",
                default_scheduler_poll_interval_secs(),
            ),
            topics_enabled: Self::env_bool("TOPICS_ENABLED", default_topics_enabled()),
            topics_min_unsegmented: Self::env_u64(
                "TOPICS_MIN_UNSEGMENTED",
                default_topics_min_unsegmented(),
            )
            .clamp(1, 100),
            topics_idle_minutes: Self::env_u64(
                "TOPICS_IDLE_MINUTES",
                default_topics_idle_minutes(),
            )
            .clamp(1, 24 * 60),
            topics_gap_hours: Self::env_u64("TOPICS_GAP_HOURS", default_topics_gap_hours())
                .clamp(1, 24 * 14),
            topics_prefer_tier2: Self::env_bool(
                "TOPICS_PREFER_TIER2",
                default_topics_prefer_tier2(),
            ),
            topics_debounce_minutes: Self::env_u64(
                "TOPICS_DEBOUNCE_MINUTES",
                default_topics_debounce_minutes(),
            )
            .clamp(1, 24 * 60),
            background_job_lease_ttl_secs: Self::env_u64(
                "BACKGROUND_JOB_LEASE_TTL_SECS",
                default_background_job_lease_ttl_secs(),
            ),
            background_job_lease_fallback_renew_secs: Self::env_u64(
                "BACKGROUND_JOB_LEASE_FALLBACK_RENEW_SECS",
                default_background_job_lease_fallback_renew_secs(),
            ),
            background_job_pending_start_timeout_secs: Self::env_u64(
                "BACKGROUND_JOB_PENDING_START_TIMEOUT_SECS",
                default_background_job_pending_start_timeout_secs(),
            ),
            background_job_notify_chat_progress: Self::env_bool(
                "BACKGROUND_JOB_NOTIFY_CHAT_PROGRESS",
                default_background_job_notify_chat_progress(),
            ),
            tool_output_debug: Self::env_bool("TOOL_OUTPUT_DEBUG", default_tool_output_debug()),
            background_shell_tmux_enabled: Self::env_bool(
                "BACKGROUND_SHELL_TMUX_ENABLED",
                default_background_shell_tmux_enabled(),
            ),
            background_shell_tmux_session_prefix: Self::env("BACKGROUND_SHELL_TMUX_SESSION_PREFIX")
                .unwrap_or_else(default_background_shell_tmux_session_prefix),
            background_shell_monitor_poll_secs: Self::env_u64(
                "BACKGROUND_SHELL_MONITOR_POLL_SECS",
                default_background_shell_monitor_poll_secs(),
            ),
            background_shell_auto_retry_on_failure: Self::env_bool(
                "BACKGROUND_SHELL_AUTO_RETRY_ON_FAILURE",
                default_background_shell_auto_retry_on_failure(),
            ),
            background_shell_auto_retry_max: Self::env_u64(
                "BACKGROUND_SHELL_AUTO_RETRY_MAX",
                default_background_shell_auto_retry_max() as u64,
            ) as u32,
            background_shell_auto_agent_on_success: Self::env_bool(
                "BACKGROUND_SHELL_AUTO_AGENT_ON_SUCCESS",
                default_background_shell_auto_agent_on_success(),
            ),
            runtime_reliability_profile: Self::env("RUNTIME_RELIABILITY_PROFILE")
                .unwrap_or_else(default_runtime_reliability_profile),
            project_auto_association_strictness: Self::env("PROJECT_AUTO_ASSOCIATION_STRICTNESS")
                .unwrap_or_else(default_project_auto_association_strictness),
            mempalace: MemPalaceConfig {
                command: Self::env("MEMPALACE_LIGHT_MCP_COMMAND").unwrap_or_default(),
                args: Self::env("MEMPALACE_LIGHT_MCP_ARGS")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                endpoint: Self::env("MEMPALACE_LIGHT_MCP_ENDPOINT").unwrap_or_default(),
                startup_timeout_secs: Self::env_u64(
                    "MEMPALACE_STARTUP_TIMEOUT_SECS",
                    default_mempalace_startup_timeout_secs(),
                ),
                query_timeout_secs: Self::env_u64(
                    "MEMPALACE_QUERY_TIMEOUT_SECS",
                    default_mempalace_query_timeout_secs(),
                ),
                recall_mode: Self::env("MEMPALACE_RECALL_MODE")
                    .unwrap_or_else(default_mempalace_recall_mode),
                selected_persona_ids: Self::env("MEMPALACE_SELECTED_PERSONA_IDS")
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|part| part.trim().parse().ok())
                    .collect(),
            },
        }
    }

    /// True when WeCom should use 智能机器人 long connection instead of the self-built app callback.
    pub fn wecom_uses_aibot(&self) -> bool {
        let mode = self.wecom_mode.trim().to_ascii_lowercase();
        if matches!(
            mode.as_str(),
            "aibot" | "websocket" | "long_connection" | "long-connection"
        ) {
            return true;
        }
        if mode == "callback" {
            return false;
        }
        self.wecom_aibot_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    }

    /// Canonical contact used by the Web UI and by channels that share that inbox.
    pub fn operator_inbox_chat_id(&self) -> i64 {
        self.universal_chat_id.unwrap_or(DEFAULT_UNIVERSAL_CHAT_ID)
    }

    /// Apply Web UI channel integration settings from SQLite onto this config.
    pub fn merge_channel_integration_from_db(
        &mut self,
        db: &crate::db::Database,
    ) -> Result<(), FinallyAValueBotError> {
        crate::channel_integration_config::merge_into_config(self, db)
    }

    /// Apply Web UI `LLM_PROVIDER` / `LLM_MODEL` / local `LLM_BASE_URL` from `app_settings`.
    pub fn merge_llm_selection_from_app_settings(
        &mut self,
        db: &crate::db::Database,
    ) -> Result<(), FinallyAValueBotError> {
        let settings: Vec<(String, String)> = db
            .list_app_settings()?
            .into_iter()
            .map(|s| (s.key, s.value))
            .collect();
        let mut had_provider_setting = false;
        let mut had_model_setting = false;
        let mut had_base_url_setting = false;
        for (key, value) in &settings {
            if key.eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_PROVIDER) {
                let v = value.trim();
                if !v.is_empty() {
                    self.llm_provider = crate::llm_catalog::resolve_catalog_provider_id(v);
                    had_provider_setting = true;
                }
            }
        }
        for (key, value) in &settings {
            if key.eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_MODEL) {
                let v = value.trim();
                if !v.is_empty() {
                    self.model = v.to_string();
                    had_model_setting = true;
                }
                break;
            }
        }

        for (key, value) in &settings {
            if key.eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_THINKING_ENABLED) {
                self.llm_thinking_enabled = parse_bool_setting(value);
            }
            if key.eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_SHOW_THINKING) {
                self.show_thinking = parse_bool_setting(value);
            }
        }

        let mut persist_defaults = false;
        let env_provider = Self::env("LLM_PROVIDER")
            .map(|value| crate::llm_catalog::resolve_catalog_provider_id(value.trim()))
            .filter(|value| !value.is_empty());
        let env_model = Self::env("LLM_MODEL").filter(|value| !value.trim().is_empty());
        let provider_ready = |provider: &str, cfg: &Self| {
            !provider.trim().is_empty()
                && (crate::llm_catalog::is_local_provider(provider)
                    || !crate::llm_catalog::resolve_api_key_for_provider_with_config(
                        provider,
                        Some(cfg),
                    )
                    .is_empty())
        };
        // The installer writes LLM_PROVIDER. It replaces a saved provider that has no key.
        if let Some(env_provider) = env_provider {
            if !provider_ready(&self.llm_provider, self) {
                if self.llm_provider != env_provider {
                    tracing::info!(
                        "LLM provider {} is not usable; using {} from LLM_PROVIDER.",
                        if self.llm_provider.is_empty() {
                            "(none)"
                        } else {
                            self.llm_provider.as_str()
                        },
                        env_provider
                    );
                }
                self.llm_provider = env_provider;
                self.model = env_model.clone().unwrap_or_default();
                had_provider_setting = false;
                had_model_setting = env_model.is_some();
                persist_defaults = true;
            }
        }
        if !self.llm_provider.is_empty() && !provider_ready(&self.llm_provider, self) {
            if let Some(provider) = crate::llm_catalog::first_cloud_provider_with_api_key() {
                tracing::info!(
                    "LLM provider {} has no API key; using {}.",
                    self.llm_provider,
                    provider
                );
                self.llm_provider = provider.to_string();
                self.model.clear();
                had_provider_setting = false;
                had_model_setting = false;
                persist_defaults = true;
            } else {
                tracing::info!(
                    "LLM provider {} has no API key; leaving the provider unset.",
                    self.llm_provider
                );
                self.llm_provider.clear();
                self.model.clear();
                had_provider_setting = false;
                had_model_setting = false;
                persist_defaults = true;
            }
        }
        if self.llm_provider.is_empty() {
            if let Some(p) = crate::llm_catalog::first_cloud_provider_with_api_key() {
                self.llm_provider = p.to_string();
                persist_defaults = true;
            }
        }
        let provider_selected = !self.llm_provider.trim().is_empty();
        if provider_selected && self.model.is_empty() {
            self.model = crate::llm_catalog::default_model_for_provider(&self.llm_provider).into();
            persist_defaults = true;
        }

        self.sync_active_llm_provider_from_env()?;

        if crate::llm_catalog::is_local_provider(&self.llm_provider) {
            if let Some(url) =
                crate::llm_catalog::local_base_url_from_app_settings(&settings, &self.llm_provider)
            {
                self.llm_base_url = Some(url);
                had_base_url_setting = true;
            } else {
                let url = crate::llm_catalog::effective_local_base_url(&self.llm_provider, None);
                self.llm_base_url = Some(url.clone());
                persist_defaults = true;
            }
        }

        if provider_selected
            && !crate::llm_catalog::is_local_provider(&self.llm_provider)
            && crate::llm_catalog::resolve_api_key_for_provider_with_config(
                &self.llm_provider,
                Some(self),
            )
            .is_empty()
        {
            let hints =
                crate::llm_catalog::provider_api_key_env_hints(&self.llm_provider).join(", ");
            tracing::warn!(
                "No API key in .env for LLM provider {}. Set one of: {hints}. The web UI still starts; add a key under Settings → Connections or switch to the Cursor engine.",
                self.llm_provider
            );
        }

        if provider_selected && (persist_defaults || !had_provider_setting || !had_model_setting) {
            db.set_app_setting(
                crate::llm_catalog::APP_SETTING_LLM_PROVIDER,
                &self.llm_provider,
            )?;
            db.set_app_setting(crate::llm_catalog::APP_SETTING_LLM_MODEL, &self.model)?;
        }
        if crate::llm_catalog::is_local_provider(&self.llm_provider)
            && (persist_defaults || !had_base_url_setting)
        {
            if let Some(ref url) = self.llm_base_url {
                db.set_app_setting(crate::llm_catalog::APP_SETTING_LLM_BASE_URL, url)?;
            }
        }
        Ok(())
    }

    /// Backward-compatible alias.
    pub fn merge_llm_model_from_app_settings(
        &mut self,
        db: &crate::db::Database,
    ) -> Result<(), FinallyAValueBotError> {
        self.merge_llm_selection_from_app_settings(db)
    }

    /// Apply API key and default base URL for the active `llm_provider` from environment / catalog.
    pub fn sync_active_llm_provider_from_env(&mut self) -> Result<(), FinallyAValueBotError> {
        self.llm_provider = crate::llm_catalog::resolve_catalog_provider_id(&self.llm_provider);
        self.api_key = crate::llm_catalog::resolve_api_key_for_provider_with_config(
            &self.llm_provider,
            Some(self),
        );
        if crate::llm_catalog::is_local_provider(&self.llm_provider) {
            // Local server URL is owned by Web UI `app_settings`, not `.env` LLM_BASE_URL.
            return Ok(());
        }
        let base_empty = self
            .llm_base_url
            .as_ref()
            .is_none_or(|u| u.trim().is_empty());
        if base_empty {
            if let Some(url) = crate::llm_catalog::default_base_url_for_provider(&self.llm_provider)
            {
                self.llm_base_url = Some(url.to_string());
            }
        }
        Ok(())
    }

    /// Web UI provider switch: refresh credentials and base URL for the new provider.
    pub fn apply_llm_provider_switch(
        &mut self,
        provider_id: &str,
        model: &str,
        local_base_url: Option<&str>,
    ) {
        self.llm_provider = crate::llm_catalog::resolve_catalog_provider_id(provider_id);
        self.model = model.trim().to_string();
        self.api_key = crate::llm_catalog::resolve_api_key_for_provider_with_config(
            &self.llm_provider,
            Some(self),
        );
        if crate::llm_catalog::is_local_provider(&self.llm_provider) {
            self.llm_base_url = Some(crate::llm_catalog::effective_local_base_url(
                &self.llm_provider,
                local_base_url,
            ));
        } else {
            self.llm_base_url =
                crate::llm_catalog::default_base_url_for_provider(&self.llm_provider)
                    .map(|s| s.to_string());
        }
    }

    /// Apply post-deserialization normalization and validation.
    pub fn post_deserialize(&mut self) -> Result<(), FinallyAValueBotError> {
        self.llm_provider = crate::llm_catalog::resolve_catalog_provider_id(&self.llm_provider);
        self.safety_output_guard_mode = self.safety_output_guard_mode.trim().to_ascii_lowercase();
        self.safety_execution_mode = self.safety_execution_mode.trim().to_ascii_lowercase();
        self.runtime_reliability_profile =
            self.runtime_reliability_profile.trim().to_ascii_lowercase();
        self.project_auto_association_strictness = self
            .project_auto_association_strictness
            .trim()
            .to_ascii_lowercase();
        self.safety_risky_categories = self
            .safety_risky_categories
            .iter()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
            .collect();

        if !self.llm_provider.is_empty() && self.model.is_empty() {
            self.model = crate::llm_catalog::default_model_for_provider(&self.llm_provider).into();
        }

        // Validate timezone
        self.timezone.parse::<chrono_tz::Tz>().map_err(|_| {
            FinallyAValueBotError::Config(format!("Invalid timezone: {}", self.timezone))
        })?;

        // Filter empty llm_base_url
        if let Some(ref url) = self.llm_base_url {
            if url.trim().is_empty() {
                self.llm_base_url = None;
            }
        }
        if self.llm_base_url.is_none() && matches!(self.llm_provider.as_str(), "llama" | "llamacpp")
        {
            self.llm_base_url = Some("http://127.0.0.1:8080/v1".into());
        }
        if !self.llm_provider.is_empty() && self.llm_base_url.is_none() {
            if let Some(url) = crate::llm_catalog::default_base_url_for_provider(&self.llm_provider)
            {
                self.llm_base_url = Some(url.to_string());
            }
        }
        if let Some(dir) = crate::meido_env::get(crate::meido_env::WORKSPACE_DIR) {
            self.workspace_dir = dir;
        }
        if self.workspace_dir.trim().is_empty() {
            self.workspace_dir = default_workspace_dir();
        }
        if self.web_host.trim().is_empty() {
            self.web_host = default_web_host();
        }
        if let Some(token) = &self.web_auth_token {
            if token.trim().is_empty() {
                self.web_auth_token = None;
            }
        }
        if self.web_enabled && !is_local_web_host(&self.web_host) && self.web_auth_token.is_none() {
            return Err(FinallyAValueBotError::Config(
                "web_auth_token is required when web_enabled=true and web_host is not local".into(),
            ));
        }
        if self.web_max_inflight_per_session == 0 {
            self.web_max_inflight_per_session = default_web_max_inflight_per_session();
        }
        if self.web_max_requests_per_window == 0 {
            self.web_max_requests_per_window = default_web_max_requests_per_window();
        }
        if self.web_rate_window_seconds == 0 {
            self.web_rate_window_seconds = default_web_rate_window_seconds();
        }
        if self.web_run_history_limit == 0 {
            self.web_run_history_limit = default_web_run_history_limit();
        }
        if self.web_session_idle_ttl_seconds == 0 {
            self.web_session_idle_ttl_seconds = default_web_session_idle_ttl_seconds();
        }
        if self.web_terminal_max_sessions == 0 {
            self.web_terminal_max_sessions = default_web_terminal_max_sessions();
        }
        if self.web_terminal_idle_timeout_secs == 0 {
            self.web_terminal_idle_timeout_secs = default_web_terminal_idle_timeout_secs();
        }
        if self.web_terminal_enabled
            && self
                .web_auth_token
                .as_ref()
                .map(|t| t.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(FinallyAValueBotError::Config(
                "web_auth_token is required when WEB_TERMINAL_ENABLED=true".into(),
            ));
        }
        // Evaluator model aliases (legacy env names).
        if !self.response_quality_evaluator_model.trim().is_empty() {
            self.evaluator_model = self.response_quality_evaluator_model.trim().to_string();
        }
        if !self.post_tool_evaluator_model.trim().is_empty() {
            self.evaluator_model = self.post_tool_evaluator_model.trim().to_string();
        }
        self.evaluator_model = self.evaluator_model.trim().to_string();
        if self.evaluator_model.is_empty() {
            self.evaluator_model = default_evaluator_model();
        }
        self.evaluator_base_url = self.evaluator_base_url.trim().to_string();
        if self.evaluator_base_url.is_empty() {
            self.evaluator_base_url = default_evaluator_base_url();
        }
        self.quality_eval_channels = self.quality_eval_channels.trim().to_string();
        if self.quality_eval_channels.is_empty() {
            self.quality_eval_channels = default_quality_eval_channels();
        }
        if self.quality_eval_min_confidence.is_nan()
            || !(0.0..=1.0).contains(&self.quality_eval_min_confidence)
        {
            self.quality_eval_min_confidence = default_quality_eval_min_confidence();
        }
        if self.max_document_size_mb == 0 {
            self.max_document_size_mb = default_max_document_size_mb();
        }
        if self.max_history_messages == 0 {
            self.max_history_messages = default_max_history_messages();
        }
        if self.recent_history_min_user_messages == 0 {
            self.recent_history_min_user_messages = default_recent_history_min_user_messages();
        }
        if self.recent_history_min_assistant_messages == 0 {
            self.recent_history_min_assistant_messages =
                default_recent_history_min_assistant_messages();
        }
        self.recent_history_min_user_messages = self.recent_history_min_user_messages.min(25);
        self.recent_history_min_assistant_messages =
            self.recent_history_min_assistant_messages.min(25);
        if self.safety_max_emojis_per_response == 0 {
            self.safety_max_emojis_per_response = default_safety_max_emojis_per_response();
        }
        if self.safety_tail_repeat_limit == 0 {
            self.safety_tail_repeat_limit = default_safety_tail_repeat_limit();
        }
        if self.safety_risky_categories.is_empty() {
            self.safety_risky_categories = default_safety_risky_categories();
        }
        let valid_guard_modes = ["off", "moderate", "strict"];
        if !valid_guard_modes.contains(&self.safety_output_guard_mode.as_str()) {
            return Err(FinallyAValueBotError::Config(format!(
                "Invalid safety_output_guard_mode: {} (expected off|moderate|strict)",
                self.safety_output_guard_mode
            )));
        }
        let valid_exec_modes = ["off", "warn_confirm", "strict"];
        if !valid_exec_modes.contains(&self.safety_execution_mode.as_str()) {
            return Err(FinallyAValueBotError::Config(format!(
                "Invalid safety_execution_mode: {} (expected off|warn_confirm|strict)",
                self.safety_execution_mode
            )));
        }
        let valid_risky_categories = ["destructive", "system", "network", "package"];
        for cat in &self.safety_risky_categories {
            if !valid_risky_categories.contains(&cat.as_str()) {
                return Err(FinallyAValueBotError::Config(format!(
                    "Invalid safety risky category: {} (expected one of destructive,system,network,package)",
                    cat
                )));
            }
        }
        match self.runtime_reliability_profile.as_str() {
            "aggressive_completion" => {
                if self.max_tool_iterations < 80 {
                    self.max_tool_iterations = 80;
                }
                self.post_tool_evaluator_enabled = true;
            }
            "safe_conservative" => {
                self.max_tool_iterations = self.max_tool_iterations.min(60);
                self.post_tool_evaluator_enabled = true;
            }
            _ => {
                self.runtime_reliability_profile = "balanced".to_string();
            }
        }
        if !["strict", "balanced", "loose"]
            .contains(&self.project_auto_association_strictness.as_str())
        {
            self.project_auto_association_strictness =
                default_project_auto_association_strictness();
        }
        if let Some(cmds) = &self.post_edit_validation_commands {
            if cmds.trim().is_empty() {
                self.post_edit_validation_commands = None;
            }
        }
        if let Some(ref mut social) = self.social {
            for platform_cfg in [
                &mut social.tiktok,
                &mut social.instagram,
                &mut social.linkedin,
            ] {
                if let Some(ref id) = platform_cfg.client_id {
                    if id.trim().is_empty() {
                        platform_cfg.client_id = None;
                    }
                }
                if let Some(ref secret) = platform_cfg.client_secret {
                    if secret.trim().is_empty() {
                        platform_cfg.client_secret = None;
                    }
                }
            }
        }

        // Channel tokens are optional at .env load time — validated after DB merge
        // (see channel_integration_config::validate_runtime_channel_or_web).
        if !self.web_enabled
            && !crate::llm_catalog::any_provider_api_key_configured()
            && !matches!(self.llm_provider.as_str(), "ollama" | "llama" | "llamacpp")
        {
            return Err(FinallyAValueBotError::Config(
                "At least one LLM API key is required in .env (e.g. ANTHROPIC_API_KEY, OPENAI_API_KEY). Provider and model are set in Web UI → Settings → LLM.".into(),
            ));
        }

        Ok(())
    }

    /// Save config as YAML to the given path (legacy; prefer save_env).
    #[allow(dead_code)]
    pub fn save_yaml(&self, path: &str) -> Result<(), FinallyAValueBotError> {
        let content = serde_yaml::to_string(self).map_err(|e| {
            FinallyAValueBotError::Config(format!("Failed to serialize config: {e}"))
        })?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Save config as .env to the given path.
    pub fn save_env(&self, path: &std::path::Path) -> Result<(), FinallyAValueBotError> {
        fn esc(s: &str) -> String {
            if s.contains(' ') || s.contains('"') || s.contains('#') || s.is_empty() {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                s.to_string()
            }
        }
        let mut lines = Vec::new();
        lines.push("# Meido configuration".into());
        lines.push("".into());
        lines.push("# Telegram".into());
        lines.push(format!(
            "TELEGRAM_BOT_TOKEN={}",
            esc(&self.telegram_bot_token)
        ));
        lines.push(format!("BOT_USERNAME={}", esc(&self.bot_username)));
        lines.push(format!(
            "AGENT_DISPLAY_NAME={}",
            esc(&self.agent_display_name)
        ));
        lines.push("".into());
        lines.push("# LLM (provider + model: Web UI → Settings → LLM; keys only in .env)".into());
        if !self.api_key.is_empty() {
            lines.push(format!("LLM_API_KEY={}", esc(&self.api_key)));
        }
        if let Some(ref u) = self.llm_base_url {
            if !u.is_empty() {
                lines.push(format!("LLM_BASE_URL={}", esc(u)));
            }
        }
        lines.push(format!("MAX_TOKENS={}", self.max_tokens));
        lines.push(format!("MAX_TOOL_ITERATIONS={}", self.max_tool_iterations));
        lines.push(format!(
            "MAX_HISTORY_MESSAGES={}",
            self.max_history_messages
        ));
        lines.push(format!(
            "RECENT_HISTORY_MIN_USER_MESSAGES={}",
            self.recent_history_min_user_messages
        ));
        lines.push(format!(
            "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES={}",
            self.recent_history_min_assistant_messages
        ));
        lines.push(format!(
            "RUNTIME_RELIABILITY_PROFILE={}",
            esc(&self.runtime_reliability_profile)
        ));
        lines.push(format!(
            "PROJECT_AUTO_ASSOCIATION_STRICTNESS={}",
            esc(&self.project_auto_association_strictness)
        ));
        lines.push(format!(
            "ALLOW_FUZZY_SEARCH_REPLACE={}",
            if self.allow_fuzzy_search_replace {
                "true"
            } else {
                "false"
            }
        ));
        lines.push(format!(
            "SYMBOL_EDIT_ENABLED={}",
            if self.symbol_edit_enabled {
                "true"
            } else {
                "false"
            }
        ));
        lines.push(format!(
            "POST_EDIT_VALIDATION_ENABLED={}",
            if self.post_edit_validation_enabled {
                "true"
            } else {
                "false"
            }
        ));
        if let Some(cmds) = &self.post_edit_validation_commands {
            if !cmds.trim().is_empty() {
                lines.push(format!("POST_EDIT_VALIDATION_COMMANDS={}", esc(cmds)));
            }
        }
        lines.push(format!(
            "MAX_DOCUMENT_SIZE_MB={}",
            self.max_document_size_mb
        ));
        lines.push(format!(
            "SHOW_THINKING={}",
            if self.show_thinking { "true" } else { "false" }
        ));
        lines.push(format!(
            "HOOK_COMMAND_TIMEOUT_SECS={}",
            self.hook_command_timeout_secs
        ));
        lines.push(format!(
            "HOOK_PROMPT_TIMEOUT_SECS={}",
            self.hook_prompt_timeout_secs
        ));
        if !self.hook_prompt_model.trim().is_empty() {
            lines.push(format!(
                "HOOK_PROMPT_MODEL={}",
                esc(&self.hook_prompt_model)
            ));
        }
        lines.push("".into());
        lines.push("# Workspace".into());
        lines.push(format!("WORKSPACE_DIR={}", esc(&self.workspace_dir)));
        lines.push(format!("TIMEZONE={}", esc(&self.timezone)));
        if let Some(id) = self.universal_chat_id {
            lines.push(format!("UNIVERSAL_CHAT_ID={id}"));
        }
        lines.push("".into());
        lines.push("# Web".into());
        lines.push(format!(
            "WEB_ENABLED={}",
            if self.web_enabled { "true" } else { "false" }
        ));
        lines.push(format!("WEB_HOST={}", esc(&self.web_host)));
        lines.push(format!("WEB_PORT={}", self.web_port));
        if let Some(ref token) = self.web_auth_token {
            if !token.is_empty() {
                lines.push(format!("WEB_AUTH_TOKEN={}", esc(token)));
            }
        }
        lines.push(format!(
            "WEB_MAX_INFLIGHT_PER_SESSION={}",
            self.web_max_inflight_per_session
        ));
        lines.push(format!(
            "WEB_MAX_REQUESTS_PER_WINDOW={}",
            self.web_max_requests_per_window
        ));
        lines.push(format!(
            "WEB_RATE_WINDOW_SECONDS={}",
            self.web_rate_window_seconds
        ));
        lines.push(format!(
            "WEB_RUN_HISTORY_LIMIT={}",
            self.web_run_history_limit
        ));
        lines.push(format!(
            "WEB_SESSION_IDLE_TTL_SECONDS={}",
            self.web_session_idle_ttl_seconds
        ));
        lines.push("".into());
        lines.push("# Runtime safety".into());
        lines.push(format!(
            "SAFETY_OUTPUT_GUARD_MODE={}",
            esc(&self.safety_output_guard_mode)
        ));
        lines.push(format!(
            "SAFETY_MAX_EMOJIS_PER_RESPONSE={}",
            self.safety_max_emojis_per_response
        ));
        lines.push(format!(
            "SAFETY_TAIL_REPEAT_LIMIT={}",
            self.safety_tail_repeat_limit
        ));
        lines.push(format!(
            "SAFETY_EXECUTION_MODE={}",
            esc(&self.safety_execution_mode)
        ));
        lines.push(format!(
            "SAFETY_RISKY_CATEGORIES={}",
            esc(&self.safety_risky_categories.join(","))
        ));
        if let Some(ref v) = self.vault {
            lines.push("".into());
            lines.push("# ORIGIN vault".into());
            lines.push(format!(
                "VAULT_ORIGIN_VAULT_PATH={}",
                esc(v.origin_vault_path.as_deref().unwrap_or("shared/ORIGIN"))
            ));
            lines.push(format!(
                "VAULT_VECTOR_DB_PATH={}",
                esc(v.vector_db_path.as_deref().unwrap_or("shared/vault_db"))
            ));
            if let Some(ref r) = v.origin_vault_repo {
                if !r.is_empty() {
                    lines.push(format!("VAULT_ORIGIN_VAULT_REPO={}", esc(r)));
                }
            }
        }
        std::fs::write(path, lines.join("\n"))?;
        Ok(())
    }
}

#[cfg(test)]
pub fn test_config() -> Config {
    Config {
        telegram_bot_token: "tok".into(),
        bot_username: "bot".into(),
        agent_display_name: "bot".into(),
        llm_provider: "anthropic".into(),
        api_key: "key".into(),
        model: "claude-sonnet-4-5-20250929".into(),
        llm_base_url: None,
        max_tokens: 8192,
        max_tool_iterations: 100,
        max_history_messages: 50,
        recent_history_min_user_messages: 2,
        recent_history_min_assistant_messages: 2,
        max_document_size_mb: 100,
        workspace_dir: "./workspace".into(),
        openai_api_key: None,
        gemini_api_key: None,
        timezone: "UTC".into(),
        allowed_groups: vec![],
        control_chat_ids: vec![],
        whatsapp_access_token: None,
        whatsapp_phone_number_id: None,
        whatsapp_verify_token: None,
        whatsapp_webhook_port: 8080,
        wecom_corp_id: None,
        wecom_corp_secret: None,
        wecom_agent_id: 0,
        wecom_callback_token: None,
        wecom_encoding_aes_key: None,
        wecom_webhook_port: 8081,
        wecom_allowed_chats: vec![],
        wecom_aibot_id: None,
        wecom_mode: String::new(),
        discord_bot_token: None,
        discord_allowed_channels: vec![],
        show_thinking: false,
        llm_thinking_enabled: false,
        web_enabled: true,
        web_host: "127.0.0.1".into(),
        web_port: 10961,
        web_auth_token: None,
        web_max_inflight_per_session: 2,
        web_max_requests_per_window: 8,
        web_rate_window_seconds: 10,
        web_run_history_limit: 512,
        web_session_idle_ttl_seconds: 300,
        web_terminal_enabled: false,
        web_terminal_max_sessions: default_web_terminal_max_sessions(),
        web_terminal_idle_timeout_secs: default_web_terminal_idle_timeout_secs(),
        web_terminal_allow_in_docker: false,
        universal_chat_id: None,
        browser_managed: false,
        steel_api_port: default_steel_api_port(),
        steel_cdp_port: default_steel_cdp_port(),
        steel_docker_image: default_steel_docker_image(),
        steel_docker_container_name: default_steel_docker_container_name(),
        browser_executable_path: None,
        browser_cdp_port_base: 9222,
        browser_idle_timeout_secs: None,
        browser_headless: false,
        safety_output_guard_mode: "moderate".into(),
        safety_max_emojis_per_response: 12,
        safety_tail_repeat_limit: 8,
        safety_execution_mode: "warn_confirm".into(),
        safety_risky_categories: vec![
            "destructive".into(),
            "system".into(),
            "network".into(),
            "package".into(),
        ],
        tavily_api_key: None,
        web_search_searxng_url: None,
        cursor_agent_cli_path: default_cursor_agent_cli_path(),
        cursor_agent_model: String::new(),
        cursor_agent_timeout_secs: default_cursor_agent_timeout_secs(),
        social: None,
        vault: None,
        post_tool_evaluator_enabled: false,
        post_tool_evaluator_model: String::new(),
        response_quality_evaluator_enabled: false,
        perplexity_api_key: None,
        evaluator_model: default_evaluator_model(),
        evaluator_base_url: default_evaluator_base_url(),
        response_quality_evaluator_model: String::new(),
        quality_eval_max_nudges_per_run: default_quality_eval_max_nudges_per_run(),
        quality_eval_min_confidence: default_quality_eval_min_confidence(),
        quality_eval_channels: default_quality_eval_channels(),
        hook_command_timeout_secs: default_hook_command_timeout_secs(),
        hook_prompt_timeout_secs: default_hook_prompt_timeout_secs(),
        hook_prompt_model: String::new(),
        allow_fuzzy_search_replace: false,
        symbol_edit_enabled: false,
        post_edit_validation_enabled: true,
        post_edit_validation_commands: None,
        cursor_agent_tmux_session_prefix: "meido-cursor".into(),
        cursor_agent_tmux_enabled: true,
        cursor_agent_runner_url: None,
        cursor_sdk_runner_url: None,
        cursor_sdk_model: default_cursor_sdk_model(),
        cursor_sdk_auto_start: default_cursor_sdk_auto_start(),
        cursor_sdk_auto_install: default_cursor_sdk_auto_install(),
        cursor_sidecar_max_uptime_secs: default_cursor_sidecar_max_uptime_secs(),
        cursor_sdk_runner_port: default_cursor_sdk_runner_port(),
        cursor_sdk_python: default_cursor_sdk_python(),
        cursor_sdk_node: default_cursor_sdk_node(),
        scheduler_task_timeout_secs: default_scheduler_task_timeout_secs(),
        scheduler_stale_running_reclaim_secs: default_scheduler_stale_running_reclaim_secs(),
        scheduler_max_concurrent_tasks: default_scheduler_max_concurrent_tasks(),
        scheduler_poll_interval_secs: default_scheduler_poll_interval_secs(),
        topics_enabled: default_topics_enabled(),
        topics_min_unsegmented: default_topics_min_unsegmented(),
        topics_idle_minutes: default_topics_idle_minutes(),
        topics_gap_hours: default_topics_gap_hours(),
        topics_prefer_tier2: default_topics_prefer_tier2(),
        topics_debounce_minutes: default_topics_debounce_minutes(),
        background_job_lease_ttl_secs: default_background_job_lease_ttl_secs(),
        background_job_lease_fallback_renew_secs: default_background_job_lease_fallback_renew_secs(
        ),
        background_job_pending_start_timeout_secs:
            default_background_job_pending_start_timeout_secs(),
        background_job_notify_chat_progress: default_background_job_notify_chat_progress(),
        tool_output_debug: default_tool_output_debug(),
        background_shell_tmux_enabled: default_background_shell_tmux_enabled(),
        background_shell_tmux_session_prefix: default_background_shell_tmux_session_prefix(),
        background_shell_monitor_poll_secs: default_background_shell_monitor_poll_secs(),
        background_shell_auto_retry_on_failure: default_background_shell_auto_retry_on_failure(),
        background_shell_auto_retry_max: default_background_shell_auto_retry_max(),
        background_shell_auto_agent_on_success: default_background_shell_auto_agent_on_success(),
        runtime_reliability_profile: default_runtime_reliability_profile(),
        project_auto_association_strictness: default_project_auto_association_strictness(),
        mempalace: default_mempalace_config(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_overrides_exported_api_key() {
        let dir =
            std::env::temp_dir().join(format!("meido_dotenv_override_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, "GEMINI_API_KEY=from-file-key\n").unwrap();
        let previous = std::env::var("GEMINI_API_KEY").ok();
        std::env::set_var("GEMINI_API_KEY", "from-shell-key");
        let loaded = Config::load_dotenv_file(&path);
        let seen = std::env::var("GEMINI_API_KEY").ok();
        match previous {
            Some(value) => std::env::set_var("GEMINI_API_KEY", value),
            None => std::env::remove_var("GEMINI_API_KEY"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        loaded.expect("dotenv override");
        assert_eq!(seen.as_deref(), Some("from-file-key"));
    }

    #[test]
    fn web_bind_host_keeps_ips_and_opens_dns_names() {
        assert_eq!(web_bind_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(web_bind_host("localhost"), "localhost");
        assert_eq!(web_bind_host("192.168.1.20"), "192.168.1.20");
        assert_eq!(web_bind_host("pp.local"), "0.0.0.0");
        assert_eq!(web_bind_host("meido.example.com"), "0.0.0.0");
    }

    #[test]
    fn test_config_struct_clone_and_debug() {
        let config = test_config();
        let cloned = config.clone();
        assert_eq!(cloned.telegram_bot_token, "tok");
        assert_eq!(cloned.max_tokens, 8192);
        assert_eq!(cloned.max_tool_iterations, 100);
        assert_eq!(cloned.max_history_messages, 50);
        assert_eq!(cloned.max_document_size_mb, 100);
        assert!(cloned.openai_api_key.is_none());
        assert_eq!(cloned.timezone, "UTC");
        assert!(cloned.allowed_groups.is_empty());
        assert!(cloned.control_chat_ids.is_empty());
        assert_eq!(cloned.max_history_messages, 50);
        assert!(cloned.discord_bot_token.is_none());
        assert!(cloned.discord_allowed_channels.is_empty());
        let _ = format!("{:?}", config);
    }

    #[test]
    fn test_config_default_values() {
        let mut config = test_config();
        config.openai_api_key = Some("sk-test".into());
        config.timezone = "US/Eastern".into();
        config.allowed_groups = vec![123, 456];
        config.control_chat_ids = vec![999];
        assert_eq!(config.model, "claude-sonnet-4-5-20250929");
        assert_eq!(config.workspace_dir, "./workspace");
        assert_eq!(config.openai_api_key.as_deref(), Some("sk-test"));
        assert_eq!(config.timezone, "US/Eastern");
        assert_eq!(config.allowed_groups, vec![123, 456]);
        assert_eq!(config.control_chat_ids, vec![999]);
        assert_eq!(config.safety_output_guard_mode, "moderate");
        assert_eq!(config.safety_max_emojis_per_response, 12);
        assert_eq!(config.safety_tail_repeat_limit, 8);
        assert_eq!(config.safety_execution_mode, "warn_confirm");
        assert_eq!(
            config.safety_risky_categories,
            vec!["destructive", "system", "network", "package"]
        );
    }

    #[test]
    fn test_config_yaml_roundtrip() {
        let config = test_config();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.telegram_bot_token, "tok");
        assert_eq!(parsed.max_tokens, 8192);
        assert_eq!(parsed.llm_provider, "anthropic");
    }

    #[test]
    fn test_config_yaml_defaults() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        // Provider now defaults to empty (set via Web UI / env); post_deserialize
        // leaves it unset when not provided.
        assert_eq!(config.llm_provider, "");
        assert_eq!(config.max_tokens, 8192);
        assert_eq!(config.max_tool_iterations, 100);
        assert_eq!(config.workspace_dir, "./workspace");
        assert_eq!(config.max_document_size_mb, 100);
        assert_eq!(config.timezone, "UTC");
    }

    #[test]
    fn test_post_deserialize_empty_workspace_dir_uses_default() {
        let yaml =
            "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nworkspace_dir: '  '\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.workspace_dir, "./workspace");
    }

    #[test]
    fn test_config_post_deserialize() {
        let yaml =
            "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: ANTHROPIC\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.llm_provider, "anthropic");
        assert_eq!(config.model, "claude-opus-4-7");
    }

    #[test]
    fn test_runtime_and_skills_dirs_from_workspace_dir() {
        let mut config = test_config();
        config.workspace_dir = "./workspace".into();
        let runtime = std::path::PathBuf::from(config.runtime_data_dir());
        let skills = std::path::PathBuf::from(config.skills_data_dir());
        assert!(runtime.ends_with(std::path::Path::new("workspace").join("runtime")));
        assert!(skills.ends_with(std::path::Path::new("workspace").join("skills")));
    }

    #[test]
    fn test_workspace_dir_default() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.workspace_dir, "./workspace");
    }

    #[test]
    fn test_post_deserialize_invalid_timezone() {
        let yaml =
            "telegram_bot_token: tok\nbot_username: bot\napi_key: key\ntimezone: Mars/Olympus\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        let err = config.post_deserialize().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Invalid timezone"));
    }

    #[test]
    fn test_post_deserialize_missing_api_key_allows_local_fallback() {
        // web_enabled defaults to true (web-first); the LLM-key requirement only
        // applies when web is disabled. Even then, a local model catalog is always
        // available, so `any_provider_api_key_configured()` is satisfied and the
        // config is accepted without a cloud API key.
        let yaml = "telegram_bot_token: tok\nbot_username: bot\nweb_enabled: false\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
    }

    #[test]
    fn test_post_deserialize_missing_bot_tokens_allowed_with_web() {
        // Channel tokens are validated after DB merge, not at .env load. Web-only is OK.
        let yaml = "bot_username: bot\napi_key: key\nweb_enabled: true\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert!(config.telegram_bot_token.is_empty());
        assert!(config.discord_bot_token.is_none());
    }

    #[test]
    fn test_post_deserialize_discord_only() {
        let yaml = "bot_username: bot\napi_key: key\ndiscord_bot_token: discord_tok\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        // Should succeed: discord_bot_token is set even though telegram_bot_token is empty
        config.post_deserialize().unwrap();
    }

    #[test]
    fn test_post_deserialize_openai_default_model() {
        let yaml =
            "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: openai\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.model, "gpt-5.4");
        assert_eq!(
            config.llm_base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn test_post_deserialize_xai_default_model_and_base_url() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: xai\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.model, "grok-4.3");
        assert_eq!(config.llm_base_url.as_deref(), Some("https://api.x.ai/v1"));
    }

    #[test]
    fn test_post_deserialize_grok_normalizes_to_xai() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: grok\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.llm_provider, "xai");
        assert_eq!(config.model, "grok-4.3");
    }

    #[test]
    fn test_post_deserialize_openai_api_key_fallback() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\nllm_provider: openai\nopenai_api_key: sk-openai-fallback\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        // openai_api_key is kept in its own field and resolved directly at call
        // time (no longer copied into api_key).
        assert_eq!(config.openai_api_key.as_deref(), Some("sk-openai-fallback"));
        assert_eq!(
            crate::llm_catalog::resolve_api_key_for_provider_with_config("openai", Some(&config)),
            "sk-openai-fallback"
        );
    }

    #[test]
    fn test_post_deserialize_ollama_default_model_and_empty_key() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\nllm_provider: ollama\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.model, "llama3.2");
    }

    #[test]
    fn test_post_deserialize_llama_default_model_base_url_and_empty_key() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\nllm_provider: llama\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.model, "qwen2.5-coder-14b-instruct");
        assert_eq!(
            config.llm_base_url.as_deref(),
            Some("http://127.0.0.1:8080/v1")
        );
    }

    #[test]
    fn test_post_deserialize_empty_base_url_fills_from_catalog_for_openai() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: openai\nllm_base_url: '  '\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(
            config.llm_base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn test_post_deserialize_empty_base_url_stays_none_for_anthropic() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: anthropic\nllm_base_url: '  '\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert!(config.llm_base_url.is_none());
    }

    #[test]
    fn test_post_deserialize_provider_case_insensitive() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nllm_provider: '  ANTHROPIC  '\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.llm_provider, "anthropic");
        assert_eq!(config.model, "claude-opus-4-7");
    }

    #[test]
    fn test_post_deserialize_invalid_safety_output_guard_mode() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nsafety_output_guard_mode: noisy\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        let err = config.post_deserialize().unwrap_err();
        assert!(err.to_string().contains("Invalid safety_output_guard_mode"));
    }

    #[test]
    fn test_post_deserialize_invalid_safety_execution_mode() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nsafety_execution_mode: ask-first\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        let err = config.post_deserialize().unwrap_err();
        assert!(err.to_string().contains("Invalid safety_execution_mode"));
    }

    #[test]
    fn test_post_deserialize_web_non_local_requires_token() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nweb_enabled: true\nweb_host: 0.0.0.0\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        let err = config.post_deserialize().unwrap_err();
        assert!(err
            .to_string()
            .contains("web_auth_token is required when web_enabled=true"));
    }

    #[test]
    fn test_post_deserialize_web_non_local_with_token_ok() {
        let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nweb_enabled: true\nweb_host: 0.0.0.0\nweb_auth_token: token123\n";
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.web_auth_token.as_deref(), Some("token123"));
    }

    #[test]
    fn test_config_yaml_with_all_optional_fields() {
        let yaml = r#"
telegram_bot_token: tok
bot_username: bot
api_key: key
openai_api_key: sk-test
timezone: US/Eastern
allowed_groups: [123, 456]
control_chat_ids: [999]
max_history_messages: 60
whatsapp_access_token: wa_token
whatsapp_phone_number_id: phone_id
whatsapp_verify_token: verify
whatsapp_webhook_port: 9090
discord_bot_token: discord_tok
discord_allowed_channels: [111, 222]
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert_eq!(config.openai_api_key.as_deref(), Some("sk-test"));
        assert_eq!(config.timezone, "US/Eastern");
        assert_eq!(config.allowed_groups, vec![123, 456]);
        assert_eq!(config.control_chat_ids, vec![999]);
        assert_eq!(config.max_history_messages, 60);
        assert_eq!(config.whatsapp_webhook_port, 9090);
        assert_eq!(config.discord_allowed_channels, vec![111, 222]);
    }

    #[test]
    fn test_config_save_yaml() {
        let config = test_config();
        let dir = std::env::temp_dir();
        let path = dir.join("meido_test_config.yaml");
        config.save_yaml(path.to_str().unwrap()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("telegram_bot_token"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_config_save_env_includes_runtime_and_web_keys() {
        let mut config = test_config();
        config.max_tokens = 4096;
        config.max_tool_iterations = 55;
        config.max_document_size_mb = 77;
        config.show_thinking = true;
        config.web_enabled = true;
        config.web_host = "0.0.0.0".into();
        config.web_port = 11999;
        config.web_auth_token = Some("secret123".into());
        config.web_max_inflight_per_session = 4;
        config.web_max_requests_per_window = 12;
        config.web_rate_window_seconds = 30;
        config.web_run_history_limit = 900;
        config.web_session_idle_ttl_seconds = 600;

        let dir = std::env::temp_dir();
        let path = dir.join("meido_test_config.env");
        config.save_env(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("MAX_TOKENS=4096"));
        assert!(content.contains("MAX_TOOL_ITERATIONS=55"));
        assert!(content.contains("MAX_DOCUMENT_SIZE_MB=77"));
        assert!(content.contains("SHOW_THINKING=true"));
        assert!(content.contains("WEB_ENABLED=true"));
        assert!(content.contains("WEB_HOST=0.0.0.0"));
        assert!(content.contains("WEB_PORT=11999"));
        assert!(content.contains("WEB_AUTH_TOKEN=secret123"));
        assert!(content.contains("WEB_MAX_INFLIGHT_PER_SESSION=4"));
        assert!(content.contains("WEB_MAX_REQUESTS_PER_WINDOW=12"));
        assert!(content.contains("WEB_RATE_WINDOW_SECONDS=30"));
        assert!(content.contains("WEB_RUN_HISTORY_LIMIT=900"));
        assert!(content.contains("WEB_SESSION_IDLE_TTL_SECONDS=600"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_config_new_framework_fields_from_yaml() {
        let yaml = r#"
telegram_bot_token: tok
bot_username: bot
api_key: key
allow_fuzzy_search_replace: true
symbol_edit_enabled: true
post_edit_validation_enabled: false
post_edit_validation_commands: "echo one ;; echo two"
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert!(config.allow_fuzzy_search_replace);
        assert!(config.symbol_edit_enabled);
        assert!(!config.post_edit_validation_enabled);
        assert_eq!(
            config.post_edit_validation_commands.as_deref(),
            Some("echo one ;; echo two")
        );
    }

    #[test]
    fn test_post_deserialize_empty_post_edit_validation_commands_none() {
        let yaml = r#"
telegram_bot_token: tok
bot_username: bot
api_key: key
post_edit_validation_commands: "   "
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.post_deserialize().unwrap();
        assert!(config.post_edit_validation_commands.is_none());
    }
}
