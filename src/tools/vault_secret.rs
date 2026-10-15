use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;
use crate::safety_redaction::EnvSecretRedactor;
use crate::secret_vault::{self, placeholder_for};

pub struct VaultSecretTool {
    runtime_data_dir: String,
    skills_dir: PathBuf,
    env_redactor: Arc<EnvSecretRedactor>,
}

impl VaultSecretTool {
    pub fn new(
        runtime_data_dir: &str,
        skills_dir: PathBuf,
        env_redactor: Arc<EnvSecretRedactor>,
    ) -> Self {
        Self {
            runtime_data_dir: runtime_data_dir.to_string(),
            skills_dir,
            env_redactor,
        }
    }
}

#[async_trait]
impl Tool for VaultSecretTool {
    fn name(&self) -> &str {
        "vault_secret"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "vault_secret",
            "Move sensitive values from the user message into this chat's secret vault. Call this first when the user pastes a password, login, API key, token, private key, recovery code, card number, or bank detail. Do not repeat the value afterwards. Use $NAME in bash and skill scripts, or [SECRET:NAME] in other tool inputs.",
            schema_object(
                json!({
                    "secrets": {
                        "type": "array",
                        "description": "One or more secrets to store",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "description": "Env var name, e.g. NOTION_TOKEN"
                                },
                                "value": {
                                    "type": "string",
                                    "description": "The secret value. Never echo this back to the user."
                                },
                                "kind": {
                                    "type": "string",
                                    "enum": ["api_key", "password", "username", "token", "other"],
                                    "description": "What kind of secret this is"
                                },
                                "skill": {
                                    "type": "string",
                                    "description": "Optional skill directory name. When set, the value is also written to skills/<skill>/.env"
                                },
                                "note": {
                                    "type": "string",
                                    "description": "Optional short label. Do not put the secret value here."
                                }
                            },
                            "required": ["name", "value"]
                        }
                    }
                }),
                &["secrets"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(auth) => auth,
            None => return ToolResult::error("Missing auth context".into()),
        };
        let chat_id = auth.caller_chat_id;
        let Some(items) = input.get("secrets").and_then(|v| v.as_array()) else {
            return ToolResult::error("Missing 'secrets' array".into());
        };
        if items.is_empty() {
            return ToolResult::error("'secrets' must contain at least one entry".into());
        }
        let mut lines = Vec::new();
        for item in items {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_uppercase();
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let kind = item
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("other")
                .trim();
            let kind = if matches!(
                kind,
                "api_key" | "password" | "username" | "token" | "other"
            ) {
                kind
            } else {
                "other"
            };
            let skill = item
                .get("skill")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match secret_vault::upsert(
                &self.runtime_data_dir,
                &self.skills_dir,
                chat_id,
                &name,
                value,
                kind,
                skill,
                Some(auth.caller_persona_id),
                self.env_redactor.as_ref(),
            ) {
                Ok(stored) => {
                    let place = placeholder_for(&stored.name, chat_id);
                    let skill_note = stored
                        .skill
                        .as_ref()
                        .map(|s| format!(" Also wrote skills/{s}/.env."))
                        .unwrap_or_default();
                    let scrub_note = if stored.chat_wide {
                        "History will be rewritten to the placeholder after this turn."
                    } else {
                        "This value is shorter than 6 characters, so older messages are left unchanged."
                    };
                    lines.push(format!(
                        "Stored {} as {place}.{skill_note} Use ${} in bash or skill scripts, or {place} in other tool inputs. {scrub_note} Do not repeat the value.",
                        stored.name, stored.name
                    ));
                }
                Err(e) => return ToolResult::error(e),
            }
        }
        tracing::info!(
            target: "redaction",
            chat_id,
            count = lines.len(),
            "vault_secret stored values"
        );
        ToolResult::success(lines.join("\n"))
    }
}
