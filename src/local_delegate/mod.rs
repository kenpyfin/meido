//! Local delegate: inverse cost routing for Classic · Cost routing engine.
//!
//! Strategy (cloud) handles mutations; local OpenAI-compat handles read-only discovery
//! and bounded delegate sub-jobs. Configuration lives in `app_settings`.

pub mod subjob;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::config::Config;
use crate::db::Database;
use crate::error::FinallyAValueBotError;
use crate::llm::{create_openai_compatible_provider, LlmProvider};
use crate::runtime_toggles::AgentEngine;

// DB keys (MULTIMODEL_* kept for migration)
pub const APP_SETTING_ROUTING_ENABLED: &str = "MULTIMODEL_ENABLED";
pub const APP_SETTING_LOCAL_BASE_URL: &str = "MULTIMODEL_LOCAL_BASE_URL";
pub const APP_SETTING_LOCAL_MODEL: &str = "MULTIMODEL_LOCAL_MODEL";
pub const APP_SETTING_LOCAL_TOOLS_OK: &str = "MULTIMODEL_LOCAL_TOOLS_OK";
pub const APP_SETTING_TIER1_BASE_URL: &str = "MULTIMODEL_TIER1_BASE_URL";
pub const APP_SETTING_TIER1_MODEL: &str = "MULTIMODEL_TIER1_MODEL";
pub const APP_SETTING_TIER2_BASE_URL: &str = "MULTIMODEL_TIER2_BASE_URL";
pub const APP_SETTING_TIER2_MODEL: &str = "MULTIMODEL_TIER2_MODEL";
pub const APP_SETTING_TIER1_TOOLS_OK: &str = "MULTIMODEL_TIER1_TOOLS_OK";
pub const APP_SETTING_TIER2_TOOLS_OK: &str = "MULTIMODEL_TIER2_TOOLS_OK";

pub const DEFAULT_LOCAL_BASE_URL: &str = "http://127.0.0.1:8080/v1";
pub const DEFAULT_LOCAL_MODEL: &str = "qwen2.5-coder-14b-instruct";

// ---------------------------------------------------------------------------
// Route target
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTarget {
    Strategy,
    LocalReadOnly,
}

impl RouteTarget {
    pub fn is_local(self) -> bool {
        matches!(self, Self::LocalReadOnly)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Strategy => "strategy",
            Self::LocalReadOnly => "local_readonly",
        }
    }
}

/// Compat alias for older call sites.
pub type ModelTier = RouteTarget;

impl RouteTarget {
    pub const LOCAL: Self = Self::LocalReadOnly;
}

// ---------------------------------------------------------------------------
// Endpoint snapshot (observability)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEndpointSnapshot {
    pub target: RouteTarget,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
}

impl RouteEndpointSnapshot {
    pub fn format_tier_line(&self) -> String {
        format!(
            "Model tier: {} | provider: {} | model: {} | endpoint: {}",
            self.target.label(),
            self.provider,
            self.model,
            self.endpoint
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tier": self.target.label(),
            "provider": self.provider,
            "model": self.model,
            "endpoint": self.endpoint,
        })
    }
}

pub type TierEndpointSnapshot = RouteEndpointSnapshot;

// ---------------------------------------------------------------------------
// Run summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LocalDelegateRunSummary {
    pub cost_routing_active: bool,
    pub strategy_provider: String,
    pub strategy_model: String,
    pub strategy_endpoint: String,
    pub local_model: String,
    pub local_endpoint: String,
}

pub type MultimodelRunSummary = LocalDelegateRunSummary;

impl LocalDelegateRunSummary {
    pub fn for_cursor_sidecar(model: &str, runner_url: &str) -> Self {
        let model = if model.trim().is_empty() {
            "composer-2.5"
        } else {
            model.trim()
        };
        Self {
            cost_routing_active: false,
            strategy_provider: "cursor_sdk".into(),
            strategy_model: model.to_string(),
            strategy_endpoint: runner_url.trim().to_string(),
            local_model: String::new(),
            local_endpoint: String::new(),
        }
    }

    pub fn format_markdown_block(&self) -> String {
        if self.strategy_provider == "cursor_sdk" {
            return format!(
                "Cursor sidecar: {} @ {}\n",
                self.strategy_model,
                if self.strategy_endpoint.is_empty() {
                    "(runner unset)"
                } else {
                    self.strategy_endpoint.as_str()
                },
            );
        }
        if self.cost_routing_active {
            format!(
                "Local delegate: cost routing active\n\
                 - Local (read-only): {} @ {}\n\
                 - Strategy: {} / {} @ {}\n",
                self.local_model,
                self.local_endpoint,
                self.strategy_provider,
                self.strategy_model,
                self.strategy_endpoint,
            )
        } else {
            format!(
                "Local delegate: inactive (strategy only: {} / {} @ {})\n",
                self.strategy_provider, self.strategy_model, self.strategy_endpoint,
            )
        }
    }

    pub fn routing_v1_json(&self, iter0: &RouteEndpointSnapshot) -> serde_json::Value {
        serde_json::json!({
            "cost_routing_active": self.cost_routing_active,
            "multimodel_enabled": self.cost_routing_active,
            "iter0": iter0.to_json(),
            "tiers": {
                "local": {
                    "tier": "local_readonly",
                    "provider": "llama",
                    "model": self.local_model,
                    "endpoint": self.local_endpoint,
                },
                "strategy": {
                    "tier": "strategy",
                    "provider": self.strategy_provider,
                    "model": self.strategy_model,
                    "endpoint": self.strategy_endpoint,
                },
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalDelegateConfig {
    pub routing_enabled: bool,
    pub local_base_url: String,
    pub local_model: String,
    pub local_tools_ok: bool,
    pub tier1_base_url: String,
    pub tier1_model: String,
    pub tier1_tools_ok: bool,
    pub tier2_base_url: String,
    pub tier2_model: String,
    pub tier2_tools_ok: bool,
}

pub type MultimodelConfig = LocalDelegateConfig;

impl LocalDelegateConfig {
    pub fn local_configured(&self) -> bool {
        !self.local_base_url.trim().is_empty() && !self.local_model.trim().is_empty()
    }

    pub fn local_routable(&self) -> bool {
        self.local_configured() && self.local_tools_ok
    }

    pub fn routing_enabled(&self) -> bool {
        self.routing_enabled
    }

    /// Legacy alias for [`Self::local_routable`].
    pub fn ready_for_routing(&self) -> bool {
        self.routing_enabled && self.local_routable()
    }

    pub fn tier1_configured(&self) -> bool {
        !self.tier1_base_url.trim().is_empty() && !self.tier1_model.trim().is_empty()
    }

    pub fn tier2_configured(&self) -> bool {
        !self.tier2_base_url.trim().is_empty() && !self.tier2_model.trim().is_empty()
    }

    pub fn normalize(mut self) -> Self {
        self.local_base_url = normalize_base_url(&self.local_base_url, "");
        self.local_model = self.local_model.trim().to_string();
        self.tier1_base_url = normalize_base_url(&self.tier1_base_url, "");
        self.tier2_base_url = normalize_base_url(&self.tier2_base_url, "");
        self.tier1_model = self.tier1_model.trim().to_string();
        self.tier2_model = self.tier2_model.trim().to_string();
        self
    }
}

pub fn cost_routing_active(engine: AgentEngine, cfg: &LocalDelegateConfig) -> bool {
    engine == AgentEngine::ClassicCostRouting && cfg.local_configured() && cfg.local_tools_ok
}

pub fn from_persona(persona: &crate::db::Persona) -> LocalDelegateConfig {
    LocalDelegateConfig {
        routing_enabled: true,
        local_base_url: persona.local_delegate_base_url.clone().unwrap_or_default(),
        local_model: persona.local_delegate_model.clone().unwrap_or_default(),
        local_tools_ok: persona.local_delegate_tools_ok.unwrap_or(false),
        ..LocalDelegateConfig::default()
    }
    .normalize()
}

pub fn cost_routing_effective(engine: AgentEngine, cfg: &LocalDelegateConfig) -> bool {
    engine == AgentEngine::ClassicCostRouting && cfg.local_routable()
}

// ---------------------------------------------------------------------------
// Tool classification
// ---------------------------------------------------------------------------

pub fn is_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "bash"
            | "write_file"
            | "edit_file"
            | "apply_search_replace"
            | "symbol_edit"
            | "spawn_background_command"
            | "schedule_task"
            | "write_tiered_memory"
            | "write_memory_state"
            | "patch_memory_state"
            | "update_bulletin_focus"
            | "add_todo"
            | "complete_todo"
            | "write_memory"
            | "send_message"
            | "vault_add"
            | "register_tracked_job"
            | "run_skill_script"
            | "delegate_local_subjob"
    )
}

pub fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "read_repo_map"
            | "glob"
            | "grep"
            | "read_memory"
            | "read_memory_state"
            | "validate_memory_state"
            | "read_tiered_memory"
            | "search_chat_history"
            | "search_vault"
            | "recall"
            | "locate_file"
            | "web_search"
            | "web_fetch"
            | "web_html"
            | "read_agent_history"
            | "list_scheduled_tasks"
            | "get_task_history"
            | "list_todos"
            | "agent_history"
            | "search_history"
            | "export_chat"
    )
}

// ---------------------------------------------------------------------------
// Inverse routing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct InverseRouteContext<'a> {
    pub iteration: usize,
    pub last_iteration_tools: &'a [String],
    pub max_iterations: usize,
    pub local_error_streak: u32,
}

pub fn resolve_inverse_route(
    engine: AgentEngine,
    cfg: &LocalDelegateConfig,
    ctx: &InverseRouteContext<'_>,
) -> RouteTarget {
    if !cost_routing_active(engine, cfg) {
        return RouteTarget::Strategy;
    }
    if ctx.iteration == 0 {
        return RouteTarget::Strategy;
    }
    if ctx.iteration + 1 >= ctx.max_iterations {
        return RouteTarget::Strategy;
    }
    if ctx.local_error_streak > 0 {
        return RouteTarget::Strategy;
    }
    if ctx.last_iteration_tools.is_empty() {
        return RouteTarget::Strategy;
    }
    if ctx
        .last_iteration_tools
        .iter()
        .all(|t| is_read_only_tool(t))
    {
        return RouteTarget::LocalReadOnly;
    }
    RouteTarget::Strategy
}

pub fn resolve_local_evaluator_endpoint(cfg: &LocalDelegateConfig) -> Option<(String, String)> {
    if cfg.local_configured() {
        Some((cfg.local_base_url.clone(), cfg.local_model.clone()))
    } else if cfg.tier1_configured() {
        Some((cfg.tier1_base_url.clone(), cfg.tier1_model.clone()))
    } else if cfg.tier2_configured() {
        Some((cfg.tier2_base_url.clone(), cfg.tier2_model.clone()))
    } else {
        None
    }
}

pub fn normalize_base_url_for_provider(raw: &str, fallback: &str) -> String {
    normalize_base_url(raw, fallback)
}

fn normalize_base_url(raw: &str, fallback: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return fallback.to_string();
    }
    let base = t.trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_string()
    } else {
        format!("{base}/v1")
    }
}

// ---------------------------------------------------------------------------
// DB persistence
// ---------------------------------------------------------------------------

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

pub fn load_from_db(db: &Database) -> Result<LocalDelegateConfig, FinallyAValueBotError> {
    let rows: Vec<(String, String)> = db
        .list_app_settings()?
        .into_iter()
        .map(|s| (s.key, s.value))
        .collect();
    let mut cfg = LocalDelegateConfig::default();

    if let Some(v) = read_setting(&rows, APP_SETTING_ROUTING_ENABLED) {
        cfg.routing_enabled = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_LOCAL_BASE_URL) {
        cfg.local_base_url = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_LOCAL_MODEL) {
        cfg.local_model = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_LOCAL_TOOLS_OK) {
        cfg.local_tools_ok = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER1_BASE_URL) {
        cfg.tier1_base_url = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER1_MODEL) {
        cfg.tier1_model = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER2_BASE_URL) {
        cfg.tier2_base_url = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER2_MODEL) {
        cfg.tier2_model = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER1_TOOLS_OK) {
        cfg.tier1_tools_ok = parse_bool(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_TIER2_TOOLS_OK) {
        cfg.tier2_tools_ok = parse_bool(&v);
    }

    if cfg.local_base_url.is_empty() && !cfg.tier1_base_url.is_empty() {
        cfg.local_base_url = cfg.tier1_base_url.clone();
        cfg.local_model = cfg.tier1_model.clone();
        cfg.local_tools_ok = cfg.tier1_tools_ok;
    }

    Ok(cfg.normalize())
}

pub fn persist_to_db(
    db: &Database,
    cfg: &LocalDelegateConfig,
) -> Result<(), FinallyAValueBotError> {
    let cfg = cfg.clone().normalize();
    db.set_app_setting(
        APP_SETTING_ROUTING_ENABLED,
        if cfg.routing_enabled { "true" } else { "false" },
    )?;
    db.set_app_setting(APP_SETTING_LOCAL_BASE_URL, &cfg.local_base_url)?;
    db.set_app_setting(APP_SETTING_LOCAL_MODEL, &cfg.local_model)?;
    db.set_app_setting(
        APP_SETTING_LOCAL_TOOLS_OK,
        if cfg.local_tools_ok { "true" } else { "false" },
    )?;
    db.set_app_setting(APP_SETTING_TIER1_BASE_URL, &cfg.tier1_base_url)?;
    db.set_app_setting(APP_SETTING_TIER1_MODEL, &cfg.tier1_model)?;
    db.set_app_setting(APP_SETTING_TIER2_BASE_URL, &cfg.tier2_base_url)?;
    db.set_app_setting(APP_SETTING_TIER2_MODEL, &cfg.tier2_model)?;
    db.set_app_setting(
        APP_SETTING_TIER1_TOOLS_OK,
        if cfg.tier1_tools_ok { "true" } else { "false" },
    )?;
    db.set_app_setting(
        APP_SETTING_TIER2_TOOLS_OK,
        if cfg.tier2_tools_ok { "true" } else { "false" },
    )?;
    Ok(())
}

pub fn persist_local_tools_ok(db: &Database, ok: bool) -> Result<(), FinallyAValueBotError> {
    db.set_app_setting(
        APP_SETTING_LOCAL_TOOLS_OK,
        if ok { "true" } else { "false" },
    )
}

pub fn persist_tier_tools_ok(
    db: &Database,
    target: RouteTarget,
    ok: bool,
) -> Result<(), FinallyAValueBotError> {
    match target {
        RouteTarget::LocalReadOnly => persist_local_tools_ok(db, ok),
        RouteTarget::Strategy => Ok(()),
    }
}

pub fn tool_choice_for_target(target: RouteTarget, has_tools: bool) -> Option<String> {
    if !has_tools {
        return None;
    }
    match target {
        RouteTarget::LocalReadOnly => Some("auto".to_string()),
        RouteTarget::Strategy => None,
    }
}

pub fn tool_choice_for_tier(tier: RouteTarget, has_tools: bool) -> Option<String> {
    tool_choice_for_target(tier, has_tools)
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

pub struct LocalDelegateRuntime {
    pub config: LocalDelegateConfig,
    local: Arc<dyn LlmProvider>,
}

pub type MultimodelRuntime = LocalDelegateRuntime;

impl LocalDelegateRuntime {
    pub fn new(base_config: &Config, config: LocalDelegateConfig) -> Self {
        let config = config.normalize();
        let local = Arc::from(create_openai_compatible_provider(
            base_config,
            &config.local_base_url,
            &config.local_model,
        ));
        Self { config, local }
    }

    pub fn provider_for_target(
        &self,
        target: RouteTarget,
        strategy: &Arc<dyn LlmProvider>,
    ) -> Arc<dyn LlmProvider> {
        if target.is_local() {
            self.local.clone()
        } else {
            strategy.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Web API request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LocalDelegatePatchRequest {
    #[serde(default)]
    pub routing_enabled: Option<bool>,
    #[serde(default, alias = "enabled")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub local_base_url: Option<String>,
    #[serde(default)]
    pub local_model: Option<String>,
    #[serde(default)]
    pub tier1_base_url: Option<String>,
    #[serde(default)]
    pub tier1_model: Option<String>,
    #[serde(default)]
    pub tier2_base_url: Option<String>,
    #[serde(default)]
    pub tier2_model: Option<String>,
}

pub type MultimodelPatchRequest = LocalDelegatePatchRequest;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDelegateTestTier {
    Local,
}

pub type MultimodelTestTier = LocalDelegateTestTier;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_toggles::AgentEngine;

    fn routable_cfg() -> LocalDelegateConfig {
        LocalDelegateConfig {
            routing_enabled: true,
            local_base_url: DEFAULT_LOCAL_BASE_URL.into(),
            local_model: DEFAULT_LOCAL_MODEL.into(),
            local_tools_ok: true,
            ..Default::default()
        }
    }

    #[test]
    fn inverse_route_first_iteration_strategy() {
        let cfg = routable_cfg();
        let ctx = InverseRouteContext {
            iteration: 0,
            last_iteration_tools: &[],
            max_iterations: 10,
            local_error_streak: 0,
        };
        assert_eq!(
            resolve_inverse_route(AgentEngine::ClassicCostRouting, &cfg, &ctx),
            RouteTarget::Strategy
        );
    }

    #[test]
    fn inverse_route_read_only_chain_goes_local() {
        let cfg = routable_cfg();
        let tools = vec!["grep".into(), "read_file".into()];
        let ctx = InverseRouteContext {
            iteration: 2,
            last_iteration_tools: &tools,
            max_iterations: 10,
            local_error_streak: 0,
        };
        assert_eq!(
            resolve_inverse_route(AgentEngine::ClassicCostRouting, &cfg, &ctx),
            RouteTarget::LocalReadOnly
        );
    }

    #[test]
    fn inverse_route_mutation_stays_strategy() {
        let cfg = routable_cfg();
        let tools = vec!["bash".into(), "read_file".into()];
        let ctx = InverseRouteContext {
            iteration: 2,
            last_iteration_tools: &tools,
            max_iterations: 10,
            local_error_streak: 0,
        };
        assert_eq!(
            resolve_inverse_route(AgentEngine::ClassicCostRouting, &cfg, &ctx),
            RouteTarget::Strategy
        );
    }

    #[test]
    fn single_turn_engine_never_routes_local() {
        let cfg = routable_cfg();
        let tools = vec!["grep".into()];
        let ctx = InverseRouteContext {
            iteration: 2,
            last_iteration_tools: &tools,
            max_iterations: 10,
            local_error_streak: 0,
        };
        assert_eq!(
            resolve_inverse_route(AgentEngine::Classic, &cfg, &ctx),
            RouteTarget::Strategy
        );
    }

    #[test]
    fn cost_routing_active_requires_engine_and_tools_ok() {
        let cfg = LocalDelegateConfig {
            routing_enabled: true,
            local_base_url: DEFAULT_LOCAL_BASE_URL.into(),
            local_model: DEFAULT_LOCAL_MODEL.into(),
            local_tools_ok: false,
            ..Default::default()
        };
        assert!(!cost_routing_active(AgentEngine::ClassicCostRouting, &cfg));
    }

    #[test]
    fn cursor_sidecar_summary_does_not_mention_classic_strategy() {
        let summary =
            LocalDelegateRunSummary::for_cursor_sidecar("composer-2.5", "http://127.0.0.1:3848");
        let md = summary.format_markdown_block();
        assert!(md.contains("Cursor sidecar: composer-2.5 @ http://127.0.0.1:3848"));
        assert!(!md.contains("Local delegate"));
        assert!(!md.to_lowercase().contains("gemini"));
    }
}
