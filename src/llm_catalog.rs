//! Provider/model catalog for Web UI model selection and cost reference.
//!
//! Provider and model are chosen in Web UI (`app_settings`); API keys stay in `.env`.
//! `PROVIDER_CATALOG` is a **curated static list** used for cost hints and as a fallback when a
//! live `/models` fetch fails. Settings loads live ids via `GET /api/llm/models` (OpenAI-compatible
//! `/models`, Anthropic `/v1/models`, Gemini model list). When the active model is not in either
//! list, the API still marks it `from_active_config`. The web UI omits that row;
//! a model can be chosen only from the curated or live list.
//!
//! Pricing values are approximate list-price hints (USD per 1M tokens), not live quotes.

use std::collections::HashSet;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct CatalogModel {
    pub id: &'static str,
    /// Input USD per 1M tokens; `None` for local / unknown.
    pub input_usd_per_mtok: Option<f64>,
    /// Output USD per 1M tokens; `None` for local / unknown.
    pub output_usd_per_mtok: Option<f64>,
    /// Short tier for UI badges: free, low, standard, high, premium.
    pub cost_tier: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct CatalogProvider {
    pub id: &'static str,
    pub label: &'static str,
    pub default_base_url: &'static str,
    pub models: &'static [CatalogModel],
}

macro_rules! catalog_model {
    ($id:expr, $in:expr, $out:expr, $tier:expr) => {
        CatalogModel {
            id: $id,
            input_usd_per_mtok: $in,
            output_usd_per_mtok: $out,
            cost_tier: $tier,
        }
    };
}

pub const PROVIDER_CATALOG: &[CatalogProvider] = &[
    CatalogProvider {
        id: "openai",
        label: "OpenAI",
        default_base_url: "https://api.openai.com/v1",
        models: &[
            catalog_model!("gpt-5.4", Some(1.75), Some(14.0), "high"),
            catalog_model!("gpt-5.2", Some(1.75), Some(14.0), "high"),
            catalog_model!("gpt-5-mini", Some(0.25), Some(2.0), "low"),
        ],
    },
    CatalogProvider {
        id: "openrouter",
        label: "OpenRouter",
        default_base_url: "https://openrouter.ai/api/v1",
        models: &[
            catalog_model!("openrouter/auto", None, None, "standard"),
            catalog_model!(
                "anthropic/claude-sonnet-4.5",
                Some(3.0),
                Some(15.0),
                "standard"
            ),
            catalog_model!("openai/gpt-5.2", Some(1.75), Some(14.0), "high"),
        ],
    },
    CatalogProvider {
        id: "anthropic",
        label: "Anthropic",
        default_base_url: "",
        models: &[
            catalog_model!("claude-opus-4-7", Some(5.0), Some(25.0), "premium"),
            catalog_model!(
                "claude-opus-4-6-20260205",
                Some(15.0),
                Some(75.0),
                "premium"
            ),
            catalog_model!("claude-sonnet-4-6", Some(3.0), Some(15.0), "standard"),
            catalog_model!(
                "claude-sonnet-4-5-20250929",
                Some(3.0),
                Some(15.0),
                "standard"
            ),
            catalog_model!("claude-haiku-4-5-20251001", Some(1.0), Some(5.0), "low"),
        ],
    },
    CatalogProvider {
        id: "ollama",
        label: "Ollama (local)",
        default_base_url: "http://127.0.0.1:11434/v1",
        models: &[
            catalog_model!("llama3.2", None, None, "free"),
            catalog_model!("qwen2.5-coder:7b", None, None, "free"),
            catalog_model!("mistral", None, None, "free"),
        ],
    },
    CatalogProvider {
        id: "llama",
        label: "Llama.cpp (local)",
        default_base_url: "http://127.0.0.1:8080/v1",
        models: &[
            catalog_model!("qwen2.5-coder-14b-instruct", None, None, "free"),
            catalog_model!("mistral-nemo-12b-instruct", None, None, "free"),
            catalog_model!("local", None, None, "free"),
        ],
    },
    CatalogProvider {
        id: "google",
        label: "Google (Gemini API)",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: &[
            catalog_model!("gemini-3.1-pro-preview", Some(2.0), Some(12.0), "high"),
            catalog_model!("gemini-3-flash-preview", Some(0.50), Some(3.0), "standard"),
            catalog_model!("gemini-3.1-flash-lite", Some(0.25), Some(1.50), "low"),
            catalog_model!("gemini-2.5-pro", Some(1.25), Some(10.0), "high"),
            catalog_model!("gemini-2.5-flash", Some(0.30), Some(2.50), "low"),
            catalog_model!("gemini-2.5-flash-lite", Some(0.10), Some(0.40), "low"),
        ],
    },
    CatalogProvider {
        id: "alibaba",
        label: "Alibaba Cloud (Qwen / DashScope)",
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &[
            catalog_model!("qwen3-max", Some(1.20), Some(6.0), "standard"),
            catalog_model!("qwen-max-latest", Some(1.60), Some(6.4), "standard"),
        ],
    },
    CatalogProvider {
        id: "deepseek",
        label: "DeepSeek",
        default_base_url: "https://api.deepseek.com/v1",
        models: &[
            catalog_model!("deepseek-chat", Some(0.28), Some(0.42), "low"),
            catalog_model!("deepseek-reasoner", Some(0.55), Some(2.19), "standard"),
        ],
    },
    CatalogProvider {
        id: "moonshot",
        label: "Moonshot AI (Kimi)",
        default_base_url: "https://api.moonshot.cn/v1",
        models: &[
            catalog_model!("kimi-k2.5", Some(0.60), Some(3.0), "standard"),
            catalog_model!("kimi-k2", Some(0.40), Some(2.0), "low"),
        ],
    },
    CatalogProvider {
        id: "mistral",
        label: "Mistral AI",
        default_base_url: "https://api.mistral.ai/v1",
        models: &[
            catalog_model!("mistral-large-latest", Some(2.0), Some(6.0), "high"),
            catalog_model!("ministral-8b-latest", Some(0.10), Some(0.10), "low"),
        ],
    },
    CatalogProvider {
        id: "azure",
        label: "Microsoft Azure AI",
        default_base_url:
            "https://YOUR-RESOURCE.openai.azure.com/openai/deployments/YOUR-DEPLOYMENT",
        models: &[
            catalog_model!("gpt-5.2", Some(1.75), Some(14.0), "high"),
            catalog_model!("gpt-5", Some(1.25), Some(10.0), "standard"),
        ],
    },
    CatalogProvider {
        id: "bedrock",
        label: "Amazon AWS Bedrock",
        default_base_url: "https://bedrock-runtime.YOUR-REGION.amazonaws.com/openai/v1",
        models: &[
            catalog_model!(
                "anthropic.claude-opus-4-6-v1",
                Some(15.0),
                Some(75.0),
                "premium"
            ),
            catalog_model!(
                "anthropic.claude-sonnet-4-5-v2",
                Some(3.0),
                Some(15.0),
                "standard"
            ),
        ],
    },
    CatalogProvider {
        id: "zhipu",
        label: "Zhipu AI (GLM / Z.AI)",
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &[
            catalog_model!("glm-4.7", Some(0.50), Some(2.0), "low"),
            catalog_model!("glm-4.7-flash", Some(0.10), Some(0.40), "low"),
        ],
    },
    CatalogProvider {
        id: "minimax",
        label: "MiniMax",
        default_base_url: "https://api.minimax.io/v1",
        models: &[catalog_model!(
            "MiniMax-M2.1",
            Some(0.30),
            Some(1.20),
            "low"
        )],
    },
    CatalogProvider {
        id: "cohere",
        label: "Cohere",
        default_base_url: "https://api.cohere.ai/compatibility/v1",
        models: &[
            catalog_model!("command-a-03-2025", Some(2.50), Some(10.0), "high"),
            catalog_model!("command-r-plus-08-2024", Some(2.50), Some(10.0), "high"),
        ],
    },
    CatalogProvider {
        id: "tencent",
        label: "Tencent AI Lab",
        default_base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        models: &[
            catalog_model!("hunyuan-t1-latest", Some(1.0), Some(4.0), "standard"),
            catalog_model!("hunyuan-turbos-latest", Some(0.30), Some(1.0), "low"),
        ],
    },
    CatalogProvider {
        id: "xai",
        label: "xAI",
        default_base_url: "https://api.x.ai/v1",
        models: &[
            catalog_model!("grok-4.3", Some(1.25), Some(2.50), "standard"),
            catalog_model!("grok-4", Some(3.0), Some(15.0), "high"),
            catalog_model!("grok-3", Some(2.0), Some(10.0), "standard"),
        ],
    },
    CatalogProvider {
        id: "huggingface",
        label: "Hugging Face",
        default_base_url: "https://router.huggingface.co/v1",
        models: &[
            catalog_model!("Qwen/Qwen3-Coder-Next", None, None, "standard"),
            catalog_model!("meta-llama/Llama-3.3-70B-Instruct", None, None, "standard"),
        ],
    },
    CatalogProvider {
        id: "together",
        label: "Together AI",
        default_base_url: "https://api.together.xyz/v1",
        models: &[
            catalog_model!(
                "deepseek-ai/DeepSeek-V3",
                Some(1.25),
                Some(1.25),
                "standard"
            ),
            catalog_model!(
                "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                Some(0.88),
                Some(0.88),
                "low"
            ),
        ],
    },
    CatalogProvider {
        id: "custom",
        label: "Custom (manual config)",
        default_base_url: "",
        models: &[catalog_model!("custom-model", None, None, "standard")],
    },
];

pub const APP_SETTING_LLM_MODEL: &str = "LLM_MODEL";
pub const APP_SETTING_LLM_PROVIDER: &str = "LLM_PROVIDER";
/// Local provider (Ollama / llama.cpp) OpenAI-compatible base URL — Web UI only, not `.env`.
pub const APP_SETTING_LLM_BASE_URL: &str = "LLM_BASE_URL";
pub const APP_SETTING_LLM_THINKING_ENABLED: &str = "LLM_THINKING_ENABLED";
pub const APP_SETTING_SHOW_THINKING: &str = "SHOW_THINKING";

pub fn normalize_local_base_url(raw: &str, provider_id: &str) -> String {
    let fallback = default_base_url_for_provider(provider_id).unwrap_or("http://127.0.0.1:8080/v1");
    crate::local_delegate::normalize_base_url_for_provider(raw, fallback)
}

pub fn local_base_url_from_app_settings(
    settings: &[(String, String)],
    provider_id: &str,
) -> Option<String> {
    if !is_local_provider(provider_id) {
        return None;
    }
    settings
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(APP_SETTING_LLM_BASE_URL))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(|v| normalize_local_base_url(&v, provider_id))
}

pub fn effective_local_base_url(provider_id: &str, app_settings_url: Option<&str>) -> String {
    let provider_id = resolve_catalog_provider_id(provider_id);
    if let Some(url) = app_settings_url.map(str::trim).filter(|u| !u.is_empty()) {
        return normalize_local_base_url(url, &provider_id);
    }
    default_base_url_for_provider(&provider_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "http://127.0.0.1:8080/v1".into())
}

/// First catalog provider that has an API key in `.env` (picker order).
pub fn first_provider_with_api_key() -> Option<&'static str> {
    PROVIDER_CATALOG
        .iter()
        .find(|p| p.id != "custom" && is_api_key_configured_for_provider(p.id))
        .map(|p| p.id)
}

/// First cloud provider with an API key. Local servers are always "configured" and are skipped.
pub fn first_cloud_provider_with_api_key() -> Option<&'static str> {
    PROVIDER_CATALOG
        .iter()
        .find(|p| {
            p.id != "custom"
                && !is_local_provider(p.id)
                && !resolve_api_key_for_provider(p.id).is_empty()
        })
        .map(|p| p.id)
}

pub fn any_provider_api_key_configured() -> bool {
    first_provider_with_api_key().is_some()
}

/// Env vars checked for API keys (first non-empty wins). Shown in Web UI as hints.
pub fn provider_api_key_env_hints(provider_id: &str) -> Vec<String> {
    provider_key_env_var_list(provider_id)
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn provider_key_env_var_list(provider_id: &str) -> Vec<&'static str> {
    let p = resolve_catalog_provider_id(provider_id);
    match p.as_str() {
        "openai" => vec!["OPENAI_API_KEY"],
        "xai" => vec!["XAI_API_KEY"],
        "anthropic" => vec!["ANTHROPIC_API_KEY", "LLM_API_KEY"],
        "google" => vec!["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "openrouter" => vec!["OPENROUTER_API_KEY", "LLM_API_KEY"],
        "deepseek" => vec!["DEEPSEEK_API_KEY", "LLM_API_KEY"],
        "mistral" => vec!["MISTRAL_API_KEY", "LLM_API_KEY"],
        "cohere" => vec!["COHERE_API_KEY", "LLM_API_KEY"],
        "huggingface" => vec!["HF_TOKEN", "HUGGINGFACE_API_KEY", "LLM_API_KEY"],
        "together" => vec!["TOGETHER_API_KEY", "LLM_API_KEY"],
        "moonshot" => vec!["MOONSHOT_API_KEY", "LLM_API_KEY"],
        "alibaba" => vec!["DASHSCOPE_API_KEY", "LLM_API_KEY"],
        "minimax" => vec!["MINIMAX_API_KEY", "LLM_API_KEY"],
        "zhipu" => vec!["ZHIPU_API_KEY", "LLM_API_KEY"],
        "ollama" | "llama" | "llamacpp" | "custom" => Vec::new(),
        _ => vec!["LLM_API_KEY"],
    }
}

/// Env var name used when a key is saved from Settings → Connections.
pub fn primary_secret_env_name(provider_id: &str) -> Option<&'static str> {
    let name = provider_key_env_var_list(provider_id).into_iter().next()?;
    if name == "LLM_API_KEY" {
        None
    } else {
        Some(name)
    }
}

pub fn is_local_provider(provider_id: &str) -> bool {
    matches!(
        resolve_catalog_provider_id(provider_id).as_str(),
        "ollama" | "llama" | "llamacpp"
    )
}

fn env_key_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn first_non_empty_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for key in keys {
        if let Some(v) = env_key_non_empty(key) {
            return Some(v);
        }
    }
    None
}

/// Resolve the API key for `provider_id` from process environment (after dotenv load).
pub fn resolve_api_key_for_provider(provider_id: &str) -> String {
    resolve_api_key_for_provider_with_config(provider_id, None)
}

/// Resolve API key from `.env`, then a key saved in Settings → Connections.
/// Optional `Config` supplies fields loaded at startup (e.g. `openai_api_key`).
pub fn resolve_api_key_for_provider_with_config(
    provider_id: &str,
    cfg: Option<&crate::config::Config>,
) -> String {
    if is_local_provider(provider_id) {
        return String::new();
    }
    let p = resolve_catalog_provider_id(provider_id);
    if let Some(c) = cfg {
        match p.as_str() {
            "openai" => {
                if let Some(ref k) = c.openai_api_key {
                    if !k.trim().is_empty() {
                        return k.trim().to_string();
                    }
                }
            }
            "google" => {
                if let Some(ref k) = c.gemini_api_key {
                    if !k.trim().is_empty() {
                        return k.trim().to_string();
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(value) =
        first_non_empty_keys(provider_key_env_var_list(provider_id).iter().copied())
    {
        return value;
    }
    if let Some(c) = cfg {
        if let Some(name) = primary_secret_env_name(&p) {
            if let Some(saved) =
                crate::engine_connections::saved_secret(&c.runtime_data_dir(), name)
            {
                let saved = saved.trim();
                if !saved.is_empty() {
                    return saved.to_string();
                }
            }
        }
    }
    if p == "anthropic" {
        if let Some(c) = cfg {
            if c.llm_provider == "anthropic" && !c.api_key.trim().is_empty() {
                return c.api_key.trim().to_string();
            }
        }
    }
    String::new()
}

pub fn is_api_key_configured_for_provider(provider_id: &str) -> bool {
    is_local_provider(provider_id) || !resolve_api_key_for_provider(provider_id).is_empty()
}

fn provider_key_available(provider_id: &str, data_dir: Option<&str>) -> bool {
    if is_api_key_configured_for_provider(provider_id) {
        return true;
    }
    let Some(dir) = data_dir else {
        return false;
    };
    let Some(name) = primary_secret_env_name(provider_id) else {
        return false;
    };
    crate::engine_connections::saved_secret(dir, name).is_some_and(|v| !v.trim().is_empty())
}

/// Normalize `.env` `LLM_PROVIDER` to a catalog entry id (`gemini` → `google`, `grok` → `xai`, etc.).
pub fn resolve_catalog_provider_id(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "gemini" => "google".to_string(),
        "grok" => "xai".to_string(),
        other => other.to_string(),
    }
}

/// Default OpenAI-compatible API base URL for a catalog provider, if defined.
pub fn default_base_url_for_provider(provider: &str) -> Option<&'static str> {
    find_provider(provider)
        .map(|p| p.default_base_url)
        .filter(|url| !url.is_empty())
}

pub fn find_provider(provider: &str) -> Option<&'static CatalogProvider> {
    let p = resolve_catalog_provider_id(provider);
    PROVIDER_CATALOG
        .iter()
        .find(|entry| entry.id.eq_ignore_ascii_case(&p))
}

pub fn default_model_for_provider(provider: &str) -> &'static str {
    find_provider(provider)
        .and_then(|p| p.models.first().map(|m| m.id))
        .unwrap_or("custom-model")
}

/// Whether `model` is listed for `provider`, or is the active custom model for unknown providers.
pub fn model_allowed_for_provider(provider: &str, model: &str, allow_custom: bool) -> bool {
    let model = model.trim();
    if model.is_empty() {
        return false;
    }
    if allow_custom {
        return true;
    }
    if let Some(preset) = find_provider(provider) {
        if preset.models.iter().any(|m| m.id == model) {
            return true;
        }
    }
    false
}

pub fn format_cost_summary(model: &CatalogModel) -> String {
    match (model.input_usd_per_mtok, model.output_usd_per_mtok) {
        (None, None) if model.cost_tier == "free" => "Local — no API usage cost".to_string(),
        (None, None) => "Pricing varies — check your provider dashboard".to_string(),
        (Some(i), Some(o)) => format!("~${i:.2} / ${o:.2} per 1M input / output tokens"),
        (Some(i), None) => format!("~${i:.2} per 1M input tokens"),
        (None, Some(o)) => format!("~${o:.2} per 1M output tokens"),
    }
}

#[derive(Debug, Serialize)]
pub struct CatalogModelJson {
    pub id: String,
    pub input_usd_per_mtok: Option<f64>,
    pub output_usd_per_mtok: Option<f64>,
    pub cost_tier: String,
    pub cost_summary: String,
    /// True when this row was added because it is the active model but not in the curated list.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_active_config: bool,
    /// True when the provider API listed this id (cost fields may still come from the curated list).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_live: bool,
}

impl CatalogModelJson {
    pub fn from_model(m: &CatalogModel) -> Self {
        CatalogModelJson {
            id: m.id.to_string(),
            input_usd_per_mtok: m.input_usd_per_mtok,
            output_usd_per_mtok: m.output_usd_per_mtok,
            cost_tier: m.cost_tier.to_string(),
            cost_summary: format_cost_summary(m),
            from_active_config: false,
            from_live: false,
        }
    }

    pub fn from_active_config(model_id: &str) -> Self {
        CatalogModelJson {
            id: model_id.to_string(),
            input_usd_per_mtok: None,
            output_usd_per_mtok: None,
            cost_tier: "standard".to_string(),
            cost_summary:
                "Active model from your config — not in curated catalog; check provider pricing."
                    .to_string(),
            from_active_config: true,
            from_live: false,
        }
    }

    pub fn from_live(model_id: &str) -> Self {
        CatalogModelJson {
            id: model_id.to_string(),
            input_usd_per_mtok: None,
            output_usd_per_mtok: None,
            cost_tier: "standard".to_string(),
            cost_summary:
                "Listed by the provider API. Pricing is not included — check your billing page."
                    .to_string(),
            from_active_config: false,
            from_live: true,
        }
    }
}

/// Max extra live ids appended after curated/active rows (OpenRouter-scale lists).
pub const LIVE_MODEL_LIST_CAP: usize = 200;

/// Skip embeddings, TTS, image, and other non-chat ids from live `/models` lists.
pub fn looks_like_chat_completion_model(id: &str) -> bool {
    let l = id.trim().to_ascii_lowercase();
    if l.is_empty() {
        return false;
    }
    const SKIP: &[&str] = &[
        "embedding",
        "embed-",
        "-embed",
        "whisper",
        "tts-",
        "-tts",
        "dall-e",
        "dall_e",
        "imagen",
        "moderation",
        "transcribe",
        "realtime",
        "gpt-image",
        "sora-",
        "computer-use",
        "omni-moderation",
        "text-moderation",
        "text-embedding",
        "aqa",
    ];
    !SKIP.iter().any(|needle| l.contains(needle))
}

/// Normalize a live model id for `provider` (strip Gemini `models/` prefix, drop non-chat ids).
pub fn normalize_live_model_id(provider: &str, raw: &str) -> Option<String> {
    let mut id = raw.trim().to_string();
    if id.is_empty() {
        return None;
    }
    if resolve_catalog_provider_id(provider) == "google" {
        if let Some(rest) = id.strip_prefix("models/") {
            id = rest.to_string();
        }
    }
    if !looks_like_chat_completion_model(&id) {
        return None;
    }
    Some(id)
}

#[derive(Debug)]
pub struct LiveCatalogMerge {
    pub models: Vec<CatalogModelJson>,
    pub truncated: bool,
    pub live_count: usize,
}

/// Overlay live provider ids onto the curated catalog, keeping cost hints when ids match.
pub fn merge_live_model_ids(
    provider: &str,
    active_model: &str,
    live_ids: &[String],
) -> LiveCatalogMerge {
    let mut normalized: Vec<String> = live_ids
        .iter()
        .filter_map(|id| normalize_live_model_id(provider, id))
        .collect();
    normalized.sort();
    normalized.dedup();
    let live_count = normalized.len();
    let live_set: HashSet<&str> = normalized.iter().map(String::as_str).collect();

    let mut models = catalog_models_json(provider, active_model);
    for row in &mut models {
        if live_set.contains(row.id.as_str()) {
            row.from_live = true;
            if row.from_active_config {
                let id = row.id.clone();
                *row = CatalogModelJson::from_live(&id);
            }
        }
    }
    let mut seen: HashSet<String> = models.iter().map(|m| m.id.clone()).collect();
    let mut extra: Vec<CatalogModelJson> = Vec::new();
    for id in &normalized {
        if seen.insert(id.clone()) {
            extra.push(CatalogModelJson::from_live(id));
        }
    }
    let cap_extra = LIVE_MODEL_LIST_CAP.saturating_sub(models.len());
    let truncated = extra.len() > cap_extra;
    extra.truncate(cap_extra);
    models.extend(extra);
    LiveCatalogMerge {
        models,
        truncated,
        live_count,
    }
}

#[derive(Debug, Serialize)]
pub struct ProviderCatalogJson {
    pub id: String,
    pub label: String,
    pub api_key_configured: bool,
    pub api_key_env_hints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<String>,
    pub models: Vec<CatalogModelJson>,
}

/// Curated providers that have an API key in `.env` (or are local) for the Web UI picker.
pub fn providers_catalog_json(
    active_provider: &str,
    active_model: &str,
    data_dir: Option<&str>,
) -> Vec<ProviderCatalogJson> {
    PROVIDER_CATALOG
        .iter()
        .map(|p| ProviderCatalogJson {
            id: p.id.to_string(),
            label: p.label.to_string(),
            api_key_configured: provider_key_available(p.id, data_dir),
            api_key_env_hints: provider_api_key_env_hints(p.id)
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            default_base_url: if is_local_provider(p.id) {
                default_base_url_for_provider(p.id).map(|s| s.to_string())
            } else {
                None
            },
            models: catalog_models_json(
                p.id,
                if p.id.eq_ignore_ascii_case(active_provider) {
                    active_model
                } else {
                    ""
                },
            ),
        })
        .filter(|p| p.api_key_configured)
        .collect()
}

/// Curated models for `provider`, plus `active_model` when it is missing from the list.
pub fn catalog_models_json(provider: &str, active_model: &str) -> Vec<CatalogModelJson> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let active = active_model.trim();
    if !active.is_empty() {
        let in_preset = find_provider(provider)
            .map(|p| p.models.iter().any(|m| m.id == active))
            .unwrap_or(false);
        if !in_preset {
            seen.insert(active.to_string());
            out.push(CatalogModelJson::from_active_config(active));
        }
    }
    if let Some(p) = find_provider(provider) {
        for m in p.models {
            if seen.insert(m.id.to_string()) {
                out.push(CatalogModelJson::from_model(m));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_catalog_has_sonnet() {
        let p = find_provider("anthropic").expect("anthropic preset");
        assert!(p.models.iter().any(|m| m.id.contains("sonnet")));
    }

    #[test]
    fn gemini_provider_alias_uses_google_catalog() {
        let models = catalog_models_json("gemini", "");
        assert!(models.iter().any(|m| m.id == "gemini-3-flash-preview"));
    }

    #[test]
    fn active_model_prepended_when_not_in_catalog() {
        let models = catalog_models_json("google", "my-experimental-gemini-model");
        assert!(models.first().is_some_and(|m| m.from_active_config));
        assert!(models.iter().any(|m| m.id == "gemini-3-flash-preview"));
    }

    #[test]
    fn grok_provider_alias_resolves_to_xai() {
        assert_eq!(resolve_catalog_provider_id("grok"), "xai");
    }

    #[test]
    fn model_allowed_rejects_unknown_for_known_provider() {
        assert!(!model_allowed_for_provider(
            "anthropic",
            "not-a-real-model",
            false
        ));
        assert!(model_allowed_for_provider(
            "anthropic",
            "claude-sonnet-4-5-20250929",
            false
        ));
    }

    #[test]
    fn normalize_local_base_url_appends_v1() {
        assert_eq!(
            normalize_local_base_url("http://127.0.0.1:11434", "ollama"),
            "http://127.0.0.1:11434/v1"
        );
    }

    #[test]
    fn effective_local_base_url_prefers_app_settings() {
        assert_eq!(
            effective_local_base_url("ollama", Some("http://10.0.0.5:11434")),
            "http://10.0.0.5:11434/v1"
        );
    }

    #[test]
    fn live_catalog_skips_embeddings_and_strips_gemini_prefix() {
        assert!(!looks_like_chat_completion_model("text-embedding-3-large"));
        assert!(looks_like_chat_completion_model("gpt-5.4"));
        assert_eq!(
            normalize_live_model_id("google", "models/gemini-2.5-flash").as_deref(),
            Some("gemini-2.5-flash")
        );
        assert_eq!(
            normalize_live_model_id("openai", "text-embedding-3-small"),
            None
        );
    }

    #[test]
    fn merge_live_keeps_curated_cost_and_appends_new_ids() {
        let merged = merge_live_model_ids(
            "openai",
            "gpt-5.4",
            &[
                "gpt-5.4".into(),
                "gpt-4o".into(),
                "text-embedding-3-large".into(),
            ],
        );
        let gpt54 = merged
            .models
            .iter()
            .find(|m| m.id == "gpt-5.4")
            .expect("gpt-5.4");
        assert!(gpt54.from_live);
        assert!(gpt54.input_usd_per_mtok.is_some());
        assert!(merged
            .models
            .iter()
            .any(|m| m.id == "gpt-4o" && m.from_live));
        assert!(merged
            .models
            .iter()
            .all(|m| m.id != "text-embedding-3-large"));
        assert_eq!(merged.live_count, 2);
        assert!(!merged.truncated);
    }

    #[test]
    fn merge_live_truncates_oversized_provider_lists() {
        let ids: Vec<String> = (0..(LIVE_MODEL_LIST_CAP + 50))
            .map(|i| format!("live-model-{i}"))
            .collect();
        let merged = merge_live_model_ids("openai", "", &ids);
        assert!(merged.truncated);
        assert_eq!(merged.live_count, LIVE_MODEL_LIST_CAP + 50);
        assert!(merged.models.len() <= LIVE_MODEL_LIST_CAP + 8);
    }

    #[test]
    fn saved_connection_key_is_used_when_env_is_empty() {
        if std::env::var("OPENAI_API_KEY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("favb-llm-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("runtime")).unwrap();
        let mut config = crate::config::test_config();
        config.workspace_dir = dir.to_string_lossy().to_string();
        config.openai_api_key = None;
        crate::engine_connections::write_secret(
            &config.runtime_data_dir(),
            "OPENAI_API_KEY",
            "saved-openai-key",
        )
        .unwrap();
        assert_eq!(
            resolve_api_key_for_provider_with_config("openai", Some(&config)),
            "saved-openai-key"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
