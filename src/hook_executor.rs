use std::path::{Path, PathBuf};

use crate::claude::{Message, MessageContent, ResponseContentBlock};
use crate::config::Config;
use crate::error::FinallyAValueBotError;
use crate::llm;
use crate::safety_redaction::EnvSecretRedactor;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

const HOOK_OUTPUT_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookOutputMemoryTier3Prune {
    #[serde(default)]
    pub terminal_pz_post_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookOutputEffects {
    #[serde(default)]
    pub memory_tier3_prune: Option<HookOutputMemoryTier3Prune>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookOutput {
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub user_message: Option<String>,
    #[serde(default)]
    pub agent_message: Option<String>,
    #[serde(default)]
    pub additional_context: Option<String>,
    #[serde(default)]
    pub updated_tool_input: Option<Value>,
    #[serde(default)]
    pub updated_assistant_text: Option<String>,
    #[serde(default)]
    pub effects: Option<HookOutputEffects>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommandPayload {
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub fail_closed: Option<bool>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPromptPayload {
    pub prompt: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub fail_closed: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
}

fn nonempty_trimmed(input: Option<&str>) -> Option<String> {
    input
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn candidate_hook_roots(config: &Config) -> Vec<PathBuf> {
    let mut roots = vec![config.workspace_root_absolute().join("hooks")];
    if let Some(p) = crate::builtin_hooks::resolve_builtin_hooks_dir(config) {
        roots.push(p);
    }
    roots
}

fn canonicalize_if_exists(path: &Path) -> Result<PathBuf, FinallyAValueBotError> {
    std::fs::canonicalize(path).map_err(|e| {
        FinallyAValueBotError::ToolExecution(format!(
            "hook path '{}' is invalid: {}",
            path.display(),
            e
        ))
    })
}

fn ensure_inside_root(path: &Path, root: &Path) -> bool {
    let Ok(path) = canonicalize_if_exists(path) else {
        return false;
    };
    let Ok(root) = canonicalize_if_exists(root) else {
        return false;
    };
    path.starts_with(root)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn resolve_hook_command_path(
    config: &Config,
    command: &str,
) -> Result<PathBuf, FinallyAValueBotError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(FinallyAValueBotError::ToolExecution(
            "hook command is required".to_string(),
        ));
    }
    if command.contains('\n') {
        return Err(FinallyAValueBotError::ToolExecution(
            "hook command must be a single relative file path".to_string(),
        ));
    }
    let rel = PathBuf::from(command);
    if rel.is_absolute() {
        return Err(FinallyAValueBotError::ToolExecution(
            "hook command must be relative to workspace hooks or builtin_hooks".to_string(),
        ));
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(FinallyAValueBotError::ToolExecution(
            "hook command cannot contain '..'".to_string(),
        ));
    }

    let roots = candidate_hook_roots(config);
    for root in roots {
        let candidate = root.join(&rel);
        if candidate.is_file() && ensure_inside_root(&candidate, &root) {
            if !is_executable(&candidate) {
                return Err(FinallyAValueBotError::ToolExecution(format!(
                    "hook command '{}' is not executable",
                    candidate.display()
                )));
            }
            return Ok(candidate);
        }
    }
    Err(FinallyAValueBotError::ToolExecution(format!(
        "hook command '{}' was not found in allowed hook directories",
        command
    )))
}

pub fn validate_command_payload(config: &Config, payload_json: &str) -> Result<(), String> {
    let payload: HookCommandPayload =
        serde_json::from_str(payload_json).map_err(|e| format!("Invalid command payload: {e}"))?;
    resolve_hook_command_path(config, &payload.command).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn parse_hook_output(raw: &str) -> Result<HookOutput, FinallyAValueBotError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(HookOutput::default());
    }
    let json_str = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        &trimmed[start..=end]
    } else {
        trimmed
    };
    serde_json::from_str::<HookOutput>(json_str).map_err(|e| {
        FinallyAValueBotError::ToolExecution(format!(
            "hook output must be valid JSON: {} (raw: {})",
            e,
            json_str.chars().take(240).collect::<String>()
        ))
    })
}

pub async fn execute_command_hook(
    config: &Config,
    payload: &HookCommandPayload,
    hook_input_json: &str,
) -> Result<HookOutput, FinallyAValueBotError> {
    let hook_path = resolve_hook_command_path(config, &payload.command)?;
    let timeout_secs = payload
        .timeout_secs
        .unwrap_or(config.hook_command_timeout_secs);
    let fail_closed = payload.fail_closed.unwrap_or(false);
    let cwd = match payload
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some("workspace") => config.workspace_root_absolute(),
        Some("shared") => config.workspace_root_absolute().join("shared"),
        Some(other) => {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "unsupported hook cwd '{}': expected 'workspace' or 'shared'",
                other
            )));
        }
        None => config.workspace_root_absolute(),
    };

    let mut cmd = tokio::process::Command::new(&hook_path);
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        FinallyAValueBotError::ToolExecution(format!(
            "failed to spawn hook command '{}': {}",
            hook_path.display(),
            e
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(hook_input_json.as_bytes())
            .await
            .map_err(FinallyAValueBotError::Io)?;
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            if fail_closed {
                return Ok(HookOutput {
                    permission: Some("deny".to_string()),
                    reason: Some(format!("hook command failed: {e}")),
                    user_message: None,
                    agent_message: None,
                    additional_context: None,
                    updated_tool_input: None,
                    updated_assistant_text: None,
                    effects: None,
                });
            }
            return Ok(HookOutput::default());
        }
        Err(_) => {
            if fail_closed {
                return Ok(HookOutput {
                    permission: Some("deny".to_string()),
                    reason: Some(format!("hook command timed out after {}s", timeout_secs)),
                    user_message: None,
                    agent_message: None,
                    additional_context: None,
                    updated_tool_input: None,
                    updated_assistant_text: None,
                    effects: None,
                });
            }
            return Ok(HookOutput::default());
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if stdout.len() > HOOK_OUTPUT_MAX_BYTES {
        if fail_closed {
            return Ok(HookOutput {
                permission: Some("deny".to_string()),
                reason: Some("hook output exceeded maximum size".to_string()),
                user_message: None,
                agent_message: None,
                additional_context: None,
                updated_tool_input: None,
                updated_assistant_text: None,
                effects: None,
            });
        }
        return Ok(HookOutput::default());
    }

    match output.status.code() {
        Some(0) => parse_hook_output(&stdout),
        Some(2) => Ok(HookOutput {
            permission: Some("deny".to_string()),
            reason: nonempty_trimmed(Some(&stderr))
                .or_else(|| nonempty_trimmed(Some(&stdout)))
                .or_else(|| Some("hook command denied operation".to_string())),
            user_message: None,
            agent_message: None,
            additional_context: None,
            updated_tool_input: None,
            updated_assistant_text: None,
            effects: None,
        }),
        _ => {
            if fail_closed {
                Ok(HookOutput {
                    permission: Some("deny".to_string()),
                    reason: nonempty_trimmed(Some(&stderr))
                        .or_else(|| nonempty_trimmed(Some(&stdout)))
                        .or_else(|| Some("hook command failed".to_string())),
                    user_message: None,
                    agent_message: None,
                    additional_context: None,
                    updated_tool_input: None,
                    updated_assistant_text: None,
                    effects: None,
                })
            } else {
                Ok(HookOutput::default())
            }
        }
    }
}

pub async fn execute_prompt_hook(
    config: &Config,
    env_redactor: &EnvSecretRedactor,
    payload: &HookPromptPayload,
    hook_input_json: &str,
) -> Result<HookOutput, FinallyAValueBotError> {
    let mut llm_config = config.clone();
    if let Some(model) = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        llm_config.model = model.to_string();
    } else if !config.hook_prompt_model.trim().is_empty() {
        llm_config.model = config.hook_prompt_model.trim().to_string();
    } else if !config.orchestrator_model.trim().is_empty() {
        llm_config.model = config.orchestrator_model.trim().to_string();
    }

    let timeout_secs = payload
        .timeout_secs
        .unwrap_or(config.hook_prompt_timeout_secs);
    let fail_closed = payload.fail_closed.unwrap_or(false);
    let rendered_prompt = payload.prompt.replace("$ARGUMENTS", hook_input_json);
    let rendered_prompt = env_redactor.redact(&rendered_prompt);
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(rendered_prompt),
    }];
    let provider = llm::create_provider(&llm_config);
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let response = match tokio::time::timeout(
        timeout,
        provider.send_message(
            "You are a hook evaluator. Return JSON only. Valid keys: permission, reason, user_message, agent_message, additional_context, updated_tool_input, effects.",
            messages,
            None,
        ),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            if fail_closed {
                return Ok(HookOutput {
                    permission: Some("deny".to_string()),
                    reason: Some(format!("prompt hook failed: {e}")),
                    ..HookOutput::default()
                });
            }
            return Ok(HookOutput::default());
        }
        Err(_) => {
            if fail_closed {
                return Ok(HookOutput {
                    permission: Some("deny".to_string()),
                    reason: Some(format!("prompt hook timed out after {}s", timeout_secs)),
                    ..HookOutput::default()
                });
            }
            return Ok(HookOutput::default());
        }
    };

    let text: String = response
        .content
        .iter()
        .filter_map(|b| match b {
            ResponseContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    parse_hook_output(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    #[cfg(unix)]
    use std::fs;

    #[test]
    fn parse_hook_output_json_ok() {
        let raw = r#"{"permission":"allow","additional_context":"hi"}"#;
        let parsed = parse_hook_output(raw).expect("parse");
        assert_eq!(parsed.permission.as_deref(), Some("allow"));
        assert_eq!(parsed.additional_context.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_hook_output_allows_empty() {
        let parsed = parse_hook_output(" ").expect("parse");
        assert!(parsed.permission.is_none());
    }

    #[test]
    fn resolve_hook_command_path_rejects_parent_dir() {
        let config = test_config();
        let err = resolve_hook_command_path(&config, "../hack.sh").expect_err("must reject");
        assert!(err.to_string().contains("cannot contain '..'"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_command_hook_parses_json_output() {
        let tmp = std::env::temp_dir().join(format!("fab_hook_exec_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(tmp.join("hooks")).expect("hooks dir");
        fs::create_dir_all(tmp.join("shared")).expect("shared dir");
        let script = tmp.join("hooks").join("test-hook.py");
        fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, sys
_ = sys.stdin.read()
print(json.dumps({
  "permission":"allow",
  "additional_context":"ctx",
  "updated_tool_input":{"rewritten":True},
  "effects":{"memory_tier3_prune":{"terminal_pz_post_ids":["PZ-20260528-X"]}}
}))
"#,
        )
        .expect("write script");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");

        let mut config = test_config();
        config.workspace_dir = tmp.to_string_lossy().to_string();
        let payload = HookCommandPayload {
            command: "test-hook.py".to_string(),
            timeout_secs: Some(10),
            fail_closed: Some(true),
            cwd: Some("shared".to_string()),
        };
        let out = execute_command_hook(&config, &payload, r#"{"event":"PreToolUse"}"#)
            .await
            .expect("execute");
        assert_eq!(out.additional_context.as_deref(), Some("ctx"));
        assert!(out.updated_tool_input.is_some());
        let ids = out
            .effects
            .and_then(|e| e.memory_tier3_prune)
            .map(|m| m.terminal_pz_post_ids)
            .unwrap_or_default();
        assert_eq!(ids, vec!["PZ-20260528-X".to_string()]);
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_command_hook_defaults_cwd_to_workspace_root() {
        let tmp = std::env::temp_dir().join(format!("fab_hook_cwd_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(tmp.join("hooks")).expect("hooks dir");
        fs::create_dir_all(tmp.join("shared")).expect("shared dir");
        fs::create_dir_all(tmp.join("skills").join("demo")).expect("skills dir");
        fs::write(tmp.join("skills").join("demo").join("probe.txt"), "ok").expect("probe");

        let script = tmp.join("hooks").join("cwd-check.py");
        fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, os, pathlib, sys
_ = sys.stdin.read()
exists = pathlib.Path("skills/demo/probe.txt").exists()
print(json.dumps({"permission":"allow","additional_context":"ok" if exists else "missing"}))
"#,
        )
        .expect("write script");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");

        let mut config = test_config();
        config.workspace_dir = tmp.to_string_lossy().to_string();
        let payload = HookCommandPayload {
            command: "cwd-check.py".to_string(),
            timeout_secs: Some(10),
            fail_closed: Some(true),
            cwd: None,
        };
        let out = execute_command_hook(&config, &payload, "{}")
            .await
            .expect("execute");
        assert_eq!(out.additional_context.as_deref(), Some("ok"));
        let _ = fs::remove_dir_all(tmp);
    }
}
