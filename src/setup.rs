use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;

use crate::error::FinallyAValueBotError;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderProtocol {
    Anthropic,
    OpenAiCompat,
}

#[derive(Clone, Copy)]
struct ProviderPreset {
    id: &'static str,
    label: &'static str,
    protocol: ProviderProtocol,
    default_base_url: &'static str,
    models: &'static [&'static str],
}

const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.openai.com/v1",
        models: &["gpt-5.4", "gpt-5.2", "gpt-5-mini"],
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        protocol: ProviderProtocol::OpenAiCompat,
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
        protocol: ProviderProtocol::Anthropic,
        default_base_url: "",
        models: &["claude-sonnet-4-5-20250929", "claude-opus-4-6-20260205"],
    },
    ProviderPreset {
        id: "ollama",
        label: "Ollama (local)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "http://127.0.0.1:11434/v1",
        models: &["llama3.2", "qwen2.5-coder:7b", "mistral"],
    },
    ProviderPreset {
        id: "llama",
        label: "Llama.cpp (local)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "http://127.0.0.1:8080/v1",
        models: &["local"],
    },
    ProviderPreset {
        id: "google",
        label: "Google DeepMind",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: &["gemini-2.5-pro", "gemini-2.5-flash"],
    },
    ProviderPreset {
        id: "alibaba",
        label: "Alibaba Cloud (Qwen / DashScope)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen3-max", "qwen-max-latest"],
    },
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.deepseek.com/v1",
        models: &["deepseek-chat", "deepseek-reasoner"],
    },
    ProviderPreset {
        id: "moonshot",
        label: "Moonshot AI (Kimi)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.moonshot.cn/v1",
        models: &["kimi-k2.5", "kimi-k2"],
    },
    ProviderPreset {
        id: "mistral",
        label: "Mistral AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.mistral.ai/v1",
        models: &["mistral-large-latest", "ministral-8b-latest"],
    },
    ProviderPreset {
        id: "azure",
        label: "Microsoft Azure AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url:
            "https://YOUR-RESOURCE.openai.azure.com/openai/deployments/YOUR-DEPLOYMENT",
        models: &["gpt-5.2", "gpt-5"],
    },
    ProviderPreset {
        id: "bedrock",
        label: "Amazon AWS Bedrock",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://bedrock-runtime.YOUR-REGION.amazonaws.com/openai/v1",
        models: &[
            "anthropic.claude-opus-4-6-v1",
            "anthropic.claude-sonnet-4-5-v2",
        ],
    },
    ProviderPreset {
        id: "zhipu",
        label: "Zhipu AI (GLM / Z.AI)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["glm-4.7", "glm-4.7-flash"],
    },
    ProviderPreset {
        id: "minimax",
        label: "MiniMax",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.minimax.io/v1",
        models: &["MiniMax-M2.1"],
    },
    ProviderPreset {
        id: "cohere",
        label: "Cohere",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.cohere.ai/compatibility/v1",
        models: &["command-a-03-2025", "command-r-plus-08-2024"],
    },
    ProviderPreset {
        id: "tencent",
        label: "Tencent AI Lab",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        models: &["hunyuan-t1-latest", "hunyuan-turbos-latest"],
    },
    ProviderPreset {
        id: "xai",
        label: "xAI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.x.ai/v1",
        models: &["grok-4.3", "grok-4", "grok-3"],
    },
    ProviderPreset {
        id: "huggingface",
        label: "Hugging Face",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://router.huggingface.co/v1",
        models: &["Qwen/Qwen3-Coder-Next", "meta-llama/Llama-3.3-70B-Instruct"],
    },
    ProviderPreset {
        id: "together",
        label: "Together AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.together.xyz/v1",
        models: &[
            "deepseek-ai/DeepSeek-V3",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ],
    },
    ProviderPreset {
        id: "custom",
        label: "Custom (manual config)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "",
        models: &["custom-model"],
    },
];

fn find_provider_preset(provider: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(provider))
}

fn provider_protocol(provider: &str) -> ProviderProtocol {
    find_provider_preset(provider)
        .map(|p| p.protocol)
        .unwrap_or(ProviderProtocol::OpenAiCompat)
}

fn default_model_for_provider(provider: &str) -> &'static str {
    find_provider_preset(provider)
        .and_then(|p| p.models.first().copied())
        .unwrap_or("gpt-5.2")
}

fn provider_display(provider: &str) -> String {
    if let Some(preset) = find_provider_preset(provider) {
        format!("{} ({})", preset.id, preset.label)
    } else {
        format!("{provider} (custom)")
    }
}

#[derive(Clone)]
struct Field {
    key: &'static str,
    label: &'static str,
    value: String,
    required: bool,
    secret: bool,
}

impl Field {
    fn display_value(&self, editing: bool) -> String {
        if editing || !self.secret {
            return self.value.clone();
        }
        if self.value.is_empty() {
            String::new()
        } else {
            mask_secret(&self.value)
        }
    }
}

struct SetupApp {
    fields: Vec<Field>,
    selected: usize,
    editing: bool,
    picker: Option<PickerState>,
    status: String,
    completed: bool,
    backup_path: Option<String>,
    completion_summary: Vec<String>,
}

#[derive(Clone, Copy)]
enum PickerKind {
    Provider,
    Model,
}

#[derive(Clone, Copy)]
struct PickerState {
    kind: PickerKind,
    selected: usize,
}

impl SetupApp {
    fn new() -> Self {
        // Try loading from existing config file first, then fall back to env vars
        let existing = Self::load_existing_config();
        let provider = existing
            .get("LLM_PROVIDER")
            .cloned()
            .unwrap_or_else(|| "anthropic".into());
        let default_model = default_model_for_provider(&provider);
        let default_base_url = find_provider_preset(&provider)
            .map(|p| p.default_base_url)
            .unwrap_or("");
        let llm_api_key = existing.get("LLM_API_KEY").cloned().unwrap_or_default();

        Self {
            fields: vec![
                // ── Telegram ──
                Field {
                    key: "TELEGRAM_BOT_TOKEN",
                    label: "Telegram bot token",
                    value: existing.get("TELEGRAM_BOT_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                Field {
                    key: "BOT_USERNAME",
                    label: "Bot username (without @)",
                    value: existing.get("BOT_USERNAME").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "ALLOWED_GROUPS",
                    label: "Allowed group chat IDs (comma-separated)",
                    value: existing.get("ALLOWED_GROUPS").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "CONTROL_CHAT_IDS",
                    label: "Control chat IDs (comma-separated)",
                    value: existing.get("CONTROL_CHAT_IDS").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── LLM ──
                Field {
                    key: "LLM_PROVIDER",
                    label: "LLM provider (preset/custom)",
                    value: provider,
                    required: true,
                    secret: false,
                },
                Field {
                    key: "LLM_API_KEY",
                    label: "LLM API key",
                    value: llm_api_key,
                    required: true,
                    secret: true,
                },
                Field {
                    key: "LLM_MODEL",
                    label: "LLM model",
                    value: existing.get("LLM_MODEL").cloned().unwrap_or_else(|| default_model.into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "LLM_BASE_URL",
                    label: "LLM base URL (optional)",
                    value: existing.get("LLM_BASE_URL").cloned().unwrap_or_else(|| default_base_url.to_string()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "OPENAI_API_KEY",
                    label: "OpenAI API key (voice transcription)",
                    value: existing.get("OPENAI_API_KEY").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                // ── Runtime ──
                Field {
                    key: "WORKSPACE_DIR",
                    label: "Workspace directory",
                    value: existing
                        .get("WORKSPACE_DIR")
                        .or(existing.get("WORKING_DIR"))
                        .cloned()
                        .unwrap_or_else(|| "./workspace".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "TIMEZONE",
                    label: "Timezone (IANA)",
                    value: existing.get("TIMEZONE").cloned().unwrap_or_else(|| "UTC".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "MAX_TOKENS",
                    label: "Max tokens per response",
                    value: existing.get("MAX_TOKENS").cloned().unwrap_or_else(|| "8192".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "MAX_TOOL_ITERATIONS",
                    label: "Max tool loop iterations",
                    value: existing.get("MAX_TOOL_ITERATIONS").cloned().unwrap_or_else(|| "100".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "MAX_HISTORY_MESSAGES",
                    label: "Chat history context size",
                    value: existing.get("MAX_HISTORY_MESSAGES").cloned().unwrap_or_else(|| "50".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "RECENT_HISTORY_MIN_USER_MESSAGES",
                    label: "Min user messages in balanced history tail",
                    value: existing
                        .get("RECENT_HISTORY_MIN_USER_MESSAGES")
                        .cloned()
                        .unwrap_or_else(|| "2".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES",
                    label: "Min assistant messages in balanced history tail",
                    value: existing
                        .get("RECENT_HISTORY_MIN_ASSISTANT_MESSAGES")
                        .cloned()
                        .unwrap_or_else(|| "2".into()),
                    required: false,
                    secret: false,
                },
                // ── Orchestrator ──
                Field {
                    key: "ORCHESTRATOR_ENABLED",
                    label: "Enable orchestrator (plan-first)",
                    value: existing.get("ORCHESTRATOR_ENABLED").cloned().unwrap_or_else(|| "true".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "ORCHESTRATOR_MODEL",
                    label: "Orchestrator model override",
                    value: existing.get("ORCHESTRATOR_MODEL").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── Discord ──
                Field {
                    key: "DISCORD_BOT_TOKEN",
                    label: "Discord bot token",
                    value: existing.get("DISCORD_BOT_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                Field {
                    key: "DISCORD_ALLOWED_CHANNELS",
                    label: "Discord allowed channel IDs (comma-separated)",
                    value: existing.get("DISCORD_ALLOWED_CHANNELS").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── WhatsApp ──
                Field {
                    key: "WHATSAPP_ACCESS_TOKEN",
                    label: "WhatsApp access token",
                    value: existing.get("WHATSAPP_ACCESS_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                Field {
                    key: "WHATSAPP_PHONE_NUMBER_ID",
                    label: "WhatsApp phone number ID",
                    value: existing.get("WHATSAPP_PHONE_NUMBER_ID").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "WHATSAPP_VERIFY_TOKEN",
                    label: "WhatsApp webhook verify token",
                    value: existing.get("WHATSAPP_VERIFY_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                Field {
                    key: "WHATSAPP_WEBHOOK_PORT",
                    label: "WhatsApp webhook port",
                    value: existing.get("WHATSAPP_WEBHOOK_PORT").cloned().unwrap_or_else(|| "8080".into()),
                    required: false,
                    secret: false,
                },
                // ── Web UI ──
                Field {
                    key: "WEB_ENABLED",
                    label: "Enable web UI",
                    value: existing.get("WEB_ENABLED").cloned().unwrap_or_else(|| "true".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "WEB_HOST",
                    label: "Web UI host",
                    value: existing.get("WEB_HOST").cloned().unwrap_or_else(|| "0.0.0.0".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "WEB_PORT",
                    label: "Web UI port",
                    value: existing.get("WEB_PORT").cloned().unwrap_or_else(|| "10961".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "WEB_AUTH_TOKEN",
                    label: "Web UI auth token (required if non-local)",
                    value: existing.get("WEB_AUTH_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
                // ── Browser ──
                Field {
                    key: "BROWSER_MANAGED",
                    label: "Managed Steel browser (auto-start Docker on bot boot)",
                    value: existing.get("BROWSER_MANAGED").cloned().unwrap_or_else(|| "false".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "STEEL_API_PORT",
                    label: "Steel API host port",
                    value: existing
                        .get("STEEL_API_PORT")
                        .cloned()
                        .unwrap_or_else(|| "13920".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "STEEL_CDP_PORT",
                    label: "Steel CDP host port",
                    value: existing
                        .get("STEEL_CDP_PORT")
                        .cloned()
                        .unwrap_or_else(|| "13923".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "STEEL_API_URL",
                    label: "Steel API URL",
                    value: existing
                        .get("STEEL_API_URL")
                        .cloned()
                        .unwrap_or_else(|| "http://127.0.0.1:13920".into()),
                    required: false,
                    secret: false,
                },
                // ── Cursor Agent ──
                Field {
                    key: "CURSOR_AGENT_CLI_PATH",
                    label: "cursor-agent CLI path",
                    value: existing.get("CURSOR_AGENT_CLI_PATH").cloned().unwrap_or_else(|| "cursor-agent".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "CURSOR_AGENT_MODEL",
                    label: "cursor-agent model",
                    value: existing.get("CURSOR_AGENT_MODEL").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "CURSOR_AGENT_RUNNER_URL",
                    label: "cursor-agent runner URL",
                    value: existing.get("CURSOR_AGENT_RUNNER_URL").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── Vault ──
                Field {
                    key: "VAULT_ORIGIN_VAULT_PATH",
                    label: "Vault path (relative to workspace)",
                    value: existing.get("VAULT_ORIGIN_VAULT_PATH").cloned().unwrap_or_else(|| "shared/ORIGIN".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "VAULT_VECTOR_DB_PATH",
                    label: "Vector DB path (relative to workspace)",
                    value: existing.get("VAULT_VECTOR_DB_PATH").cloned().unwrap_or_else(|| "shared/vault_db".into()),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "VAULT_GIT_URL",
                    label: "Vault git URL (optional)",
                    value: existing.get("VAULT_GIT_URL").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "VAULT_SEARCH_COMMAND",
                    label: "Vault search command",
                    value: existing.get("VAULT_SEARCH_COMMAND").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "VAULT_INDEX_COMMAND",
                    label: "Vault index command",
                    value: existing.get("VAULT_INDEX_COMMAND").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "VAULT_VECTOR_DB_URL",
                    label: "ChromaDB server URL",
                    value: existing.get("VAULT_VECTOR_DB_URL").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── Universal ──
                Field {
                    key: "UNIVERSAL_CHAT_ID",
                    label: "Universal chat ID (cross-channel)",
                    value: existing.get("UNIVERSAL_CHAT_ID").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                // ── Git ──
                Field {
                    key: "GIT_USERNAME",
                    label: "Git username (for push inside container)",
                    value: existing.get("GIT_USERNAME").cloned().unwrap_or_default(),
                    required: false,
                    secret: false,
                },
                Field {
                    key: "GIT_TOKEN",
                    label: "Git token / PAT",
                    value: existing.get("GIT_TOKEN").cloned().unwrap_or_default(),
                    required: false,
                    secret: true,
                },
            ],
            selected: 0,
            editing: false,
            picker: None,
            status:
                "Use ↑/↓ select field, Enter to edit or choose list, F2 validate, s/Ctrl+S save, q quit"
                    .into(),
            completed: false,
            backup_path: None,
            completion_summary: Vec::new(),
        }
    }

    /// Load existing config values from .env.
    fn load_existing_config() -> HashMap<String, String> {
        let path = Path::new("./.env");
        if !path.exists() {
            return HashMap::new();
        }
        let Ok(content) = fs::read_to_string(path) else {
            return HashMap::new();
        };
        let mut map = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !k.is_empty() {
                    map.insert(k.to_string(), v);
                }
            }
        }
        if map.get("VAULT_GIT_URL").map_or(true, |s| s.is_empty()) {
            if let Some(repo) = map.get("VAULT_ORIGIN_VAULT_REPO") {
                if !repo.is_empty() {
                    map.insert("VAULT_GIT_URL".into(), repo.clone());
                }
            }
        }
        map
    }

    fn next(&mut self) {
        if self.selected + 1 < self.fields.len() {
            self.selected += 1;
        }
    }

    fn prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn selected_field_mut(&mut self) -> &mut Field {
        &mut self.fields[self.selected]
    }

    fn selected_field(&self) -> &Field {
        &self.fields[self.selected]
    }

    fn field_value(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }

    fn to_env_map(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for field in &self.fields {
            if !field.value.trim().is_empty() {
                out.insert(field.key.to_string(), field.value.trim().to_string());
            }
        }
        out
    }

    fn validate_local(&self) -> Result<(), FinallyAValueBotError> {
        for field in &self.fields {
            if field.key == "LLM_API_KEY"
                && matches!(
                    self.field_value("LLM_PROVIDER").as_str(),
                    "ollama" | "llama" | "llamacpp"
                )
            {
                continue;
            }
            if field.required && field.value.trim().is_empty() {
                return Err(FinallyAValueBotError::Config(format!(
                    "{} is required",
                    field.key
                )));
            }
        }

        let provider = self.field_value("LLM_PROVIDER");
        if provider.is_empty() {
            return Err(FinallyAValueBotError::Config(
                "LLM_PROVIDER is required".into(),
            ));
        }

        // At least one channel must be configured
        let tg_token = self.field_value("TELEGRAM_BOT_TOKEN");
        let discord_token = self.field_value("DISCORD_BOT_TOKEN");
        if tg_token.is_empty() && discord_token.is_empty() {
            return Err(FinallyAValueBotError::Config(
                "At least one of TELEGRAM_BOT_TOKEN or DISCORD_BOT_TOKEN must be set".into(),
            ));
        }

        let username = self.field_value("BOT_USERNAME");
        if username.starts_with('@') {
            return Err(FinallyAValueBotError::Config(
                "BOT_USERNAME should not include '@'".into(),
            ));
        }

        let timezone = self.field_value("TIMEZONE");
        let tz = if timezone.is_empty() {
            "UTC".to_string()
        } else {
            timezone
        };
        tz.parse::<chrono_tz::Tz>()
            .map_err(|_| FinallyAValueBotError::Config(format!("Invalid TIMEZONE: {tz}")))?;

        let workspace_dir = self.field_value("WORKSPACE_DIR");
        let dir = if workspace_dir.trim().is_empty() {
            "./workspace".to_string()
        } else {
            workspace_dir.trim().to_string()
        };
        fs::create_dir_all(&dir)?;
        let shared = Path::new(&dir).join("shared");
        let _ = fs::create_dir_all(&shared);
        let _ = fs::create_dir_all(shared.join("vault_db"));
        let probe = Path::new(&dir).join(".setup_probe");
        fs::write(&probe, "ok")?;
        let _ = fs::remove_file(probe);

        Ok(())
    }

    fn validate_online(&self) -> Result<Vec<String>, FinallyAValueBotError> {
        let tg_token = self.field_value("TELEGRAM_BOT_TOKEN");
        let env_username = self
            .field_value("BOT_USERNAME")
            .trim_start_matches('@')
            .to_string();
        let provider = self.field_value("LLM_PROVIDER").to_lowercase();
        let api_key = self.field_value("LLM_API_KEY");
        let base_url = self.field_value("LLM_BASE_URL");
        let model = self.field_value("LLM_MODEL");
        std::thread::spawn(move || {
            perform_online_validation(
                &tg_token,
                &env_username,
                &provider,
                &api_key,
                &base_url,
                &model,
            )
        })
        .join()
        .map_err(|_| FinallyAValueBotError::Config("Validation thread panicked".into()))?
    }

    fn set_provider(&mut self, provider: &str) {
        let old_provider = self.field_value("LLM_PROVIDER");
        let old_base_url = self.field_value("LLM_BASE_URL");
        let old_model = self.field_value("LLM_MODEL");

        if let Some(field) = self.fields.iter_mut().find(|f| f.key == "LLM_PROVIDER") {
            field.value = provider.to_string();
        }
        if let Some(base) = self.fields.iter_mut().find(|f| f.key == "LLM_BASE_URL") {
            let next_default = find_provider_preset(provider)
                .map(|p| p.default_base_url)
                .unwrap_or("");
            let old_default = find_provider_preset(&old_provider)
                .map(|p| p.default_base_url)
                .unwrap_or("");
            if old_base_url.trim().is_empty() || old_base_url == old_default {
                base.value = next_default.to_string();
            }
        }
        if let Some(model) = self.fields.iter_mut().find(|f| f.key == "LLM_MODEL") {
            let old_in_old_preset = find_provider_preset(&old_provider)
                .map(|p| p.models.iter().any(|m| *m == old_model))
                .unwrap_or(false);
            if old_model.trim().is_empty() || old_in_old_preset {
                model.value = default_model_for_provider(provider).to_string();
            }
        }
    }

    fn cycle_provider(&mut self, direction: i32) {
        let current = self.field_value("LLM_PROVIDER");
        let current_idx = PROVIDER_PRESETS
            .iter()
            .position(|p| p.id.eq_ignore_ascii_case(&current))
            .unwrap_or(PROVIDER_PRESETS.len() - 1);
        let next_idx = if direction < 0 {
            if current_idx == 0 {
                PROVIDER_PRESETS.len() - 1
            } else {
                current_idx - 1
            }
        } else {
            (current_idx + 1) % PROVIDER_PRESETS.len()
        };
        self.set_provider(PROVIDER_PRESETS[next_idx].id);
    }

    fn cycle_model(&mut self, direction: i32) {
        let provider = self.field_value("LLM_PROVIDER");
        let preset = match find_provider_preset(&provider) {
            Some(p) => p,
            None => return,
        };
        if preset.models.is_empty() {
            return;
        }
        let current = self.field_value("LLM_MODEL");
        let current_idx = preset
            .models
            .iter()
            .position(|m| *m == current)
            .unwrap_or(0);
        let next_idx = if direction < 0 {
            if current_idx == 0 {
                preset.models.len() - 1
            } else {
                current_idx - 1
            }
        } else {
            (current_idx + 1) % preset.models.len()
        };
        if let Some(model) = self.fields.iter_mut().find(|f| f.key == "LLM_MODEL") {
            model.value = preset.models[next_idx].to_string();
        }
    }

    fn provider_index(&self, provider: &str) -> usize {
        PROVIDER_PRESETS
            .iter()
            .position(|p| p.id.eq_ignore_ascii_case(provider))
            .unwrap_or(PROVIDER_PRESETS.len().saturating_sub(1))
    }

    fn model_options(&self) -> Vec<String> {
        let provider = self.field_value("LLM_PROVIDER");
        if let Some(preset) = find_provider_preset(&provider) {
            preset.models.iter().map(|m| (*m).to_string()).collect()
        } else {
            vec![self.field_value("LLM_MODEL")]
        }
    }

    fn open_picker_for_selected(&mut self) -> bool {
        match self.selected_field().key {
            "LLM_PROVIDER" => {
                let idx = self.provider_index(&self.field_value("LLM_PROVIDER"));
                self.picker = Some(PickerState {
                    kind: PickerKind::Provider,
                    selected: idx,
                });
                true
            }
            "LLM_MODEL" => {
                let provider = self.field_value("LLM_PROVIDER");
                if provider.eq_ignore_ascii_case("custom") {
                    return false;
                }
                let options = self.model_options();
                if options.is_empty() {
                    return false;
                }
                let current_model = self.field_value("LLM_MODEL");
                let idx = options
                    .iter()
                    .position(|m| *m == current_model)
                    .unwrap_or(0);
                self.picker = Some(PickerState {
                    kind: PickerKind::Model,
                    selected: idx,
                });
                true
            }
            _ => false,
        }
    }

    fn move_picker(&mut self, direction: i32) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let kind = picker.kind;
        let selected = picker.selected;
        let options_len = match kind {
            PickerKind::Provider => PROVIDER_PRESETS.len(),
            PickerKind::Model => self.model_options().len(),
        };
        if options_len == 0 {
            return;
        }
        let next = if direction < 0 {
            if selected == 0 {
                options_len - 1
            } else {
                selected - 1
            }
        } else {
            (selected + 1) % options_len
        };
        if let Some(picker_mut) = self.picker.as_mut() {
            picker_mut.selected = next;
        }
    }

    fn apply_picker_selection(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        match picker.kind {
            PickerKind::Provider => {
                if let Some(preset) = PROVIDER_PRESETS.get(picker.selected) {
                    self.set_provider(preset.id);
                    self.status = format!("Provider set to {}", preset.id);
                }
            }
            PickerKind::Model => {
                let options = self.model_options();
                if let Some(chosen) = options.get(picker.selected) {
                    if let Some(model) = self.fields.iter_mut().find(|f| f.key == "LLM_MODEL") {
                        model.value = chosen.clone();
                        self.status = format!("Model set to {chosen}");
                    }
                }
            }
        }
    }

    fn default_value_for_field(&self, key: &str) -> String {
        let provider = self.field_value("LLM_PROVIDER");
        match key {
            "LLM_PROVIDER" => "anthropic".into(),
            "LLM_MODEL" => default_model_for_provider(&provider).into(),
            "LLM_BASE_URL" => find_provider_preset(&provider)
                .map(|p| p.default_base_url.to_string())
                .unwrap_or_default(),
            "WORKSPACE_DIR" => "./workspace".into(),
            "TIMEZONE" => "UTC".into(),
            "MAX_TOKENS" => "8192".into(),
            "MAX_TOOL_ITERATIONS" => "100".into(),
            "MAX_HISTORY_MESSAGES" => "50".into(),
            "RECENT_HISTORY_MIN_USER_MESSAGES" => "2".into(),
            "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES" => "2".into(),
            "ORCHESTRATOR_ENABLED" => "true".into(),
            "WHATSAPP_WEBHOOK_PORT" => "8080".into(),
            "WEB_ENABLED" => "true".into(),
            "WEB_HOST" => "0.0.0.0".into(),
            "WEB_PORT" => "10961".into(),
            "BROWSER_MANAGED" => "false".into(),
            "CURSOR_AGENT_CLI_PATH" => "cursor-agent".into(),
            "VAULT_ORIGIN_VAULT_PATH" => "shared/ORIGIN".into(),
            "VAULT_VECTOR_DB_PATH" => "shared/vault_db".into(),
            _ => String::new(),
        }
    }

    fn clear_selected_field(&mut self) {
        let key = self.selected_field().key;
        self.selected_field_mut().value.clear();
        self.status = format!("Cleared {key}");
    }

    fn restore_selected_field_default(&mut self) {
        let key = self.selected_field().key;
        let default = self.default_value_for_field(key);
        if default.is_empty() {
            self.status = format!("{key} has no default");
        } else {
            self.selected_field_mut().value = default.clone();
            self.status = format!("Restored {key} to default: {default}");
        }
    }

    fn current_section(&self) -> &'static str {
        // Find the section by looking at the field key at the current index
        if self.selected < self.fields.len() {
            match self.fields[self.selected].key {
                "TELEGRAM_BOT_TOKEN" | "BOT_USERNAME" | "ALLOWED_GROUPS" | "CONTROL_CHAT_IDS" => {
                    "Telegram"
                }
                "LLM_PROVIDER" | "LLM_API_KEY" | "LLM_MODEL" | "LLM_BASE_URL"
                | "OPENAI_API_KEY" => "LLM",
                "WORKSPACE_DIR"
                | "TIMEZONE"
                | "MAX_TOKENS"
                | "MAX_TOOL_ITERATIONS"
                | "MAX_HISTORY_MESSAGES"
                | "RECENT_HISTORY_MIN_USER_MESSAGES"
                | "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES" => "Runtime",
                "ORCHESTRATOR_ENABLED" | "ORCHESTRATOR_MODEL" => "Orchestrator",
                "DISCORD_BOT_TOKEN" | "DISCORD_ALLOWED_CHANNELS" => "Discord",
                "WHATSAPP_ACCESS_TOKEN"
                | "WHATSAPP_PHONE_NUMBER_ID"
                | "WHATSAPP_VERIFY_TOKEN"
                | "WHATSAPP_WEBHOOK_PORT" => "WhatsApp",
                "WEB_ENABLED" | "WEB_HOST" | "WEB_PORT" | "WEB_AUTH_TOKEN" => "Web UI",
                "BROWSER_MANAGED" | "STEEL_API_URL" | "STEEL_API_PORT" | "STEEL_CDP_PORT" => {
                    "Browser"
                }
                "CURSOR_AGENT_CLI_PATH" | "CURSOR_AGENT_MODEL" | "CURSOR_AGENT_RUNNER_URL" => {
                    "Cursor Agent"
                }
                "VAULT_ORIGIN_VAULT_PATH"
                | "VAULT_VECTOR_DB_PATH"
                | "VAULT_GIT_URL"
                | "VAULT_SEARCH_COMMAND"
                | "VAULT_INDEX_COMMAND"
                | "VAULT_VECTOR_DB_URL" => "Vault",
                "UNIVERSAL_CHAT_ID" => "Universal",
                "GIT_USERNAME" | "GIT_TOKEN" => "Git",
                _ => "Setup",
            }
        } else {
            "Setup"
        }
    }

    fn progress_bar(&self, width: usize) -> String {
        let total = self.fields.len().max(1);
        let done = self.selected + 1;
        let fill = (done * width) / total;
        let mut s = String::new();
        for i in 0..width {
            if i < fill {
                s.push('█');
            } else {
                s.push('░');
            }
        }
        s
    }
}

fn perform_online_validation(
    tg_token: &str,
    env_username: &str,
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
) -> Result<Vec<String>, FinallyAValueBotError> {
    let mut checks = Vec::new();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // --- Telegram validation (skip if no token) ---
    if !tg_token.is_empty() {
        let tg_resp: serde_json::Value = client
            .get(format!("https://api.telegram.org/bot{tg_token}/getMe"))
            .send()?
            .json()?;
        let ok = tg_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            return Err(FinallyAValueBotError::Config(
                "Telegram getMe failed (check TELEGRAM_BOT_TOKEN)".into(),
            ));
        }
        let actual_username = tg_resp
            .get("result")
            .and_then(|r| r.get("username"))
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();
        if !env_username.is_empty()
            && !actual_username.is_empty()
            && env_username != actual_username
        {
            checks.push(format!(
                "Telegram OK (token user={actual_username}, configured={env_username})"
            ));
        } else {
            checks.push(format!("Telegram OK ({actual_username})"));
        }
    } else {
        checks.push("Telegram skipped (no token)".into());
    }

    // --- LLM validation: send a minimal "hi" message ---
    let preset = find_provider_preset(provider);
    let protocol = provider_protocol(provider);
    let model = if model.is_empty() {
        default_model_for_provider(provider).to_string()
    } else {
        model.to_string()
    };

    if protocol == ProviderProtocol::Anthropic {
        let mut base = if base_url.is_empty() {
            "https://api.anthropic.com".to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        if base.ends_with("/v1/messages") {
            base = base.trim_end_matches("/v1/messages").to_string();
        }
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let resp = client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(FinallyAValueBotError::Config(format!(
                "LLM validation failed: {detail}"
            )));
        }
        checks.push(format!("LLM OK (anthropic, model={model})"));
    } else {
        let mut base = if base_url.is_empty() {
            preset
                .map(|p| p.default_base_url)
                .filter(|s| !s.is_empty())
                .unwrap_or("https://api.openai.com/v1")
                .to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        if !base.ends_with("/v1") {
            base = format!("{}/v1", base.trim_end_matches('/'));
        }
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let mut req = client
            .post(format!("{base}/chat/completions"))
            .header("content-type", "application/json")
            .body(body.to_string());
        if !api_key.trim().is_empty() {
            req = req.bearer_auth(api_key);
        }
        let resp = req.send()?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(FinallyAValueBotError::Config(format!(
                "LLM validation failed: {detail}"
            )));
        }
        checks.push(format!("LLM OK (openai-compatible, model={model})"));
    }
    Ok(checks)
}

fn mask_secret(s: &str) -> String {
    if s.len() <= 6 {
        return "***".into();
    }
    format!("{}***{}", &s[..3], &s[s.len() - 2..])
}

fn save_config_env(
    path: &Path,
    values: &HashMap<String, String>,
) -> Result<Option<String>, FinallyAValueBotError> {
    let mut backup = None;
    if path.exists() {
        let ts = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let backup_path = format!("{}.bak.{}", path.display(), ts);
        fs::copy(path, &backup_path)?;
        backup = Some(backup_path);
    }

    let get = |key: &str| {
        values
            .get(key)
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    let mut lines = Vec::new();

    // Helper: emit a key=value line
    macro_rules! emit {
        ($section:expr, $key:expr, $val:expr, $always:expr) => {
            if !$section.is_empty() {
                lines.push("".into());
                lines.push(format!("# {}", $section));
            }
            let v = $val;
            if $always || !v.is_empty() {
                lines.push(format!("{}={}", $key, escape_env(&v)));
            }
        };
    }

    let workspace_dir = {
        let w = get("WORKSPACE_DIR");
        if w.is_empty() {
            "./workspace".to_string()
        } else {
            w
        }
    };

    // Clone vault if needed
    let vault_url = get("VAULT_GIT_URL");
    let shared_dir = Path::new(&workspace_dir).join("shared");
    let origin_path = shared_dir.join("ORIGIN");
    if !vault_url.is_empty() {
        let _ = fs::create_dir_all(&shared_dir);
        if !origin_path.exists() {
            let _ = Command::new("git")
                .args(["clone", &vault_url])
                .arg(&origin_path)
                .status();
        }
    } else {
        let _ = fs::create_dir_all(&origin_path);
    }

    lines.push("# FinallyAValueBot configuration".into());

    // Telegram
    emit!(
        "Telegram",
        "TELEGRAM_BOT_TOKEN",
        get("TELEGRAM_BOT_TOKEN"),
        true
    );
    emit!("", "BOT_USERNAME", get("BOT_USERNAME"), false);
    emit!("", "ALLOWED_GROUPS", get("ALLOWED_GROUPS"), false);
    emit!("", "CONTROL_CHAT_IDS", get("CONTROL_CHAT_IDS"), false);

    // LLM
    emit!("LLM", "LLM_PROVIDER", get("LLM_PROVIDER"), true);
    emit!("", "LLM_API_KEY", get("LLM_API_KEY"), true);
    emit!("", "LLM_MODEL", get("LLM_MODEL"), false);
    emit!("", "LLM_BASE_URL", get("LLM_BASE_URL"), false);
    emit!("", "OPENAI_API_KEY", get("OPENAI_API_KEY"), false);

    // Runtime
    emit!("Runtime", "WORKSPACE_DIR", workspace_dir, true);
    emit!("", "TIMEZONE", get("TIMEZONE"), true);
    emit!("", "MAX_TOKENS", get("MAX_TOKENS"), false);
    emit!("", "MAX_TOOL_ITERATIONS", get("MAX_TOOL_ITERATIONS"), false);
    emit!(
        "",
        "MAX_HISTORY_MESSAGES",
        get("MAX_HISTORY_MESSAGES"),
        false
    );
    emit!(
        "",
        "RECENT_HISTORY_MIN_USER_MESSAGES",
        get("RECENT_HISTORY_MIN_USER_MESSAGES"),
        false
    );
    emit!(
        "",
        "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES",
        get("RECENT_HISTORY_MIN_ASSISTANT_MESSAGES"),
        false
    );

    // Orchestrator
    emit!(
        "Orchestrator",
        "ORCHESTRATOR_ENABLED",
        get("ORCHESTRATOR_ENABLED"),
        false
    );
    emit!("", "ORCHESTRATOR_MODEL", get("ORCHESTRATOR_MODEL"), false);

    // Discord
    emit!(
        "Discord",
        "DISCORD_BOT_TOKEN",
        get("DISCORD_BOT_TOKEN"),
        false
    );
    emit!(
        "",
        "DISCORD_ALLOWED_CHANNELS",
        get("DISCORD_ALLOWED_CHANNELS"),
        false
    );

    // WhatsApp
    emit!(
        "WhatsApp",
        "WHATSAPP_ACCESS_TOKEN",
        get("WHATSAPP_ACCESS_TOKEN"),
        false
    );
    emit!(
        "",
        "WHATSAPP_PHONE_NUMBER_ID",
        get("WHATSAPP_PHONE_NUMBER_ID"),
        false
    );
    emit!(
        "",
        "WHATSAPP_VERIFY_TOKEN",
        get("WHATSAPP_VERIFY_TOKEN"),
        false
    );
    emit!(
        "",
        "WHATSAPP_WEBHOOK_PORT",
        get("WHATSAPP_WEBHOOK_PORT"),
        false
    );

    // Web UI
    emit!("Web UI", "WEB_ENABLED", get("WEB_ENABLED"), false);
    emit!("", "WEB_HOST", get("WEB_HOST"), false);
    emit!("", "WEB_PORT", get("WEB_PORT"), false);
    emit!("", "WEB_AUTH_TOKEN", get("WEB_AUTH_TOKEN"), false);

    // Browser
    emit!("Browser", "BROWSER_MANAGED", get("BROWSER_MANAGED"), false);
    emit!("", "STEEL_API_PORT", get("STEEL_API_PORT"), false);
    emit!("", "STEEL_CDP_PORT", get("STEEL_CDP_PORT"), false);
    emit!("", "STEEL_API_URL", get("STEEL_API_URL"), false);

    // Cursor Agent
    emit!(
        "Cursor Agent",
        "CURSOR_AGENT_CLI_PATH",
        get("CURSOR_AGENT_CLI_PATH"),
        false
    );
    emit!("", "CURSOR_AGENT_MODEL", get("CURSOR_AGENT_MODEL"), false);
    emit!(
        "",
        "CURSOR_AGENT_RUNNER_URL",
        get("CURSOR_AGENT_RUNNER_URL"),
        false
    );

    // Vault
    emit!(
        "Vault",
        "VAULT_ORIGIN_VAULT_PATH",
        get("VAULT_ORIGIN_VAULT_PATH"),
        false
    );
    emit!(
        "",
        "VAULT_VECTOR_DB_PATH",
        get("VAULT_VECTOR_DB_PATH"),
        false
    );
    if !vault_url.is_empty() {
        emit!("", "VAULT_ORIGIN_VAULT_REPO", vault_url.clone(), false);
    }
    emit!("", "VAULT_GIT_URL", vault_url, false);
    emit!(
        "",
        "VAULT_SEARCH_COMMAND",
        get("VAULT_SEARCH_COMMAND"),
        false
    );
    emit!("", "VAULT_INDEX_COMMAND", get("VAULT_INDEX_COMMAND"), false);
    emit!("", "VAULT_VECTOR_DB_URL", get("VAULT_VECTOR_DB_URL"), false);

    // Universal
    emit!(
        "Universal",
        "UNIVERSAL_CHAT_ID",
        get("UNIVERSAL_CHAT_ID"),
        false
    );

    // Git
    emit!("Git", "GIT_USERNAME", get("GIT_USERNAME"), false);
    emit!("", "GIT_TOKEN", get("GIT_TOKEN"), false);

    let content = lines.join("\n");
    fs::write(path, content)?;
    Ok(backup)
}

fn escape_env(s: &str) -> String {
    if s.contains(' ') || s.contains('"') || s.contains('#') || s.is_empty() {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn draw_ui(frame: &mut ratatui::Frame<'_>, app: &SetupApp) {
    if app.completed {
        let done = Paragraph::new(vec![
            Line::from(Span::styled(
                "✅ Setup saved successfully",
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Checks:"),
            Line::from(
                app.completion_summary
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Config validated".into()),
            ),
            Line::from(app.completion_summary.get(1).cloned().unwrap_or_default()),
            Line::from(""),
            Line::from(format!(
                "Backup: {}",
                app.backup_path.as_deref().unwrap_or("none")
            )),
            Line::from(""),
            Line::from("Next steps:"),
            Line::from("  1) Run in foreground:"),
            Line::from("     finally-a-value-bot start"),
            Line::from(""),
            Line::from("  2) Run in background (systemd service):"),
            Line::from("     finally-a-value-bot gateway install"),
            Line::from(""),
            Line::from("Press Enter to finish."),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Setup Complete"),
        );
        frame.render_widget(done, frame.area().inner(Margin::new(2, 2)));
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(14),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            "FinallyAValueBot • Interactive Setup",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                format!(
                    "Field {}/{}  ·  Section: {}  ·  ",
                    app.selected + 1,
                    app.fields.len(),
                    app.current_section()
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(app.progress_bar(16), Style::default().fg(Color::LightCyan)),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    let mut lines = Vec::<Line>::new();
    for (i, f) in app.fields.iter().enumerate() {
        let selected = i == app.selected;
        let label = if f.required {
            format!("{}  [required]", f.label)
        } else {
            f.label.to_string()
        };
        let value = if f.key == "LLM_PROVIDER" {
            provider_display(&f.value)
        } else {
            f.display_value(selected && app.editing)
        };
        let prefix = if selected { "▶" } else { " " };
        let color = if selected {
            Color::Yellow
        } else {
            Color::White
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix} {label}: "), Style::default().fg(color)),
            Span::styled(value, Style::default().fg(Color::Green)),
        ]));
    }
    let body = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Fields"))
        .wrap(Wrap { trim: false });
    frame.render_widget(body, body_chunks[0].inner(Margin::new(1, 0)));

    let field = app.selected_field();
    let help = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Key: ", Style::default().fg(Color::DarkGray)),
            Span::styled(field.key, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled("Required: ", Style::default().fg(Color::DarkGray)),
            Span::raw(if field.required { "yes" } else { "no" }),
        ]),
        Line::from(vec![
            Span::styled("Editing: ", Style::default().fg(Color::DarkGray)),
            Span::raw(if app.editing { "active" } else { "idle" }),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Tips",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("• Enter: edit field / open selection list"),
        Line::from("• Tab / Shift+Tab: next/prev field"),
        Line::from("• ↑/↓ in list: move, Enter: confirm, Esc: close"),
        Line::from("• ←/→ on provider/model: rotate presets"),
        Line::from("• e: force manual text edit"),
        Line::from("• Ctrl+D / Del: clear field"),
        Line::from("• Ctrl+R: restore field default"),
        Line::from("• F2: validate + online checks"),
        Line::from("• s / Ctrl+S: save config"),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Details / Help"),
    )
    .wrap(Wrap { trim: false });
    frame.render_widget(help, body_chunks[1].inner(Margin::new(1, 0)));

    let (status_icon, status_color) =
        if app.status.contains("failed") || app.status.contains("Cannot save") {
            ("✖ ", Color::LightRed)
        } else if app.status.contains("saved") || app.status.contains("Saved") {
            ("✔ ", Color::LightGreen)
        } else {
            ("• ", Color::White)
        };
    let status = Paragraph::new(vec![Line::from(vec![
        Span::styled(status_icon, Style::default().fg(status_color)),
        Span::styled(app.status.clone(), Style::default().fg(status_color)),
    ])])
    .block(Block::default().borders(Borders::ALL).title("Status"));
    frame.render_widget(status, chunks[2]);

    if let Some(picker) = app.picker {
        let overlay_area = frame.area().inner(Margin::new(8, 4));
        let (title, options): (&str, Vec<String>) = match picker.kind {
            PickerKind::Provider => (
                "Select LLM Provider",
                PROVIDER_PRESETS
                    .iter()
                    .map(|p| format!("{} ({})", p.id, p.label))
                    .collect(),
            ),
            PickerKind::Model => ("Select LLM Model", app.model_options()),
        };
        let mut list_lines = Vec::with_capacity(options.len());
        for (i, item) in options.iter().enumerate() {
            let selected = i == picker.selected;
            let prefix = if selected { "▶ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            list_lines.push(Line::from(Span::styled(format!("{prefix}{item}"), style)));
        }
        let overlay = Paragraph::new(list_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .style(Style::default().bg(Color::Black)),
            )
            .style(Style::default().bg(Color::Black))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, overlay_area);
        frame.render_widget(overlay, overlay_area);
    }
}

fn try_save(app: &mut SetupApp) {
    match app
        .validate_local()
        .and_then(|_| app.validate_online())
        .and_then(|checks| {
            let values = app.to_env_map();
            let backup = save_config_env(Path::new(".env"), &values)?;
            app.backup_path = backup;
            app.completion_summary = checks;
            Ok(())
        }) {
        Ok(_) => {
            app.status = "Saved .env".into();
            app.completed = true;
        }
        Err(e) => app.status = format!("Cannot save: {e}"),
    }
}

fn run_wizard(mut terminal: DefaultTerminal) -> Result<bool, FinallyAValueBotError> {
    let mut app = SetupApp::new();

    loop {
        terminal.draw(|f| draw_ui(f, &app))?;
        if event::poll(Duration::from_millis(250))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if app.completed {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('q') => return Ok(true),
                    _ => continue,
                }
            }

            if app.picker.is_some() {
                match key.code {
                    KeyCode::Esc => {
                        app.picker = None;
                        app.status = "Selection closed".into();
                    }
                    KeyCode::Up => app.move_picker(-1),
                    KeyCode::Down => app.move_picker(1),
                    KeyCode::Enter => app.apply_picker_selection(),
                    _ => {}
                }
                continue;
            }

            if app.editing {
                match key.code {
                    KeyCode::Esc => {
                        app.editing = false;
                        app.status = "Edit canceled".into();
                    }
                    KeyCode::Enter => {
                        app.editing = false;
                        app.status = format!("Updated {}", app.selected_field().key);
                    }
                    KeyCode::Backspace => {
                        app.selected_field_mut().value.pop();
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.clear_selected_field();
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.restore_selected_field_default();
                    }
                    KeyCode::Char(c) => {
                        app.selected_field_mut().value.push(c);
                    }
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') => return Ok(false),
                KeyCode::Up => app.prev(),
                KeyCode::Down => app.next(),
                KeyCode::Tab => app.next(),
                KeyCode::BackTab => app.prev(),
                KeyCode::Enter => {
                    if app.open_picker_for_selected() {
                        app.status = format!("Selecting {}", app.selected_field().key);
                    } else {
                        app.editing = true;
                        app.status = format!("Editing {}", app.selected_field().key);
                    }
                }
                KeyCode::Left => {
                    if app.selected_field().key == "LLM_PROVIDER" {
                        app.cycle_provider(-1);
                        app.status = format!("Provider set to {}", app.field_value("LLM_PROVIDER"));
                    } else if app.selected_field().key == "LLM_MODEL" {
                        app.cycle_model(-1);
                        app.status = format!("Model set to {}", app.field_value("LLM_MODEL"));
                    }
                }
                KeyCode::Right => {
                    if app.selected_field().key == "LLM_PROVIDER" {
                        app.cycle_provider(1);
                        app.status = format!("Provider set to {}", app.field_value("LLM_PROVIDER"));
                    } else if app.selected_field().key == "LLM_MODEL" {
                        app.cycle_model(1);
                        app.status = format!("Model set to {}", app.field_value("LLM_MODEL"));
                    }
                }
                KeyCode::Char('e') => {
                    app.editing = true;
                    app.status = format!("Editing {}", app.selected_field().key);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.clear_selected_field();
                }
                KeyCode::Delete => {
                    app.clear_selected_field();
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.restore_selected_field_default();
                }
                KeyCode::F(2) => match app.validate_local().and_then(|_| app.validate_online()) {
                    Ok(checks) => app.status = format!("Validation passed: {}", checks.join(" | ")),
                    Err(e) => app.status = format!("Validation failed: {e}"),
                },
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    try_save(&mut app);
                }
                KeyCode::Char('s') => {
                    try_save(&mut app);
                }
                _ => {}
            }
        }
    }
}

pub fn run_setup_wizard() -> Result<bool, FinallyAValueBotError> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;
    let result = run_wizard(terminal);
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_secret() {
        assert_eq!(mask_secret("abcdefghi"), "abc***hi");
        assert_eq!(mask_secret("abc"), "***");
    }

    #[test]
    fn test_save_config_yaml() {
        let yaml_path = std::env::temp_dir().join(format!(
            "finally_a_value_bot_setup_test_{}.yaml",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));

        let mut values = HashMap::new();
        values.insert("TELEGRAM_BOT_TOKEN".into(), "new_tok".into());
        values.insert("BOT_USERNAME".into(), "new_bot".into());
        values.insert("LLM_PROVIDER".into(), "anthropic".into());
        values.insert("LLM_API_KEY".into(), "key".into());

        let backup = save_config_env(&yaml_path, &values).unwrap();
        assert!(backup.is_none()); // No previous file to back up

        let s = fs::read_to_string(&yaml_path).unwrap();
        assert!(s.contains("TELEGRAM_BOT_TOKEN=new_tok"));
        assert!(s.contains("BOT_USERNAME=new_bot"));
        assert!(s.contains("LLM_PROVIDER=anthropic"));
        assert!(s.contains("LLM_API_KEY=key"));

        // Save again to test backup
        let backup2 = save_config_env(&yaml_path, &values).unwrap();
        assert!(backup2.is_some());

        let _ = fs::remove_file(&yaml_path);
    }
}
