//! Gemini ADK engine settings (default model, readiness). API key comes from `.env`
//! or a key saved under Settings → Connections.

use crate::config::Config;
use crate::db::Database;
use crate::error::FinallyAValueBotError;

pub const APP_SETTING_GEMINI_ADK_DEFAULT_MODEL: &str = "GEMINI_ADK_DEFAULT_MODEL";
pub const APP_SETTING_GEMINI_ADK_MAX_ITERATIONS: &str = "GEMINI_ADK_MAX_ITERATIONS";

pub const DEFAULT_GEMINI_ADK_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_MAX_AGENT_ITERATIONS: usize = 40;

#[derive(Debug, Clone)]
pub struct GeminiAdkSettings {
    pub default_model: String,
    pub max_iterations: usize,
}

impl GeminiAdkSettings {
    pub fn from_env(config: &Config) -> Self {
        let model = if (config.llm_provider.eq_ignore_ascii_case("google")
            || config.llm_provider.eq_ignore_ascii_case("gemini"))
            && !config.model.trim().is_empty()
        {
            config.model.clone()
        } else {
            DEFAULT_GEMINI_ADK_MODEL.to_string()
        };
        Self {
            default_model: model,
            max_iterations: DEFAULT_MAX_AGENT_ITERATIONS.max(config.max_tool_iterations.min(100)),
        }
    }

    pub fn api_key_configured(config: &Config) -> bool {
        !crate::llm_catalog::resolve_api_key_for_provider_with_config("google", Some(config))
            .is_empty()
    }

    pub fn engine_ready(&self, config: &Config) -> bool {
        Self::api_key_configured(config) && !self.default_model.trim().is_empty()
    }
}

pub fn load_from_db(
    db: &Database,
    config: &Config,
) -> Result<GeminiAdkSettings, FinallyAValueBotError> {
    let mut settings = GeminiAdkSettings::from_env(config);
    let rows = db.list_app_settings()?;
    for s in rows {
        if s.key
            .eq_ignore_ascii_case(APP_SETTING_GEMINI_ADK_DEFAULT_MODEL)
        {
            let v = s.value.trim();
            if !v.is_empty() {
                settings.default_model = v.to_string();
            }
        } else if s
            .key
            .eq_ignore_ascii_case(APP_SETTING_GEMINI_ADK_MAX_ITERATIONS)
        {
            if let Ok(n) = s.value.trim().parse::<usize>() {
                if n > 0 {
                    settings.max_iterations = n.min(200);
                }
            }
        }
    }
    Ok(settings)
}

pub fn persist_default_model(db: &Database, model: &str) -> Result<(), FinallyAValueBotError> {
    let v = model.trim();
    if v.is_empty() {
        return Err(FinallyAValueBotError::Config(
            "GEMINI_ADK_DEFAULT_MODEL must not be empty".into(),
        ));
    }
    db.set_app_setting(APP_SETTING_GEMINI_ADK_DEFAULT_MODEL, v)
}

pub fn persist_max_iterations(db: &Database, max: usize) -> Result<(), FinallyAValueBotError> {
    if max == 0 {
        return Err(FinallyAValueBotError::Config(
            "GEMINI_ADK_MAX_ITERATIONS must be >= 1".into(),
        ));
    }
    db.set_app_setting(
        APP_SETTING_GEMINI_ADK_MAX_ITERATIONS,
        &max.min(200).to_string(),
    )
}
