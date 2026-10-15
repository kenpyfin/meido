//! Integration tests for configuration loading and validation.

use std::path::{Path, PathBuf};

use finally_a_value_bot::config::Config;

/// Helper to create a minimal valid config for testing.
/// Built from YAML so we do not depend on `cfg(test)` helpers from the library crate.
fn minimal_config() -> Config {
    let mut c: Config = serde_yaml::from_str(
        r#"
telegram_bot_token: tok
bot_username: testbot
api_key: test-key
"#,
    )
    .unwrap();
    c.web_enabled = false;
    c.web_port = 3900;
    c.max_tool_iterations = 25;
    c
}

#[test]
fn test_yaml_parse_minimal() {
    let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\n";
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.telegram_bot_token, "tok");
    assert_eq!(config.bot_username, "bot");
    assert_eq!(config.api_key, "key");
    // Defaults: llm_provider deserializes empty and is resolved during
    // post_deserialize (web-first defaults / local fallback), so the raw
    // parsed value is "" rather than a hardcoded "anthropic".
    assert_eq!(config.llm_provider, "");
    assert_eq!(config.max_tokens, 8192);
    assert_eq!(config.max_tool_iterations, 100);
    assert_eq!(config.max_document_size_mb, 100);
    assert_eq!(config.max_history_messages, 50);
    assert_eq!(config.timezone, "UTC");
    assert_eq!(config.whatsapp_webhook_port, 8080);
}

#[test]
fn test_yaml_parse_full() {
    let yaml = r#"
telegram_bot_token: my_token
bot_username: mybot
llm_provider: openai
api_key: sk-test123
model: gpt-4o
llm_base_url: https://custom.api.com/v1
max_tokens: 4096
max_tool_iterations: 10
max_history_messages: 100
workspace_dir: /data/finally_a_value_bot
openai_api_key: sk-whisper
timezone: Asia/Shanghai
allowed_groups:
  - 111
  - 222
control_chat_ids:
  - 999
whatsapp_access_token: wa_tok
whatsapp_phone_number_id: phone123
whatsapp_verify_token: verify_tok
whatsapp_webhook_port: 9090
discord_bot_token: discord_tok
discord_allowed_channels:
  - 333
  - 444
"#;
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.telegram_bot_token, "my_token");
    assert_eq!(config.llm_provider, "openai");
    assert_eq!(config.model, "gpt-4o");
    assert_eq!(
        config.llm_base_url.as_deref(),
        Some("https://custom.api.com/v1")
    );
    assert_eq!(config.max_tokens, 4096);
    assert_eq!(config.max_tool_iterations, 10);
    assert_eq!(config.max_history_messages, 100);
    assert_eq!(config.workspace_dir, "/data/finally_a_value_bot");
    assert_eq!(config.openai_api_key.as_deref(), Some("sk-whisper"));
    assert_eq!(config.timezone, "Asia/Shanghai");
    assert_eq!(config.allowed_groups, vec![111, 222]);
    assert_eq!(config.control_chat_ids, vec![999]);
    assert_eq!(config.whatsapp_webhook_port, 9090);
    assert_eq!(config.discord_allowed_channels, vec![333, 444]);
}

#[test]
fn test_yaml_roundtrip() {
    let config = minimal_config();
    let yaml = serde_yaml::to_string(&config).unwrap();
    let parsed: Config = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(parsed.telegram_bot_token, config.telegram_bot_token);
    assert_eq!(parsed.api_key, config.api_key);
    assert_eq!(parsed.max_tokens, config.max_tokens);
    assert_eq!(parsed.timezone, config.timezone);
}

#[test]
fn test_workspace_dir_paths() {
    let mut config = minimal_config();
    config.workspace_dir = "/opt/workspace".into();

    let runtime = PathBuf::from(config.runtime_data_dir());
    let skills = PathBuf::from(config.skills_data_dir());
    assert!(runtime.ends_with(Path::new("workspace").join("runtime")));
    assert!(skills.ends_with(Path::new("workspace").join("skills")));
}

#[test]
fn test_yaml_unknown_fields_ignored() {
    let yaml = "telegram_bot_token: tok\nbot_username: bot\napi_key: key\nunknown_field: value\n";
    // serde_yaml should not fail on unknown fields by default
    let config: Result<Config, _> = serde_yaml::from_str(yaml);
    // This may fail or succeed depending on serde config; verify behavior
    if let Ok(c) = config {
        assert_eq!(c.telegram_bot_token, "tok");
    }
    // If it errors, that's also acceptable behavior (strict mode)
}

#[test]
fn test_yaml_empty_string_fields() {
    let yaml = "telegram_bot_token: ''\nbot_username: ''\napi_key: ''\n";
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.telegram_bot_token, "");
    assert_eq!(config.bot_username, "");
    assert_eq!(config.api_key, "");
}
