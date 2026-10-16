use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;

use crate::config::Config;
use crate::error::FinallyAValueBotError;

// Provider/model prompts retired — Web UI Settings → LLM; presets kept for future CLI use.
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct ProviderPreset {
    id: &'static str,
    label: &'static str,
    default_base_url: &'static str,
    models: &'static [&'static str],
}

#[allow(dead_code)]
const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        default_base_url: "https://api.openai.com/v1",
        models: &["gpt-5.4", "gpt-5.2", "gpt-5-mini"],
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        default_base_url: "https://openrouter.ai/api/v1",
        models: &[
            "openrouter/auto",
            "anthropic/claude-sonnet-4.5",
            "openai/gpt-5.2",
        ],
    },
    ProviderPreset {
        id: "anthropic",
        label: "Anthropic",
        default_base_url: "",
        models: &["claude-sonnet-4-5-20250929", "claude-opus-4-6-20260205"],
    },
    ProviderPreset {
        id: "ollama",
        label: "Ollama (local)",
        default_base_url: "http://127.0.0.1:11434/v1",
        models: &["llama3.2", "qwen2.5-coder:7b", "mistral"],
    },
    ProviderPreset {
        id: "llama",
        label: "Llama.cpp (local)",
        default_base_url: "http://127.0.0.1:8080/v1",
        models: &["local"],
    },
    ProviderPreset {
        id: "google",
        label: "Google DeepMind",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: &["gemini-2.5-pro", "gemini-2.5-flash"],
    },
    ProviderPreset {
        id: "alibaba",
        label: "Alibaba Cloud (Qwen / DashScope)",
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen3-max", "qwen-max-latest"],
    },
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        default_base_url: "https://api.deepseek.com/v1",
        models: &["deepseek-chat", "deepseek-reasoner"],
    },
    ProviderPreset {
        id: "moonshot",
        label: "Moonshot AI (Kimi)",
        default_base_url: "https://api.moonshot.cn/v1",
        models: &["kimi-k2.5", "kimi-k2"],
    },
    ProviderPreset {
        id: "mistral",
        label: "Mistral AI",
        default_base_url: "https://api.mistral.ai/v1",
        models: &["mistral-large-latest", "ministral-8b-latest"],
    },
    ProviderPreset {
        id: "azure",
        label: "Microsoft Azure AI",
        default_base_url:
            "https://YOUR-RESOURCE.openai.azure.com/openai/deployments/YOUR-DEPLOYMENT",
        models: &["gpt-5.2", "gpt-5"],
    },
    ProviderPreset {
        id: "bedrock",
        label: "Amazon AWS Bedrock",
        default_base_url: "https://bedrock-runtime.YOUR-REGION.amazonaws.com/openai/v1",
        models: &[
            "anthropic.claude-opus-4-6-v1",
            "anthropic.claude-sonnet-4-5-v2",
        ],
    },
    ProviderPreset {
        id: "zhipu",
        label: "Zhipu AI (GLM / Z.AI)",
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["glm-4.7", "glm-4.7-flash"],
    },
    ProviderPreset {
        id: "minimax",
        label: "MiniMax",
        default_base_url: "https://api.minimax.io/v1",
        models: &["MiniMax-M2.1"],
    },
    ProviderPreset {
        id: "cohere",
        label: "Cohere",
        default_base_url: "https://api.cohere.ai/compatibility/v1",
        models: &["command-a-03-2025", "command-r-plus-08-2024"],
    },
    ProviderPreset {
        id: "tencent",
        label: "Tencent AI Lab",
        default_base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        models: &["hunyuan-t1-latest", "hunyuan-turbos-latest"],
    },
    ProviderPreset {
        id: "xai",
        label: "xAI",
        default_base_url: "https://api.x.ai/v1",
        models: &["grok-4.3", "grok-4", "grok-3"],
    },
    ProviderPreset {
        id: "huggingface",
        label: "Hugging Face",
        default_base_url: "https://router.huggingface.co/v1",
        models: &["Qwen/Qwen3-Coder-Next", "meta-llama/Llama-3.3-70B-Instruct"],
    },
    ProviderPreset {
        id: "together",
        label: "Together AI",
        default_base_url: "https://api.together.xyz/v1",
        models: &[
            "deepseek-ai/DeepSeek-V3",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ],
    },
    ProviderPreset {
        id: "custom",
        label: "Custom (manual config)",
        default_base_url: "",
        models: &["custom-model"],
    },
];

#[allow(dead_code)]
fn find_provider_preset(provider: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(provider))
}

fn resolve_config_path() -> PathBuf {
    if let Some(custom) = crate::meido_env::get(crate::meido_env::CONFIG) {
        return PathBuf::from(custom);
    }
    if Path::new("./.env").exists() {
        PathBuf::from("./.env")
    } else {
        PathBuf::from("./.env")
    }
}

fn load_existing_config(path: &Path) -> Option<Config> {
    Config::load_from_path(path).ok()
}

fn prompt_line(
    prompt: &str,
    default: Option<&str>,
    required: bool,
) -> Result<Option<String>, FinallyAValueBotError> {
    loop {
        let suffix = match default {
            Some(d) if !d.is_empty() => format!(" [{d}]"),
            _ => String::new(),
        };
        print!("{prompt}{suffix}: ");
        io::stdout().flush()?;

        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        let trimmed = buf.trim();

        if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
            return Ok(None);
        }
        if trimmed.is_empty() {
            if let Some(d) = default {
                return Ok(Some(d.to_string()));
            }
            if !required {
                return Ok(Some(String::new()));
            }
            println!("This field is required. Enter a value or type 'q' to cancel.");
            continue;
        }
        return Ok(Some(trimmed.to_string()));
    }
}

#[allow(dead_code)]
fn prompt_provider(default_provider: &str) -> Result<Option<String>, FinallyAValueBotError> {
    println!();
    println!("Select LLM provider (press Enter for default):");
    let default_idx = PROVIDER_PRESETS
        .iter()
        .position(|p| p.id.eq_ignore_ascii_case(default_provider))
        .unwrap_or(0);
    for (i, preset) in PROVIDER_PRESETS.iter().enumerate() {
        let mark = if i == default_idx { "*" } else { " " };
        println!("  {mark} {}. {} ({})", i + 1, preset.id, preset.label);
    }

    loop {
        print!("Provider number [{}]: ", default_idx + 1);
        io::stdout().flush()?;
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        let trimmed = buf.trim();

        if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
            return Ok(None);
        }
        if trimmed.is_empty() {
            return Ok(Some(PROVIDER_PRESETS[default_idx].id.to_string()));
        }
        if let Ok(n) = trimmed.parse::<usize>() {
            if (1..=PROVIDER_PRESETS.len()).contains(&n) {
                return Ok(Some(PROVIDER_PRESETS[n - 1].id.to_string()));
            }
        }
        if let Some(p) = find_provider_preset(trimmed) {
            return Ok(Some(p.id.to_string()));
        }
        println!("Invalid provider. Enter a number, provider id, or 'q' to cancel.");
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[allow(dead_code)]
fn ollama_tags_url(base_url: &str) -> String {
    let mut base = base_url.trim().trim_end_matches('/').to_string();
    if base.ends_with("/v1") {
        base = base.trim_end_matches("/v1").to_string();
    }
    if base.is_empty() {
        base = "http://127.0.0.1:11434".to_string();
    }
    format!("{base}/api/tags")
}

#[allow(dead_code)]
fn detect_ollama_models(base_url: &str) -> Vec<String> {
    let tags_url = ollama_tags_url(base_url);
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let resp = match client.get(tags_url).send() {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let parsed = match resp.json::<OllamaTagsResponse>() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    parsed.models.into_iter().map(|m| m.name).collect()
}

#[allow(dead_code)]
fn prompt_model(
    provider: &str,
    default_model: &str,
    base_url: &str,
) -> Result<Option<String>, FinallyAValueBotError> {
    let mut options: Vec<String> = find_provider_preset(provider)
        .map(|p| p.models.iter().map(|m| m.to_string()).collect())
        .unwrap_or_else(|| vec![default_model.to_string()]);

    if provider.eq_ignore_ascii_case("ollama") {
        let installed = detect_ollama_models(base_url);
        if !installed.is_empty() {
            println!("Detected Ollama local models:");
            for m in &installed {
                println!("  - {m}");
            }
            options = installed;
        } else {
            println!("No local Ollama models detected (or Ollama not reachable).");
            println!(
                "Tip: run `ollama pull llama3.2` then pick the model again in Web UI → Settings → LLM."
            );
        }
    }

    if !options.iter().any(|m| m == default_model) {
        options.insert(0, default_model.to_string());
    }

    println!();
    println!("Select model (number) or type a custom model id:");
    for (i, model) in options.iter().enumerate() {
        let mark = if model == default_model { "*" } else { " " };
        println!("  {mark} {}. {model}", i + 1);
    }

    print!("Model [{}]: ", default_model);
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim();

    if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
        return Ok(None);
    }
    if trimmed.is_empty() {
        return Ok(Some(default_model.to_string()));
    }
    if let Ok(n) = trimmed.parse::<usize>() {
        if (1..=options.len()).contains(&n) {
            return Ok(Some(options[n - 1].clone()));
        }
    }
    Ok(Some(trimmed.to_string()))
}

fn escape_env_val(s: &str) -> String {
    if s.contains(' ') || s.contains('"') || s.contains('#') || s.is_empty() {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn save_config_env(path: &Path, config: &Config) -> Result<Option<PathBuf>, FinallyAValueBotError> {
    let mut backup = None;
    if path.exists() {
        let ts = Utc::now().format("%Y%m%d%H%M%S");
        let backup_path = path.with_extension(format!("env.bak.{ts}"));
        fs::copy(path, &backup_path)?;
        backup = Some(backup_path);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut lines = Vec::new();
    lines.push("# Meido configuration".into());
    lines.push("".into());
    lines.push("# Telegram".into());
    lines.push(format!(
        "TELEGRAM_BOT_TOKEN={}",
        escape_env_val(&config.telegram_bot_token)
    ));
    lines.push(format!(
        "BOT_USERNAME={}",
        escape_env_val(&config.bot_username)
    ));
    lines.push(format!(
        "AGENT_DISPLAY_NAME={}",
        escape_env_val(&config.agent_display_name)
    ));
    lines.push("".into());
    lines.push("# LLM — API keys in .env; provider + model in Web UI → Settings → LLM".into());
    if !config.api_key.is_empty() {
        lines.push(format!("LLM_API_KEY={}", escape_env_val(&config.api_key)));
    }
    if let Some(ref u) = config.llm_base_url {
        if !u.is_empty() {
            lines.push(format!("LLM_BASE_URL={}", escape_env_val(u)));
        }
    }
    lines.push("".into());
    lines.push("# Workspace".into());
    lines.push(format!(
        "WORKSPACE_DIR={}",
        escape_env_val(&config.workspace_dir)
    ));
    lines.push(format!("TIMEZONE={}", escape_env_val(&config.timezone)));
    if let Some(ref v) = config.vault {
        lines.push("".into());
        lines.push("# ORIGIN vault".into());
        if let Some(ref p) = v.origin_vault_path {
            lines.push(format!("VAULT_ORIGIN_VAULT_PATH={}", escape_env_val(p)));
        } else {
            lines.push("VAULT_ORIGIN_VAULT_PATH=shared/ORIGIN".into());
        }
        if let Some(ref p) = v.vector_db_path {
            lines.push(format!("VAULT_VECTOR_DB_PATH={}", escape_env_val(p)));
        } else {
            lines.push("VAULT_VECTOR_DB_PATH=shared/vault_db".into());
        }
        if let Some(ref r) = v.origin_vault_repo {
            if !r.is_empty() {
                lines.push(format!("VAULT_ORIGIN_VAULT_REPO={}", escape_env_val(r)));
            }
        }
    }
    let content = lines.join("\n");
    fs::write(path, content)?;
    Ok(backup)
}

fn default_config() -> Config {
    Config {
        telegram_bot_token: String::new(),
        bot_username: String::new(),
        agent_display_name: String::new(),
        llm_provider: "anthropic".into(),
        api_key: String::new(),
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
        web_terminal_max_sessions: 2,
        web_terminal_idle_timeout_secs: 1800,
        web_terminal_allow_in_docker: false,
        universal_chat_id: None,
        browser_managed: false,
        steel_api_port: crate::config::default_steel_api_port(),
        steel_cdp_port: crate::config::default_steel_cdp_port(),
        steel_docker_image: crate::config::default_steel_docker_image(),
        steel_docker_container_name: crate::config::default_steel_docker_container_name(),
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
        cursor_agent_cli_path: crate::config::default_cursor_agent_cli_path(),
        cursor_agent_model: String::new(),
        cursor_agent_timeout_secs: 3600,
        social: None,
        vault: None,
        post_tool_evaluator_enabled: false,
        post_tool_evaluator_model: String::new(),
        response_quality_evaluator_enabled: false,
        perplexity_api_key: None,
        evaluator_model: "sonar".into(),
        evaluator_base_url: "https://api.perplexity.ai".into(),
        response_quality_evaluator_model: String::new(),
        quality_eval_max_nudges_per_run: 1,
        quality_eval_min_confidence: 0.85,
        quality_eval_channels: "*".into(),
        hook_command_timeout_secs: 10,
        hook_prompt_timeout_secs: 15,
        hook_prompt_model: String::new(),
        allow_fuzzy_search_replace: false,
        symbol_edit_enabled: false,
        post_edit_validation_enabled: true,
        post_edit_validation_commands: None,
        cursor_agent_tmux_session_prefix: "meido-cursor".into(),
        cursor_agent_tmux_enabled: true,
        cursor_agent_runner_url: None,
        cursor_sdk_runner_url: None,
        cursor_sdk_model: crate::config::default_cursor_sdk_model(),
        cursor_sdk_auto_start: true,
        cursor_sdk_auto_install: true,
        cursor_sidecar_max_uptime_secs: 86_400,
        cursor_sdk_runner_port: 3848,
        cursor_sdk_python: "python3".into(),
        cursor_sdk_node: "node".into(),
        scheduler_task_timeout_secs: 3600,
        scheduler_stale_running_reclaim_secs: 7200,
        scheduler_max_concurrent_tasks: 2,
        scheduler_poll_interval_secs: 60,
        topics_enabled: true,
        topics_min_unsegmented: 6,
        topics_idle_minutes: 20,
        topics_gap_hours: 6,
        topics_prefer_tier2: true,
        topics_debounce_minutes: 2,
        background_job_lease_ttl_secs: 1800,
        background_job_lease_fallback_renew_secs: 60,
        background_job_pending_start_timeout_secs: 300,
        background_job_notify_chat_progress: false,
        tool_output_debug: false,
        background_shell_tmux_enabled: true,
        background_shell_tmux_session_prefix: "meido-bg".into(),
        background_shell_monitor_poll_secs: 8,
        background_shell_auto_retry_on_failure: true,
        background_shell_auto_retry_max: 1,
        background_shell_auto_agent_on_success: true,
        runtime_reliability_profile: "balanced".into(),
        project_auto_association_strictness: "balanced".into(),
        mempalace: crate::config::default_mempalace_config(),
    }
}

pub fn run_config_wizard() -> Result<bool, FinallyAValueBotError> {
    let config_path = resolve_config_path();
    let existing = load_existing_config(&config_path).unwrap_or_else(default_config);

    println!("Meido interactive config");
    println!("Press Enter to accept default values. Type 'q' to cancel.");
    println!("Config path: {}", config_path.display());

    let telegram_bot_token = match prompt_line(
        "Telegram bot token",
        Some(&existing.telegram_bot_token),
        true,
    )? {
        Some(v) => v,
        None => return Ok(false),
    };
    let bot_username = match prompt_line(
        "Bot username (without @)",
        Some(&existing.bot_username),
        true,
    )? {
        Some(v) => v.trim_start_matches('@').to_string(),
        None => return Ok(false),
    };
    let agent_display_name = match prompt_line(
        "Agent display name (used to seed identity memory)",
        Some(&existing.agent_display_name),
        false,
    )? {
        Some(v) => v,
        None => return Ok(false),
    };

    println!(
        "LLM provider and model are configured in Web UI → Settings → LLM (not written to .env)."
    );
    let api_key = match prompt_line(
        "LLM API key (optional legacy ANTHROPIC_API_KEY / LLM_API_KEY; prefer provider-specific keys in .env)",
        Some(&existing.api_key),
        false,
    )? {
        Some(v) => v,
        None => return Ok(false),
    };

    let workspace_dir = match prompt_line(
        "Workspace directory (root for runtime, skills, shared)",
        Some(&existing.workspace_dir),
        false,
    )? {
        Some(v) => {
            if v.trim().is_empty() {
                "./workspace".to_string()
            } else {
                v.trim().to_string()
            }
        }
        None => return Ok(false),
    };
    let timezone = match prompt_line("Timezone (IANA)", Some(&existing.timezone), false)? {
        Some(v) => {
            if v.trim().is_empty() {
                "UTC".to_string()
            } else {
                v
            }
        }
        None => return Ok(false),
    };

    let mut out = existing.clone();
    out.telegram_bot_token = telegram_bot_token;
    out.bot_username = bot_username;
    out.agent_display_name = agent_display_name;
    out.llm_provider.clear();
    out.model.clear();
    out.llm_base_url = None;
    out.api_key = api_key;
    out.workspace_dir = workspace_dir;
    out.timezone = timezone;
    out.post_deserialize()?;

    fs::create_dir_all(&out.workspace_dir)?;
    let _ = fs::create_dir_all(std::path::Path::new(&out.workspace_dir).join("shared"));

    let backup = save_config_env(&config_path, &out)?;
    println!();
    println!("Saved config to {}", config_path.display());
    if let Some(b) = backup {
        println!("Backup created at {}", b.display());
    }
    Ok(true)
}
