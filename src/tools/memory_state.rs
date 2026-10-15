use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use crate::claude::ToolDefinition;
use crate::memory::{MemoryManager, PersonaMemoryState};

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};

pub struct ReadMemoryStateTool {
    memory: MemoryManager,
}

impl ReadMemoryStateTool {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        Self {
            memory: MemoryManager::new(data_dir, working_dir),
        }
    }
}

#[async_trait]
impl Tool for ReadMemoryStateTool {
    fn name(&self) -> &str {
        "read_memory_state"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_memory_state",
            "Read canonical per-persona memory_state.json (single source of truth).",
            schema_object(
                json!({
                    "chat_id": {"type": "integer", "description": "Chat ID (defaults from auth context)"},
                    "persona_id": {"type": "integer", "description": "Persona ID (defaults from auth context)"}
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: Value) -> ToolResult {
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
        let state = match self
            .memory
            .read_or_migrate_persona_memory_state(chat_id, persona_id)
        {
            Some(s) => s,
            None => return ToolResult::success("No memory state found (not yet created).".into()),
        };
        match serde_json::to_string_pretty(&state) {
            Ok(s) => ToolResult::success(s),
            Err(e) => ToolResult::error(format!("Failed to serialize state: {e}")),
        }
    }
}

pub struct ValidateMemoryStateTool {
    memory: MemoryManager,
}

impl ValidateMemoryStateTool {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        Self {
            memory: MemoryManager::new(data_dir, working_dir),
        }
    }
}

#[async_trait]
impl Tool for ValidateMemoryStateTool {
    fn name(&self) -> &str {
        "validate_memory_state"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "validate_memory_state",
            "Validate candidate memory_state JSON against schema and invariants.",
            schema_object(
                json!({
                    "chat_id": {"type": "integer", "description": "Chat ID (defaults from auth context)"},
                    "persona_id": {"type": "integer", "description": "Persona ID (defaults from auth context)"},
                    "content": {"type": "string", "description": "Optional JSON content to validate. If omitted, validates current state from disk."}
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: Value) -> ToolResult {
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

        let mut state = if let Some(content) = input.get("content").and_then(|v| v.as_str()) {
            match serde_json::from_str::<PersonaMemoryState>(content) {
                Ok(s) => s,
                Err(e) => return ToolResult::error(format!("Invalid JSON for memory state: {e}")),
            }
        } else {
            match self
                .memory
                .read_or_migrate_persona_memory_state(chat_id, persona_id)
            {
                Some(s) => s,
                None => {
                    return ToolResult::success(
                        "No memory state found; nothing to validate yet.".into(),
                    )
                }
            }
        };
        state.normalize();
        match self.memory.validate_memory_state(&state) {
            Ok(()) => ToolResult::success("Memory state is valid.".into()),
            Err(e) => ToolResult::error(format!("Memory state validation failed: {e}")),
        }
    }
}

pub struct WriteMemoryStateTool {
    memory: MemoryManager,
}

impl WriteMemoryStateTool {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        Self {
            memory: MemoryManager::new(data_dir, working_dir),
        }
    }
}

#[async_trait]
impl Tool for WriteMemoryStateTool {
    fn name(&self) -> &str {
        "write_memory_state"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_memory_state",
            "Write canonical memory_state.json with validation and optional revision conflict guard.",
            schema_object(
                json!({
                    "chat_id": {"type": "integer", "description": "Chat ID (defaults from auth context)"},
                    "persona_id": {"type": "integer", "description": "Persona ID (defaults from auth context)"},
                    "content": {"type": "string", "description": "Required JSON memory_state payload"},
                    "expected_revision": {"type": "integer", "description": "Optional optimistic concurrency guard for meta.revision"}
                }),
                &["content"],
            ),
        )
    }

    async fn execute(&self, input: Value) -> ToolResult {
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
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error("Missing 'content' JSON payload".into()),
        };
        let expected_revision = input.get("expected_revision").and_then(|v| v.as_u64());
        let mut state: PersonaMemoryState = match serde_json::from_str(content) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Invalid JSON for memory state: {e}")),
        };
        state.normalize();
        if let Err(e) = self.memory.validate_memory_state(&state) {
            return ToolResult::error(format!("Memory state validation failed: {e}"));
        }
        if let Some(expected) = expected_revision {
            let current_revision = self
                .memory
                .read_persona_memory_state(chat_id, persona_id)
                .map(|s| s.meta.revision)
                .unwrap_or(0);
            if current_revision != expected {
                return ToolResult::error(format!(
                    "Revision conflict: expected {}, current {}",
                    expected, current_revision
                ));
            }
        }
        match self
            .memory
            .write_persona_memory_state(chat_id, persona_id, state)
        {
            Ok(()) => {
                let _ = self.memory.append_persona_memory_event(
                    chat_id,
                    persona_id,
                    "manual_memory_state_write",
                    "user_manual",
                    json!({"source":"write_memory_state"}),
                );
                ToolResult::success("memory_state.json updated.".into())
            }
            Err(e) => ToolResult::error(format!("Failed to write memory state: {e}")),
        }
    }
}

pub struct PatchMemoryStateTool {
    memory: MemoryManager,
}

impl PatchMemoryStateTool {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        Self {
            memory: MemoryManager::new(data_dir, working_dir),
        }
    }
}

#[async_trait]
impl Tool for PatchMemoryStateTool {
    fn name(&self) -> &str {
        "patch_memory_state"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "patch_memory_state",
            "Apply a JSON object patch to canonical memory state (deep-merge for objects, replace scalars/arrays).",
            schema_object(
                json!({
                    "chat_id": {"type": "integer", "description": "Chat ID (defaults from auth context)"},
                    "persona_id": {"type": "integer", "description": "Persona ID (defaults from auth context)"},
                    "patch": {"type": "object", "description": "Required JSON object patch"},
                    "expected_revision": {"type": "integer", "description": "Optional optimistic concurrency guard for meta.revision"}
                }),
                &["patch"],
            ),
        )
    }

    async fn execute(&self, input: Value) -> ToolResult {
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
        let patch = match input.get("patch") {
            Some(Value::Object(map)) => Value::Object(map.clone()),
            _ => return ToolResult::error("patch must be a JSON object".into()),
        };
        let expected_revision = input.get("expected_revision").and_then(|v| v.as_u64());

        let mut state = self
            .memory
            .read_or_migrate_persona_memory_state(chat_id, persona_id)
            .unwrap_or_default();
        if let Some(expected) = expected_revision {
            if state.meta.revision != expected {
                return ToolResult::error(format!(
                    "Revision conflict: expected {}, current {}",
                    expected, state.meta.revision
                ));
            }
        }
        let mut state_value = match serde_json::to_value(&state) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("Failed to serialize state: {e}")),
        };
        merge_json_objects(&mut state_value, &patch);
        state = match serde_json::from_value::<PersonaMemoryState>(state_value) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Patched state shape is invalid: {e}")),
        };
        state.normalize();
        if let Err(e) = self.memory.validate_memory_state(&state) {
            return ToolResult::error(format!("Patched state validation failed: {e}"));
        }
        info!(
            "Patching memory_state for chat={} persona={} (expected_revision={:?})",
            chat_id, persona_id, expected_revision
        );
        match self
            .memory
            .write_persona_memory_state(chat_id, persona_id, state)
        {
            Ok(()) => {
                let _ = self.memory.append_persona_memory_event(
                    chat_id,
                    persona_id,
                    "manual_memory_state_patch",
                    "user_manual",
                    json!({"expected_revision": expected_revision}),
                );
                ToolResult::success("memory_state.json patched.".into())
            }
            Err(e) => ToolResult::error(format!("Failed to patch memory state: {e}")),
        }
    }
}

fn merge_json_objects(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                match target_map.get_mut(key) {
                    Some(target_value) => merge_json_objects(target_value, patch_value),
                    None => {
                        target_map.insert(key.clone(), patch_value.clone());
                    }
                }
            }
        }
        (target_value, patch_value) => {
            *target_value = patch_value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_json_objects_nested() {
        let mut target = json!({
            "identity": {"display_name": "Old", "voice_style": "plain"},
            "tier3": {"recent_focus": ["a"]}
        });
        let patch = json!({
            "identity": {"display_name": "New"},
            "tier3": {"recent_focus": ["b", "c"]}
        });
        merge_json_objects(&mut target, &patch);
        assert_eq!(target["identity"]["display_name"], "New");
        assert_eq!(target["identity"]["voice_style"], "plain");
        assert_eq!(target["tier3"]["recent_focus"], json!(["b", "c"]));
    }
}
