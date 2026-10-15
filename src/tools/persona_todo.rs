use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};
use crate::claude::ToolDefinition;
use crate::db::Database;

fn resolve_chat_persona(input: &serde_json::Value) -> Result<(i64, i64), String> {
    let auth = auth_context_from_input(input).ok_or_else(|| "Missing auth context".to_string())?;
    let chat_id = input
        .get("chat_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(auth.caller_chat_id);
    let persona_id = input
        .get("persona_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(auth.caller_persona_id);
    authorize_chat_persona_access(input, chat_id, persona_id)?;
    Ok((chat_id, persona_id))
}

pub struct AddTodoTool {
    db: Arc<Database>,
}

impl AddTodoTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for AddTodoTool {
    fn name(&self) -> &str {
        "add_todo"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "add_todo",
            "Create an operator action item for this persona. Items appear in the web Inbox for the human operator — use for concrete follow-ups the operator should track (not Tier 3 memory, not schedules).",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Target chat ID (default: caller chat)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Target persona ID (default: caller persona)"
                    },
                    "title": {
                        "type": "string",
                        "description": "Short actionable todo title"
                    },
                    "source_hint": {
                        "type": "string",
                        "description": "Optional short context (why / from which conversation)"
                    }
                }),
                &["title"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match resolve_chat_persona(&input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(e),
        };
        let title = match input.get("title").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return ToolResult::error("Missing or empty 'title'".into()),
        };
        let source_hint = input
            .get("source_hint")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || {
            db.add_persona_todo(chat_id, persona_id, &title, source_hint.as_deref())
        })
        .await;
        match result {
            Ok(Ok(todo)) => ToolResult::success(format!(
                "Created todo #{} for chat {} persona {}: {}",
                todo.id, todo.chat_id, todo.persona_id, todo.title
            )),
            Ok(Err(e)) => ToolResult::error(format!("Failed to create todo: {e}")),
            Err(e) => ToolResult::error(format!("Task error while creating todo: {e}")),
        }
    }
}

pub struct ListTodosTool {
    db: Arc<Database>,
}

impl ListTodosTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ListTodosTool {
    fn name(&self) -> &str {
        "list_todos"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_todos",
            "List operator todos for this persona (default: open only). Use before completing or when the operator asks what is outstanding.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Target chat ID (default: caller chat)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Target persona ID (default: caller persona)"
                    },
                    "status": {
                        "type": "string",
                        "description": "Filter: open (default), done, or all"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max items (default 50)"
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match resolve_chat_persona(&input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(e),
        };
        let status_raw = input
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("open")
            .trim()
            .to_ascii_lowercase();
        let status_filter: Option<String> = match status_raw.as_str() {
            "all" => None,
            "open" | "done" => Some(status_raw.clone()),
            other => {
                return ToolResult::error(format!(
                    "Invalid status '{other}'; use open, done, or all"
                ))
            }
        };
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .clamp(1, 200) as usize;

        let db = self.db.clone();
        let filter = status_filter.clone();
        let result = tokio::task::spawn_blocking(move || {
            db.list_persona_todos(chat_id, persona_id, filter.as_deref(), limit)
        })
        .await;
        match result {
            Ok(Ok(items)) => {
                if items.is_empty() {
                    return ToolResult::success(format!(
                        "No todos for chat {} persona {} (filter={}).",
                        chat_id, persona_id, status_raw
                    ));
                }
                let mut lines = vec![format!(
                    "Todos for chat {} persona {} ({}):",
                    chat_id,
                    persona_id,
                    items.len()
                )];
                for t in items {
                    let hint = t
                        .source_hint
                        .as_deref()
                        .map(|h| format!(" — {h}"))
                        .unwrap_or_default();
                    lines.push(format!("- #{} [{}] {}{}", t.id, t.status, t.title, hint));
                }
                ToolResult::success(lines.join("\n"))
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to list todos: {e}")),
            Err(e) => ToolResult::error(format!("Task error while listing todos: {e}")),
        }
    }
}

pub struct CompleteTodoTool {
    db: Arc<Database>,
}

impl CompleteTodoTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for CompleteTodoTool {
    fn name(&self) -> &str {
        "complete_todo"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "complete_todo",
            "Mark an operator todo as done by id. Prefer list_todos first if the id is unknown.",
            schema_object(
                json!({
                    "todo_id": {
                        "type": "integer",
                        "description": "Todo id from add_todo / list_todos"
                    }
                }),
                &["todo_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => return ToolResult::error("Missing auth context".into()),
        };
        let todo_id = match input.get("todo_id").and_then(|v| v.as_i64()) {
            Some(id) if id > 0 => id,
            _ => return ToolResult::error("Missing or invalid 'todo_id'".into()),
        };

        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || {
            let Some(existing) = db.get_persona_todo(todo_id)? else {
                return Ok::<_, crate::error::FinallyAValueBotError>(None);
            };
            if !auth.can_access_chat_persona(existing.chat_id, existing.persona_id) {
                return Err(crate::error::FinallyAValueBotError::ToolExecution(
                    "Not authorized for that todo's chat/persona".into(),
                ));
            }
            db.set_persona_todo_status(todo_id, "done")
        })
        .await;
        match result {
            Ok(Ok(Some(todo))) => {
                ToolResult::success(format!("Completed todo #{}: {}", todo.id, todo.title))
            }
            Ok(Ok(None)) => ToolResult::error(format!("Todo #{todo_id} not found")),
            Ok(Err(e)) => ToolResult::error(format!("Failed to complete todo: {e}")),
            Err(e) => ToolResult::error(format!("Task error while completing todo: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_input(chat_id: i64, persona_id: i64) -> serde_json::Value {
        json!({
            "__finally_a_value_bot_auth": {
                "caller_channel": "web",
                "caller_chat_id": chat_id,
                "caller_persona_id": persona_id,
                "control_chat_ids": [],
                "is_scheduled_task": false
            }
        })
    }

    #[tokio::test]
    async fn test_add_list_complete_todo() {
        let dir =
            std::env::temp_dir().join(format!("finally_a_value_bot_todo_{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        db.upsert_chat(7, None, "private").unwrap();
        let pid = db.get_or_create_default_persona(7).unwrap();

        let add = AddTodoTool::new(db.clone());
        let mut input = auth_input(7, pid);
        input["title"] = json!("Review draft");
        input["source_hint"] = json!("from chat");
        let result = add.execute(input).await;
        assert!(!result.is_error, "{}", result.content);

        let list = ListTodosTool::new(db.clone());
        let result = list.execute(auth_input(7, pid)).await;
        assert!(!result.is_error);
        assert!(result.content.contains("Review draft"));

        let todos = db.list_persona_todos(7, pid, Some("open"), 10).unwrap();
        assert_eq!(todos.len(), 1);
        let todo_id = todos[0].id;

        let complete = CompleteTodoTool::new(db.clone());
        let mut input = auth_input(7, pid);
        input["todo_id"] = json!(todo_id);
        let result = complete.execute(input).await;
        assert!(!result.is_error, "{}", result.content);
        let open = db.list_persona_todos(7, pid, Some("open"), 10).unwrap();
        assert!(open.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
