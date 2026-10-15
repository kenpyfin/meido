use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::claude::{
    CacheControl, ContentBlock, ImageSource, Message, MessageContent, MessagesRequest,
    MessagesResponse, ResponseContentBlock, SystemBlock, SystemContent, ToolDefinition, Usage,
};
use crate::config::Config;
use crate::error::FinallyAValueBotError;

const LLM_HTTP_CONNECT_TIMEOUT_SECS: u64 = 5;
const LLM_HTTP_REQUEST_TIMEOUT_SECS: u64 = 120;
/// Settings / persona connection probes (not full agent turns).
const LLM_PROBE_TIMEOUT_SECS: u64 = 15;
const LLM_TEST_MAX_TOKENS: u32 = 8;

fn build_llm_http_client(request_timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(
            LLM_HTTP_CONNECT_TIMEOUT_SECS,
        ))
        .timeout(request_timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn default_llm_http_client() -> reqwest::Client {
    build_llm_http_client(std::time::Duration::from_secs(
        LLM_HTTP_REQUEST_TIMEOUT_SECS,
    ))
}

fn probe_llm_http_client() -> reqwest::Client {
    build_llm_http_client(std::time::Duration::from_secs(LLM_PROBE_TIMEOUT_SECS))
}

fn llm_error_body_preview(body: &str, max_len: usize) -> String {
    if body.len() <= max_len {
        body.to_string()
    } else {
        format!("{}...", &body[..max_len])
    }
}

/// Remove orphaned `ToolResult` blocks whose `tool_use_id` does not match any
/// `ToolUse` block in the conversation.  This can happen after session
/// compaction splits a tool_use / tool_result pair.
fn sanitize_messages(messages: Vec<Message>) -> Vec<Message> {
    // Collect all tool_use IDs from assistant messages (owned to avoid borrow conflicts).
    let known_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();

    messages
        .into_iter()
        .filter_map(|msg| {
            if msg.role != "user" {
                return Some(msg);
            }
            match msg.content {
                MessageContent::Blocks(blocks) => {
                    let filtered: Vec<ContentBlock> = blocks
                        .into_iter()
                        .filter(|b| match b {
                            ContentBlock::ToolResult { tool_use_id, .. } => {
                                known_ids.contains(tool_use_id)
                            }
                            _ => true,
                        })
                        .collect();
                    if filtered.is_empty() {
                        None // Drop entirely empty user messages
                    } else {
                        Some(Message {
                            role: msg.role,
                            content: MessageContent::Blocks(filtered),
                        })
                    }
                }
                other => Some(Message {
                    role: msg.role,
                    content: other,
                }),
            }
        })
        .collect()
}

#[derive(Default)]
struct SseEventParser {
    pending: String,
    data_lines: Vec<String>,
}

impl SseEventParser {
    fn push_chunk(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.pending.find('\n') {
            let mut line = self.pending[..pos].to_string();
            self.pending = self.pending[pos + 1..].to_string();
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event_data) = self.handle_line(&line) {
                events.push(event_data);
            }
        }

        events
    }

    fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.pending.is_empty() {
            let mut line = std::mem::take(&mut self.pending);
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event_data) = self.handle_line(&line) {
                events.push(event_data);
            }
        }
        if let Some(event_data) = self.flush_event() {
            events.push(event_data);
        }
        events
    }

    fn handle_line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            return self.flush_event();
        }
        if line.starts_with(':') {
            return None;
        }

        let (field, value) = match line.split_once(':') {
            Some((f, v)) => {
                let v = v.strip_prefix(' ').unwrap_or(v);
                (f, v)
            }
            None => (line, ""),
        };

        if field == "data" {
            self.data_lines.push(value.to_string());
        }
        None
    }

    fn flush_event(&mut self) -> Option<String> {
        if self.data_lines.is_empty() {
            return None;
        }
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        Some(data)
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Per-request options for LLM calls (local OpenAI-compat tiers use `tool_choice`).
#[derive(Debug, Clone, Default)]
pub struct LlmSendOptions {
    /// OpenAI `tool_choice`: `"required"`, `"auto"`, `"none"`, or a function name.
    pub tool_choice: Option<String>,
}

impl LlmSendOptions {
    pub fn with_tool_choice(mut self, choice: impl Into<String>) -> Self {
        self.tool_choice = Some(choice.into());
        self
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn send_message(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError>;

    async fn send_message_with_options(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        options: LlmSendOptions,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let _ = options;
        self.send_message(system, messages, tools).await
    }

    async fn send_message_stream(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let response = self.send_message(system, messages, tools).await?;
        if let Some(tx) = text_tx {
            for block in &response.content {
                if let ResponseContentBlock::Text { text } = block {
                    let _ = tx.send(text.clone());
                }
            }
        }
        Ok(response)
    }
}

pub fn create_provider(config: &Config) -> Box<dyn LlmProvider> {
    match config.llm_provider.trim().to_lowercase().as_str() {
        "anthropic" => Box::new(AnthropicProvider::new(config)),
        "google" | "gemini" => Box::new(GeminiProvider::new(config)),
        _ => Box::new(OpenAiProvider::new(config)),
    }
}

/// OpenAI-compatible client for local llama.cpp / Ollama tiers (multi-model routing).
pub fn create_openai_compatible_provider(
    base_config: &Config,
    base_url: &str,
    model: &str,
) -> Box<dyn LlmProvider> {
    create_openai_compatible_provider_with_timeout(
        base_config,
        base_url,
        model,
        std::time::Duration::from_secs(LLM_HTTP_REQUEST_TIMEOUT_SECS),
    )
}

/// Same as [`create_openai_compatible_provider`] but with a custom HTTP request timeout
/// (e.g. run optimizer jobs that may run long on local 30B + tool-use).
pub fn create_openai_compatible_provider_with_timeout(
    base_config: &Config,
    base_url: &str,
    model: &str,
    request_timeout: std::time::Duration,
) -> Box<dyn LlmProvider> {
    let mut cfg = base_config.clone();
    cfg.llm_provider = "llama".into();
    cfg.api_key = String::new();
    cfg.model = model.trim().to_string();
    cfg.llm_base_url = Some(crate::local_delegate::normalize_base_url_for_provider(
        base_url,
        crate::local_delegate::DEFAULT_LOCAL_BASE_URL,
    ));
    Box::new(OpenAiProvider::new_with_request_timeout(
        &cfg,
        request_timeout,
    ))
}

/// Max output tokens for PTE/PDQE sidecar calls (JSON-only; avoids slow local generation).
pub const EVALUATOR_MAX_TOKENS: u32 = 512;

/// Tokio + HTTP timeout for PTE/PDQE sidecar calls.
pub const EVALUATOR_TIMEOUT_SECS: u64 = 120;

pub struct EvaluatorProviderBundle {
    pub provider: Box<dyn LlmProvider>,
    pub label: String,
}

fn perplexity_api_key(config: &Config) -> String {
    config
        .perplexity_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            crate::engine_connections::saved_secret(
                &config.runtime_data_dir(),
                "PERPLEXITY_API_KEY",
            )
        })
        .unwrap_or_default()
}

/// Human-readable backend label for agent history / UI (`local · model @ url` or `perplexity · sonar`).
pub fn resolve_evaluator_provider_label(
    config: &Config,
    multimodel: Option<&crate::local_delegate::LocalDelegateConfig>,
) -> String {
    if let Some(mm) = multimodel {
        if let Some((base_url, model)) = crate::local_delegate::resolve_local_evaluator_endpoint(mm)
        {
            return format!("local · {} @ {}", model, base_url);
        }
    }
    if !perplexity_api_key(config).is_empty() {
        return format!("perplexity · {}", config.evaluator_model);
    }
    String::new()
}

/// Sidecar LLM for PTE / PDQE. Prefers local multimodel endpoint; falls back to Perplexity.
/// Never used for the main agent loop.
pub fn create_evaluator_provider(
    config: &Config,
    multimodel: Option<&crate::local_delegate::LocalDelegateConfig>,
) -> Result<EvaluatorProviderBundle, FinallyAValueBotError> {
    let request_timeout = std::time::Duration::from_secs(EVALUATOR_TIMEOUT_SECS);
    if let Some(mm) = multimodel {
        if let Some((base_url, model)) = crate::local_delegate::resolve_local_evaluator_endpoint(mm)
        {
            let mut eval_config = config.clone();
            eval_config.max_tokens = EVALUATOR_MAX_TOKENS;
            let label = format!("local · {} @ {}", model, base_url);
            let provider = create_openai_compatible_provider_with_timeout(
                &eval_config,
                &base_url,
                &model,
                request_timeout,
            );
            return Ok(EvaluatorProviderBundle { provider, label });
        }
    }
    let key = perplexity_api_key(config);
    if key.is_empty() {
        return Err(FinallyAValueBotError::Config(
            "No evaluator provider: configure a local model server, or set PERPLEXITY_API_KEY in .env or Settings → Connections"
                .into(),
        ));
    }
    let mut eval_config = config.clone();
    eval_config.max_tokens = EVALUATOR_MAX_TOKENS;
    eval_config.llm_provider = "openai".into();
    eval_config.api_key = key.to_string();
    eval_config.model = config.evaluator_model.clone();
    eval_config.llm_base_url = Some(config.evaluator_base_url.clone());
    let label = format!("perplexity · {}", config.evaluator_model);
    Ok(EvaluatorProviderBundle {
        provider: create_provider(&eval_config),
        label,
    })
}

/// Hot-swappable main agent LLM (model changes from Web UI without full process restart).
pub struct LlmHandle {
    model: std::sync::RwLock<String>,
    provider: std::sync::RwLock<Arc<dyn LlmProvider>>,
    base_config: std::sync::RwLock<Config>,
    local_delegate_config: std::sync::RwLock<crate::local_delegate::LocalDelegateConfig>,
    local_delegate_runtime: std::sync::RwLock<Option<crate::local_delegate::LocalDelegateRuntime>>,
}

/// Resolved classic strategy provider for one agent run (persona override or global default).
#[derive(Clone)]
pub struct StrategyLlmSelection {
    pub provider_id: String,
    pub model: String,
    pub base_url: Option<String>,
    pub endpoint: String,
    pub client: Arc<dyn LlmProvider>,
    /// True when any persona LLM override field contributed to this selection.
    pub from_persona: bool,
}

impl LlmHandle {
    pub fn new(config: &Config) -> Arc<Self> {
        Self::from_provider(config, Arc::from(create_provider(config)))
    }

    pub fn from_provider(config: &Config, provider: Arc<dyn LlmProvider>) -> Arc<Self> {
        Arc::new(Self {
            model: std::sync::RwLock::new(config.model.clone()),
            base_config: std::sync::RwLock::new(config.clone()),
            provider: std::sync::RwLock::new(provider),
            local_delegate_config: std::sync::RwLock::new(
                crate::local_delegate::LocalDelegateConfig::default(),
            ),
            local_delegate_runtime: std::sync::RwLock::new(None),
        })
    }

    pub fn local_delegate_config(&self) -> crate::local_delegate::LocalDelegateConfig {
        self.local_delegate_config
            .read()
            .ok()
            .map(|cfg| cfg.clone())
            .unwrap_or_default()
    }

    /// Backward-compatible alias.
    pub fn multimodel_config(&self) -> crate::local_delegate::LocalDelegateConfig {
        self.local_delegate_config()
    }

    pub fn apply_local_delegate_config(
        &self,
        config: crate::local_delegate::LocalDelegateConfig,
    ) -> Result<(), String> {
        let config = config.normalize();
        *self
            .local_delegate_config
            .write()
            .map_err(|_| "local delegate config lock poisoned".to_string())? = config.clone();
        let base = self
            .base_config
            .read()
            .map_err(|_| "config lock poisoned".to_string())?
            .clone();
        let runtime = if config.local_configured() {
            Some(crate::local_delegate::LocalDelegateRuntime::new(
                &base,
                config.clone(),
            ))
        } else {
            None
        };
        *self
            .local_delegate_runtime
            .write()
            .map_err(|_| "local delegate runtime lock poisoned".to_string())? = runtime;
        Ok(())
    }

    /// Backward-compatible alias.
    pub fn apply_multimodel_config(
        &self,
        config: crate::local_delegate::LocalDelegateConfig,
    ) -> Result<(), String> {
        self.apply_local_delegate_config(config)
    }

    fn provider_for_target(
        &self,
        target: crate::local_delegate::RouteTarget,
    ) -> Result<Arc<dyn LlmProvider>, FinallyAValueBotError> {
        let strategy = self
            .provider
            .read()
            .map_err(|_| FinallyAValueBotError::LlmApi("LLM provider lock poisoned".into()))?
            .clone();
        if !target.is_local() {
            return Ok(strategy);
        }
        let rt = self.local_delegate_runtime.read().map_err(|_| {
            FinallyAValueBotError::LlmApi("local delegate runtime lock poisoned".into())
        })?;
        Ok(if let Some(ref runtime) = *rt {
            runtime.provider_for_target(target, &strategy)
        } else {
            strategy
        })
    }

    pub async fn send_message_for_target(
        &self,
        target: crate::local_delegate::RouteTarget,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let provider = self.provider_for_target(target)?;
        let has_tools = tools.as_ref().is_some_and(|t| !t.is_empty());
        let options = LlmSendOptions {
            tool_choice: crate::local_delegate::tool_choice_for_target(target, has_tools),
        };
        provider
            .send_message_with_options(system, messages, tools, options)
            .await
    }

    pub async fn send_message_for_tier(
        &self,
        tier: crate::local_delegate::RouteTarget,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        self.send_message_for_target(tier, system, messages, tools)
            .await
    }

    /// Strategy-tier calls use `strategy`; local tiers keep using the shared local-delegate runtime.
    pub async fn send_message_for_tier_with_strategy(
        &self,
        tier: crate::local_delegate::RouteTarget,
        strategy: &StrategyLlmSelection,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        if tier.is_local() {
            return self
                .send_message_for_target(tier, system, messages, tools)
                .await;
        }
        let has_tools = tools.as_ref().is_some_and(|t| !t.is_empty());
        let options = LlmSendOptions {
            tool_choice: crate::local_delegate::tool_choice_for_target(tier, has_tools),
        };
        strategy
            .client
            .send_message_with_options(system, messages, tools, options)
            .await
    }

    pub fn current_model(&self) -> String {
        self.model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| String::new())
    }

    pub fn current_provider(&self) -> String {
        self.base_config
            .read()
            .map(|c| c.llm_provider.clone())
            .unwrap_or_else(|_| String::new())
    }

    pub fn current_base_url(&self) -> Option<String> {
        self.base_config
            .read()
            .ok()
            .and_then(|c| c.llm_base_url.clone())
    }

    pub fn thinking_enabled(&self) -> bool {
        self.base_config
            .read()
            .map(|c| c.llm_thinking_enabled)
            .unwrap_or(false)
    }

    pub fn show_thinking(&self) -> bool {
        self.base_config
            .read()
            .map(|c| c.show_thinking)
            .unwrap_or(false)
    }

    fn strategy_endpoint_for(provider: &str, base_url: Option<&str>) -> String {
        if let Some(url) = base_url.map(str::trim).filter(|u| !u.is_empty()) {
            return url.to_string();
        }
        crate::llm_catalog::default_base_url_for_provider(provider)
            .map(|s| s.to_string())
            .unwrap_or_else(|| match provider {
                "anthropic" => "https://api.anthropic.com/v1/messages".to_string(),
                "google" => "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
                "xai" => "https://api.x.ai/v1".to_string(),
                _ => "https://api.openai.com/v1".to_string(),
            })
    }

    fn strategy_endpoint(&self) -> String {
        Self::strategy_endpoint_for(&self.current_provider(), self.current_base_url().as_deref())
    }

    fn strategy_tier_snapshot(&self) -> crate::local_delegate::RouteEndpointSnapshot {
        crate::local_delegate::RouteEndpointSnapshot {
            target: crate::local_delegate::RouteTarget::Strategy,
            provider: self.current_provider(),
            model: self.current_model(),
            endpoint: self.strategy_endpoint(),
        }
    }

    pub fn strategy_tier_snapshot_for(
        &self,
        strategy: &StrategyLlmSelection,
    ) -> crate::local_delegate::RouteEndpointSnapshot {
        crate::local_delegate::RouteEndpointSnapshot {
            target: crate::local_delegate::RouteTarget::Strategy,
            provider: strategy.provider_id.clone(),
            model: strategy.model.clone(),
            endpoint: strategy.endpoint.clone(),
        }
    }

    /// Validate provider/model/base_url without mutating the global handle.
    pub fn validate_selection(
        &self,
        provider: String,
        model: String,
        local_base_url: Option<String>,
    ) -> Result<(String, String, Option<String>), String> {
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err("model cannot be empty".into());
        }
        if model.len() > 256 {
            return Err("model id is too long".into());
        }
        let provider_id = crate::llm_catalog::resolve_catalog_provider_id(&provider);
        if provider_id.is_empty() {
            return Err("provider cannot be empty".into());
        }
        if crate::llm_catalog::find_provider(&provider_id).is_none() {
            return Err(format!("Unknown provider {provider_id:?}"));
        }
        if !crate::llm_catalog::is_local_provider(&provider_id)
            && crate::llm_catalog::resolve_api_key_for_provider(&provider_id).is_empty()
        {
            let hints = crate::llm_catalog::provider_api_key_env_hints(&provider_id).join(", ");
            return Err(format!(
                "No API key in environment for provider {provider_id}. Set one of: {hints}"
            ));
        }
        let base_url = if crate::llm_catalog::is_local_provider(&provider_id) {
            let raw = local_base_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| {
                    "base_url is required when provider is Ollama or llama.cpp (configure in Settings → LLM)."
                        .to_string()
                })?;
            Some(crate::llm_catalog::normalize_local_base_url(
                raw,
                &provider_id,
            ))
        } else {
            None
        };
        Ok((provider_id, model, base_url))
    }

    /// Resolve classic strategy LLM for a persona (NULL overrides inherit the global handle).
    pub fn resolve_strategy_selection(
        &self,
        persona: Option<&crate::db::Persona>,
    ) -> Result<StrategyLlmSelection, String> {
        let global_provider = self.current_provider();
        let global_model = self.current_model();
        let global_base = self.current_base_url();

        let provider_ov = persona
            .and_then(|p| p.llm_provider_override.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let model_ov = persona
            .and_then(|p| p.model_override.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let base_ov = persona
            .and_then(|p| p.llm_base_url_override.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let from_persona = provider_ov.is_some() || model_ov.is_some() || base_ov.is_some();

        if !from_persona {
            let client = self
                .provider
                .read()
                .map_err(|_| "LLM provider lock poisoned".to_string())?
                .clone();
            return Ok(StrategyLlmSelection {
                provider_id: global_provider,
                model: global_model,
                base_url: global_base,
                endpoint: self.strategy_endpoint(),
                client,
                from_persona: false,
            });
        }

        let provider_id = crate::llm_catalog::resolve_catalog_provider_id(
            provider_ov.unwrap_or(global_provider.as_str()),
        );
        let model = model_ov.unwrap_or(global_model.as_str()).to_string();
        let local_base = if crate::llm_catalog::is_local_provider(&provider_id) {
            Some(
                base_ov
                    .map(str::to_string)
                    .or_else(|| global_base.clone())
                    .filter(|u| !u.trim().is_empty())
                    .ok_or_else(|| {
                        "base_url is required when provider is Ollama or llama.cpp (configure in Settings → LLM)."
                            .to_string()
                    })?,
            )
        } else {
            None
        };
        let (provider_id, model, base_url) =
            self.validate_selection(provider_id, model, local_base)?;

        let mut cfg = self
            .base_config
            .read()
            .map_err(|_| "config lock poisoned".to_string())?
            .clone();
        cfg.apply_llm_provider_switch(&provider_id, &model, base_url.as_deref());
        let client = Arc::from(create_provider(&cfg));
        let endpoint = Self::strategy_endpoint_for(&provider_id, base_url.as_deref());
        Ok(StrategyLlmSelection {
            provider_id,
            model,
            base_url,
            endpoint,
            client,
            from_persona: true,
        })
    }

    /// Resolved provider/model/endpoint for a route target (for agent history and debugging).
    pub fn tier_endpoint_snapshot(
        &self,
        target: crate::local_delegate::RouteTarget,
    ) -> crate::local_delegate::RouteEndpointSnapshot {
        self.tier_endpoint_snapshot_with_strategy(target, None)
    }

    pub fn tier_endpoint_snapshot_with_strategy(
        &self,
        target: crate::local_delegate::RouteTarget,
        strategy: Option<&StrategyLlmSelection>,
    ) -> crate::local_delegate::RouteEndpointSnapshot {
        if !target.is_local() {
            return match strategy {
                Some(s) => self.strategy_tier_snapshot_for(s),
                None => self.strategy_tier_snapshot(),
            };
        }
        let mm_cfg = self.local_delegate_config();
        crate::local_delegate::RouteEndpointSnapshot {
            target,
            provider: "llama".into(),
            model: mm_cfg.local_model.clone(),
            endpoint: mm_cfg.local_base_url.clone(),
        }
    }

    /// Run-level routing summary for agent history (captured once per run).
    pub fn local_delegate_run_summary(
        &self,
        cost_routing_active: bool,
    ) -> crate::local_delegate::LocalDelegateRunSummary {
        self.local_delegate_run_summary_with_strategy(cost_routing_active, None)
    }

    pub fn local_delegate_run_summary_with_strategy(
        &self,
        cost_routing_active: bool,
        strategy: Option<&StrategyLlmSelection>,
    ) -> crate::local_delegate::LocalDelegateRunSummary {
        let mm_cfg = self.local_delegate_config();
        let strategy_snap = match strategy {
            Some(s) => self.strategy_tier_snapshot_for(s),
            None => self.strategy_tier_snapshot(),
        };
        crate::local_delegate::LocalDelegateRunSummary {
            cost_routing_active,
            strategy_provider: strategy_snap.provider,
            strategy_model: strategy_snap.model,
            strategy_endpoint: strategy_snap.endpoint,
            local_model: mm_cfg.local_model.clone(),
            local_endpoint: mm_cfg.local_base_url.clone(),
        }
    }

    /// Backward-compatible alias.
    pub fn multimodel_run_summary(&self) -> crate::local_delegate::LocalDelegateRunSummary {
        let mm_cfg = self.local_delegate_config();
        self.local_delegate_run_summary(mm_cfg.ready_for_routing())
    }

    /// Update active provider and model, rebuild LLM client, return `(provider_id, model)`.
    pub fn apply_thinking_settings(
        &self,
        thinking_enabled: bool,
        show_thinking: bool,
    ) -> Result<(), String> {
        {
            let mut cfg = self
                .base_config
                .write()
                .map_err(|_| "config lock poisoned".to_string())?;
            cfg.llm_thinking_enabled = thinking_enabled;
            cfg.show_thinking = show_thinking;
        }
        let cfg = self
            .base_config
            .read()
            .map_err(|_| "config lock poisoned".to_string())?
            .clone();
        let new_provider = Arc::from(create_provider(&cfg));
        *self
            .provider
            .write()
            .map_err(|_| "provider lock poisoned".to_string())? = new_provider;
        Ok(())
    }

    /// Update active provider and model, rebuild LLM client, return `(provider_id, model)`.
    pub fn apply_selection(
        &self,
        provider: String,
        model: String,
        local_base_url: Option<String>,
    ) -> Result<(String, String), String> {
        let (provider_id, model, base_url) =
            self.validate_selection(provider, model, local_base_url)?;
        let mut cfg = self
            .base_config
            .read()
            .map_err(|_| "config lock poisoned".to_string())?
            .clone();
        cfg.apply_llm_provider_switch(&provider_id, &model, base_url.as_deref());
        let new_provider = Arc::from(create_provider(&cfg));
        *self
            .model
            .write()
            .map_err(|_| "model lock poisoned".to_string())? = model.clone();
        *self
            .base_config
            .write()
            .map_err(|_| "config lock poisoned".to_string())? = cfg;
        *self
            .provider
            .write()
            .map_err(|_| "provider lock poisoned".to_string())? = new_provider;
        // Clone multimodel config before apply — holding a read guard across
        // apply_local_delegate_config would deadlock (it needs a write lock).
        let mm_cfg = self.local_delegate_config();
        let _ = self.apply_local_delegate_config(mm_cfg);
        Ok((provider_id, model))
    }

    /// Update active model for the current provider, rebuild client, return the new model id.
    pub fn set_model(&self, model: String) -> Result<String, String> {
        let provider = self.current_provider();
        let base_url = if crate::llm_catalog::is_local_provider(&provider) {
            self.current_base_url()
        } else {
            None
        };
        self.apply_selection(provider, model, base_url)
            .map(|(_, m)| m)
    }
}

#[async_trait]
impl LlmProvider for LlmHandle {
    async fn send_message(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let provider = self
            .provider
            .read()
            .map_err(|_| FinallyAValueBotError::LlmApi("LLM provider lock poisoned".into()))?
            .clone();
        provider.send_message(system, messages, tools).await
    }

    async fn send_message_with_options(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        options: LlmSendOptions,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let provider = self
            .provider
            .read()
            .map_err(|_| FinallyAValueBotError::LlmApi("LLM provider lock poisoned".into()))?
            .clone();
        provider
            .send_message_with_options(system, messages, tools, options)
            .await
    }

    async fn send_message_stream(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let provider = self
            .provider
            .read()
            .map_err(|_| FinallyAValueBotError::LlmApi("LLM provider lock poisoned".into()))?
            .clone();
        provider
            .send_message_stream(system, messages, tools, text_tx)
            .await
    }
}

/// Lightweight reachability check for OpenAI-compatible local servers (llama.cpp, Ollama).
async fn probe_openai_compatible_server(base_url: &str, model: &str) -> Result<(), String> {
    let base = base_url.trim().trim_end_matches('/');
    let models_url = format!("{base}/models");
    let client = probe_llm_http_client();
    let resp = client
        .get(&models_url)
        .send()
        .await
        .map_err(|e| format!("Could not reach server at {base_url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Server at {base_url} returned HTTP {status}. Response: {}",
            llm_error_body_preview(&body, 200)
        ));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
            if !data.is_empty() && !model.trim().is_empty() {
                let found = data.iter().any(|entry| {
                    entry
                        .get("id")
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| id == model.trim())
                });
                if !found {
                    let listed: Vec<&str> = data
                        .iter()
                        .filter_map(|e| e.get("id").and_then(|id| id.as_str()))
                        .take(8)
                        .collect();
                    return Err(format!(
                        "Server reachable at {base_url}, but model {:?} was not listed in /models. Loaded models: {}",
                        model,
                        listed.join(", ")
                    ));
                }
            }
        }
    }
    Ok(())
}

const LIVE_CATALOG_TIMEOUT_SECS: u64 = 20;
const LIVE_CATALOG_MAX_PAGES: usize = 5;

fn parse_openai_compatible_model_ids(parsed: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = parsed
        .get("data")
        .and_then(|d| d.as_array())
        .map(|data| {
            data.iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        if let Some(models) = parsed.get("models").and_then(|m| m.as_array()) {
            ids = models
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                        .or_else(|| entry.as_str().map(str::to_string))
                })
                .collect();
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn provider_models_http_error(label: &str, status: reqwest::StatusCode, body: &str) -> String {
    format!(
        "{label} returned HTTP {status}. Response: {}",
        llm_error_body_preview(body, 200)
    )
}

/// List model ids from an OpenAI-compatible server (`GET /v1/models`).
pub async fn fetch_openai_compatible_models(base_url: &str) -> Result<Vec<String>, String> {
    fetch_openai_compatible_models_authenticated(base_url, None).await
}

/// List model ids from an OpenAI-compatible `/models` endpoint, optionally sending a Bearer token.
pub async fn fetch_openai_compatible_models_authenticated(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("base_url is required".into());
    }
    let models_url = format!("{base}/models");
    let client = probe_llm_http_client();
    let mut req = client.get(&models_url);
    if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    if base.contains("openrouter.ai") {
        req = req
            .header("HTTP-Referer", "https://github.com/finally-a-value-bot")
            .header("X-Title", "finally-a-value-bot");
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Could not reach models endpoint at {base}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(provider_models_http_error(base, status, &body));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid models JSON: {e}"))?;
    let ids = parse_openai_compatible_model_ids(&parsed);
    if ids.is_empty() {
        return Err(format!(
            "Server at {base} returned no models. Response: {}",
            llm_error_body_preview(&text, 200)
        ));
    }
    Ok(ids)
}

async fn fetch_anthropic_model_ids(api_key: &str) -> Result<Vec<String>, String> {
    let client = probe_llm_http_client();
    let mut ids = Vec::new();
    let mut after_id: Option<String> = None;
    for _ in 0..LIVE_CATALOG_MAX_PAGES {
        let mut req = client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .query(&[("limit", "100")]);
        if let Some(ref after) = after_id {
            req = req.query(&[("after_id", after.as_str())]);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Could not reach Anthropic models API: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(provider_models_http_error("Anthropic", status, &text));
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("Invalid Anthropic models JSON: {e}"))?;
        let page = parse_openai_compatible_model_ids(&parsed);
        let last_id = parsed
            .get("last_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| page.last().cloned());
        let has_more = parsed
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        ids.extend(page);
        if !has_more {
            break;
        }
        match last_id {
            Some(next) if after_id.as_ref() != Some(&next) => after_id = Some(next),
            _ => break,
        }
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("Anthropic models API returned no models".into());
    }
    Ok(ids)
}

fn parse_gemini_native_model_ids(parsed: &serde_json::Value) -> Vec<String> {
    parsed
        .get("models")
        .and_then(|m| m.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|entry| {
                    let methods = entry
                        .get("supportedGenerationMethods")
                        .and_then(|v| v.as_array());
                    if let Some(methods) = methods {
                        let chat_ok = methods.iter().any(|m| {
                            m.as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case("generateContent"))
                        });
                        if !chat_ok {
                            return None;
                        }
                    }
                    entry
                        .get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| entry.get("id").and_then(|n| n.as_str()))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_gemini_model_ids(api_key: &str) -> Result<Vec<String>, String> {
    if let Ok(ids) = fetch_openai_compatible_models_authenticated(
        "https://generativelanguage.googleapis.com/v1beta/openai",
        Some(api_key),
    )
    .await
    {
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    let client = probe_llm_http_client();
    let mut ids = Vec::new();
    let mut page_token: Option<String> = None;
    for _ in 0..LIVE_CATALOG_MAX_PAGES {
        let mut req = client
            .get("https://generativelanguage.googleapis.com/v1beta/models")
            .header("x-goog-api-key", api_key)
            .query(&[("pageSize", "100")]);
        if let Some(ref token) = page_token {
            req = req.query(&[("pageToken", token.as_str())]);
        }
        let resp = req
            .send()
            .await
            .map_err(|_| "Could not reach Gemini models API".to_string())?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(provider_models_http_error("Gemini", status, &text));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("Invalid Gemini models JSON: {e}"))?;
        ids.extend(parse_gemini_native_model_ids(&parsed));
        let next = parsed
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|t| !t.is_empty());
        match next {
            Some(token) if page_token.as_ref() != Some(&token) => page_token = Some(token),
            _ => break,
        }
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("Gemini models API returned no models".into());
    }
    Ok(ids)
}

/// Fetch live model ids for a catalog provider. Never logs or returns the API key.
pub async fn fetch_live_provider_models(
    provider_id: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<Vec<String>, String> {
    let p = crate::llm_catalog::resolve_catalog_provider_id(provider_id);
    let fetch = async {
        match p.as_str() {
            "azure" | "bedrock" | "custom" => Err(
                "This provider does not expose a listable model catalog. Use a custom model id."
                    .into(),
            ),
            "anthropic" => {
                if api_key.trim().is_empty() {
                    return Err("Anthropic API key is not configured in .env".into());
                }
                fetch_anthropic_model_ids(api_key).await
            }
            "google" => {
                if api_key.trim().is_empty() {
                    return Err("Gemini API key is not configured in .env".into());
                }
                fetch_gemini_model_ids(api_key).await
            }
            "ollama" | "llama" | "llamacpp" => {
                let url = base_url
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map(|u| crate::llm_catalog::normalize_local_base_url(u, &p))
                    .or_else(|| {
                        crate::llm_catalog::default_base_url_for_provider(&p).map(str::to_string)
                    })
                    .ok_or_else(|| "base_url is required for local providers".to_string())?;
                fetch_openai_compatible_models(&url).await
            }
            _ => {
                if api_key.trim().is_empty() {
                    return Err(format!("API key is not configured in .env for {p}"));
                }
                let url = crate::llm_catalog::default_base_url_for_provider(&p)
                    .filter(|u| !u.is_empty() && !u.contains("YOUR-"))
                    .ok_or_else(|| format!("No default API base URL for provider {p}"))?;
                fetch_openai_compatible_models_authenticated(url, Some(api_key)).await
            }
        }
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(LIVE_CATALOG_TIMEOUT_SECS),
        fetch,
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Live model catalog timed out after {LIVE_CATALOG_TIMEOUT_SECS}s"
        )),
    }
}

/// Test that a model override is reachable with the current provider/config.
/// Returns Ok(()) on success, or an error string suitable for showing to the user.
pub async fn test_model(config: &Config, model_override: &str) -> Result<(), String> {
    let model = model_override.trim();
    if model.is_empty() {
        return Err("model is required".into());
    }

    if crate::llm_catalog::is_local_provider(&config.llm_provider) {
        let base_url = config
            .llm_base_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| "base_url is required for local providers".to_string())?;
        return match tokio::time::timeout(
            std::time::Duration::from_secs(LLM_PROBE_TIMEOUT_SECS),
            probe_openai_compatible_server(base_url, model),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!(
                "Connection test timed out after {LLM_PROBE_TIMEOUT_SECS}s (is the server running at {base_url}?)"
            )),
        };
    }

    let mut test_config = config.clone();
    test_config.model = model.to_string();
    test_config.max_tokens = LLM_TEST_MAX_TOKENS.min(test_config.max_tokens);
    let provider = create_provider(&test_config);
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text("Hi".into()),
    }];
    match tokio::time::timeout(
        std::time::Duration::from_secs(LLM_PROBE_TIMEOUT_SECS),
        provider.send_message("Test.", messages, None),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "Connection test timed out after {LLM_PROBE_TIMEOUT_SECS}s (server did not respond in time)"
        )),
    }
}

/// Probe tool-calling on a local OpenAI-compatible tier (llama.cpp).
pub async fn test_local_delegate_tools(
    config: &Config,
    model: &str,
    tier: crate::local_delegate::ModelTier,
) -> Result<(), String> {
    let mut test_config = config.clone();
    test_config.model = model.to_string();
    test_config.max_tokens = 256;
    let base_url = config
        .llm_base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "base_url is required for local providers".to_string())?;
    let provider = create_openai_compatible_provider(&test_config, base_url, model);
    let tools = vec![ToolDefinition::new(
        "add",
        "Add two integers",
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        }),
    )];
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text("Use add to compute 2+3. You must call the tool.".into()),
    }];
    let has_tools = true;
    let options = LlmSendOptions {
        tool_choice: crate::local_delegate::tool_choice_for_tier(tier, has_tools),
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(LLM_PROBE_TIMEOUT_SECS.saturating_mul(4)),
        provider.send_message_with_options("Test.", messages, Some(tools), options),
    )
    .await
    .map_err(|_| {
        format!(
            "Tool-calling probe timed out after {}s",
            LLM_PROBE_TIMEOUT_SECS.saturating_mul(4)
        )
    })?
    .map_err(|e| e.to_string())?;
    let has_tool_use = response
        .content
        .iter()
        .any(|b| matches!(b, ResponseContentBlock::ToolUse { .. }));
    if has_tool_use {
        Ok(())
    } else {
        Err("Model responded without tool_calls (tool-calling not verified)".into())
    }
}

// ---------------------------------------------------------------------------
// Anthropic provider
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(config: &Config) -> Self {
        AnthropicProvider {
            http: default_llm_http_client(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            base_url: config
                .llm_base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".into()),
        }
    }

    async fn send_message_stream_single_pass(
        &self,
        request: &MessagesRequest,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let mut streamed_request = request.clone();
        streamed_request.stream = Some(true);

        let mut retries = 0u32;
        let max_retries = 3;

        let response = loop {
            let response_res = self
                .http
                .post(&self.base_url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31")
                .header("content-type", "application/json")
                .json(&streamed_request)
                .send()
                .await;

            let resp = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending Anthropic stream request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = resp.status();
            if status.is_success() {
                break resp;
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient stream error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let body = resp.text().await.unwrap_or_default();
            if let Ok(api_err) = serde_json::from_str::<AnthropicApiError>(&body) {
                return Err(FinallyAValueBotError::LlmApi(format!(
                    "{}: {}",
                    api_err.error.error_type, api_err.error.message
                )));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {body}"
            )));
        };

        let mut byte_stream = response.bytes_stream();
        let mut sse = SseEventParser::default();
        let mut stop_reason: Option<String> = None;
        let mut usage: Option<Usage> = None;
        let mut text_blocks: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        let mut tool_blocks: std::collections::HashMap<usize, StreamToolUseBlock> =
            std::collections::HashMap::new();
        let mut ordered_indexes: Vec<usize> = Vec::new();

        'outer: while let Some(chunk_res) = byte_stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(_) => break,
            };
            for data in sse.push_chunk(&String::from_utf8_lossy(&chunk)) {
                if data == "[DONE]" {
                    break 'outer;
                }
                process_anthropic_stream_event(
                    &data,
                    text_tx,
                    &mut stop_reason,
                    &mut usage,
                    &mut text_blocks,
                    &mut tool_blocks,
                    &mut ordered_indexes,
                );
            }
        }
        for data in sse.finish() {
            if data == "[DONE]" {
                break;
            }
            process_anthropic_stream_event(
                &data,
                text_tx,
                &mut stop_reason,
                &mut usage,
                &mut text_blocks,
                &mut tool_blocks,
                &mut ordered_indexes,
            );
        }

        Ok(build_stream_response(
            ordered_indexes,
            text_blocks,
            tool_blocks,
            stop_reason,
            usage,
        ))
    }
}

#[derive(Default)]
struct StreamToolUseBlock {
    id: String,
    name: String,
    input_json: String,
    thought_signature: Option<String>,
}

fn usage_from_json(v: &serde_json::Value) -> Option<Usage> {
    let input = v.get("input_tokens").and_then(|n| n.as_u64())?;
    let output = v
        .get("output_tokens")
        .and_then(|n| n.as_u64())
        .or_else(|| v.get("completion_tokens").and_then(|n| n.as_u64()))
        .unwrap_or(0);
    Some(Usage {
        input_tokens: u32::try_from(input).unwrap_or(u32::MAX),
        output_tokens: u32::try_from(output).unwrap_or(u32::MAX),
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
    })
}

fn process_anthropic_stream_event(
    data: &str,
    text_tx: Option<&UnboundedSender<String>>,
    stop_reason: &mut Option<String>,
    usage: &mut Option<Usage>,
    text_blocks: &mut std::collections::HashMap<usize, String>,
    tool_blocks: &mut std::collections::HashMap<usize, StreamToolUseBlock>,
    ordered_indexes: &mut Vec<usize>,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    match event_type {
        "content_block_start" => {
            if let Some(index) = v
                .get("index")
                .and_then(|i| i.as_u64())
                .and_then(|i| usize::try_from(i).ok())
            {
                if !ordered_indexes.contains(&index) {
                    ordered_indexes.push(index);
                }
                if let Some(block) = v.get("content_block") {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            let text = block
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or_default()
                                .to_string();
                            text_blocks.insert(index, text);
                        }
                        Some("tool_use") => {
                            let id = block
                                .get("id")
                                .and_then(|s| s.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|s| s.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                            let input_json = if input.is_object()
                                && input.as_object().is_some_and(|m| m.is_empty())
                            {
                                String::new()
                            } else {
                                serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())
                            };
                            tool_blocks.insert(
                                index,
                                StreamToolUseBlock {
                                    id,
                                    name,
                                    input_json,
                                    thought_signature: block
                                        .get("thought_signature")
                                        .or_else(|| block.get("thoughtSignature"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from),
                                },
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        "content_block_delta" => {
            let Some(index) = v
                .get("index")
                .and_then(|i| i.as_u64())
                .and_then(|i| usize::try_from(i).ok())
            else {
                return;
            };
            let Some(delta) = v.get("delta") else {
                return;
            };
            match delta.get("type").and_then(|t| t.as_str()) {
                Some("text_delta") => {
                    let piece = delta
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default();
                    if !piece.is_empty() {
                        text_blocks.entry(index).or_default().push_str(piece);
                        if let Some(tx) = text_tx {
                            let _ = tx.send(piece.to_string());
                        }
                    }
                }
                Some("input_json_delta") => {
                    let piece = delta
                        .get("partial_json")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default();
                    if !piece.is_empty() {
                        tool_blocks
                            .entry(index)
                            .or_default()
                            .input_json
                            .push_str(piece);
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            if let Some(reason) = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|s| s.as_str())
            {
                *stop_reason = Some(reason.to_string());
            }
            if let Some(u) = v.get("usage") {
                *usage = usage_from_json(u);
            }
        }
        "message_start" => {
            if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                *usage = usage_from_json(u);
            }
        }
        _ => {}
    }
}

fn process_openai_stream_event(
    data: &str,
    text_tx: Option<&UnboundedSender<String>>,
    text: &mut String,
    stop_reason: &mut Option<String>,
    usage: &mut Option<Usage>,
    tool_calls: &mut std::collections::BTreeMap<usize, StreamToolUseBlock>,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };

    if usage.is_none() {
        *usage = v.get("usage").and_then(usage_from_json);
    }

    let Some(choice) = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
    else {
        return;
    };

    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
        *stop_reason = Some(reason.to_string());
    }

    let Some(delta) = choice.get("delta") else {
        return;
    };

    if let Some(piece) = delta.get("content").and_then(|t| t.as_str()) {
        if !piece.is_empty() {
            text.push_str(piece);
            if let Some(tx) = text_tx {
                let _ = tx.send(piece.to_string());
            }
        }
    }

    if let Some(tc_arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tc_arr {
            let Some(index) = tc
                .get("index")
                .and_then(|i| i.as_u64())
                .and_then(|i| usize::try_from(i).ok())
            else {
                continue;
            };
            let entry = tool_calls.entry(index).or_default();
            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                entry.id = id.to_string();
            }
            if let Some(s) = tc
                .get("thought_signature")
                .or_else(|| tc.get("thoughtSignature"))
                .and_then(|v| v.as_str())
            {
                entry.thought_signature = Some(s.to_string());
            }
            if let Some(function) = tc.get("function") {
                if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                    entry.name = name.to_string();
                }
                if let Some(args) = function.get("arguments").and_then(|v| v.as_str()) {
                    entry.input_json.push_str(args);
                }
                if entry.thought_signature.is_none() {
                    if let Some(s) = function
                        .get("thought_signature")
                        .or_else(|| function.get("thoughtSignature"))
                        .and_then(|v| v.as_str())
                    {
                        entry.thought_signature = Some(s.to_string());
                    }
                }
            }
        }
    }
}

fn normalize_stop_reason(reason: Option<String>) -> Option<String> {
    match reason.as_deref() {
        None => Some("end_turn".into()),
        Some("tool_use") | Some("tool_calls") => Some("tool_use".into()),
        Some("max_tokens") | Some("length") => Some("max_tokens".into()),
        Some("end_turn") => Some("end_turn".into()),
        Some(r) if r.eq_ignore_ascii_case("stop") => Some("end_turn".into()),
        Some(r) if r.eq_ignore_ascii_case("completed") => Some("end_turn".into()),
        Some("clarification") | Some("ask_user") | Some("needs_clarification") => {
            Some("ask_clarification".into())
        }
        Some(other) => Some(other.to_string()),
    }
}

/// OpenAI-compatible APIs (notably xAI Grok) may return `finish_reason: "stop"` / `"completed"`
/// while still populating `tool_calls`. Prefer tool content over finish_reason.
fn oai_stop_reason_from_content(
    finish_reason: Option<&str>,
    has_tool_calls: bool,
) -> Option<String> {
    if has_tool_calls {
        return Some("tool_use".into());
    }
    normalize_stop_reason(finish_reason.map(str::to_string))
}

fn parse_tool_input(input_json: &str) -> serde_json::Value {
    let trimmed = input_json.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

fn build_stream_response(
    ordered_indexes: Vec<usize>,
    text_blocks: std::collections::HashMap<usize, String>,
    tool_blocks: std::collections::HashMap<usize, StreamToolUseBlock>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
) -> MessagesResponse {
    let mut content = Vec::new();
    for index in ordered_indexes {
        if let Some(text) = text_blocks.get(&index) {
            if !text.is_empty() {
                content.push(ResponseContentBlock::Text { text: text.clone() });
            }
        }
        if let Some(tool) = tool_blocks.get(&index) {
            content.push(ResponseContentBlock::ToolUse {
                id: tool.id.clone(),
                name: tool.name.clone(),
                input: parse_tool_input(&tool.input_json),
                thought_signature: tool.thought_signature.clone(),
            });
        }
    }

    if content.is_empty() {
        content.push(ResponseContentBlock::Text {
            text: String::new(),
        });
    }

    let has_tool_calls = content
        .iter()
        .any(|b| matches!(b, ResponseContentBlock::ToolUse { .. }));
    let stop_reason = if has_tool_calls {
        Some("tool_use".into())
    } else {
        normalize_stop_reason(stop_reason)
    };

    MessagesResponse {
        content,
        stop_reason,
        usage,
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicApiError {
    error: AnthropicApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct AnthropicApiErrorDetail {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
}

fn build_cached_system(text: &str) -> SystemContent {
    if text.len() > 1024 {
        SystemContent::Blocks(vec![SystemBlock {
            block_type: "text".into(),
            text: text.to_string(),
            cache_control: Some(CacheControl {
                control_type: "ephemeral".into(),
            }),
        }])
    } else {
        SystemContent::Text(text.to_string())
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn send_message(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let messages = sanitize_messages(messages);

        let mut request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: build_cached_system(system),
            messages,
            tools,
            stream: None,
        };
        if let Some(ref mut tools) = request.tools {
            if let Some(last) = tools.last_mut() {
                last.cache_control = Some(CacheControl {
                    control_type: "ephemeral".into(),
                });
            }
        }

        let mut retries = 0u32;
        let max_retries = 3;

        loop {
            let response_res = self
                .http
                .post(&self.base_url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31")
                .header("content-type", "application/json")
                .json(&request)
                .send()
                .await;

            let response = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending Anthropic request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = response.status();

            if status.is_success() {
                let body = response.text().await?;
                let parsed: MessagesResponse = serde_json::from_str(&body).map_err(|e| {
                    FinallyAValueBotError::LlmApi(format!(
                        "Failed to parse response: {e}\nBody: {body}"
                    ))
                })?;
                return Ok(parsed);
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let body = response.text().await.unwrap_or_default();
            if let Ok(api_err) = serde_json::from_str::<AnthropicApiError>(&body) {
                return Err(FinallyAValueBotError::LlmApi(format!(
                    "{}: {}",
                    api_err.error.error_type, api_err.error.message
                )));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {body}"
            )));
        }
    }

    async fn send_message_stream(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let messages = sanitize_messages(messages);
        let mut request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: build_cached_system(system),
            messages,
            tools,
            stream: Some(true),
        };
        if let Some(ref mut tools) = request.tools {
            if let Some(last) = tools.last_mut() {
                last.cache_control = Some(CacheControl {
                    control_type: "ephemeral".into(),
                });
            }
        }

        self.send_message_stream_single_pass(&request, text_tx)
            .await
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider  (OpenAI, OpenRouter, DeepSeek, Groq, Ollama …)
// ---------------------------------------------------------------------------

fn oai_resolve_base_url(config: &Config) -> String {
    if let Some(ref url) = config.llm_base_url {
        let t = url.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    crate::llm_catalog::default_base_url_for_provider(&config.llm_provider)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
}

/// GPT-5 / o-series and some Grok reasoning models reject `max_tokens` on Chat Completions.
fn oai_uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("gpt-5")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.contains("grok-4.20")
        || m.contains("-reasoning")
}

fn oai_error_wants_max_completion_tokens(body: &str) -> bool {
    body.contains("max_completion_tokens")
}

fn build_oai_chat_request_body(
    model: &str,
    max_tokens: u32,
    oai_messages: &[serde_json::Value],
    tools: Option<&[ToolDefinition]>,
    tool_choice: Option<&str>,
    stream: bool,
    use_max_completion_tokens: bool,
) -> serde_json::Value {
    let mut body = json!({
        "model": model,
        "messages": oai_messages,
    });
    if use_max_completion_tokens {
        body["max_completion_tokens"] = json!(max_tokens);
    } else {
        body["max_tokens"] = json!(max_tokens);
    }
    if stream {
        body["stream"] = json!(true);
    }
    if let Some(tool_defs) = tools {
        if !tool_defs.is_empty() {
            body["tools"] = json!(translate_tools_to_oai(tool_defs));
            if let Some(choice) = tool_choice {
                body["tool_choice"] = json!(choice);
            }
        }
    }
    body
}

pub struct OpenAiProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    chat_url: String,
}

impl OpenAiProvider {
    pub fn new(config: &Config) -> Self {
        Self::new_with_request_timeout(
            config,
            std::time::Duration::from_secs(LLM_HTTP_REQUEST_TIMEOUT_SECS),
        )
    }

    pub fn new_with_request_timeout(config: &Config, request_timeout: std::time::Duration) -> Self {
        let base = oai_resolve_base_url(config);
        let chat_url = format!("{}/chat/completions", base.trim_end_matches('/'));

        OpenAiProvider {
            http: build_llm_http_client(request_timeout),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            chat_url,
        }
    }
}

// --- OpenAI response types ---

#[derive(Debug, Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Debug, Deserialize)]
struct OaiChoice {
    message: OaiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OaiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OaiToolCall {
    id: String,
    function: OaiFunction,
    #[serde(default, alias = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiFunction {
    name: String,
    arguments: String,
    #[serde(default, alias = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OaiErrorResponse {
    error: OaiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct OaiErrorDetail {
    message: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
}

fn format_oai_error(status: reqwest::StatusCode, err: &OaiErrorDetail, body: &str) -> String {
    let status = status.as_u16();
    let extra: Vec<String> = [
        err.r#type.as_deref().map(|t| format!("type={t}")),
        err.code.as_deref().map(|c| format!("code={c}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let extra_str = if extra.is_empty() {
        String::new()
    } else {
        format!(" ({})", extra.join(", "))
    };
    let body_preview = if body.len() > 400 {
        format!("{}...", &body[..400])
    } else {
        body.to_string()
    };
    format!(
        "HTTP {}: {}{}. Response: {}",
        status, err.message, extra_str, body_preview
    )
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn send_message(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        self.send_message_with_options(system, messages, tools, LlmSendOptions::default())
            .await
    }

    async fn send_message_with_options(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        options: LlmSendOptions,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let oai_messages = translate_messages_to_oai(system, &messages);
        let tool_slice = tools.as_deref();
        let tool_choice = options.tool_choice.as_deref();
        let mut use_max_completion_tokens = oai_uses_max_completion_tokens(&self.model);
        let mut token_field_retried = false;
        let mut retries = 0u32;
        let max_retries = 3;

        loop {
            let body = build_oai_chat_request_body(
                &self.model,
                self.max_tokens,
                &oai_messages,
                tool_slice,
                tool_choice,
                false,
                use_max_completion_tokens,
            );

            let mut req = self
                .http
                .post(&self.chat_url)
                .header("Content-Type", "application/json")
                .json(&body);
            if !self.api_key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
            let response_res = req.send().await;

            let response = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending OpenAI request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = response.status();

            if status.is_success() {
                let text = response.text().await?;
                let oai: OaiResponse = serde_json::from_str(&text).map_err(|e| {
                    FinallyAValueBotError::LlmApi(format!(
                        "Failed to parse OpenAI response: {e}\nBody: {text}"
                    ))
                })?;
                return Ok(translate_oai_response(oai));
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let text = response.text().await.unwrap_or_default();
            if status.as_u16() == 400
                && !token_field_retried
                && oai_error_wants_max_completion_tokens(&text)
            {
                token_field_retried = true;
                use_max_completion_tokens = true;
                continue;
            }
            if let Ok(err) = serde_json::from_str::<OaiErrorResponse>(&text) {
                let msg = format_oai_error(status, &err.error, &text);
                return Err(FinallyAValueBotError::LlmApi(msg));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {text}"
            )));
        }
    }

    async fn send_message_stream(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let oai_messages = translate_messages_to_oai(system, &messages);
        let tool_slice = tools.as_deref();
        let mut use_max_completion_tokens = oai_uses_max_completion_tokens(&self.model);
        let mut token_field_retried = false;
        let mut retries = 0u32;
        let max_retries = 3;

        let response = loop {
            let body = build_oai_chat_request_body(
                &self.model,
                self.max_tokens,
                &oai_messages,
                tool_slice,
                None,
                true,
                use_max_completion_tokens,
            );

            let mut req = self
                .http
                .post(&self.chat_url)
                .header("Content-Type", "application/json")
                .json(&body);
            if !self.api_key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
            let response_res = req.send().await;

            let resp = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending OpenAI stream request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = resp.status();
            if status.is_success() {
                break resp;
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient stream error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 400
                && !token_field_retried
                && oai_error_wants_max_completion_tokens(&text)
            {
                token_field_retried = true;
                use_max_completion_tokens = true;
                continue;
            }
            if let Ok(err) = serde_json::from_str::<OaiErrorResponse>(&text) {
                let msg = format_oai_error(status, &err.error, &text);
                return Err(FinallyAValueBotError::LlmApi(msg));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {text}"
            )));
        };

        let mut byte_stream = response.bytes_stream();
        let mut sse = SseEventParser::default();
        let mut text = String::new();
        let mut stop_reason: Option<String> = None;
        let mut usage: Option<Usage> = None;
        let mut tool_calls: std::collections::BTreeMap<usize, StreamToolUseBlock> =
            std::collections::BTreeMap::new();

        'outer: while let Some(chunk_res) = byte_stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(_) => break,
            };
            for data in sse.push_chunk(&String::from_utf8_lossy(&chunk)) {
                if data == "[DONE]" {
                    break 'outer;
                }
                process_openai_stream_event(
                    &data,
                    text_tx,
                    &mut text,
                    &mut stop_reason,
                    &mut usage,
                    &mut tool_calls,
                );
            }
        }
        for data in sse.finish() {
            if data == "[DONE]" {
                break;
            }
            process_openai_stream_event(
                &data,
                text_tx,
                &mut text,
                &mut stop_reason,
                &mut usage,
                &mut tool_calls,
            );
        }

        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(ResponseContentBlock::Text { text });
        }
        for (_index, tool) in tool_calls {
            content.push(ResponseContentBlock::ToolUse {
                id: tool.id,
                name: tool.name,
                input: parse_tool_input(&tool.input_json),
                thought_signature: tool.thought_signature,
            });
        }
        if content.is_empty() {
            content.push(ResponseContentBlock::Text {
                text: String::new(),
            });
        }

        let has_tool_calls = content
            .iter()
            .any(|b| matches!(b, ResponseContentBlock::ToolUse { .. }));

        Ok(MessagesResponse {
            content,
            stop_reason: oai_stop_reason_from_content(stop_reason.as_deref(), has_tool_calls),
            usage,
        })
    }
}

// ---------------------------------------------------------------------------
// Google Gemini native provider
// ---------------------------------------------------------------------------

pub struct GeminiProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    thinking_enabled: bool,
    show_thinking: bool,
}

impl GeminiProvider {
    pub fn new(config: &Config) -> Self {
        GeminiProvider {
            http: default_llm_http_client(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            thinking_enabled: config.llm_thinking_enabled,
            show_thinking: config.show_thinking,
        }
    }

    fn generate_url(&self) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        )
    }

    fn stream_url(&self) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.model
        )
    }
}

#[derive(Debug, Deserialize)]
struct GeminiError {
    error: GeminiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorDetail {
    code: i32,
    message: String,
    #[allow(dead_code)]
    #[serde(default)]
    status: Option<String>,
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn send_message(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let messages = sanitize_messages(messages);
        let request_body = build_gemini_request(
            system,
            &messages,
            tools,
            self.max_tokens,
            &self.model,
            self.thinking_enabled,
            self.show_thinking,
        );

        let mut retries = 0u32;
        let max_retries = 3;

        loop {
            let response_res = self
                .http
                .post(&self.generate_url())
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &self.api_key)
                .json(&request_body)
                .send()
                .await;

            let response = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending Gemini request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = response.status();

            if status.is_success() {
                let body = response.text().await?;
                return parse_gemini_response(&body, self.show_thinking);
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let body = response.text().await.unwrap_or_default();
            if let Ok(err) = serde_json::from_str::<GeminiError>(&body) {
                return Err(FinallyAValueBotError::LlmApi(format!(
                    "Gemini API error {}: {}",
                    err.error.code, err.error.message
                )));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {body}"
            )));
        }
    }

    async fn send_message_stream(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        text_tx: Option<&UnboundedSender<String>>,
    ) -> Result<MessagesResponse, FinallyAValueBotError> {
        let messages = sanitize_messages(messages);
        let request_body = build_gemini_request(
            system,
            &messages,
            tools,
            self.max_tokens,
            &self.model,
            self.thinking_enabled,
            self.show_thinking,
        );

        let mut retries = 0u32;
        let max_retries = 3;

        let response = loop {
            let response_res = self
                .http
                .post(&self.stream_url())
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &self.api_key)
                .json(&request_body)
                .send()
                .await;

            let resp = match response_res {
                Ok(resp) => resp,
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2u64.pow(retries));
                        warn!(
                            "Network error sending Gemini stream request: {e}. Retrying in {:?} (attempt {retries}/{max_retries})",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(e.into());
                }
            };

            let status = resp.status();
            if status.is_success() {
                break resp;
            }

            let is_transient = status.as_u16() == 429
                || status.as_u16() == 500
                || status.as_u16() == 502
                || status.as_u16() == 503
                || status.as_u16() == 504;

            if is_transient && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(retries));
                warn!(
                    "Transient stream error status {status}, retrying in {:?} (attempt {retries}/{max_retries})",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }

            let text = resp.text().await.unwrap_or_default();
            if let Ok(err) = serde_json::from_str::<GeminiError>(&text) {
                return Err(FinallyAValueBotError::LlmApi(format!(
                    "Gemini API error {}: {}",
                    err.error.code, err.error.message
                )));
            }
            return Err(FinallyAValueBotError::LlmApi(format!(
                "HTTP {status}: {text}"
            )));
        };

        let mut byte_stream = response.bytes_stream();
        let mut sse = SseEventParser::default();
        let mut accumulated_text = String::new();
        let mut tool_calls: Vec<(String, String, serde_json::Value, Option<String>)> = Vec::new();
        let mut stop_reason: Option<String> = None;
        let mut usage: Option<Usage> = None;

        while let Some(chunk_res) = byte_stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(_) => break,
            };
            for data in sse.push_chunk(&String::from_utf8_lossy(&chunk)) {
                process_gemini_stream_event(
                    &data,
                    text_tx,
                    &mut accumulated_text,
                    &mut tool_calls,
                    &mut stop_reason,
                    &mut usage,
                    self.show_thinking,
                );
            }
        }
        for data in sse.finish() {
            process_gemini_stream_event(
                &data,
                text_tx,
                &mut accumulated_text,
                &mut tool_calls,
                &mut stop_reason,
                &mut usage,
                self.show_thinking,
            );
        }

        let has_tool_calls = !tool_calls.is_empty();
        let mut content = Vec::new();
        if !accumulated_text.is_empty() {
            content.push(ResponseContentBlock::Text {
                text: accumulated_text,
            });
        }
        for (id, name, input, thought_sig) in tool_calls {
            content.push(ResponseContentBlock::ToolUse {
                id,
                name,
                input,
                thought_signature: thought_sig,
            });
        }
        if content.is_empty() {
            content.push(ResponseContentBlock::Text {
                text: String::new(),
            });
        }

        let final_stop_reason = if has_tool_calls {
            Some("tool_use".into())
        } else {
            stop_reason
        };

        Ok(MessagesResponse {
            content,
            stop_reason: normalize_stop_reason(final_stop_reason),
            usage,
        })
    }
}

fn process_gemini_stream_event(
    data: &str,
    text_tx: Option<&UnboundedSender<String>>,
    accumulated_text: &mut String,
    tool_calls: &mut Vec<(String, String, serde_json::Value, Option<String>)>,
    stop_reason: &mut Option<String>,
    usage: &mut Option<Usage>,
    show_thinking: bool,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };

    if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                *stop_reason = Some(reason.to_string());
            }

            if let Some(content) = candidate.get("content") {
                if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                    for part in parts {
                        // Text parts (skip if thought == true)
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            let is_thought = part
                                .get("thought")
                                .and_then(|t| t.as_bool())
                                .unwrap_or(false);
                            if is_thought && !show_thinking {
                                continue;
                            }
                            let chunk = if is_thought {
                                format!("<think>{text}</think>")
                            } else {
                                text.to_string()
                            };
                            if !chunk.is_empty() {
                                accumulated_text.push_str(&chunk);
                                if let Some(tx) = text_tx {
                                    let _ = tx.send(chunk);
                                }
                            }
                        }
                        // Function call parts
                        if let Some(fc) = part.get("functionCall") {
                            if let (Some(name), Some(args)) =
                                (fc.get("name").and_then(|n| n.as_str()), fc.get("args"))
                            {
                                let id = Uuid::new_v4().to_string();
                                let thought_sig = part
                                    .get("thoughtSignature")
                                    .or_else(|| fc.get("thoughtSignature"))
                                    .and_then(|ts| ts.as_str())
                                    .map(String::from);
                                tool_calls.push((id, name.to_string(), args.clone(), thought_sig));
                            }
                        }
                    }
                }
            }
        }
    }

    // Capture usage from any chunk
    if usage.is_none() {
        if let Some(metadata) = v.get("usageMetadata") {
            *usage = Some(Usage {
                input_tokens: metadata
                    .get("promptTokenCount")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
                output_tokens: metadata
                    .get("candidatesTokenCount")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            });
        }
    }
}

fn gemini_thinking_config(
    model: &str,
    thinking_enabled: bool,
    include_thoughts: bool,
) -> Option<serde_json::Value> {
    let m = model.trim().to_ascii_lowercase();
    if !thinking_enabled {
        if m.contains("2.5-flash") && !m.contains("pro") {
            return Some(json!({ "thinkingBudget": 0 }));
        }
        return None;
    }
    if m.contains("gemini-3") {
        Some(json!({
            "thinkingLevel": "MEDIUM",
            "includeThoughts": include_thoughts,
        }))
    } else if m.contains("2.5") {
        Some(json!({
            "thinkingBudget": -1,
            "includeThoughts": include_thoughts,
        }))
    } else {
        None
    }
}

fn build_gemini_request(
    system: &str,
    messages: &[Message],
    tools: Option<Vec<ToolDefinition>>,
    max_tokens: u32,
    model: &str,
    thinking_enabled: bool,
    show_thinking: bool,
) -> serde_json::Value {
    let contents = translate_messages_to_gemini(messages);
    let mut generation_config = json!({
        "maxOutputTokens": max_tokens,
    });
    if let Some(thinking_config) = gemini_thinking_config(model, thinking_enabled, show_thinking) {
        generation_config["thinkingConfig"] = thinking_config;
    }
    let mut request = json!({
        "contents": contents,
        "generationConfig": generation_config,
    });

    if !system.is_empty() {
        request["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }

    if let Some(tool_defs) = tools {
        if !tool_defs.is_empty() {
            let func_decls: Vec<serde_json::Value> = tool_defs
                .iter()
                .map(|t| {
                    let parameters = sanitize_oai_parameters(&t.input_schema);
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": parameters,
                    })
                })
                .collect();
            request["tools"] = json!([{ "functionDeclarations": func_decls }]);
        }
    }

    request
}

fn translate_messages_to_gemini(messages: &[Message]) -> Vec<serde_json::Value> {
    // Build id → name map from all tool_use blocks
    let mut tool_id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in messages {
        if msg.role == "assistant" {
            if let MessageContent::Blocks(blocks) = &msg.content {
                for block in blocks {
                    if let ContentBlock::ToolUse { id, name, .. } = block {
                        tool_id_to_name.insert(id.clone(), name.clone());
                    }
                }
            }
        }
    }

    let mut out: Vec<serde_json::Value> = Vec::new();

    for msg in messages {
        let role = if msg.role == "assistant" {
            "model"
        } else {
            "user"
        };

        match &msg.content {
            MessageContent::Text(text) => {
                let content = if text.is_empty() {
                    GEMINI_MINIMAL_PARTS.to_string()
                } else {
                    text.clone()
                };
                out.push(json!({
                    "role": role,
                    "parts": [{ "text": content }]
                }));
            }
            MessageContent::Blocks(blocks) => {
                let mut parts: Vec<serde_json::Value> = Vec::new();

                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            parts.push(json!({ "text": text }));
                        }
                        ContentBlock::ToolUse {
                            name,
                            input,
                            thought_signature,
                            ..
                        } => {
                            let sig = thought_signature
                                .clone()
                                .unwrap_or_else(|| GEMINI_SKIP_THOUGHT_SIGNATURE.to_string());
                            let mut part = json!({
                                "functionCall": {
                                    "name": name,
                                    "args": input,
                                }
                            });
                            part["thoughtSignature"] = json!(sig);
                            parts.push(part);
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            // Look up function name
                            if let Some(fn_name) = tool_id_to_name.get(tool_use_id) {
                                let response = if is_error == &Some(true) {
                                    json!({"error": content})
                                } else {
                                    json!({"result": content})
                                };
                                parts.push(json!({
                                    "functionResponse": {
                                        "name": fn_name,
                                        "response": response,
                                    }
                                }));
                            }
                        }
                        ContentBlock::Image { source } => {
                            parts.push(json!({
                                "inlineData": {
                                    "mimeType": source.media_type,
                                    "data": source.data,
                                }
                            }));
                        }
                    }
                }

                if parts.is_empty() {
                    parts.push(json!({ "text": GEMINI_MINIMAL_PARTS }));
                }

                out.push(json!({
                    "role": role,
                    "parts": parts,
                }));
            }
        }
    }

    // Ensure first message is "user"
    if !out.is_empty() {
        let first_role = out[0].get("role").and_then(|r| r.as_str());
        if first_role != Some("user") {
            out.insert(
                0,
                json!({"role": "user", "parts": [{"text": GEMINI_MINIMAL_PARTS}]}),
            );
        }
    }

    out
}

fn parse_gemini_response(
    body: &str,
    show_thinking: bool,
) -> Result<MessagesResponse, FinallyAValueBotError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        FinallyAValueBotError::LlmApi(format!(
            "Failed to parse Gemini response: {e}\nBody: {body}"
        ))
    })?;

    let mut content = Vec::new();
    let mut stop_reason: Option<String> = None;
    let mut usage: Option<Usage> = None;

    if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                stop_reason = Some(reason.to_string());
            }

            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                let mut has_tool_calls = false;
                for part in parts {
                    // Text parts (skip if thought == true)
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        let is_thought = part
                            .get("thought")
                            .and_then(|t| t.as_bool())
                            .unwrap_or(false);
                        if is_thought && !show_thinking {
                            continue;
                        }
                        let rendered = if is_thought {
                            format!("<think>{text}</think>")
                        } else {
                            text.to_string()
                        };
                        if !rendered.is_empty() {
                            content.push(ResponseContentBlock::Text { text: rendered });
                        }
                    }
                    // Function call parts
                    if let Some(fc) = part.get("functionCall") {
                        has_tool_calls = true;
                        if let (Some(name), Some(args)) =
                            (fc.get("name").and_then(|n| n.as_str()), fc.get("args"))
                        {
                            let id = Uuid::new_v4().to_string();
                            let thought_sig = part
                                .get("thoughtSignature")
                                .or_else(|| fc.get("thoughtSignature"))
                                .and_then(|ts| ts.as_str())
                                .map(String::from);
                            content.push(ResponseContentBlock::ToolUse {
                                id,
                                name: name.to_string(),
                                input: args.clone(),
                                thought_signature: thought_sig,
                            });
                        }
                    }
                }

                // Override stop_reason if tool calls present
                if has_tool_calls {
                    stop_reason = Some("tool_use".into());
                }
            }
        }
    }

    // Capture usage
    if let Some(metadata) = v.get("usageMetadata") {
        usage = Some(Usage {
            input_tokens: metadata
                .get("promptTokenCount")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: metadata
                .get("candidatesTokenCount")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        });
    }

    if content.is_empty() {
        content.push(ResponseContentBlock::Text {
            text: String::new(),
        });
    }

    Ok(MessagesResponse {
        content,
        stop_reason: normalize_stop_reason(stop_reason),
        usage,
    })
}

// ---------------------------------------------------------------------------
// Format translation helpers  (internal Anthropic-style ↔ OpenAI)
// ---------------------------------------------------------------------------

/// Minimal non-whitespace content so Vertex AI / Gemini gateways never treat the prompt as "no parts".
const GEMINI_MINIMAL_PARTS: &str = ".";

/// When Gemini returns a functionCall without thoughtSignature, or we cannot preserve it (e.g. after
/// session load), use this dummy value so Gemini skips signature validation. Using a random UUID
/// would cause "Corrupted thought signature" because Gemini expects the exact signature it returned.
const GEMINI_SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

fn translate_messages_to_oai(system: &str, messages: &[Message]) -> Vec<serde_json::Value> {
    // Collect all tool_use IDs present in assistant messages so we can
    // skip orphaned tool_results (e.g. after session compaction).
    let known_tool_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();

    let mut out: Vec<serde_json::Value> = Vec::new();

    // System message. Gemini requires at least one "parts" in the request; empty content can cause INVALID_ARGUMENT.
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }

    for msg in messages {
        match &msg.content {
            MessageContent::Text(text) => {
                // Ensure non-empty content so Gemini/Vertex receives at least one parts field.
                let content = if text.is_empty() {
                    GEMINI_MINIMAL_PARTS
                } else {
                    text.as_str()
                };
                out.push(json!({"role": msg.role, "content": content}));
            }
            MessageContent::Blocks(blocks) => {
                if msg.role == "assistant" {
                    // Collect text and tool_calls
                    let text: String = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    let tool_calls: Vec<serde_json::Value> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                thought_signature,
                            } => {
                                let args = serde_json::to_string(input).unwrap_or_default();
                                // Gemini requires thought_signature in function calls. Use preserved
                                // signature if available; otherwise use skip value to avoid "Corrupted
                                // thought signature" (a random UUID would fail validation).
                                let sig = thought_signature
                                    .clone()
                                    .unwrap_or_else(|| GEMINI_SKIP_THOUGHT_SIGNATURE.to_string());

                                let tool_call = json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args,
                                        "thought_signature": &sig,
                                        "thoughtSignature": &sig,
                                    },
                                    "thought_signature": &sig,
                                    "thoughtSignature": &sig,
                                });
                                Some(tool_call)
                            }
                            _ => None,
                        })
                        .collect();

                    let mut m = json!({"role": "assistant"});
                    // Gemini/Vertex require every message to have at least one "parts"; assistant with only
                    // tool_calls has no content otherwise, so always set non-whitespace placeholder when empty.
                    let content = if text.is_empty() {
                        GEMINI_MINIMAL_PARTS
                    } else {
                        text.as_str()
                    };
                    m["content"] = json!(content);
                    if !tool_calls.is_empty() {
                        m["tool_calls"] = json!(tool_calls);
                    }
                    out.push(m);
                } else {
                    // User role — tool_results, images, or text
                    let has_tool_results = blocks
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result → separate "tool" message
                        // Skip orphaned tool_results whose IDs are not in any assistant message
                        for block in blocks {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } = block
                            {
                                if !known_tool_ids.contains(tool_use_id.as_str()) {
                                    continue;
                                }
                                let c = if is_error == &Some(true) {
                                    format!("[Error] {content}")
                                } else {
                                    content.clone()
                                };
                                out.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": c,
                                }));
                            }
                        }
                    } else {
                        // Images + text → multipart content array
                        let has_images = blocks
                            .iter()
                            .any(|b| matches!(b, ContentBlock::Image { .. }));
                        if has_images {
                            let parts: Vec<serde_json::Value> = blocks
                                .iter()
                                .filter_map(|b| match b {
                                    ContentBlock::Text { text } => {
                                        Some(json!({"type": "text", "text": text}))
                                    }
                                    ContentBlock::Image {
                                        source:
                                            ImageSource {
                                                media_type, data, ..
                                            },
                                    } => {
                                        let url = format!("data:{media_type};base64,{data}");
                                        Some(json!({
                                            "type": "image_url",
                                            "image_url": {"url": url}
                                        }))
                                    }
                                    _ => None,
                                })
                                .collect();
                            // Gemini/Vertex require at least one part; use non-whitespace placeholder if empty.
                            let content = if parts.is_empty() {
                                json!([{"type": "text", "text": GEMINI_MINIMAL_PARTS}])
                            } else {
                                json!(parts)
                            };
                            out.push(json!({"role": "user", "content": content}));
                        } else {
                            let text: String = blocks
                                .iter()
                                .filter_map(|b| match b {
                                    ContentBlock::Text { text } => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            let content = if text.is_empty() {
                                GEMINI_MINIMAL_PARTS.to_string()
                            } else {
                                text
                            };
                            out.push(json!({"role": "user", "content": content}));
                        }
                    }
                }
            }
        }
    }

    // Gemini/Vertex require the prompt to have at least one "parts" field. Only prepend a
    // placeholder user message when the first message is assistant or tool (so the prompt would
    // have no user content). When the first message is system, gateways typically put it in
    // system_instruction and use the next user message as the prompt — so we must not prepend
    // a fake "user: ." or the model will respond to "." and may return nothing for the real request.
    let first_role = out
        .first()
        .and_then(|m| m.get("role").and_then(|r| r.as_str()));
    if !out.is_empty()
        && first_role != Some("user")
        && (first_role == Some("assistant") || first_role == Some("tool"))
    {
        out.insert(0, json!({"role": "user", "content": GEMINI_MINIMAL_PARTS}));
    }

    out
}

/// Sanitize tool parameters schema so that `required` only lists keys present in
/// `properties`. Google Gemini rejects schemas where required[i] is not in properties.
/// MCP and other sources can send schemas with missing properties or required not in properties.
/// Strips "enum" from each property so gateways (e.g. OpenRouter→Gemini) don't drop the
/// property and leave required referencing missing keys.
fn sanitize_oai_parameters(schema: &serde_json::Value) -> serde_json::Value {
    let props_raw = schema
        .get("properties")
        .filter(|p| p.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    // Build properties with "enum" stripped per property; some gateways drop properties
    // that contain "enum" when converting to Gemini, causing required to reference missing keys.
    let props: serde_json::Map<String, serde_json::Value> = props_raw
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let cleaned = if let Some(obj) = v.as_object() {
                        let mut copy = serde_json::Map::new();
                        for (pk, pv) in obj {
                            if pk != "enum" {
                                copy.insert(pk.clone(), pv.clone());
                            }
                        }
                        serde_json::Value::Object(copy)
                    } else {
                        v.clone()
                    };
                    (k.clone(), cleaned)
                })
                .collect()
        })
        .unwrap_or_default();
    let props_keys: std::collections::HashSet<String> = props.keys().cloned().collect();
    let required_filtered: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|k| props_keys.contains(*k))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    // Deduplicate and sort required for deterministic output; omit key when empty (Gemini-safe).
    let required_set: std::collections::HashSet<String> = required_filtered.into_iter().collect();
    let mut required_vec: Vec<String> = required_set.into_iter().collect();
    required_vec.sort();

    let mut out = serde_json::Map::new();
    out.insert(
        "type".to_string(),
        schema.get("type").cloned().unwrap_or(json!("object")),
    );
    out.insert("properties".to_string(), serde_json::Value::Object(props));
    if !required_vec.is_empty() {
        out.insert("required".to_string(), json!(required_vec));
    }
    serde_json::Value::Object(out)
}

fn translate_tools_to_oai(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            let parameters = sanitize_oai_parameters(&t.input_schema);
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": parameters,
                }
            })
        })
        .collect()
}

fn translate_oai_response(oai: OaiResponse) -> MessagesResponse {
    let choice = match oai.choices.into_iter().next() {
        Some(c) => c,
        None => {
            return MessagesResponse {
                content: vec![ResponseContentBlock::Text {
                    text: "(empty response)".into(),
                }],
                stop_reason: Some("end_turn".into()),
                usage: None,
            };
        }
    };

    let mut content = Vec::new();
    let tool_calls_from_api = choice.message.tool_calls.is_some();

    let mut text_content = choice.message.content.unwrap_or_default();

    if !tool_calls_from_api {
        let markup_tools = parse_embedded_tool_calls_from_content(&text_content);
        if !markup_tools.is_empty() {
            text_content = strip_tools_markup_from_content(&text_content);
            for (idx, (name, input)) in markup_tools.into_iter().enumerate() {
                content.push(ResponseContentBlock::ToolUse {
                    id: format!("embedded_tool_{idx}_{}", Uuid::new_v4()),
                    name,
                    input,
                    thought_signature: None,
                });
            }
        }
    }

    if !text_content.is_empty() {
        content.push(ResponseContentBlock::Text { text: text_content });
    }

    if let Some(tool_calls) = choice.message.tool_calls {
        for tc in tool_calls {
            let input: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_default();
            let thought_signature = tc
                .thought_signature
                .clone()
                .or(tc.function.thought_signature.clone());
            content.push(ResponseContentBlock::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
                thought_signature,
            });
        }
    }

    if content.is_empty() {
        content.push(ResponseContentBlock::Text {
            text: String::new(),
        });
    }

    let has_tool_calls = content
        .iter()
        .any(|b| matches!(b, ResponseContentBlock::ToolUse { .. }));

    let stop_reason = oai_stop_reason_from_content(choice.finish_reason.as_deref(), has_tool_calls);

    let usage = oai.usage.map(|u| Usage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
    });

    MessagesResponse {
        content,
        stop_reason,
        usage,
    }
}

/// Qwen-Coder via llama.cpp may return `<tools>{ "name": "...", "arguments": {...} }</tools>` in content.
fn parse_embedded_tool_calls_from_content(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    let mut rest = text;
    const OPEN: &str = "<tools>";
    const CLOSE: &str = "</tools>";
    while let Some(start) = rest.find(OPEN) {
        let after_open = &rest[start + OPEN.len()..];
        let Some(close_idx) = after_open.find(CLOSE) else {
            break;
        };
        let inner = after_open[..close_idx].trim();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(inner) {
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                let input = v.get("arguments").cloned().unwrap_or_else(|| json!({}));
                out.push((name.to_string(), input));
            } else if let Some(arr) = v.as_array() {
                for item in arr {
                    if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                        let input = item.get("arguments").cloned().unwrap_or_else(|| json!({}));
                        out.push((name.to_string(), input));
                    }
                }
            }
        }
        rest = &after_open[close_idx + CLOSE.len()..];
    }
    out
}

fn strip_tools_markup_from_content(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(start) = out.find("<tools>") {
        let Some(rel_end) = out[start..].find("</tools>") else {
            break;
        };
        let end = start + rel_end + "</tools>".len();
        out.replace_range(start..end, "");
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_oai_uses_max_completion_tokens() {
        assert!(oai_uses_max_completion_tokens("gpt-5.4"));
        assert!(oai_uses_max_completion_tokens("o3-mini"));
        assert!(oai_uses_max_completion_tokens("grok-4.20-0309-reasoning"));
        assert!(!oai_uses_max_completion_tokens("gpt-4o"));
        assert!(!oai_uses_max_completion_tokens("grok-4.3"));
    }

    #[test]
    fn test_build_oai_chat_request_body_token_fields() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let legacy = build_oai_chat_request_body("gpt-4o", 100, &msgs, None, None, false, false);
        assert!(legacy.get("max_tokens").is_some());
        assert!(legacy.get("max_completion_tokens").is_none());
        let modern = build_oai_chat_request_body("gpt-5.2", 100, &msgs, None, None, false, true);
        assert!(modern.get("max_completion_tokens").is_some());
        assert!(modern.get("max_tokens").is_none());
    }

    #[test]
    fn test_build_oai_chat_request_body_tool_choice() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let tools = vec![ToolDefinition::new("add", "add", json!({"type": "object"}))];
        let body = build_oai_chat_request_body(
            "qwen",
            100,
            &msgs,
            Some(&tools),
            Some("required"),
            false,
            false,
        );
        assert_eq!(body["tool_choice"], json!("required"));
        assert!(body.get("tools").is_some());
    }

    #[test]
    fn parse_openai_compatible_model_ids_reads_data_array() {
        let parsed = json!({
            "data": [
                {"id": "gpt-5.4"},
                {"id": "gpt-4o"},
                {"id": "gpt-4o"}
            ]
        });
        assert_eq!(
            parse_openai_compatible_model_ids(&parsed),
            vec!["gpt-4o".to_string(), "gpt-5.4".to_string()]
        );
    }

    #[test]
    fn parse_gemini_native_model_ids_keeps_generate_content() {
        let parsed = json!({
            "models": [
                {
                    "name": "models/gemini-2.5-flash",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/text-embedding-004",
                    "supportedGenerationMethods": ["embedContent"]
                }
            ]
        });
        assert_eq!(
            parse_gemini_native_model_ids(&parsed),
            vec!["models/gemini-2.5-flash".to_string()]
        );
    }

    #[test]
    fn test_parse_embedded_tool_calls_from_qwen_markup() {
        let raw = r#"<tools>
{
  "name": "add",
  "arguments": { "a": 2, "b": 3 }
}
</tools>"#;
        let parsed = parse_embedded_tool_calls_from_content(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "add");
        assert_eq!(parsed[0].1["a"], 2);
    }

    // -----------------------------------------------------------------------
    // translate_messages_to_oai
    // -----------------------------------------------------------------------

    #[test]
    fn test_translate_messages_system_only() {
        let msgs: Vec<Message> = vec![];
        let out = translate_messages_to_oai("You are a bot.", &msgs);
        // We only prepend for assistant/tool; system-first is left as-is so gateways use first user as prompt.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], "You are a bot.");
    }

    #[test]
    fn test_translate_messages_empty_system_omitted() {
        let msgs: Vec<Message> = vec![];
        let out = translate_messages_to_oai("", &msgs);
        assert!(out.is_empty());
    }

    #[test]
    fn test_translate_messages_text_roundtrip() {
        let msgs = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text("hello".into()),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("hi".into()),
            },
        ];
        let out = translate_messages_to_oai("sys", &msgs);
        // We do not prepend when first is system so the first user message is the prompt.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[1]["role"], "user");
        assert_eq!(out[1]["content"], "hello");
        assert_eq!(out[2]["role"], "assistant");
        assert_eq!(out[2]["content"], "hi");
    }

    #[test]
    fn test_translate_messages_assistant_tool_use() {
        let msgs = vec![Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "Let me check.".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                    thought_signature: None,
                },
            ]),
        }];
        let out = translate_messages_to_oai("", &msgs);
        assert_eq!(out.len(), 2); // placeholder user first (Gemini), then assistant
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], GEMINI_MINIMAL_PARTS);
        assert_eq!(out[1]["role"], "assistant");
        assert_eq!(out[1]["content"], "Let me check.");
        let tc = out[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["id"], "t1");
        assert_eq!(tc[0]["function"]["name"], "bash");
    }

    #[test]
    fn test_translate_messages_tool_result() {
        let msgs = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "glob".into(),
                    input: json!({}),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "file1.rs\nfile2.rs".into(),
                    is_error: None,
                }]),
            },
        ];
        let out = translate_messages_to_oai("", &msgs);
        // placeholder user first (Gemini), then assistant + tool
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], GEMINI_MINIMAL_PARTS);
        assert_eq!(out[1]["role"], "assistant");
        assert_eq!(out[2]["role"], "tool");
        assert_eq!(out[2]["tool_call_id"], "t1");
        assert_eq!(out[2]["content"], "file1.rs\nfile2.rs");
    }

    #[test]
    fn test_translate_messages_tool_result_error() {
        let msgs = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "glob".into(),
                    input: json!({}),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "not found".into(),
                    is_error: Some(true),
                }]),
            },
        ];
        let out = translate_messages_to_oai("", &msgs);
        // Prepend placeholder user (Gemini): [user, assistant, tool]
        assert_eq!(out.len(), 3);
        assert_eq!(out[2]["role"], "tool");
        assert_eq!(out[2]["content"], "[Error] not found");
    }

    #[test]
    fn test_translate_messages_orphaned_tool_result_skipped() {
        // tool_result without matching tool_use should be stripped
        let msgs = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "orphan_id".into(),
                content: "stale result".into(),
                is_error: None,
            }]),
        }];
        let out = translate_messages_to_oai("", &msgs);
        assert!(out.is_empty());
    }

    #[test]
    fn test_translate_messages_image_block() {
        let msgs = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![
                ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
                ContentBlock::Text {
                    text: "describe".into(),
                },
            ]),
        }];
        let out = translate_messages_to_oai("", &msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        let content = out[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image_url");
        assert!(content[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "describe");
    }

    // -----------------------------------------------------------------------
    // translate_tools_to_oai
    // -----------------------------------------------------------------------

    #[test]
    fn test_translate_tools_to_oai() {
        let tools = vec![ToolDefinition::new(
            "bash",
            "Run bash",
            json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
        )];
        let out = translate_tools_to_oai(&tools);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "function");
        assert_eq!(out[0]["function"]["name"], "bash");
        assert_eq!(out[0]["function"]["description"], "Run bash");
    }

    #[test]
    fn test_sanitize_oai_parameters_required_filtered() {
        // Gemini rejects required[] that are not in properties. Sanitizer should filter.
        let schema = json!({
            "type": "object",
            "properties": { "a": {"type": "string"}, "b": {"type": "integer"} },
            "required": ["a", "missing1", "b", "missing2"]
        });
        let out = sanitize_oai_parameters(&schema);
        let required = out["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
        assert_eq!(required[0], "a");
        assert_eq!(required[1], "b");
    }

    #[test]
    fn test_sanitize_oai_parameters_no_properties_or_invalid() {
        // MCP etc can send schema without properties or with required not in properties; output safe schema.
        let schema = json!({
            "type": "object",
            "required": ["ghost1", "ghost2"]
        });
        let out = sanitize_oai_parameters(&schema);
        // When no valid required keys, we omit "required" or send empty.
        assert!(
            out.get("required")
                .map(|r| r.as_array().unwrap().len())
                .unwrap_or(0)
                == 0
        );
        assert!(out.get("properties").is_some());
    }

    #[test]
    fn test_sanitize_oai_parameters_strips_enum_keeps_required() {
        // Schema with enum (e.g. write_tiered_memory) must still have required ⊆ properties
        // after sanitize, so Gemini/OpenRouter don't reject (required[0] not defined).
        let schema = json!({
            "type": "object",
            "properties": {
                "tier": { "type": "integer", "description": "Tier", "enum": [1, 2, 3] },
                "content": { "type": "string", "description": "Content" }
            },
            "required": ["tier", "content"]
        });
        let out = sanitize_oai_parameters(&schema);
        let props = out["properties"].as_object().unwrap();
        assert!(props.contains_key("tier"));
        assert!(props.contains_key("content"));
        assert!(
            props["tier"].get("enum").is_none(),
            "enum should be stripped"
        );
        let required = out["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
        assert!(required
            .iter()
            .all(|r| props.contains_key(r.as_str().unwrap())));
    }

    // -----------------------------------------------------------------------
    // translate_oai_response
    // -----------------------------------------------------------------------

    #[test]
    fn test_translate_oai_response_text() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: Some("Hello!".into()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(OaiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.content.len(), 1);
        match &resp.content[0] {
            ResponseContentBlock::Text { text } => assert_eq!(text, "Hello!"),
            _ => panic!("Expected Text"),
        }
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_translate_oai_response_tool_calls() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: None,
                    tool_calls: Some(vec![OaiToolCall {
                        id: "call_1".into(),
                        function: OaiFunction {
                            name: "bash".into(),
                            arguments: r#"{"command":"ls"}"#.into(),
                            thought_signature: None,
                        },
                        thought_signature: None,
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        match &resp.content[0] {
            ResponseContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            _ => panic!("Expected ToolUse"),
        }
    }

    #[test]
    fn test_translate_oai_response_empty_choices() {
        let oai = OaiResponse {
            choices: vec![],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        match &resp.content[0] {
            ResponseContentBlock::Text { text } => assert_eq!(text, "(empty response)"),
            _ => panic!("Expected Text"),
        }
    }

    #[test]
    fn test_translate_oai_response_length_stop() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: Some("partial".into()),
                    tool_calls: None,
                },
                finish_reason: Some("length".into()),
            }],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("max_tokens"));
    }

    #[test]
    fn test_translate_oai_response_text_and_tool_calls() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: Some("thinking...".into()),
                    tool_calls: Some(vec![OaiToolCall {
                        id: "c1".into(),
                        function: OaiFunction {
                            name: "read_file".into(),
                            arguments: r#"{"path":"/tmp/x"}"#.into(),
                            thought_signature: None,
                        },
                        thought_signature: None,
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.content.len(), 2);
        match &resp.content[0] {
            ResponseContentBlock::Text { text } => assert_eq!(text, "thinking..."),
            _ => panic!("Expected Text"),
        }
        match &resp.content[1] {
            ResponseContentBlock::ToolUse { name, .. } => assert_eq!(name, "read_file"),
            _ => panic!("Expected ToolUse"),
        }
    }

    #[test]
    fn test_translate_oai_response_xai_stop_with_tool_calls() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: None,
                    tool_calls: Some(vec![OaiToolCall {
                        id: "call_abc".into(),
                        function: OaiFunction {
                            name: "bash".into(),
                            arguments: r#"{"command":"ls"}"#.into(),
                            thought_signature: None,
                        },
                        thought_signature: None,
                    }]),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert!(resp.content.iter().any(|b| matches!(
            b,
            ResponseContentBlock::ToolUse { name, .. } if name == "bash"
        )));
    }

    #[test]
    fn test_translate_oai_response_xai_completed_with_tool_calls() {
        let oai = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiMessage {
                    content: None,
                    tool_calls: Some(vec![OaiToolCall {
                        id: "call_xyz".into(),
                        function: OaiFunction {
                            name: "read_file".into(),
                            arguments: "{}".into(),
                            thought_signature: None,
                        },
                        thought_signature: None,
                    }]),
                },
                finish_reason: Some("completed".into()),
            }],
            usage: None,
        };
        let resp = translate_oai_response(oai);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn test_normalize_stop_reason_stream_variants() {
        assert_eq!(
            normalize_stop_reason(Some("tool_calls".into())).as_deref(),
            Some("tool_use")
        );
        assert_eq!(
            normalize_stop_reason(Some("length".into())).as_deref(),
            Some("max_tokens")
        );
        assert_eq!(
            normalize_stop_reason(Some("stop".into())).as_deref(),
            Some("end_turn")
        );
    }

    #[test]
    fn test_build_stream_response_tool_json_parsing() {
        let mut tool_blocks = std::collections::HashMap::new();
        tool_blocks.insert(
            0,
            StreamToolUseBlock {
                id: "call_1".into(),
                name: "bash".into(),
                input_json: r#"{"command":"ls","cwd":"/tmp"}"#.into(),
                thought_signature: None,
            },
        );
        let resp = build_stream_response(
            vec![0],
            std::collections::HashMap::new(),
            tool_blocks,
            Some("tool_use".into()),
            None,
        );
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        match &resp.content[0] {
            ResponseContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
                assert_eq!(input["cwd"], "/tmp");
            }
            _ => panic!("Expected ToolUse"),
        }
    }

    // -----------------------------------------------------------------------
    // create_provider
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_provider_anthropic() {
        let mut config = crate::config::test_config();
        config.workspace_dir = "/tmp".into();
        // Should not panic
        let _provider = create_provider(&config);
    }

    #[test]
    fn test_create_provider_openai() {
        let mut config = crate::config::test_config();
        config.workspace_dir = "/tmp".into();
        config.llm_provider = "openai".into();
        config.model = "gpt-5.2".into();
        let _provider = create_provider(&config);
    }

    #[test]
    fn test_translate_messages_user_text_blocks_no_images_no_tool_results() {
        // User message with only text blocks (no images, no tool results) → plain text
        let msgs = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "first".into(),
                },
                ContentBlock::Text {
                    text: "second".into(),
                },
            ]),
        }];
        let out = translate_messages_to_oai("", &msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "first\nsecond");
    }

    // -----------------------------------------------------------------------
    // sanitize_messages
    // -----------------------------------------------------------------------

    #[test]
    fn test_sanitize_messages_removes_orphaned_tool_results() {
        let msgs = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "ok".into(),
                        is_error: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "orphan".into(),
                        content: "stale".into(),
                        is_error: None,
                    },
                ]),
            },
        ];
        let sanitized = sanitize_messages(msgs);
        assert_eq!(sanitized.len(), 2);
        // The user message should only contain t1's result
        if let MessageContent::Blocks(blocks) = &sanitized[1].content {
            assert_eq!(blocks.len(), 1);
            if let ContentBlock::ToolResult { tool_use_id, .. } = &blocks[0] {
                assert_eq!(tool_use_id, "t1");
            } else {
                panic!("Expected ToolResult");
            }
        } else {
            panic!("Expected Blocks");
        }
    }

    #[test]
    fn test_sanitize_messages_drops_empty_user_message() {
        // User message with only orphaned tool_results → dropped entirely
        let msgs = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "orphan".into(),
                content: "stale".into(),
                is_error: None,
            }]),
        }];
        let sanitized = sanitize_messages(msgs);
        assert!(sanitized.is_empty());
    }

    #[test]
    fn test_sanitize_messages_preserves_text_messages() {
        let msgs = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text("hello".into()),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("hi".into()),
            },
        ];
        let sanitized = sanitize_messages(msgs);
        assert_eq!(sanitized.len(), 2);
    }

    #[test]
    fn test_sse_event_parser_multiline_data() {
        let mut parser = SseEventParser::default();
        let events = parser
            .push_chunk("event: message\n: keep-alive\ndata: {\"type\":\"x\",\ndata: \"v\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], "{\"type\":\"x\",\n\"v\":1}");
    }

    #[test]
    fn test_sse_event_parser_finish_flushes_unterminated_event() {
        let mut parser = SseEventParser::default();
        let events = parser.push_chunk("data: hello");
        assert!(events.is_empty());
        let tail = parser.finish();
        assert_eq!(tail, vec!["hello".to_string()]);
    }

    // -----------------------------------------------------------------------
    // translate_messages_to_gemini
    // -----------------------------------------------------------------------

    #[test]
    fn test_translate_messages_to_gemini_text() {
        let msgs = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text("hello".into()),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("hi".into()),
            },
        ];
        let out = translate_messages_to_gemini(&msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["parts"][0]["text"], "hello");
        assert_eq!(out[1]["role"], "model");
        assert_eq!(out[1]["parts"][0]["text"], "hi");
    }

    #[test]
    fn test_translate_messages_to_gemini_tool_use_and_result() {
        let msgs = vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                    thought_signature: Some("sig_abc".into()),
                }]),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "file1.txt\nfile2.txt".into(),
                    is_error: None,
                }]),
            },
        ];
        let out = translate_messages_to_gemini(&msgs);
        // Should have: user placeholder, assistant with tool_use, user with tool_result
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["role"], "user"); // placeholder
        assert_eq!(out[1]["role"], "model");
        let fc = &out[1]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "bash");
        assert_eq!(out[1]["parts"][0]["thoughtSignature"], "sig_abc");

        assert_eq!(out[2]["role"], "user");
        let fr = &out[2]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "bash");
        assert_eq!(fr["response"]["result"], "file1.txt\nfile2.txt");
    }

    #[test]
    fn test_translate_messages_to_gemini_preserves_thought_signature() {
        let msgs = vec![Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "search".into(),
                input: json!({"q": "test"}),
                thought_signature: Some("preserved_sig".into()),
            }]),
        }];
        let out = translate_messages_to_gemini(&msgs);
        assert_eq!(out[1]["parts"][0]["thoughtSignature"], "preserved_sig");
    }

    // -----------------------------------------------------------------------
    // parse_gemini_response
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_gemini_response_text() {
        let response = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "Hello, world!" }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });
        let result = parse_gemini_response(&response.to_string(), false).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ResponseContentBlock::Text { text } => assert_eq!(text, "Hello, world!"),
            _ => panic!("Expected Text"),
        }
        assert_eq!(result.usage.unwrap().input_tokens, 10);
    }

    #[test]
    fn test_parse_gemini_response_function_call() {
        let response = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "search_web",
                            "args": { "query": "rust programming" }
                        },
                        "thoughtSignature": "sig_xyz"
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 20,
                "candidatesTokenCount": 10
            }
        });
        let result = parse_gemini_response(&response.to_string(), false).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ResponseContentBlock::ToolUse {
                id,
                name,
                input,
                thought_signature,
            } => {
                assert_eq!(name, "search_web");
                assert_eq!(input["query"], "rust programming");
                assert_eq!(thought_signature.as_deref(), Some("sig_xyz"));
                assert!(!id.is_empty()); // UUID generated
            }
            _ => panic!("Expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_gemini_response_text_and_function_call() {
        let response = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        { "text": "Let me search for that." },
                        {
                            "functionCall": {
                                "name": "search",
                                "args": { "q": "test" }
                            },
                            "thoughtSignature": "sig_abc"
                        }
                    ]
                },
                "finishReason": "STOP"
            }]
        });
        let result = parse_gemini_response(&response.to_string(), false).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(result.content.len(), 2);
        match &result.content[0] {
            ResponseContentBlock::Text { text } => {
                assert_eq!(text, "Let me search for that.")
            }
            _ => panic!("Expected Text"),
        }
        match &result.content[1] {
            ResponseContentBlock::ToolUse { name, .. } => assert_eq!(name, "search"),
            _ => panic!("Expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_gemini_response_empty() {
        let response = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": []
                },
                "finishReason": "STOP"
            }]
        });
        let result = parse_gemini_response(&response.to_string(), false).unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ResponseContentBlock::Text { text } => assert_eq!(text, ""),
            _ => panic!("Expected Text"),
        }
    }

    #[test]
    fn gemini_urls_do_not_embed_the_api_key() {
        let mut config = crate::config::test_config();
        config.workspace_dir = "/tmp".into();
        config.api_key = "super-secret-gemini-key".into();
        config.model = "gemini-2.5-flash".into();
        let provider = GeminiProvider::new(&config);
        assert!(!provider.generate_url().contains("key="));
        assert!(!provider.generate_url().contains("super-secret"));
        assert!(!provider.stream_url().contains("key="));
        assert!(!provider.stream_url().contains("super-secret"));
    }

    #[test]
    fn test_create_provider_google() {
        let mut config = crate::config::test_config();
        config.workspace_dir = "/tmp".into();
        config.llm_provider = "google".into();
        config.model = "gemini-2.5-flash".into();
        // Should not panic
        let _provider = create_provider(&config);
    }
}
