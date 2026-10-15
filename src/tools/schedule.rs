use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{
    auth_context_from_input, authorize_chat_access, default_persona_id_for_chat, schema_object,
    Tool, ToolResult,
};
use crate::channel::enforce_channel_policy;
use crate::claude::ToolDefinition;
use crate::db::{call_blocking, Database, ScheduledTask};

const UTC_TIMEZONE: &str = "UTC";

/// Control chats list every persona's schedules in a chat; normal runs see caller persona only.
fn list_schedules_for_caller(
    db: &Database,
    chat_id: i64,
    input: &serde_json::Value,
) -> Result<Vec<ScheduledTask>, String> {
    if auth_context_from_input(input)
        .map(|a| a.is_control_chat())
        .unwrap_or(false)
    {
        return db
            .get_scheduled_tasks_for_chat_for_display(chat_id)
            .map_err(|e| format!("Failed to list tasks: {e}"));
    }
    let persona_id = resolve_list_persona_id(db, chat_id, input)?;
    db.get_scheduled_tasks_for_chat_and_persona(chat_id, persona_id)
        .map_err(|e| format!("Failed to list tasks: {e}"))
}

fn resolve_list_persona_id(
    db: &Database,
    chat_id: i64,
    input: &serde_json::Value,
) -> Result<i64, String> {
    if let Some(pid) = default_persona_id_for_chat(input, chat_id) {
        return Ok(pid);
    }
    if let Some(auth) = auth_context_from_input(input) {
        if auth.caller_chat_id == chat_id && auth.caller_persona_id > 0 {
            return Ok(auth.caller_persona_id);
        }
    }
    db.get_current_persona_id(chat_id)
        .map_err(|e| format!("Failed to resolve persona: {e}"))
}

fn authorize_scheduled_task_access(
    input: &serde_json::Value,
    task: &ScheduledTask,
) -> Result<(), String> {
    authorize_chat_access(input, task.chat_id)?;
    if let Some(auth) = auth_context_from_input(input) {
        if !auth.can_access_chat_persona(task.chat_id, task.persona_id) {
            return Err(format!(
                "Permission denied: cannot access scheduled task for chat {} persona {}",
                task.chat_id, task.persona_id
            ));
        }
    }
    Ok(())
}

/// Format scheduled tasks for slash-command or UI display.
pub fn format_tasks_list(tasks: &[ScheduledTask]) -> String {
    format_tasks_list_impl(tasks, false)
}

/// Format all scheduled tasks (slash /schedule), including chat_id for each.
pub fn format_tasks_list_all(tasks: &[ScheduledTask]) -> String {
    format_tasks_list_impl(tasks, true)
}

/// Format scheduled tasks for Telegram /schedule in a single chat context:
/// include persona id, omit chat id.
pub fn format_tasks_list_persona(tasks: &[ScheduledTask]) -> String {
    if tasks.is_empty() {
        return "No scheduled tasks. Ask me to schedule one (recurring or one-time).".to_string();
    }
    let mut output = String::from("Scheduled tasks:\n");
    for t in tasks {
        output.push_str(&format!(
            "#{} [{}] persona:{} | {} | {} '{}' | next: {}\n",
            t.id, t.status, t.persona_id, t.prompt, t.schedule_type, t.schedule_value, t.next_run
        ));
    }
    output.push_str("\nUse `schedule_task` to add; `update_scheduled_task` to change prompt/timing/status; `pause_scheduled_task` / `resume_scheduled_task` / `cancel_scheduled_task` to manage.");
    output
}

fn format_tasks_list_impl(tasks: &[ScheduledTask], include_chat_id: bool) -> String {
    if tasks.is_empty() {
        return "No scheduled tasks. Ask me to schedule one (recurring or one-time).".to_string();
    }
    let mut output = if include_chat_id {
        String::from("Scheduled tasks (all chats/personas):\n")
    } else {
        String::from("Scheduled tasks:\n")
    };
    for t in tasks {
        if include_chat_id {
            output.push_str(&format!(
                "#{} [{}] chat:{} | {} | {} '{}' | next: {}\n",
                t.id, t.status, t.chat_id, t.prompt, t.schedule_type, t.schedule_value, t.next_run
            ));
        } else {
            output.push_str(&format!(
                "#{} [{}] {} | {} '{}' | next: {}\n",
                t.id, t.status, t.prompt, t.schedule_type, t.schedule_value, t.next_run
            ));
        }
    }
    output.push_str("\nUse `schedule_task` to add; `update_scheduled_task` to change prompt/timing/status; `pause_scheduled_task` / `resume_scheduled_task` / `cancel_scheduled_task` to manage.");
    output
}

/// Compute next run time for a cron expression (used by schedule_task and web API).
pub(crate) fn compute_next_run(cron_expr: &str, tz_name: &str) -> Result<String, String> {
    let tz: chrono_tz::Tz = tz_name
        .parse()
        .map_err(|_| format!("Invalid timezone: {tz_name}"))?;
    let schedule =
        cron::Schedule::from_str(cron_expr).map_err(|e| format!("Invalid cron expression: {e}"))?;
    let next = schedule
        .upcoming(tz)
        .next()
        .ok_or_else(|| "No upcoming run found for this cron expression".to_string())?;
    Ok(next.with_timezone(&chrono::Utc).to_rfc3339())
}

#[derive(Debug, Clone)]
pub struct SchedulePreflight {
    pub schedule_value: String,
    pub next_run: String,
    pub timezone_used: String,
    pub timezone_defaulted_to_utc: bool,
}

fn normalize_cron_expression(schedule_value: &str) -> Result<String, String> {
    let fields: Vec<&str> = schedule_value.split_whitespace().collect();
    match fields.len() {
        6 => Ok(fields.join(" ")),
        5 => Ok(format!("0 {}", fields.join(" "))),
        _ => Err(
            "Invalid cron expression: expected 5 or 6 fields (sec min hour dom month dow)"
                .to_string(),
        ),
    }
}

pub fn preflight_schedule_request(
    schedule_type: &str,
    schedule_value: &str,
    timezone: Option<&str>,
) -> Result<SchedulePreflight, String> {
    let timezone_input = timezone.map(str::trim).filter(|tz| !tz.is_empty());
    let timezone = timezone_input.unwrap_or(UTC_TIMEZONE);
    let timezone_defaulted_to_utc = timezone_input.is_none();

    match schedule_type {
        "cron" => {
            let normalized = normalize_cron_expression(schedule_value)?;
            let next_run = compute_next_run(&normalized, timezone)?;
            Ok(SchedulePreflight {
                schedule_value: normalized,
                next_run,
                timezone_used: timezone.to_string(),
                timezone_defaulted_to_utc,
            })
        }
        "once" => {
            let dt = match chrono::DateTime::parse_from_rfc3339(schedule_value) {
                Ok(dt) => dt,
                Err(_) => return Err("Invalid ISO 8601 timestamp for one-time schedule".into()),
            };
            let next_run = dt.with_timezone(&chrono::Utc).to_rfc3339();
            Ok(SchedulePreflight {
                schedule_value: schedule_value.to_string(),
                next_run,
                timezone_used: timezone.to_string(),
                timezone_defaulted_to_utc,
            })
        }
        _ => Err("schedule_type must be 'cron' or 'once'".into()),
    }
}

// --- schedule_task ---

pub struct ScheduleTaskTool {
    db: Arc<Database>,
    default_timezone: String,
}

impl ScheduleTaskTool {
    pub fn new(db: Arc<Database>, default_timezone: String) -> Self {
        ScheduleTaskTool {
            db,
            default_timezone,
        }
    }
}

#[async_trait]
impl Tool for ScheduleTaskTool {
    fn name(&self) -> &str {
        "schedule_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "schedule_task",
            "Create a new scheduled task (cron or one-time). Cron schedules live in `scheduled_tasks` only — not authored YAML workflows. Always activate the `schedule-job` skill before using this tool. For recurring tasks, provide a 5- or 6-field cron expression (5-field is normalized to 6-field: sec min hour dom month dow). For one-time tasks, provide an ISO 8601 timestamp. If timezone is omitted, the bot's configured default timezone is used. To change an existing task, use `update_scheduled_task`.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "The chat ID where results will be sent"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt/instruction to execute at the scheduled time"
                    },
                    "schedule_type": {
                        "type": "string",
                        "enum": ["cron", "once"],
                        "description": "Type of schedule: 'cron' for recurring (6-field: sec min hour dom month dow), 'once' for one-time"
                    },
                    "schedule_value": {
                        "type": "string",
                        "description": "The cron expression (6-field format, e.g. '0 */5 * * * *' for every 5 minutes) or ISO 8601 timestamp for one-time tasks"
                    },
                    "timezone": {
                        "type": "string",
                        "description": "Optional IANA timezone name (e.g. 'US/Eastern', 'Europe/London'). If omitted, the bot's configured default timezone is used."
                    }
                }),
                &["chat_id", "prompt", "schedule_type", "schedule_value"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: chat_id".into()),
        };
        if let Err(e) = authorize_chat_access(&input, chat_id) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, chat_id).await {
            return ToolResult::error(e);
        }
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required parameter: prompt".into()),
        };
        let schedule_type = match input.get("schedule_type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return ToolResult::error("Missing required parameter: schedule_type".into()),
        };
        let schedule_value = match input.get("schedule_value").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: schedule_value".into()),
        };
        let timezone_input = input.get("timezone").and_then(|v| v.as_str());
        let configured_default_tz = self.default_timezone.trim();
        let effective_tz = timezone_input.or_else(|| {
            if configured_default_tz.is_empty() {
                None
            } else {
                Some(configured_default_tz)
            }
        });
        let preflight =
            match preflight_schedule_request(schedule_type, schedule_value, effective_tz) {
                Ok(p) => p,
                Err(e) => return ToolResult::error(e),
            };

        let prompt_owned = prompt.to_string();
        let schedule_type_owned = schedule_type.to_string();
        let schedule_value_owned = preflight.schedule_value.clone();
        let next_run_owned = preflight.next_run.clone();
        let persona_id = auth_context_from_input(&input)
            .map(|auth| {
                if auth.caller_chat_id == chat_id {
                    auth.caller_persona_id
                } else {
                    0
                }
            })
            .filter(|id| *id > 0)
            .unwrap_or_else(|| 0);
        match call_blocking(self.db.clone(), move |db| {
            let resolved_persona_id = if persona_id > 0 {
                persona_id
            } else {
                db.get_current_persona_id(chat_id)?
            };
            db.create_scheduled_task_for_persona(
                chat_id,
                resolved_persona_id,
                &prompt_owned,
                &schedule_type_owned,
                &schedule_value_owned,
                &next_run_owned,
            )
        })
        .await
        {
            Ok(id) => {
                let mut message = format!(
                    "Task #{id} scheduled (tz: {}). Next run: {}",
                    preflight.timezone_used, preflight.next_run
                );
                if preflight.timezone_defaulted_to_utc {
                    message.push_str(" Timezone was not provided, so UTC was assumed.");
                }
                ToolResult::success(message)
            }
            Err(e) => ToolResult::error(format!("Failed to create task: {e}")),
        }
    }
}

// --- list_tasks ---

pub struct ListTasksTool {
    db: Arc<Database>,
}

impl ListTasksTool {
    pub fn new(db: Arc<Database>) -> Self {
        ListTasksTool { db }
    }
}

#[async_trait]
impl Tool for ListTasksTool {
    fn name(&self) -> &str {
        "list_scheduled_tasks"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_scheduled_tasks",
            "List scheduled tasks (active, running, paused, completed) for the current chat and caller persona. Control chats see all personas in the chat. Learned workflows (queue `workflow_id`) are unrelated to these cron/once schedules.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Caller's chat ID (for auth/filtering)."
                    }
                }),
                &["chat_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: chat_id".into()),
        };
        if let Err(e) = authorize_chat_access(&input, chat_id) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, chat_id).await {
            return ToolResult::error(e);
        }

        let input_for_list = input.clone();
        match call_blocking(self.db.clone(), move |db| {
            list_schedules_for_caller(&db, chat_id, &input_for_list)
                .map_err(crate::error::FinallyAValueBotError::ToolExecution)
        })
        .await
        {
            Ok(tasks) => ToolResult::success(format_tasks_list(&tasks)),
            Err(e) => ToolResult::error(format!("Failed to list tasks: {e}")),
        }
    }
}

// --- pause_task ---

pub struct PauseTaskTool {
    db: Arc<Database>,
}

impl PauseTaskTool {
    pub fn new(db: Arc<Database>) -> Self {
        PauseTaskTool { db }
    }
}

#[async_trait]
impl Tool for PauseTaskTool {
    fn name(&self) -> &str {
        "pause_scheduled_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "pause_scheduled_task",
            "Pause a scheduled task (cron/once row in `scheduled_tasks`). It will not run until resumed.",
            schema_object(
                json!({
                    "task_id": {
                        "type": "integer",
                        "description": "The task ID to pause"
                    }
                }),
                &["task_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let task_id = match input.get("task_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: task_id".into()),
        };
        let task = match call_blocking(self.db.clone(), move |db| db.get_task_by_id(task_id)).await
        {
            Ok(Some(t)) => t,
            Ok(None) => return ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => return ToolResult::error(format!("Failed to load task: {e}")),
        };
        if let Err(e) = authorize_scheduled_task_access(&input, &task) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, task.chat_id).await {
            return ToolResult::error(e);
        }

        match call_blocking(self.db.clone(), move |db| {
            db.update_task_status(task_id, "paused")
        })
        .await
        {
            Ok(true) => ToolResult::success(format!("Task #{task_id} paused.")),
            Ok(false) => ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => ToolResult::error(format!("Failed to pause task: {e}")),
        }
    }
}

// --- resume_task ---

pub struct ResumeTaskTool {
    db: Arc<Database>,
}

impl ResumeTaskTool {
    pub fn new(db: Arc<Database>) -> Self {
        ResumeTaskTool { db }
    }
}

#[async_trait]
impl Tool for ResumeTaskTool {
    fn name(&self) -> &str {
        "resume_scheduled_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "resume_scheduled_task",
            "Resume a paused scheduled task (`scheduled_tasks`).",
            schema_object(
                json!({
                    "task_id": {
                        "type": "integer",
                        "description": "The task ID to resume"
                    }
                }),
                &["task_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let task_id = match input.get("task_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: task_id".into()),
        };
        let task = match call_blocking(self.db.clone(), move |db| db.get_task_by_id(task_id)).await
        {
            Ok(Some(t)) => t,
            Ok(None) => return ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => return ToolResult::error(format!("Failed to load task: {e}")),
        };
        if let Err(e) = authorize_scheduled_task_access(&input, &task) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, task.chat_id).await {
            return ToolResult::error(e);
        }

        match call_blocking(self.db.clone(), move |db| {
            db.update_task_status(task_id, "active")
        })
        .await
        {
            Ok(true) => ToolResult::success(format!("Task #{task_id} resumed.")),
            Ok(false) => ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => ToolResult::error(format!("Failed to resume task: {e}")),
        }
    }
}

// --- cancel_task ---

pub struct CancelTaskTool {
    db: Arc<Database>,
}

impl CancelTaskTool {
    pub fn new(db: Arc<Database>) -> Self {
        CancelTaskTool { db }
    }
}

#[async_trait]
impl Tool for CancelTaskTool {
    fn name(&self) -> &str {
        "cancel_scheduled_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "cancel_scheduled_task",
            "Cancel a scheduled task permanently (marks cancelled).",
            schema_object(
                json!({
                    "task_id": {
                        "type": "integer",
                        "description": "The task ID to cancel"
                    }
                }),
                &["task_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let task_id = match input.get("task_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: task_id".into()),
        };
        let task = match call_blocking(self.db.clone(), move |db| db.get_task_by_id(task_id)).await
        {
            Ok(Some(t)) => t,
            Ok(None) => return ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => return ToolResult::error(format!("Failed to load task: {e}")),
        };
        if let Err(e) = authorize_scheduled_task_access(&input, &task) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, task.chat_id).await {
            return ToolResult::error(e);
        }

        match call_blocking(self.db.clone(), move |db| {
            db.update_task_status(task_id, "cancelled")
        })
        .await
        {
            Ok(true) => ToolResult::success(format!("Task #{task_id} cancelled.")),
            Ok(false) => ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => ToolResult::error(format!("Failed to cancel task: {e}")),
        }
    }
}

// --- update_scheduled_task ---

pub struct UpdateScheduledTaskTool {
    db: Arc<Database>,
    default_timezone: String,
}

impl UpdateScheduledTaskTool {
    pub fn new(db: Arc<Database>, default_timezone: String) -> Self {
        UpdateScheduledTaskTool {
            db,
            default_timezone,
        }
    }
}

#[async_trait]
impl Tool for UpdateScheduledTaskTool {
    fn name(&self) -> &str {
        "update_scheduled_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "update_scheduled_task",
            "Update an existing scheduled task by id: optional status (paused/active/cancelled), persona_id, prompt, and/or schedule (schedule_type + schedule_value with optional timezone). Same preflight rules as `schedule_task`. Activate the `schedule-job` skill before changing schedule fields.",
            schema_object(
                json!({
                    "task_id": {
                        "type": "integer",
                        "description": "Scheduled task id (from list_scheduled_tasks)"
                    },
                    "status": {
                        "type": "string",
                        "description": "Optional: paused, active (or resumed), or cancelled"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Optional: persona id for this chat"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional: new prompt text (non-empty)"
                    },
                    "schedule_type": {
                        "type": "string",
                        "enum": ["cron", "once"],
                        "description": "Optional: must be sent with schedule_value to change timing"
                    },
                    "schedule_value": {
                        "type": "string",
                        "description": "Optional: cron expression or ISO 8601 once timestamp; pair with schedule_type"
                    },
                    "timezone": {
                        "type": "string",
                        "description": "Optional IANA timezone for schedule preflight; defaults to bot config or UTC"
                    }
                }),
                &["task_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let task_id = match input.get("task_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: task_id".into()),
        };

        let task = match call_blocking(self.db.clone(), move |db| db.get_task_by_id(task_id)).await
        {
            Ok(Some(t)) => t,
            Ok(None) => return ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => return ToolResult::error(format!("Failed to load task: {e}")),
        };
        if let Err(e) = authorize_scheduled_task_access(&input, &task) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, task.chat_id).await {
            return ToolResult::error(e);
        }

        let status = match input.get("status").and_then(|v| v.as_str()) {
            Some("paused") => Some("paused"),
            Some("active") | Some("resumed") => Some("active"),
            Some("cancelled") => Some("cancelled"),
            Some(other) => {
                return ToolResult::error(format!(
                    "status must be paused, active, resumed, or cancelled; got {other}"
                ));
            }
            None => None,
        };
        let persona_id = input.get("persona_id").and_then(|v| v.as_i64());
        let prompt_update = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(|p| p.trim().to_string());
        let schedule_type_in = input
            .get("schedule_type")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let schedule_value_in = input
            .get("schedule_value")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let has_schedule_pair = schedule_type_in.is_some() && schedule_value_in.is_some();
        let schedule_partial = schedule_type_in.is_some() ^ schedule_value_in.is_some();
        if schedule_partial {
            return ToolResult::error(
                "Provide both schedule_type and schedule_value to change the schedule.".into(),
            );
        }

        if status.is_none() && persona_id.is_none() && prompt_update.is_none() && !has_schedule_pair
        {
            return ToolResult::error(
                "Provide at least one of: status, persona_id, prompt, or schedule_type+schedule_value."
                    .into(),
            );
        }

        if let Some(pid) = persona_id {
            if pid <= 0 {
                return ToolResult::error("persona_id must be a positive integer".into());
            }
            if pid != task.persona_id {
                let auth = match auth_context_from_input(&input) {
                    Some(a) => a,
                    None => {
                        return ToolResult::error(
                            "Reassigning scheduled task persona requires auth context".into(),
                        );
                    }
                };
                if !auth.is_control_chat() {
                    return ToolResult::error(
                        "Only control chats may reassign a scheduled task to another persona"
                            .into(),
                    );
                }
            }
            let exists = match call_blocking(self.db.clone(), move |db| {
                db.persona_exists(task.chat_id, pid)
            })
            .await
            {
                Ok(b) => b,
                Err(e) => return ToolResult::error(format!("Failed to check persona: {e}")),
            };
            if !exists {
                return ToolResult::error(format!("Persona {} does not exist for this chat", pid));
            }
        }

        if let Some(ref p) = prompt_update {
            if p.is_empty() {
                return ToolResult::error("prompt must not be empty".into());
            }
        }

        let timezone_input = input.get("timezone").and_then(|v| v.as_str());
        let configured_default_tz = self.default_timezone.trim();
        let effective_tz = timezone_input.or_else(|| {
            if configured_default_tz.is_empty() {
                None
            } else {
                Some(configured_default_tz)
            }
        });

        let schedule_preflight = if has_schedule_pair {
            let st = schedule_type_in.as_ref().unwrap().as_str();
            let sv = schedule_value_in.as_ref().unwrap().as_str();
            Some(match preflight_schedule_request(st, sv, effective_tz) {
                Ok(p) => p,
                Err(e) => return ToolResult::error(e),
            })
        } else {
            None
        };

        if let Some(next_status) = status {
            match call_blocking(self.db.clone(), move |db| {
                db.update_task_status(task_id, next_status)
            })
            .await
            {
                Ok(true) => {}
                Ok(false) => return ToolResult::error(format!("Task #{task_id} not found.")),
                Err(e) => return ToolResult::error(format!("Failed to update status: {e}")),
            }
        }
        if let Some(pid) = persona_id {
            match call_blocking(self.db.clone(), move |db| {
                db.update_task_persona(task_id, pid)
            })
            .await
            {
                Ok(true) => {}
                Ok(false) => return ToolResult::error(format!("Task #{task_id} not found.")),
                Err(e) => return ToolResult::error(format!("Failed to update persona: {e}")),
            }
        }
        if let Some(p) = prompt_update {
            match call_blocking(self.db.clone(), move |db| {
                db.update_task_prompt(task_id, &p)
            })
            .await
            {
                Ok(true) => {}
                Ok(false) => return ToolResult::error(format!("Task #{task_id} not found.")),
                Err(e) => return ToolResult::error(format!("Failed to update prompt: {e}")),
            }
        }
        if let Some(pref) = &schedule_preflight {
            let schedule_type_owned = schedule_type_in.as_ref().unwrap().clone();
            let schedule_value_owned = pref.schedule_value.clone();
            let next_run_owned = pref.next_run.clone();
            match call_blocking(self.db.clone(), move |db| {
                db.update_task_schedule(
                    task_id,
                    &schedule_type_owned,
                    &schedule_value_owned,
                    &next_run_owned,
                )
            })
            .await
            {
                Ok(true) => {}
                Ok(false) => return ToolResult::error(format!("Task #{task_id} not found.")),
                Err(e) => return ToolResult::error(format!("Failed to update schedule: {e}")),
            }
        }

        let mut msg = format!("Task #{task_id} updated.");
        if let Some(pref) = schedule_preflight {
            msg.push_str(&format!(
                " Next run: {} (tz: {}).",
                pref.next_run, pref.timezone_used
            ));
            if pref.timezone_defaulted_to_utc {
                msg.push_str(" Timezone was not provided for preflight, so UTC was assumed.");
            }
        }
        ToolResult::success(msg)
    }
}

// --- get_task_history ---

pub struct GetTaskHistoryTool {
    db: Arc<Database>,
}

impl GetTaskHistoryTool {
    pub fn new(db: Arc<Database>) -> Self {
        GetTaskHistoryTool { db }
    }
}

#[async_trait]
impl Tool for GetTaskHistoryTool {
    fn name(&self) -> &str {
        "get_task_history"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_task_history",
            "Get the execution history/run logs for a scheduled task.",
            schema_object(
                json!({
                    "task_id": {
                        "type": "integer",
                        "description": "The task ID to get history for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of log entries to return (default: 10)"
                    }
                }),
                &["task_id"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let task_id = match input.get("task_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::error("Missing required parameter: task_id".into()),
        };
        let task = match call_blocking(self.db.clone(), move |db| db.get_task_by_id(task_id)).await
        {
            Ok(Some(t)) => t,
            Ok(None) => return ToolResult::error(format!("Task #{task_id} not found.")),
            Err(e) => return ToolResult::error(format!("Failed to load task: {e}")),
        };
        if let Err(e) = authorize_scheduled_task_access(&input, &task) {
            return ToolResult::error(e);
        }
        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, task.chat_id).await {
            return ToolResult::error(e);
        }
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        match call_blocking(self.db.clone(), move |db| {
            db.get_task_run_logs(task_id, limit)
        })
        .await
        {
            Ok(logs) => {
                if logs.is_empty() {
                    return ToolResult::success(format!(
                        "No run history found for task #{task_id}."
                    ));
                }
                let mut output =
                    format!("Run history for task #{task_id} (most recent first):\n\n");
                for log in &logs {
                    let status = if log.success { "OK" } else { "FAIL" };
                    output.push_str(&format!(
                        "- [{}] {} | duration: {}ms | {}\n",
                        status,
                        log.started_at,
                        log.duration_ms,
                        log.result_summary.as_deref().unwrap_or("(no summary)"),
                    ));
                }
                ToolResult::success(output)
            }
            Err(e) => ToolResult::error(format!("Failed to get task history: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use serde_json::json;

    fn test_db() -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_sched_{}",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_compute_next_run_valid() {
        let result = compute_next_run("0 */5 * * * *", "UTC");
        assert!(result.is_ok());
        let ts = result.unwrap();
        // Should be a valid RFC3339 timestamp
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok());
    }

    #[test]
    fn test_compute_next_run_with_timezone() {
        let result = compute_next_run("0 */5 * * * *", "US/Eastern");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compute_next_run_invalid_cron() {
        let result = compute_next_run("not a cron", "UTC");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid cron"));
    }

    #[test]
    fn test_compute_next_run_invalid_timezone() {
        let result = compute_next_run("0 */5 * * * *", "Not/A/Zone");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid timezone"));
    }

    #[tokio::test]
    async fn test_schedule_task_cron() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "prompt": "say hi",
                "schedule_type": "cron",
                "schedule_value": "0 0 * * * *"
            }))
            .await;
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(result.content.contains("scheduled"));
        assert!(result.content.contains("Next run"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_once() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "prompt": "one time thing",
                "schedule_type": "once",
                "schedule_value": "2099-12-31T23:59:59+00:00"
            }))
            .await;
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(result.content.contains("scheduled"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_invalid_once_timestamp() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "prompt": "test",
                "schedule_type": "once",
                "schedule_value": "not-a-timestamp"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Invalid ISO 8601"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_invalid_type() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "prompt": "test",
                "schedule_type": "weekly",
                "schedule_value": "Monday"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("must be 'cron' or 'once'"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_missing_params() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_list_tasks_empty() {
        let (db, dir) = test_db();
        let tool = ListTasksTool::new(db);
        let result = tool.execute(json!({"chat_id": 100})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No scheduled tasks"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_list_tasks_with_tasks() {
        let (db, dir) = test_db();
        db.create_scheduled_task(100, "task A", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        db.create_scheduled_task(
            100,
            "task B",
            "once",
            "2024-06-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let tool = ListTasksTool::new(db);
        let result = tool.execute(json!({"chat_id": 100})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("task A"));
        assert!(result.content.contains("task B"));
        assert!(result.content.contains("[active]"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_pause_and_resume_task() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        let pause_tool = PauseTaskTool::new(db.clone());
        let result = pause_tool.execute(json!({"task_id": id})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("paused"));

        let resume_tool = ResumeTaskTool::new(db.clone());
        let result = resume_tool.execute(json!({"task_id": id})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("resumed"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_pause_nonexistent_task() {
        let (db, dir) = test_db();
        let tool = PauseTaskTool::new(db);
        let result = tool.execute(json!({"task_id": 9999})).await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_cancel_task() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        let tool = CancelTaskTool::new(db.clone());
        let result = tool.execute(json!({"task_id": id})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("cancelled"));

        // Task should no longer appear in list
        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert!(tasks.is_empty());
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_with_timezone() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "prompt": "tz test",
                "schedule_type": "cron",
                "schedule_value": "0 0 * * * *",
                "timezone": "US/Eastern"
            }))
            .await;
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(result.content.contains("scheduled"));
        assert!(result.content.contains("US/Eastern"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_get_task_history_empty() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        let tool = GetTaskHistoryTool::new(db);
        let result = tool.execute(json!({"task_id": task_id})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No run history"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_get_task_history_with_logs() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        db.log_task_run(
            task_id,
            100,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:05Z",
            5000,
            true,
            Some("All good"),
        )
        .unwrap();
        db.log_task_run(
            task_id,
            100,
            "2024-01-01T00:01:00Z",
            "2024-01-01T00:01:02Z",
            2000,
            false,
            Some("Error: timeout"),
        )
        .unwrap();

        let tool = GetTaskHistoryTool::new(db);
        let result = tool.execute(json!({"task_id": task_id})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("OK"));
        assert!(result.content.contains("FAIL"));
        assert!(result.content.contains("All good"));
        assert!(result.content.contains("Error: timeout"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_permission_denied_cross_chat() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "prompt": "say hi",
                "schedule_type": "once",
                "schedule_value": "2099-12-31T23:59:59+00:00",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_pause_task_permission_denied_cross_chat() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(200, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        let tool = PauseTaskTool::new(db);
        let result = tool
            .execute(json!({
                "task_id": task_id,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_web_caller_schedule_cross_chat_denied_even_for_control_chat() {
        let (db, dir) = test_db();
        db.upsert_chat(100, Some("web-main"), "web").unwrap();
        db.upsert_chat(200, Some("other"), "private").unwrap();
        let tool = ScheduleTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "prompt": "say hi",
                "schedule_type": "once",
                "schedule_value": "2099-12-31T23:59:59+00:00",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result
            .content
            .contains("web UI sessions cannot operate on other chats"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_schedule_task_allowed_for_control_chat_cross_chat() {
        let (db, dir) = test_db();
        let tool = ScheduleTaskTool::new(db.clone(), "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "prompt": "say hi",
                "schedule_type": "once",
                "schedule_value": "2099-12-31T23:59:59+00:00",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let tasks = db.get_tasks_for_chat(200).unwrap();
        assert_eq!(tasks.len(), 1);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_pause_task_allowed_for_control_chat_cross_chat() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(200, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        let tool = PauseTaskTool::new(db.clone());
        let result = tool
            .execute(json!({
                "task_id": task_id,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let task = db.get_task_by_id(task_id).unwrap().unwrap();
        assert_eq!(task.status, "paused");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_update_scheduled_task_prompt_and_schedule() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "hello", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        let tool = UpdateScheduledTaskTool::new(db.clone(), "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "task_id": id,
                "prompt": "updated prompt",
                "schedule_type": "cron",
                "schedule_value": "0 0 * * * *"
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let t = db.get_task_by_id(id).unwrap().unwrap();
        assert_eq!(t.prompt, "updated prompt");
        assert_eq!(t.schedule_type, "cron");
        assert_eq!(t.schedule_value, "0 0 * * * *");
        assert!(t.next_run.contains("T"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_list_scheduled_tasks_persona_scoped() {
        let (db, dir) = test_db();
        let pid_a = db.get_or_create_default_persona(100).unwrap();
        let pid_b = db.create_persona(100, "B", None).unwrap();
        db.create_scheduled_task_for_persona(
            100,
            pid_a,
            "task for A",
            "cron",
            "0 * * * * *",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.create_scheduled_task_for_persona(
            100,
            pid_b,
            "task for B",
            "cron",
            "0 * * * * *",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();

        let tool = ListTasksTool::new(db);
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("task for A"));
        assert!(!result.content.contains("task for B"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_list_scheduled_tasks_control_chat_sees_all_personas() {
        let (db, dir) = test_db();
        let pid_a = db.get_or_create_default_persona(100).unwrap();
        let pid_b = db.create_persona(100, "B", None).unwrap();
        db.create_scheduled_task_for_persona(
            100,
            pid_a,
            "task for A",
            "cron",
            "0 * * * * *",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.create_scheduled_task_for_persona(
            100,
            pid_b,
            "task for B",
            "cron",
            "0 * * * * *",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();

        let tool = ListTasksTool::new(db);
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("task for A"));
        assert!(result.content.contains("task for B"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_pause_scheduled_task_other_persona_denied() {
        let (db, dir) = test_db();
        let pid_a = db.get_or_create_default_persona(100).unwrap();
        let pid_b = db.create_persona(100, "B", None).unwrap();
        let task_b = db
            .create_scheduled_task_for_persona(
                100,
                pid_b,
                "B only",
                "cron",
                "0 * * * * *",
                "2024-01-01T00:00:00Z",
            )
            .unwrap();

        let tool = PauseTaskTool::new(db);
        let result = tool
            .execute(json!({
                "task_id": task_b,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_update_scheduled_task_persona_reassign_requires_control_chat() {
        let (db, dir) = test_db();
        let pid_a = db.get_or_create_default_persona(100).unwrap();
        let pid_b = db.create_persona(100, "B", None).unwrap();
        let task_id = db
            .create_scheduled_task_for_persona(
                100,
                pid_a,
                "owned by A",
                "cron",
                "0 * * * * *",
                "2024-01-01T00:00:00Z",
            )
            .unwrap();

        let tool = UpdateScheduledTaskTool::new(db.clone(), "UTC".into());
        let denied = tool
            .execute(json!({
                "task_id": task_id,
                "persona_id": pid_b,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(denied.is_error);
        assert!(denied.content.contains("control chats"));

        let allowed = tool
            .execute(json!({
                "task_id": task_id,
                "persona_id": pid_b,
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 100,
                    "caller_persona_id": pid_a,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!allowed.is_error, "{}", allowed.content);
        let t = db.get_task_by_id(task_id).unwrap().unwrap();
        assert_eq!(t.persona_id, pid_b);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_update_scheduled_task_schedule_only_one_field_errors() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "hello", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        let tool = UpdateScheduledTaskTool::new(db, "UTC".into());
        let result = tool
            .execute(json!({
                "chat_id": 100,
                "task_id": id,
                "schedule_type": "cron"
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("both schedule_type"));
        cleanup(&dir);
    }
}
