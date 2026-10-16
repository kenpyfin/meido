use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::memory::MemoryManager;

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};

pub struct ReadMemoryTool {
    memory: MemoryManager,
    /// Principles file: workspace_dir/AGENTS.md (read-only for "global" scope).
    workspace_agents_path: PathBuf,
}

impl ReadMemoryTool {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        ReadMemoryTool {
            workspace_agents_path: PathBuf::from(working_dir).join("AGENTS.md"),
            memory: MemoryManager::new(data_dir, working_dir),
        }
    }
}

#[async_trait]
impl Tool for ReadMemoryTool {
    fn name(&self) -> &str {
        "read_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_memory",
            "Read memory. Use scope 'global' to read principles (AGENTS.md at workspace root, read-only), or 'chat' to read this persona's canonical memory_state.json. For tiered operations use read_tiered_memory/write_tiered_memory.",
            schema_object(
                json!({
                    "scope": {
                        "type": "string",
                        "description": "Memory scope: 'global' (principles) or 'chat' (persona memory_state.json)",
                        "enum": ["global", "chat"]
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (required for scope 'chat'; can default from context)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Persona ID (required for scope 'chat'; can default from context)"
                    }
                }),
                &["scope"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let scope = match input.get("scope").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'scope' parameter".into()),
        };

        let path = match scope {
            "global" => self.workspace_agents_path.clone(),
            "chat" => {
                let auth = match auth_context_from_input(&input) {
                    Some(a) => a,
                    None => return ToolResult::error("Missing auth context".into()),
                };
                let chat_id = input
                    .get("chat_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(auth.caller_chat_id);
                let persona_id = input
                    .get("persona_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(auth.caller_persona_id);
                if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
                    return ToolResult::error(e);
                }
                self.memory.persona_memory_state_path(chat_id, persona_id)
            }
            _ => return ToolResult::error("scope must be 'global' or 'chat'".into()),
        };

        info!("Reading memory: {}", path.display());

        if scope == "chat" {
            let auth = auth_context_from_input(&input).unwrap();
            let chat_id = input
                .get("chat_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(auth.caller_chat_id);
            let persona_id = input
                .get("persona_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(auth.caller_persona_id);
            if let Some(state) = self
                .memory
                .read_or_migrate_persona_memory_state(chat_id, persona_id)
            {
                return match serde_json::to_string_pretty(&state) {
                    Ok(serialized) => ToolResult::success(serialized),
                    Err(e) => ToolResult::error(format!("Failed to serialize memory state: {e}")),
                };
            }
            return ToolResult::success("No memory state found (not yet created).".into());
        }

        match std::fs::read_to_string(&path) {
            Ok(content) if content.trim().is_empty() => {
                ToolResult::success("Memory file is empty.".into())
            }
            Ok(content) => ToolResult::success(content),
            Err(_) => ToolResult::success("No memory file found (not yet created).".into()),
        }
    }
}

pub struct WriteMemoryTool {
    groups_dir: PathBuf,
}

impl WriteMemoryTool {
    pub fn new(data_dir: &str, _working_dir: &str) -> Self {
        WriteMemoryTool {
            groups_dir: PathBuf::from(data_dir).join("groups"),
        }
    }
}

#[async_trait]
impl Tool for WriteMemoryTool {
    fn name(&self) -> &str {
        "write_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_memory",
            "Write memory daily log only. Use scope 'chat_daily' to append a note to groups/{chat_id}/{persona_id}/memory/YYYY-MM-DD.md. For tiered state updates use write_tiered_memory.",
            schema_object(
                json!({
                    "scope": {
                        "type": "string",
                        "description": "Memory scope: 'chat_daily' only (appends daily log)",
                        "enum": ["chat_daily"]
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (optional; defaults from auth context)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Persona ID (optional; defaults from auth context)"
                    },
                    "date": {
                        "type": "string",
                        "description": "Date for scope 'chat_daily' only (YYYY-MM-DD, default: today UTC)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write (replaces for chat; appends for chat_daily)"
                    }
                }),
                &["scope", "content"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let scope = match input.get("scope").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'scope' parameter".into()),
        };
        if scope != "chat_daily" {
            return ToolResult::error(
                "scope must be 'chat_daily'. Use write_tiered_memory to update canonical memory tiers."
                    .into(),
            );
        }
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error("Missing 'content' parameter".into()),
        };

        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => return ToolResult::error("Missing auth context for chat_daily scope".into()),
        };
        let chat_id = input
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_chat_id);
        let persona_id = input
            .get("persona_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_persona_id);
        if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
            return ToolResult::error(e);
        }
        let date = input
            .get("date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
        let path = self
            .groups_dir
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join("memory")
            .join(format!("{date}.md"));
        info!("Appending to daily log: {}", path.display());
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("Failed to create directory: {e}"));
            }
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut f) => {
                if !content.ends_with('\n') {
                    let _ = f.write_all(b"\n");
                }
                match f.write_all(content.as_bytes()) {
                    Ok(()) => ToolResult::success(format!(
                        "Appended to daily log for {date} (chat_daily scope)."
                    )),
                    Err(e) => ToolResult::error(format!("Failed to append to daily log: {e}")),
                }
            }
            Err(e) => ToolResult::error(format!("Failed to open daily log: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "finally_a_value_bot_memtool_{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn test_tools(dir: &std::path::Path) -> (ReadMemoryTool, WriteMemoryTool) {
        let s = dir.to_str().unwrap();
        (ReadMemoryTool::new(s, s), WriteMemoryTool::new(s, s))
    }

    #[tokio::test]
    async fn test_read_memory_global_not_exists() {
        let dir = test_dir();
        let (tool, _) = test_tools(&dir);
        let result = tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No memory file found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_global_from_workspace_agents_md() {
        let dir = test_dir();
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("AGENTS.md"), "user prefers Rust").unwrap();
        let (tool, _) = test_tools(&dir);
        let result = tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "user prefers Rust");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_invalid_scope_not_allowed() {
        let dir = test_dir();
        let (_, tool) = test_tools(&dir);
        let result = tool
            .execute(json!({
                "scope": "chat",
                "content": "user prefers Rust",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": 1,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("chat_daily"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_chat_daily_appends_file() {
        let dir = test_dir();
        let (_, write_tool) = test_tools(&dir);
        let auth = json!({
            "__finally_a_value_bot_auth": {
                "caller_chat_id": 42,
                "caller_persona_id": 1,
                "control_chat_ids": []
            }
        });

        let mut write_input = json!({
            "scope": "chat_daily",
            "chat_id": 42,
            "persona_id": 1,
            "date": "2026-04-01",
            "content": "daily note"
        });
        if let (Some(obj), Some(auth_obj)) = (
            write_input.as_object_mut(),
            auth.get("__finally_a_value_bot_auth"),
        ) {
            obj.insert("__finally_a_value_bot_auth".to_string(), auth_obj.clone());
        }
        let result = write_tool.execute(write_input).await;
        assert!(!result.is_error);
        let path = dir
            .join("groups")
            .join("42")
            .join("1")
            .join("memory")
            .join("2026-04-01.md");
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("daily note"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_chat_missing_auth() {
        let dir = test_dir();
        let (tool, _) = test_tools(&dir);
        let result = tool.execute(json!({"scope": "chat", "chat_id": 42})).await;
        assert!(result.is_error);
        assert!(result.content.contains("auth"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_missing_scope() {
        let dir = test_dir();
        let (_, tool) = test_tools(&dir);
        let result = tool.execute(json!({"content": "data"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'scope'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_invalid_scope() {
        let dir = test_dir();
        let (tool, _) = test_tools(&dir);
        let result = tool.execute(json!({"scope": "invalid"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("must be 'global' or 'chat'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_empty_file() {
        let dir = test_dir();
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("AGENTS.md"), "   ").unwrap();
        let (read_tool, _) = test_tools(&dir);
        let result = read_tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("empty"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_chat_permission_denied() {
        let dir = test_dir();
        let (tool, _) = test_tools(&dir);
        let result = tool
            .execute(json!({
                "scope": "chat",
                "chat_id": 200,
                "persona_id": 2,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": 1,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_chat_daily_allowed_for_control_chat_cross_chat() {
        let dir = test_dir();
        let (_, write_tool) = test_tools(&dir);
        let result = write_tool
            .execute(json!({
                "scope": "chat_daily",
                "chat_id": 200,
                "persona_id": 2,
                "content": "chat200",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": 1,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
