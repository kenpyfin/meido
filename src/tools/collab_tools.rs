//! Board updates and the Coordinator's read-only drill-down tools.
//! Cross-persona reads stay inside the caller's chat and return at most 20 rows.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};
use crate::claude::ToolDefinition;
use crate::collab::is_coordinator_name;
use crate::db::{call_blocking, Database};
use crate::db_collab::BoardPatch;

fn caller(input: &serde_json::Value) -> Result<(i64, i64), String> {
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

pub struct UpdateBoardItemTool {
    db: Arc<Database>,
}

impl UpdateBoardItemTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for UpdateBoardItemTool {
    fn name(&self) -> &str {
        "update_board_item"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "update_board_item",
            "Update a board item you own. Status is todo, doing, blocked, or done. You cannot assign it to another persona.",
            schema_object(
                json!({
                    "item_id": {"type": "integer"},
                    "status": {"type": "string"},
                    "title": {"type": "string"},
                    "blocked_reason": {"type": "string"},
                    "due_at": {"type": "string"}
                }),
                &["item_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match caller(&input) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };
        let Some(item_id) = input.get("item_id").and_then(|v| v.as_i64()) else {
            return ToolResult::error("item_id is required".into());
        };
        let db = self.db.clone();
        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let blocked_reason = input
            .get("blocked_reason")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let due_at = input
            .get("due_at")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let result = call_blocking(db, move |db| {
            let Some(existing) = db.get_persona_todo(item_id)? else {
                return Ok(None);
            };
            if existing.chat_id != chat_id {
                return Ok(None);
            }
            let owner = existing.owner_persona_id.unwrap_or(existing.persona_id);
            if owner != persona_id && existing.persona_id != persona_id {
                return Err(crate::error::FinallyAValueBotError::ToolExecution(
                    "you can only update your own board items".into(),
                ));
            }
            let patch = BoardPatch {
                status,
                title,
                description: None,
                owner_persona_id: None,
                priority: None,
                due_at,
                blocked_reason,
                position: None,
                before_position: None,
                after_position: None,
                actor_kind: "persona",
                actor_persona_id: Some(persona_id),
            };
            db.apply_board_patch(chat_id, item_id, &patch)
        })
        .await;
        match result {
            Ok(Some(item)) => ToolResult::success(format!("#{} is now {}", item.id, item.status)),
            Ok(None) => ToolResult::error("board item not found".into()),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

pub struct ListBoardTool {
    db: Arc<Database>,
}

impl ListBoardTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ListBoardTool {
    fn name(&self) -> &str {
        "list_board"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_board",
            "List board items in this chat. Coordinator only. At most 20 rows.",
            schema_object(
                json!({
                    "status": {"type": "string", "description": "todo, doing, blocked, done, open, or all"}
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match caller(&input) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };
        if !require_coordinator(&self.db, chat_id, persona_id).await {
            return ToolResult::error("list_board is only available to the Coordinator".into());
        }
        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        let filter = if status == "all" {
            None
        } else {
            Some(status.to_string())
        };
        let db = self.db.clone();
        match call_blocking(db, move |db| {
            db.list_todos_for_chat(chat_id, filter.as_deref(), 20)
        })
        .await
        {
            Ok(items) => {
                let mut lines = Vec::new();
                for item in items {
                    lines.push(format!(
                        "#{} [{}] owner {} {}",
                        item.id,
                        item.status,
                        item.owner_persona_id.unwrap_or(item.persona_id),
                        item.title
                    ));
                }
                ToolResult::success(if lines.is_empty() {
                    "No board items.".into()
                } else {
                    lines.join("\n")
                })
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

pub struct ListPersonaActivityTool {
    db: Arc<Database>,
}

impl ListPersonaActivityTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ListPersonaActivityTool {
    fn name(&self) -> &str {
        "list_persona_activity"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_persona_activity",
            "Latest topic title and summary for each persona. Coordinator only. Titles only, at most 12.",
            schema_object(json!({}), &[]),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match caller(&input) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };
        if !require_coordinator(&self.db, chat_id, persona_id).await {
            return ToolResult::error(
                "list_persona_activity is only available to the Coordinator".into(),
            );
        }
        let db = self.db.clone();
        match call_blocking(db, move |db| db.latest_topic_lines(chat_id, 12)).await {
            Ok(rows) => {
                if rows.is_empty() {
                    return ToolResult::success("No topics yet.".into());
                }
                let lines: Vec<String> = rows
                    .into_iter()
                    .map(|(name, title, summary)| format!("{name}: {title} — {summary}"))
                    .collect();
                ToolResult::success(lines.join("\n"))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

pub struct ListScheduledRunsTool {
    db: Arc<Database>,
}

impl ListScheduledRunsTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ListScheduledRunsTool {
    fn name(&self) -> &str {
        "list_scheduled_runs"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_scheduled_runs",
            "Recent scheduled-task runs for this chat. Previews only, at most 20. Pass run_id to read one output, capped.",
            schema_object(
                json!({
                    "run_id": {"type": "integer"},
                    "task_id": {"type": "integer"}
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, _) = match caller(&input) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };
        let run_id = input.get("run_id").and_then(|v| v.as_i64());
        let task_id = input.get("task_id").and_then(|v| v.as_i64());
        let db = self.db.clone();
        if let Some(run_id) = run_id {
            return match call_blocking(db, move |db| db.get_scheduled_run(chat_id, run_id)).await {
                Ok(Some(row)) => {
                    let body = crate::db_collab::clip_chars(
                        &crate::db_collab::load_run_body(&row),
                        crate::db_collab::ATTACH_EXPAND_CHARS,
                    );
                    ToolResult::success(format!("run #{} {}\n{body}", row.id, row.status))
                }
                Ok(None) => ToolResult::error("run not found".into()),
                Err(e) => ToolResult::error(e.to_string()),
            };
        }
        match call_blocking(db, move |db| {
            db.list_scheduled_runs(chat_id, task_id, None, None, 20)
        })
        .await
        {
            Ok(rows) => {
                let lines: Vec<String> = rows
                    .into_iter()
                    .map(|row| {
                        format!(
                            "#{} task {} {} {}",
                            row.id, row.task_id, row.status, row.preview
                        )
                    })
                    .collect();
                ToolResult::success(if lines.is_empty() {
                    "No scheduled runs.".into()
                } else {
                    lines.join("\n")
                })
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

pub struct ProposeBoardChangeTool {
    db: Arc<Database>,
}

impl ProposeBoardChangeTool {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Tool for ProposeBoardChangeTool {
    fn name(&self) -> &str {
        "propose_board_change"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "propose_board_change",
            "Propose a board change for the operator to confirm. Coordinator only. Does not apply the change.",
            schema_object(
                json!({
                    "item_id": {"type": "integer"},
                    "status": {"type": "string"},
                    "title": {"type": "string"},
                    "owner_persona_id": {"type": "integer"},
                    "due_at": {"type": "string"},
                    "blocked_reason": {"type": "string"}
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let (chat_id, persona_id) = match caller(&input) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };
        if !require_coordinator(&self.db, chat_id, persona_id).await {
            return ToolResult::error(
                "propose_board_change is only available to the Coordinator".into(),
            );
        }
        let item_id = input.get("item_id").and_then(|v| v.as_i64());
        let change = json!({
            "item_id": item_id,
            "status": input.get("status").and_then(|v| v.as_str()),
            "title": input.get("title").and_then(|v| v.as_str()),
            "owner_persona_id": input.get("owner_persona_id").and_then(|v| v.as_i64()),
            "due_at": input.get("due_at").and_then(|v| v.as_str()),
            "blocked_reason": input.get("blocked_reason").and_then(|v| v.as_str()),
        });
        let payload = change.to_string();
        let db = self.db.clone();
        match call_blocking(db, move |db| db.insert_proposal(chat_id, item_id, &payload)).await {
            Ok(id) => ToolResult::success(format!("proposal #{id} is waiting for confirmation")),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

async fn require_coordinator(db: &Arc<Database>, chat_id: i64, persona_id: i64) -> bool {
    let db = db.clone();
    call_blocking(db, move |db| {
        Ok(db
            .get_persona(persona_id)?
            .filter(|p| p.chat_id == chat_id)
            .map(|p| is_coordinator_name(&p.name))
            .unwrap_or(false))
    })
    .await
    .unwrap_or(false)
}
