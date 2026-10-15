//! Chat channel integration settings persisted in SQLite (`app_settings` + `channel_bot_instances`).
//!
//! Bootstrap (`WEB_*`, `WORKSPACE_DIR`) stays in `.env`. Telegram / Discord / WhatsApp / WeCom tokens and
//! platform options are configured in Web UI → Settings → Integrations.

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::Config;
use crate::db::{
    Database, BOT_INSTANCE_DISCORD_PRIMARY, BOT_INSTANCE_TELEGRAM_PRIMARY,
    BOT_INSTANCE_WECOM_PRIMARY, BOT_INSTANCE_WHATSAPP_PRIMARY,
};
use crate::error::FinallyAValueBotError;

pub const APP_SETTING_BOT_USERNAME: &str = "BOT_USERNAME";
pub const APP_SETTING_ALLOWED_GROUPS: &str = "ALLOWED_GROUPS";
pub const APP_SETTING_CONTROL_CHAT_IDS: &str = "CONTROL_CHAT_IDS";
pub const APP_SETTING_DISCORD_ALLOWED_CHANNELS: &str = "DISCORD_ALLOWED_CHANNELS";
pub const APP_SETTING_WHATSAPP_PHONE_NUMBER_ID: &str = "WHATSAPP_PHONE_NUMBER_ID";
pub const APP_SETTING_WHATSAPP_VERIFY_TOKEN: &str = "WHATSAPP_VERIFY_TOKEN";
pub const APP_SETTING_WHATSAPP_WEBHOOK_PORT: &str = "WHATSAPP_WEBHOOK_PORT";
/// Set after the one-time env → DB channel integration import so clearing UI tokens
/// is not clobbered by leftover `.env` values on the next boot.
pub const APP_SETTING_CHANNEL_INTEGRATION_SEEDED: &str = "CHANNEL_INTEGRATION_SEEDED";

const CHANNEL_INTEGRATION_APP_KEYS: &[&str] = &[
    APP_SETTING_BOT_USERNAME,
    APP_SETTING_ALLOWED_GROUPS,
    APP_SETTING_CONTROL_CHAT_IDS,
    APP_SETTING_DISCORD_ALLOWED_CHANNELS,
    APP_SETTING_WHATSAPP_PHONE_NUMBER_ID,
    APP_SETTING_WHATSAPP_VERIFY_TOKEN,
    APP_SETTING_WHATSAPP_WEBHOOK_PORT,
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelIntegrationSettings {
    pub bot_username: String,
    pub allowed_groups: Vec<i64>,
    pub control_chat_ids: Vec<i64>,
    pub discord_allowed_channels: Vec<u64>,
    pub whatsapp_phone_number_id: String,
    pub whatsapp_verify_token: String,
    pub whatsapp_webhook_port: u16,
}

impl Default for ChannelIntegrationSettings {
    fn default() -> Self {
        Self {
            bot_username: String::new(),
            allowed_groups: Vec::new(),
            control_chat_ids: Vec::new(),
            discord_allowed_channels: Vec::new(),
            whatsapp_phone_number_id: String::new(),
            whatsapp_verify_token: String::new(),
            whatsapp_webhook_port: 8080,
        }
    }
}

impl ChannelIntegrationSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            bot_username: config.bot_username.trim().to_string(),
            allowed_groups: config.allowed_groups.clone(),
            control_chat_ids: config.control_chat_ids.clone(),
            discord_allowed_channels: config.discord_allowed_channels.clone(),
            whatsapp_phone_number_id: config
                .whatsapp_phone_number_id
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string(),
            whatsapp_verify_token: config
                .whatsapp_verify_token
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string(),
            whatsapp_webhook_port: if config.whatsapp_webhook_port == 0 {
                8080
            } else {
                config.whatsapp_webhook_port
            },
        }
    }

    pub fn validate(&self) -> Result<(), FinallyAValueBotError> {
        if self.whatsapp_webhook_port == 0 {
            return Err(FinallyAValueBotError::Config(
                "whatsapp_webhook_port must be between 1 and 65535".into(),
            ));
        }
        Ok(())
    }
}

fn read_setting(settings: &[(String, String)], key: &str) -> Option<String> {
    settings
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim().to_string())
}

fn parse_csv_i64(raw: &str) -> Vec<i64> {
    raw.split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

fn parse_csv_u64(raw: &str) -> Vec<u64> {
    raw.split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

fn format_csv_i64(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn format_csv_u64(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn app_settings_map(db: &Database) -> Result<Vec<(String, String)>, FinallyAValueBotError> {
    Ok(db
        .list_app_settings()?
        .into_iter()
        .map(|s| (s.key, s.value))
        .collect())
}

fn has_app_key(settings: &[(String, String)], key: &str) -> bool {
    settings.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
}

/// Load platform options from `app_settings`, falling back to values already on `Config`
/// (typically env defaults before merge).
pub fn load_from_db(
    db: &Database,
    fallback: &Config,
) -> Result<ChannelIntegrationSettings, FinallyAValueBotError> {
    let rows = app_settings_map(db)?;
    let mut cfg = ChannelIntegrationSettings::from_config(fallback);

    if let Some(v) = read_setting(&rows, APP_SETTING_BOT_USERNAME) {
        cfg.bot_username = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_ALLOWED_GROUPS) {
        cfg.allowed_groups = parse_csv_i64(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_CONTROL_CHAT_IDS) {
        cfg.control_chat_ids = parse_csv_i64(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_DISCORD_ALLOWED_CHANNELS) {
        cfg.discord_allowed_channels = parse_csv_u64(&v);
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_WHATSAPP_PHONE_NUMBER_ID) {
        cfg.whatsapp_phone_number_id = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_WHATSAPP_VERIFY_TOKEN) {
        cfg.whatsapp_verify_token = v;
    }
    if let Some(v) = read_setting(&rows, APP_SETTING_WHATSAPP_WEBHOOK_PORT) {
        if let Ok(port) = v.parse::<u16>() {
            if port > 0 {
                cfg.whatsapp_webhook_port = port;
            }
        }
    }
    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)? {
        if !inst.bot_username.trim().is_empty() {
            cfg.bot_username = inst.bot_username.trim().to_string();
        }
        cfg.allowed_groups = inst.allowed_groups;
    }
    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)? {
        cfg.discord_allowed_channels = inst.discord_allowed_channels;
    }
    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)? {
        cfg.whatsapp_phone_number_id = inst.whatsapp_phone_number_id.trim().to_string();
        cfg.whatsapp_verify_token = inst.whatsapp_verify_token.trim().to_string();
        cfg.whatsapp_webhook_port = inst.whatsapp_webhook_port;
    }
    cfg.validate()?;
    Ok(cfg)
}

pub fn save_to_db(
    db: &Database,
    settings: &ChannelIntegrationSettings,
) -> Result<(), FinallyAValueBotError> {
    settings.validate()?;
    db.set_app_setting(APP_SETTING_BOT_USERNAME, settings.bot_username.trim())?;
    db.set_app_setting(
        APP_SETTING_ALLOWED_GROUPS,
        &format_csv_i64(&settings.allowed_groups),
    )?;
    db.set_app_setting(
        APP_SETTING_CONTROL_CHAT_IDS,
        &format_csv_i64(&settings.control_chat_ids),
    )?;
    db.set_app_setting(
        APP_SETTING_DISCORD_ALLOWED_CHANNELS,
        &format_csv_u64(&settings.discord_allowed_channels),
    )?;
    db.set_app_setting(
        APP_SETTING_WHATSAPP_PHONE_NUMBER_ID,
        settings.whatsapp_phone_number_id.trim(),
    )?;
    db.set_app_setting(
        APP_SETTING_WHATSAPP_VERIFY_TOKEN,
        settings.whatsapp_verify_token.trim(),
    )?;
    db.set_app_setting(
        APP_SETTING_WHATSAPP_WEBHOOK_PORT,
        &settings.whatsapp_webhook_port.to_string(),
    )?;
    if db
        .get_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)?
        .is_some()
    {
        db.update_channel_bot_instance_options(
            BOT_INSTANCE_TELEGRAM_PRIMARY,
            Some(&settings.bot_username),
            Some(&settings.allowed_groups),
            None,
            None,
            None,
            None,
        )?;
    }
    if db
        .get_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)?
        .is_some()
    {
        db.update_channel_bot_instance_options(
            BOT_INSTANCE_DISCORD_PRIMARY,
            None,
            None,
            Some(&settings.discord_allowed_channels),
            None,
            None,
            None,
        )?;
    }
    if db
        .get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)?
        .is_some()
    {
        db.update_channel_bot_instance_options(
            BOT_INSTANCE_WHATSAPP_PRIMARY,
            None,
            None,
            None,
            Some(&settings.whatsapp_phone_number_id),
            Some(&settings.whatsapp_verify_token),
            Some(settings.whatsapp_webhook_port),
        )?;
    }
    Ok(())
}

/// One-time import from env-backed `Config` when DB has no channel integration data yet.
pub fn migrate_from_env_if_empty(
    db: &Database,
    config: &Config,
) -> Result<(), FinallyAValueBotError> {
    let rows = app_settings_map(db)?;
    let already_seeded = has_app_key(&rows, APP_SETTING_CHANNEL_INTEGRATION_SEEDED);
    let from_env = ChannelIntegrationSettings::from_config(config);

    for key in CHANNEL_INTEGRATION_APP_KEYS {
        if has_app_key(&rows, key) {
            continue;
        }
        let value = match *key {
            APP_SETTING_BOT_USERNAME => from_env.bot_username.clone(),
            APP_SETTING_ALLOWED_GROUPS => format_csv_i64(&from_env.allowed_groups),
            APP_SETTING_CONTROL_CHAT_IDS => format_csv_i64(&from_env.control_chat_ids),
            APP_SETTING_DISCORD_ALLOWED_CHANNELS => {
                format_csv_u64(&from_env.discord_allowed_channels)
            }
            APP_SETTING_WHATSAPP_PHONE_NUMBER_ID => from_env.whatsapp_phone_number_id.clone(),
            APP_SETTING_WHATSAPP_VERIFY_TOKEN => from_env.whatsapp_verify_token.clone(),
            APP_SETTING_WHATSAPP_WEBHOOK_PORT => from_env.whatsapp_webhook_port.to_string(),
            _ => continue,
        };
        // Skip writing empty optional strings except webhook port (always meaningful).
        if value.is_empty() && *key != APP_SETTING_WHATSAPP_WEBHOOK_PORT {
            continue;
        }
        db.set_app_setting(key, &value)?;
    }

    // Import primary tokens from env only once. After seeding, UI/DB is authoritative even if .env still has tokens.
    if !already_seeded {
        if db
            .get_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)?
            .is_none()
        {
            let tok = config.telegram_bot_token.trim();
            if !tok.is_empty() {
                db.upsert_primary_channel_bot_instance(
                    BOT_INSTANCE_TELEGRAM_PRIMARY,
                    "telegram",
                    "Primary Telegram",
                    tok,
                )?;
            }
        }

        if db
            .get_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)?
            .is_none()
        {
            if let Some(ref t) = config.discord_bot_token {
                let tok = t.trim();
                if !tok.is_empty() {
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_DISCORD_PRIMARY,
                        "discord",
                        "Primary Discord",
                        tok,
                    )?;
                }
            }
        }

        if db
            .get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)?
            .is_none()
        {
            let wa = config.whatsapp_access_token.as_deref().unwrap_or("").trim();
            if !wa.is_empty() {
                db.upsert_primary_channel_bot_instance(
                    BOT_INSTANCE_WHATSAPP_PRIMARY,
                    "whatsapp",
                    "Primary WhatsApp",
                    wa,
                )?;
            }
        }

        if db
            .get_channel_bot_instance(BOT_INSTANCE_WECOM_PRIMARY)?
            .is_none()
        {
            let secret = config.wecom_corp_secret.as_deref().unwrap_or("").trim();
            if !secret.is_empty() {
                db.upsert_primary_channel_bot_instance(
                    BOT_INSTANCE_WECOM_PRIMARY,
                    "wecom",
                    "Primary WeCom",
                    secret,
                )?;
                let allowed = config.wecom_allowed_chats.join(",");
                db.update_channel_bot_instance_wecom_options(
                    BOT_INSTANCE_WECOM_PRIMARY,
                    config.wecom_corp_id.as_deref(),
                    Some(config.wecom_agent_id),
                    config.wecom_callback_token.as_deref(),
                    config.wecom_encoding_aes_key.as_deref(),
                    Some(if config.wecom_webhook_port == 0 {
                        8081
                    } else {
                        config.wecom_webhook_port
                    }),
                    Some(allowed.as_str()),
                    config.wecom_aibot_id.as_deref(),
                    Some(config.wecom_mode.as_str()),
                )?;
            }
        }

        save_to_db(db, &from_env)?;
        db.set_app_setting(APP_SETTING_CHANNEL_INTEGRATION_SEEDED, "1")?;
    }

    warn_if_stale_env_channel_vars(config);
    Ok(())
}

fn warn_if_stale_env_channel_vars(config: &Config) {
    let mut present = Vec::new();
    if !config.telegram_bot_token.trim().is_empty() {
        present.push("TELEGRAM_BOT_TOKEN");
    }
    if config
        .discord_bot_token
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        present.push("DISCORD_BOT_TOKEN");
    }
    if config
        .whatsapp_access_token
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        present.push("WHATSAPP_ACCESS_TOKEN");
    }
    if config
        .wecom_corp_secret
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        present.push("WECOM_SECRET");
    }
    if !config.bot_username.trim().is_empty() {
        present.push("BOT_USERNAME");
    }
    if !config.allowed_groups.is_empty() {
        present.push("ALLOWED_GROUPS");
    }
    if !config.control_chat_ids.is_empty() {
        present.push("CONTROL_CHAT_IDS");
    }
    if !config.discord_allowed_channels.is_empty() {
        present.push("DISCORD_ALLOWED_CHANNELS");
    }
    if config
        .whatsapp_phone_number_id
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        present.push("WHATSAPP_PHONE_NUMBER_ID");
    }
    if config
        .whatsapp_verify_token
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        present.push("WHATSAPP_VERIFY_TOKEN");
    }
    if !present.is_empty() {
        warn!(
            "Channel env vars still set ({}), but SQLite is the source of truth. Remove them from .env after confirming Integrations settings.",
            present.join(", ")
        );
    }
}

/// Apply DB channel integration settings (and primary tokens) onto `Config`.
pub fn merge_into_config(config: &mut Config, db: &Database) -> Result<(), FinallyAValueBotError> {
    let settings = load_from_db(db, config)?;
    config.bot_username = settings.bot_username;
    config.allowed_groups = settings.allowed_groups;
    config.control_chat_ids = settings.control_chat_ids;
    config.discord_allowed_channels = settings.discord_allowed_channels;
    config.whatsapp_phone_number_id = if settings.whatsapp_phone_number_id.is_empty() {
        None
    } else {
        Some(settings.whatsapp_phone_number_id)
    };
    config.whatsapp_verify_token = if settings.whatsapp_verify_token.is_empty() {
        None
    } else {
        Some(settings.whatsapp_verify_token)
    };
    config.whatsapp_webhook_port = settings.whatsapp_webhook_port;

    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)? {
        config.telegram_bot_token = inst.token.trim().to_string();
    } else {
        config.telegram_bot_token.clear();
    }

    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)? {
        let tok = inst.token.trim().to_string();
        config.discord_bot_token = if tok.is_empty() { None } else { Some(tok) };
    } else {
        config.discord_bot_token = None;
    }

    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)? {
        let tok = inst.token.trim().to_string();
        config.whatsapp_access_token = if tok.is_empty() { None } else { Some(tok) };
    } else {
        config.whatsapp_access_token = None;
    }

    if let Some(inst) = db.get_channel_bot_instance(BOT_INSTANCE_WECOM_PRIMARY)? {
        let tok = inst.token.trim().to_string();
        config.wecom_corp_secret = if tok.is_empty() { None } else { Some(tok) };
        config.wecom_corp_id = if inst.wecom_corp_id.trim().is_empty() {
            None
        } else {
            Some(inst.wecom_corp_id.trim().to_string())
        };
        config.wecom_agent_id = inst.wecom_agent_id;
        config.wecom_callback_token = if inst.wecom_callback_token.trim().is_empty() {
            None
        } else {
            Some(inst.wecom_callback_token.trim().to_string())
        };
        config.wecom_encoding_aes_key = if inst.wecom_encoding_aes_key.trim().is_empty() {
            None
        } else {
            Some(inst.wecom_encoding_aes_key.trim().to_string())
        };
        config.wecom_webhook_port = if inst.wecom_webhook_port == 0 {
            8081
        } else {
            inst.wecom_webhook_port
        };
        config.wecom_allowed_chats = inst
            .wecom_allowed_chats
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        config.wecom_aibot_id = if inst.wecom_aibot_id.trim().is_empty() {
            None
        } else {
            Some(inst.wecom_aibot_id.trim().to_string())
        };
        config.wecom_mode = inst.wecom_mode.trim().to_string();
    } else {
        config.wecom_corp_secret = None;
        config.wecom_corp_id = None;
        config.wecom_agent_id = 0;
        config.wecom_callback_token = None;
        config.wecom_encoding_aes_key = None;
        config.wecom_webhook_port = 8081;
        config.wecom_allowed_chats = Vec::new();
        config.wecom_aibot_id = None;
        config.wecom_mode = String::new();
    }

    Ok(())
}

/// True when any Telegram / Discord / WhatsApp bot instance has a non-empty token.
pub fn has_any_messaging_bot_token(db: &Database) -> Result<bool, FinallyAValueBotError> {
    for inst in db.list_all_channel_bot_instances()? {
        if matches!(
            inst.platform.as_str(),
            "telegram" | "discord" | "whatsapp" | "wecom"
        ) && !inst.token.trim().is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Process may start if web UI is enabled or at least one messaging bot token exists in DB.
pub fn validate_runtime_channel_or_web(
    config: &Config,
    db: &Database,
) -> Result<(), FinallyAValueBotError> {
    if config.web_enabled {
        return Ok(());
    }
    if has_any_messaging_bot_token(db)? {
        return Ok(());
    }
    Err(FinallyAValueBotError::Config(
        "No messaging channel configured. Enable WEB_ENABLED=true or add a Telegram/Discord/WhatsApp/WeCom bot in Web UI → Settings → Integrations (or migrate from .env on first boot).".into(),
    ))
}

/// Patch body for `PATCH /api/channels/integration` (partial updates).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelIntegrationPatch {
    #[serde(default)]
    pub bot_username: Option<String>,
    #[serde(default)]
    pub allowed_groups: Option<String>,
    #[serde(default)]
    pub control_chat_ids: Option<String>,
    #[serde(default)]
    pub discord_allowed_channels: Option<String>,
    #[serde(default)]
    pub whatsapp_phone_number_id: Option<String>,
    #[serde(default)]
    pub whatsapp_verify_token: Option<String>,
    #[serde(default)]
    pub whatsapp_webhook_port: Option<u16>,
    #[serde(default)]
    pub telegram_token: Option<String>,
    #[serde(default)]
    pub telegram_label: Option<String>,
    #[serde(default)]
    pub discord_token: Option<String>,
    #[serde(default)]
    pub discord_label: Option<String>,
    #[serde(default)]
    pub whatsapp_access_token: Option<String>,
    #[serde(default)]
    pub whatsapp_label: Option<String>,
}

impl ChannelIntegrationPatch {
    pub fn apply_and_save(
        &self,
        db: &Database,
        current_config: &Config,
    ) -> Result<ChannelIntegrationSettings, FinallyAValueBotError> {
        let mut settings = load_from_db(db, current_config)?;

        if let Some(ref v) = self.bot_username {
            settings.bot_username = v.trim().to_string();
        }
        if let Some(ref v) = self.allowed_groups {
            settings.allowed_groups = parse_csv_i64(v);
        }
        if let Some(ref v) = self.control_chat_ids {
            settings.control_chat_ids = parse_csv_i64(v);
        }
        if let Some(ref v) = self.discord_allowed_channels {
            settings.discord_allowed_channels = parse_csv_u64(v);
        }
        if let Some(ref v) = self.whatsapp_phone_number_id {
            settings.whatsapp_phone_number_id = v.trim().to_string();
        }
        if let Some(ref v) = self.whatsapp_verify_token {
            // Empty string clears; omit field leaves unchanged. Sentinel "***" / masked not accepted.
            if looks_like_masked_secret(v) {
                // leave unchanged
            } else {
                settings.whatsapp_verify_token = v.trim().to_string();
            }
        }
        if let Some(port) = self.whatsapp_webhook_port {
            settings.whatsapp_webhook_port = port;
        }

        let whatsapp_token_set = db
            .get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)?
            .map(|i| !i.token.trim().is_empty())
            .unwrap_or(false)
            || self
                .whatsapp_access_token
                .as_deref()
                .map(|t| !t.trim().is_empty() && !looks_like_masked_secret(t))
                .unwrap_or(false);
        if whatsapp_token_set
            && !settings.whatsapp_phone_number_id.trim().is_empty()
            && settings.whatsapp_verify_token.trim().is_empty()
        {
            return Err(FinallyAValueBotError::Config(
                "whatsapp_verify_token is required when WhatsApp phone number id is set".into(),
            ));
        }

        save_to_db(db, &settings)?;

        if let Some(ref token) = self.telegram_token {
            if !looks_like_masked_secret(token) {
                let label = self
                    .telegram_label
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Primary Telegram");
                let tok = token.trim();
                if tok.is_empty() {
                    let _ = db.delete_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)?;
                } else {
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_TELEGRAM_PRIMARY,
                        "telegram",
                        label,
                        tok,
                    )?;
                }
            }
        } else if let Some(ref label) = self.telegram_label {
            if let Some(mut inst) = db.get_channel_bot_instance(BOT_INSTANCE_TELEGRAM_PRIMARY)? {
                let lbl = label.trim();
                if !lbl.is_empty() {
                    inst.label = lbl.to_string();
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_TELEGRAM_PRIMARY,
                        "telegram",
                        &inst.label,
                        &inst.token,
                    )?;
                }
            }
        }

        if let Some(ref token) = self.discord_token {
            if !looks_like_masked_secret(token) {
                let label = self
                    .discord_label
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Primary Discord");
                let tok = token.trim();
                if tok.is_empty() {
                    let _ = db.delete_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)?;
                } else {
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_DISCORD_PRIMARY,
                        "discord",
                        label,
                        tok,
                    )?;
                }
            }
        } else if let Some(ref label) = self.discord_label {
            if let Some(mut inst) = db.get_channel_bot_instance(BOT_INSTANCE_DISCORD_PRIMARY)? {
                let lbl = label.trim();
                if !lbl.is_empty() {
                    inst.label = lbl.to_string();
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_DISCORD_PRIMARY,
                        "discord",
                        &inst.label,
                        &inst.token,
                    )?;
                }
            }
        }

        if let Some(ref token) = self.whatsapp_access_token {
            if !looks_like_masked_secret(token) {
                let label = self
                    .whatsapp_label
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Primary WhatsApp");
                let tok = token.trim();
                if tok.is_empty() {
                    let _ = db.delete_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)?;
                } else {
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_WHATSAPP_PRIMARY,
                        "whatsapp",
                        label,
                        tok,
                    )?;
                }
            }
        } else if let Some(ref label) = self.whatsapp_label {
            if let Some(mut inst) = db.get_channel_bot_instance(BOT_INSTANCE_WHATSAPP_PRIMARY)? {
                let lbl = label.trim();
                if !lbl.is_empty() {
                    inst.label = lbl.to_string();
                    db.upsert_primary_channel_bot_instance(
                        BOT_INSTANCE_WHATSAPP_PRIMARY,
                        "whatsapp",
                        &inst.label,
                        &inst.token,
                    )?;
                }
            }
        }

        // If this patch created a primary row, propagate platform options now that
        // the row exists. Existing rows were already updated by the first save.
        save_to_db(db, &settings)?;

        load_from_db(db, current_config)
    }
}

fn looks_like_masked_secret(value: &str) -> bool {
    let v = value.trim();
    v.contains("***") || v == "***"
}
