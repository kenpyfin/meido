//! Collaboration storage: scheduled runs, the board, and persona groups.
//!
//! Kept out of `db.rs` so that file stays a migration hook plus the existing API.

use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use std::path::Path;

use crate::db::{map_persona_todo, Database, PersonaTodo, StoredMessage, PERSONA_TODO_COLS};
use crate::error::FinallyAValueBotError;

pub const RUN_INLINE_CAP: usize = 65_536;
pub const RUN_PREVIEW_CHARS: usize = 280;
pub const ATTACH_EXPAND_CHARS: usize = 8_000;

pub fn load_run_body(row: &ScheduledRunRow) -> String {
    if let Some(path) = row.output_path.as_deref().filter(|p| !p.is_empty()) {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    row.output.clone().unwrap_or_default()
}

pub fn migrate(conn: &Connection) -> Result<(), FinallyAValueBotError> {
    if !column_exists(conn, "messages", "speaker_persona_id")? {
        conn.execute(
            "ALTER TABLE messages ADD COLUMN speaker_persona_id INTEGER",
            [],
        )?;
    }
    for (column, ddl) in [
        (
            "description",
            "ALTER TABLE persona_todos ADD COLUMN description TEXT",
        ),
        (
            "owner_persona_id",
            "ALTER TABLE persona_todos ADD COLUMN owner_persona_id INTEGER",
        ),
        (
            "priority",
            "ALTER TABLE persona_todos ADD COLUMN priority INTEGER NOT NULL DEFAULT 0",
        ),
        ("due_at", "ALTER TABLE persona_todos ADD COLUMN due_at TEXT"),
        (
            "blocked_reason",
            "ALTER TABLE persona_todos ADD COLUMN blocked_reason TEXT",
        ),
        (
            "position",
            "ALTER TABLE persona_todos ADD COLUMN position REAL NOT NULL DEFAULT 0",
        ),
        (
            "source_kind",
            "ALTER TABLE persona_todos ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'manual'",
        ),
        (
            "source_ref",
            "ALTER TABLE persona_todos ADD COLUMN source_ref TEXT",
        ),
        (
            "auto_advance",
            "ALTER TABLE persona_todos ADD COLUMN auto_advance INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "stage_run_id",
            "ALTER TABLE persona_todos ADD COLUMN stage_run_id TEXT",
        ),
        (
            "stage_outcome",
            "ALTER TABLE persona_todos ADD COLUMN stage_outcome TEXT",
        ),
        (
            "deploy_prompt",
            "ALTER TABLE persona_todos ADD COLUMN deploy_prompt TEXT",
        ),
    ] {
        if !column_exists(conn, "persona_todos", column)? {
            conn.execute(ddl, [])?;
        }
    }
    conn.execute(
        "UPDATE persona_todos SET status = 'todo' WHERE status = 'open'",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET completed_at = COALESCE(completed_at, updated_at) WHERE status = 'done'",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET status = 'deploy' WHERE status = 'done'",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET status = 'implement' WHERE status IN ('doing', 'blocked')",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET status = 'plan' WHERE status = 'design'",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET status = 'build' WHERE status = 'implement'",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET status = 'verify' WHERE status IN ('test', 'deploy')",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET owner_persona_id = persona_id WHERE owner_persona_id IS NULL",
        [],
    )?;
    conn.execute(
        "UPDATE persona_todos SET position = id WHERE position = 0",
        [],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_persona_todos_board
            ON persona_todos(chat_id, status, position);

        CREATE TABLE IF NOT EXISTS board_item_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chat_id INTEGER NOT NULL,
            item_id INTEGER NOT NULL,
            from_status TEXT,
            to_status TEXT NOT NULL,
            actor_kind TEXT NOT NULL,
            actor_persona_id INTEGER,
            at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_board_item_events_chat_time
            ON board_item_events(chat_id, at DESC);
        CREATE INDEX IF NOT EXISTS idx_board_item_events_item
            ON board_item_events(item_id, at);

        CREATE TABLE IF NOT EXISTS scheduled_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL,
            chat_id INTEGER NOT NULL,
            persona_id INTEGER NOT NULL,
            started_at TEXT NOT NULL,
            finished_at TEXT NOT NULL,
            status TEXT NOT NULL,
            preview TEXT NOT NULL DEFAULT '',
            output TEXT,
            output_path TEXT,
            error TEXT,
            read_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_chat_time
            ON scheduled_runs(chat_id, finished_at DESC, id);
        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_task_time
            ON scheduled_runs(task_id, finished_at DESC);
        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_unread
            ON scheduled_runs(chat_id) WHERE read_at IS NULL;

        CREATE TABLE IF NOT EXISTS coordinator_proposals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chat_id INTEGER NOT NULL,
            item_id INTEGER,
            change_json TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL,
            decided_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_coordinator_proposals_chat
            ON coordinator_proposals(chat_id, status, created_at DESC);

        CREATE TABLE IF NOT EXISTS persona_groups (
            id TEXT PRIMARY KEY,
            chat_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            turn_mode TEXT NOT NULL DEFAULT 'moderator',
            moderator_provider TEXT NOT NULL DEFAULT '',
            moderator_model TEXT NOT NULL DEFAULT '',
            max_speakers INTEGER NOT NULL DEFAULT 2,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            archived_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_persona_groups_chat
            ON persona_groups(chat_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS persona_group_members (
            group_id TEXT NOT NULL,
            persona_id INTEGER NOT NULL,
            role_note TEXT NOT NULL DEFAULT '',
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (group_id, persona_id)
        );

        CREATE TABLE IF NOT EXISTS group_sessions (
            id TEXT PRIMARY KEY,
            group_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            summary TEXT NOT NULL DEFAULT '',
            summary_upto_id TEXT,
            created_at TEXT NOT NULL,
            archived_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_group_sessions_group
            ON group_sessions(group_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS group_messages (
            id TEXT PRIMARY KEY,
            group_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            speaker_persona_id INTEGER,
            content TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            run_key TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_group_messages_session
            ON group_messages(session_id, timestamp, id);",
    )?;
    Ok(())
}

fn column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, FinallyAValueBotError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn clip_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[derive(Debug, Clone)]
pub struct ScheduledRunRow {
    pub id: i64,
    pub task_id: i64,
    pub chat_id: i64,
    pub persona_id: i64,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub preview: String,
    pub output: Option<String>,
    pub output_path: Option<String>,
    pub error: Option<String>,
    pub read_at: Option<String>,
}

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledRunRow> {
    Ok(ScheduledRunRow {
        id: row.get(0)?,
        task_id: row.get(1)?,
        chat_id: row.get(2)?,
        persona_id: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        status: row.get(6)?,
        preview: row.get(7)?,
        output: row.get(8)?,
        output_path: row.get(9)?,
        error: row.get(10)?,
        read_at: row.get(11)?,
    })
}

const RUN_LIST_COLS: &str = "id, task_id, chat_id, persona_id, started_at, finished_at, status, preview, NULL, NULL, error, read_at";
const RUN_FULL_COLS: &str = "id, task_id, chat_id, persona_id, started_at, finished_at, status, preview, output, output_path, error, read_at";

impl Database {
    pub fn insert_scheduled_run(
        &self,
        task_id: i64,
        chat_id: i64,
        persona_id: i64,
        started_at: &str,
        finished_at: &str,
        status: &str,
        body: &str,
        error: Option<&str>,
        runtime_dir: &Path,
    ) -> Result<i64, FinallyAValueBotError> {
        let preview = clip_chars(body, RUN_PREVIEW_CHARS);
        let (output, output_path) = if body.len() <= RUN_INLINE_CAP {
            (Some(body.to_string()), None)
        } else {
            (None, Some(String::new()))
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO scheduled_runs (
                task_id, chat_id, persona_id, started_at, finished_at, status, preview, output, output_path, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_id,
                chat_id,
                persona_id,
                started_at,
                finished_at,
                status,
                preview,
                output,
                output_path,
                error
            ],
        )?;
        let id = conn.last_insert_rowid();
        if body.len() > RUN_INLINE_CAP {
            let dir = runtime_dir.join("scheduled-runs");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{id}.txt"));
            std::fs::write(&path, body)?;
            let stored = path.to_string_lossy().to_string();
            conn.execute(
                "UPDATE scheduled_runs SET output_path = ?1 WHERE id = ?2",
                params![stored, id],
            )?;
        }
        prune_runs_conn(&conn, task_id)?;
        let cutoff = (Utc::now() - Duration::days(90)).to_rfc3339();
        conn.execute(
            "DELETE FROM board_item_events WHERE at < ?1",
            params![cutoff],
        )?;
        Ok(id)
    }

    pub fn list_scheduled_runs(
        &self,
        chat_id: i64,
        task_id: Option<i64>,
        before_finished: Option<&str>,
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ScheduledRunRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 50) as i64;
        let sql = format!(
            "SELECT {RUN_LIST_COLS} FROM scheduled_runs
             WHERE chat_id = ?1
               AND (?2 IS NULL OR task_id = ?2)
               AND (
                 ?3 IS NULL
                 OR finished_at < ?3
                 OR (finished_at = ?3 AND id < COALESCE(?4, id))
               )
             ORDER BY finished_at DESC, id DESC
             LIMIT ?5"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![chat_id, task_id, before_finished, before_id, limit],
            map_run,
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_scheduled_run(
        &self,
        chat_id: i64,
        run_id: i64,
    ) -> Result<Option<ScheduledRunRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let sql =
            format!("SELECT {RUN_FULL_COLS} FROM scheduled_runs WHERE id = ?1 AND chat_id = ?2");
        match conn.query_row(&sql, params![run_id, chat_id], map_run) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn mark_scheduled_run_read(
        &self,
        chat_id: i64,
        run_id: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE scheduled_runs SET read_at = ?1 WHERE id = ?2 AND chat_id = ?3 AND read_at IS NULL",
            params![now, run_id, chat_id],
        )?;
        Ok(())
    }

    /// Marks every unread run in the chat read. One write, so the header count drops once.
    pub fn mark_all_scheduled_runs_read(
        &self,
        chat_id: i64,
    ) -> Result<usize, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let cleared = conn.execute(
            "UPDATE scheduled_runs SET read_at = ?1 WHERE chat_id = ?2 AND read_at IS NULL",
            params![now, chat_id],
        )?;
        Ok(cleared)
    }

    pub fn count_unread_runs(&self, chat_id: i64) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM scheduled_runs WHERE chat_id = ?1 AND read_at IS NULL",
            params![chat_id],
            |row| row.get(0),
        )?;
        Ok(n)
    }

    pub fn count_blocked_items(&self, chat_id: i64) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM persona_todos WHERE chat_id = ?1 AND blocked_reason IS NOT NULL AND blocked_reason != ''",
            params![chat_id],
            |row| row.get(0),
        )?;
        Ok(n)
    }

    pub fn expand_trailing_attachments(&self, history: &mut [StoredMessage]) {
        let Some(start) = trailing_user_start(history) else {
            return;
        };
        let conn = self.conn.lock().unwrap();
        for message in history.iter_mut().skip(start) {
            let expanded = expand_one(&conn, message);
            if let Some(text) = expanded {
                message.content = text;
            }
        }
    }

    pub fn apply_board_patch(
        &self,
        chat_id: i64,
        item_id: i64,
        patch: &BoardPatch,
    ) -> Result<Option<PersonaTodo>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let existing = match conn.query_row(
            &format!("SELECT {PERSONA_TODO_COLS} FROM persona_todos WHERE id = ?1"),
            params![item_id],
            map_persona_todo,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if existing.chat_id != chat_id {
            return Ok(None);
        }
        let now = Utc::now().to_rfc3339();
        let raw_status = patch
            .status
            .as_deref()
            .map(|status| status.trim().to_ascii_lowercase());
        let mark_complete = patch.completed == Some(true) || raw_status.as_deref() == Some("done");
        let clear_complete = patch.completed == Some(false) && !mark_complete;
        let status = if let Some(raw) = patch.status.as_deref() {
            crate::db::normalize_board_status(raw)?.to_string()
        } else {
            existing.status.clone()
        };
        let title = patch
            .title
            .as_deref()
            .map(|t| clip_chars(t.trim(), 500))
            .filter(|t| !t.is_empty())
            .unwrap_or(existing.title.clone());
        let description = patch
            .description
            .as_ref()
            .map(|d| clip_chars(d, 4000))
            .or(existing.description.clone());
        let deploy_prompt = match patch.deploy_prompt.as_deref() {
            Some(text) => {
                let clipped = clip_chars(text, 4000);
                if clipped.trim().is_empty() {
                    None
                } else {
                    Some(clipped)
                }
            }
            None => existing.deploy_prompt.clone(),
        };
        let owner = patch.owner_persona_id.or(existing.owner_persona_id);
        let priority = patch.priority.unwrap_or(existing.priority);
        let due_at = patch.due_at.clone().or(existing.due_at.clone());
        let blocked_reason = match patch.blocked_reason.as_deref() {
            Some(reason) => {
                let trimmed = reason.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(clip_chars(trimmed, 500))
                }
            }
            None => existing.blocked_reason.clone(),
        };
        let auto_advance = match patch.auto_advance {
            Some(true) => 1,
            Some(false) => 0,
            None => existing.auto_advance,
        };
        let position = match (patch.before_position, patch.after_position) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (None, Some(b)) => b - 1.0,
            (Some(a), None) => a + 1.0,
            (None, None) => patch.position.unwrap_or(existing.position),
        };
        let completed_at: Option<String> = if mark_complete {
            Some(existing.completed_at.clone().unwrap_or_else(|| now.clone()))
        } else if clear_complete {
            None
        } else if status == "verify" {
            existing.completed_at.clone()
        } else if patch.status.is_some() {
            None
        } else {
            existing.completed_at.clone()
        };
        let status_changed = existing.status != status;
        let clear_stage = status_changed || mark_complete;
        let clear_stage_flag: i64 = if clear_stage { 1 } else { 0 };
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE persona_todos SET
                title = ?1, status = ?2, description = ?3, owner_persona_id = ?4,
                priority = ?5, due_at = ?6, blocked_reason = ?7, position = ?8,
                updated_at = ?9, completed_at = ?10, auto_advance = ?11,
                stage_run_id = CASE WHEN ?12 = 1 THEN NULL ELSE stage_run_id END,
                stage_outcome = CASE WHEN ?12 = 1 THEN NULL ELSE stage_outcome END,
                deploy_prompt = ?13
             WHERE id = ?14",
            params![
                title,
                status,
                description,
                owner,
                priority,
                due_at,
                blocked_reason,
                position,
                now,
                completed_at,
                auto_advance,
                clear_stage_flag,
                deploy_prompt,
                item_id
            ],
        )?;
        if status_changed || (mark_complete && existing.completed_at.is_none()) {
            tx.execute(
                "INSERT INTO board_item_events (chat_id, item_id, from_status, to_status, actor_kind, actor_persona_id, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    chat_id,
                    item_id,
                    existing.status,
                    status,
                    patch.actor_kind,
                    patch.actor_persona_id,
                    now
                ],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_persona_todo(item_id)
    }

    pub fn mark_board_stage_running(
        &self,
        item_id: i64,
        run_id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE persona_todos
             SET stage_run_id = ?1, stage_outcome = 'running', blocked_reason = NULL, updated_at = ?2
             WHERE id = ?3",
            params![run_id, now, item_id],
        )?;
        Ok(())
    }

    pub fn mark_board_stage_failed(
        &self,
        item_id: i64,
        error: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let reason = clip_chars(error.trim(), 500);
        conn.execute(
            "UPDATE persona_todos
             SET stage_run_id = NULL,
                 stage_outcome = 'failed',
                 blocked_reason = CASE
                   WHEN blocked_reason IS NULL OR blocked_reason = '' THEN ?1
                   ELSE blocked_reason
                 END,
                 updated_at = ?2
             WHERE id = ?3",
            params![reason, now, item_id],
        )?;
        Ok(())
    }

    pub fn clear_board_stage_run(&self, item_id: i64) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE persona_todos
             SET stage_run_id = NULL,
                 stage_outcome = CASE
                   WHEN stage_outcome IN ('running', 'passed') THEN NULL
                   ELSE stage_outcome
                 END,
                 updated_at = ?1
             WHERE id = ?2",
            params![now, item_id],
        )?;
        Ok(())
    }

    pub fn mark_board_stage_outcome(
        &self,
        chat_id: i64,
        item_id: i64,
        outcome: &str,
        blocked_reason: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<PersonaTodo>, FinallyAValueBotError> {
        if outcome != "passed" && outcome != "blocked" {
            return Err(FinallyAValueBotError::ToolExecution(
                "stage outcome must be pass or blocked".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let existing = match conn.query_row(
            &format!("SELECT {PERSONA_TODO_COLS} FROM persona_todos WHERE id = ?1"),
            params![item_id],
            map_persona_todo,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if existing.chat_id != chat_id {
            return Ok(None);
        }
        let blocked = if outcome == "passed" {
            None
        } else {
            let reason = blocked_reason.unwrap_or("").trim();
            if reason.is_empty() {
                return Err(FinallyAValueBotError::ToolExecution(
                    "blocked_reason is required when the stage is blocked".into(),
                ));
            }
            Some(clip_chars(reason, 500))
        };
        let description = match note.map(str::trim).filter(|text| !text.is_empty()) {
            Some(note) => {
                let merged = match existing
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    Some(previous) => format!("{previous}\n\n{note}"),
                    None => note.to_string(),
                };
                Some(clip_chars(&merged, 4000))
            }
            None => existing.description.clone(),
        };
        let now = Utc::now().to_rfc3339();
        let completed_at = if outcome == "passed" && existing.status == "verify" {
            Some(now.clone())
        } else {
            existing.completed_at.clone()
        };
        let stored_outcome = if outcome == "passed" {
            "passed"
        } else {
            "blocked"
        };
        conn.execute(
            "UPDATE persona_todos
             SET stage_outcome = ?1, stage_run_id = NULL, blocked_reason = ?2,
                 description = ?3, completed_at = ?4, updated_at = ?5
             WHERE id = ?6",
            params![
                stored_outcome,
                blocked,
                description,
                completed_at,
                now,
                item_id
            ],
        )?;
        drop(conn);
        self.get_persona_todo(item_id)
    }

    pub fn set_todo_source(
        &self,
        chat_id: i64,
        item_id: i64,
        source_kind: &str,
        source_ref: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE persona_todos SET source_kind = ?1, source_ref = ?2
             WHERE id = ?3 AND chat_id = ?4",
            params![source_kind, source_ref, item_id, chat_id],
        )?;
        Ok(())
    }

    pub fn list_board_item_events(
        &self,
        chat_id: i64,
        item_id: i64,
        limit: usize,
    ) -> Result<Option<Vec<BoardItemEvent>>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let owner_chat: Option<i64> = match conn.query_row(
            "SELECT chat_id FROM persona_todos WHERE id = ?1",
            params![item_id],
            |row| row.get(0),
        ) {
            Ok(chat) => Some(chat),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(err) => return Err(err.into()),
        };
        if owner_chat != Some(chat_id) {
            return Ok(None);
        }
        let limit = limit.clamp(1, 100) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, from_status, to_status, actor_kind, actor_persona_id, at
             FROM board_item_events
             WHERE chat_id = ?1 AND item_id = ?2
             ORDER BY at ASC, id ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![chat_id, item_id, limit], |row| {
            Ok(BoardItemEvent {
                id: row.get(0)?,
                from_status: row.get(1)?,
                to_status: row.get(2)?,
                actor_kind: row.get(3)?,
                actor_persona_id: row.get(4)?,
                at: row.get(5)?,
            })
        })?;
        Ok(Some(rows.collect::<Result<Vec<_>, _>>()?))
    }

    pub fn list_board_events_since(
        &self,
        chat_id: i64,
        since: &str,
        limit: usize,
    ) -> Result<Vec<BoardEvent>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 40) as i64;
        let mut stmt = conn.prepare(
            "SELECT e.id, e.item_id, t.title, e.from_status, e.to_status, e.actor_kind, e.at
             FROM board_item_events e
             LEFT JOIN persona_todos t ON t.id = e.item_id
             WHERE e.chat_id = ?1 AND e.at >= ?2
             ORDER BY e.at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![chat_id, since, limit], |row| {
            Ok(BoardEvent {
                id: row.get(0)?,
                item_id: row.get(1)?,
                title: row.get(2)?,
                from_status: row.get(3)?,
                to_status: row.get(4)?,
                actor_kind: row.get(5)?,
                at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert_proposal(
        &self,
        chat_id: i64,
        item_id: Option<i64>,
        change_json: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO coordinator_proposals (chat_id, item_id, change_json, status, created_at)
             VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![chat_id, item_id, change_json, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_proposals(
        &self,
        chat_id: i64,
        status: Option<&str>,
        since: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProposalRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 50) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, item_id, change_json, status, created_at, decided_at
             FROM coordinator_proposals
             WHERE chat_id = ?1
               AND (?2 IS NULL OR status = ?2)
               AND (?3 IS NULL OR created_at >= ?3)
             ORDER BY created_at DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![chat_id, status, since, limit], map_proposal)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn decide_proposal(
        &self,
        chat_id: i64,
        proposal_id: i64,
        status: &str,
    ) -> Result<Option<ProposalRow>, FinallyAValueBotError> {
        if status != "accepted" && status != "rejected" {
            return Err(FinallyAValueBotError::ToolExecution(
                "proposal status must be accepted or rejected".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE coordinator_proposals
             SET status = ?1, decided_at = ?2
             WHERE id = ?3 AND chat_id = ?4 AND status = 'pending'",
            params![status, now, proposal_id, chat_id],
        )?;
        if rows == 0 {
            return Ok(None);
        }
        match conn.query_row(
            "SELECT id, chat_id, item_id, change_json, status, created_at, decided_at
             FROM coordinator_proposals WHERE id = ?1",
            params![proposal_id],
            map_proposal,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn create_group(
        &self,
        chat_id: i64,
        name: &str,
        turn_mode: &str,
        max_speakers: i64,
    ) -> Result<GroupRow, FinallyAValueBotError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let mode = normalize_turn_mode(turn_mode)?;
        let speakers = max_speakers.clamp(1, 4);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO persona_groups (
                id, chat_id, name, turn_mode, max_speakers, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                id,
                chat_id,
                clip_chars(name.trim(), 80),
                mode,
                speakers,
                now
            ],
        )?;
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO group_sessions (id, group_id, title, created_at) VALUES (?1, ?2, 'Session', ?3)",
            params![session_id, id, now],
        )?;
        drop(conn);
        self.get_group(chat_id, &id)?
            .ok_or_else(|| FinallyAValueBotError::ToolExecution("group insert failed".into()))
    }

    pub fn list_groups(&self, chat_id: i64) -> Result<Vec<GroupRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, name, turn_mode, moderator_provider, moderator_model, max_speakers, created_at, updated_at, archived_at
             FROM persona_groups
             WHERE chat_id = ?1 AND archived_at IS NULL
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![chat_id], map_group)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_group(
        &self,
        chat_id: i64,
        group_id: &str,
    ) -> Result<Option<GroupRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT id, chat_id, name, turn_mode, moderator_provider, moderator_model, max_speakers, created_at, updated_at, archived_at
             FROM persona_groups WHERE id = ?1 AND chat_id = ?2",
            params![group_id, chat_id],
            map_group,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update_group(
        &self,
        chat_id: i64,
        group_id: &str,
        patch: &GroupPatch,
    ) -> Result<Option<GroupRow>, FinallyAValueBotError> {
        let Some(existing) = self.get_group(chat_id, group_id)? else {
            return Ok(None);
        };
        let name = patch
            .name
            .as_deref()
            .map(|n| clip_chars(n.trim(), 80))
            .filter(|n| !n.is_empty())
            .unwrap_or(existing.name);
        let mode = if let Some(mode) = patch.turn_mode.as_deref() {
            normalize_turn_mode(mode)?.to_string()
        } else {
            existing.turn_mode
        };
        let provider = patch
            .moderator_provider
            .clone()
            .unwrap_or(existing.moderator_provider);
        let model = patch
            .moderator_model
            .clone()
            .unwrap_or(existing.moderator_model);
        let speakers = patch
            .max_speakers
            .unwrap_or(existing.max_speakers)
            .clamp(1, 4);
        let archived = if patch.archive == Some(true) {
            Some(Utc::now().to_rfc3339())
        } else {
            existing.archived_at
        };
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE persona_groups SET
                name = ?1, turn_mode = ?2, moderator_provider = ?3, moderator_model = ?4,
                max_speakers = ?5, updated_at = ?6, archived_at = ?7
             WHERE id = ?8 AND chat_id = ?9",
            params![name, mode, provider, model, speakers, now, archived, group_id, chat_id],
        )?;
        drop(conn);
        self.get_group(chat_id, group_id)
    }

    pub fn set_group_members(
        &self,
        chat_id: i64,
        group_id: &str,
        members: &[(i64, String)],
    ) -> Result<(), FinallyAValueBotError> {
        let Some(_) = self.get_group(chat_id, group_id)? else {
            return Err(FinallyAValueBotError::ToolExecution(
                "group not found".into(),
            ));
        };
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM persona_group_members WHERE group_id = ?1",
            params![group_id],
        )?;
        for (index, (persona_id, note)) in members.iter().enumerate() {
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM personas WHERE id = ?1 AND chat_id = ?2",
                params![persona_id, chat_id],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(FinallyAValueBotError::ToolExecution(format!(
                    "persona {persona_id} is not in this chat"
                )));
            }
            tx.execute(
                "INSERT INTO persona_group_members (group_id, persona_id, role_note, position)
                 VALUES (?1, ?2, ?3, ?4)",
                params![group_id, persona_id, clip_chars(note, 500), index as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<GroupMember>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.persona_id, p.name, m.role_note, m.position
             FROM persona_group_members m
             JOIN personas p ON p.id = m.persona_id
             WHERE m.group_id = ?1
             ORDER BY m.position ASC, m.persona_id ASC",
        )?;
        let rows = stmt.query_map(params![group_id], |row| {
            Ok(GroupMember {
                persona_id: row.get(0)?,
                name: row.get(1)?,
                role_note: row.get(2)?,
                position: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_group_session(
        &self,
        session_id: &str,
    ) -> Result<Option<GroupSessionRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT id, group_id, title, summary, summary_upto_id, created_at, archived_at
             FROM group_sessions WHERE id = ?1",
            params![session_id],
            map_group_session,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn current_group_session(
        &self,
        group_id: &str,
    ) -> Result<Option<GroupSessionRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT id, group_id, title, summary, summary_upto_id, created_at, archived_at
             FROM group_sessions
             WHERE group_id = ?1 AND archived_at IS NULL
             ORDER BY created_at DESC
             LIMIT 1",
            params![group_id],
            map_group_session,
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn start_group_session(
        &self,
        chat_id: i64,
        group_id: &str,
    ) -> Result<GroupSessionRow, FinallyAValueBotError> {
        let Some(_) = self.get_group(chat_id, group_id)? else {
            return Err(FinallyAValueBotError::ToolExecution(
                "group not found".into(),
            ));
        };
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE group_sessions SET archived_at = ?1 WHERE group_id = ?2 AND archived_at IS NULL",
            params![now, group_id],
        )?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO group_sessions (id, group_id, title, created_at) VALUES (?1, ?2, 'Session', ?3)",
            params![id, group_id, now],
        )?;
        drop(conn);
        self.current_group_session(group_id)?
            .ok_or_else(|| FinallyAValueBotError::ToolExecution("session insert failed".into()))
    }

    pub fn append_group_message(
        &self,
        group_id: &str,
        session_id: &str,
        speaker_persona_id: Option<i64>,
        content: &str,
        run_key: Option<&str>,
    ) -> Result<String, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO group_messages (id, group_id, session_id, speaker_persona_id, content, timestamp, run_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                group_id,
                session_id,
                speaker_persona_id,
                content,
                now,
                run_key
            ],
        )?;
        conn.execute(
            "UPDATE persona_groups SET updated_at = ?1 WHERE id = ?2",
            params![now, group_id],
        )?;
        Ok(id)
    }

    pub fn list_group_messages(
        &self,
        session_id: &str,
        before_ts: Option<&str>,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GroupMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 50) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, session_id, speaker_persona_id, content, timestamp, run_key
             FROM group_messages
             WHERE session_id = ?1
               AND (
                 ?2 IS NULL
                 OR timestamp < ?2
                 OR (timestamp = ?2 AND id < COALESCE(?3, id))
               )
             ORDER BY timestamp DESC, id DESC
             LIMIT ?4",
        )?;
        let mut rows = stmt
            .query_map(
                params![session_id, before_ts, before_id, limit],
                map_group_message,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn recent_group_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<GroupMessage>, FinallyAValueBotError> {
        self.list_group_messages(session_id, None, None, limit)
    }

    pub fn group_messages_since_summary(
        &self,
        session_id: &str,
        summary_upto_id: Option<&str>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = if let Some(upto) = summary_upto_id {
            conn.query_row(
                "SELECT COUNT(*) FROM group_messages
                 WHERE session_id = ?1 AND timestamp > COALESCE(
                   (SELECT timestamp FROM group_messages WHERE id = ?2),
                   ''
                 )",
                params![session_id, upto],
                |row| row.get(0),
            )?
        } else {
            conn.query_row(
                "SELECT COUNT(*) FROM group_messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?
        };
        Ok(n)
    }

    pub fn set_group_summary(
        &self,
        session_id: &str,
        summary: &str,
        upto_id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE group_sessions SET summary = ?1, summary_upto_id = ?2 WHERE id = ?3",
            params![clip_chars(summary, 2000), upto_id, session_id],
        )?;
        Ok(())
    }

    pub fn latest_topic_lines(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 20) as i64;
        let mut stmt = conn.prepare(
            "SELECT p.name, t.title, t.summary
             FROM personas p
             JOIN conversation_topics t ON t.id = (
               SELECT id FROM conversation_topics
               WHERE chat_id = p.chat_id AND persona_id = p.id
               ORDER BY end_ts DESC LIMIT 1
             )
             WHERE p.chat_id = ?1
             ORDER BY t.end_ts DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![chat_id, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn ensure_coordinator_day_session(
        &self,
        chat_id: i64,
        persona_id: i64,
        day_title: &str,
    ) -> Result<String, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        if let Ok(id) = conn.query_row(
            "SELECT id FROM chat_sessions
             WHERE chat_id = ?1 AND persona_id = ?2 AND title = ?3 AND status = 'active'
             LIMIT 1",
            params![chat_id, persona_id, day_title],
            |row| row.get::<_, String>(0),
        ) {
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO chat_sessions (id, chat_id, persona_id, title, intent, status, created_at, last_active_at, ttl_hours, mirror_main_chat)
             VALUES (?1, ?2, ?3, ?4, 'coordinator-day', 'active', ?5, ?5, 0, 0)",
            params![id, chat_id, persona_id, day_title, now],
        )?;
        Ok(id)
    }

    pub fn coordinator_persona_id(
        &self,
        chat_id: i64,
    ) -> Result<Option<i64>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT id FROM personas WHERE chat_id = ?1 AND lower(name) = 'coordinator' LIMIT 1",
            params![chat_id],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Latest assistant line for this persona at or after `since` (RFC3339).
    pub fn latest_bot_message_since(
        &self,
        chat_id: i64,
        persona_id: i64,
        since: &str,
    ) -> Result<Option<(String, String)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT content, timestamp FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND is_from_bot = 1 AND timestamp >= ?3
             ORDER BY timestamp DESC, id DESC
             LIMIT 1",
            params![chat_id, persona_id, since],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ) {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn latest_brief_run(
        &self,
        chat_id: i64,
        prompt_prefix: &str,
    ) -> Result<Option<ScheduledRunRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let like = format!("{prompt_prefix}%");
        let sql = "SELECT r.id, r.task_id, r.chat_id, r.persona_id, r.started_at, r.finished_at,
                    r.status, r.preview, r.output, r.output_path, r.error, r.read_at
             FROM scheduled_runs r
             JOIN scheduled_tasks t ON t.id = r.task_id
             WHERE r.chat_id = ?1 AND t.prompt LIKE ?2
             ORDER BY r.finished_at DESC, r.id DESC
             LIMIT 1";
        match conn.query_row(&sql, params![chat_id, like], map_run) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn previous_brief_preview(
        &self,
        chat_id: i64,
        prompt_prefix: &str,
    ) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let like = format!("{prompt_prefix}%");
        match conn.query_row(
            "SELECT r.preview FROM scheduled_runs r
             JOIN scheduled_tasks t ON t.id = r.task_id
             WHERE r.chat_id = ?1 AND t.prompt LIKE ?2
             ORDER BY r.finished_at DESC
             LIMIT 1",
            params![chat_id, like],
            |row| row.get::<_, String>(0),
        ) {
            Ok(text) => Ok(Some(text)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn prune_runs_conn(conn: &Connection, task_id: i64) -> Result<(), FinallyAValueBotError> {
    let cutoff = (Utc::now() - Duration::days(90)).to_rfc3339();
    conn.execute(
        "DELETE FROM scheduled_runs
         WHERE task_id = ?1
           AND finished_at < ?2
           AND id NOT IN (
             SELECT id FROM scheduled_runs
             WHERE task_id = ?1
             ORDER BY finished_at DESC, id DESC
             LIMIT 200
           )",
        params![task_id, cutoff],
    )?;
    Ok(())
}

fn trailing_user_start(history: &[StoredMessage]) -> Option<usize> {
    let mut last_assistant = None;
    for (index, message) in history.iter().enumerate() {
        if message.is_from_bot
            && message.origin != crate::db::MESSAGE_ORIGIN_ATTACHED_RUN
            && message.origin != crate::db::MESSAGE_ORIGIN_ATTACHED_TASK
        {
            last_assistant = Some(index);
        }
    }
    let start = last_assistant.map(|i| i + 1).unwrap_or(0);
    if start >= history.len() {
        None
    } else {
        Some(start)
    }
}

fn expand_one(conn: &Connection, message: &StoredMessage) -> Option<String> {
    let (kind, id) = attachment_ref(&message.content)?;
    if message.origin != crate::db::MESSAGE_ORIGIN_ATTACHED_RUN
        && message.origin != crate::db::MESSAGE_ORIGIN_ATTACHED_TASK
    {
        return None;
    }
    let body = if kind == "run" {
        conn.query_row(
            "SELECT output, output_path, preview FROM scheduled_runs WHERE id = ?1 AND chat_id = ?2",
            params![id, message.chat_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok()
        .map(|(output, path, preview)| {
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                std::fs::read_to_string(path).unwrap_or(output.unwrap_or(preview))
            } else {
                output.unwrap_or(preview)
            }
        })
    } else {
        conn.query_row(
            "SELECT prompt, schedule_value, status FROM scheduled_tasks WHERE id = ?1 AND chat_id = ?2",
            params![id, message.chat_id],
            |row| Ok(format!("task #{} status {} cron {}\n{}", id, row.get::<_, String>(2)?, row.get::<_, String>(1)?, row.get::<_, String>(0)?)),
        )
        .ok()
    }?;
    Some(clip_chars(&body, ATTACH_EXPAND_CHARS))
}

fn attachment_ref(content: &str) -> Option<(&'static str, i64)> {
    let rest = content.trim_start();
    let (kind, rest) = if let Some(rest) = rest.strip_prefix("[[scheduled_run:") {
        ("run", rest)
    } else {
        let rest = rest.strip_prefix("[[scheduled_task:")?;
        ("task", rest)
    };
    let id_txt = rest.split(']').next()?;
    let id = id_txt.parse().ok()?;
    Some((kind, id))
}

#[derive(Debug, Clone)]
pub struct BoardPatch<'a> {
    pub status: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub owner_persona_id: Option<i64>,
    pub priority: Option<i64>,
    pub due_at: Option<String>,
    pub blocked_reason: Option<String>,
    pub position: Option<f64>,
    pub before_position: Option<f64>,
    pub after_position: Option<f64>,
    pub actor_kind: &'a str,
    pub actor_persona_id: Option<i64>,
    pub auto_advance: Option<bool>,
    /// `Some(true)` marks the card complete without requiring a column change.
    /// `Some(false)` clears `completed_at`.
    pub completed: Option<bool>,
    pub deploy_prompt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoardItemEvent {
    pub id: i64,
    pub from_status: Option<String>,
    pub to_status: String,
    pub actor_kind: String,
    pub actor_persona_id: Option<i64>,
    pub at: String,
}

#[derive(Debug, Clone)]
pub struct BoardEvent {
    pub id: i64,
    pub item_id: i64,
    pub title: Option<String>,
    pub from_status: Option<String>,
    pub to_status: String,
    pub actor_kind: String,
    pub at: String,
}

#[derive(Debug, Clone)]
pub struct ProposalRow {
    pub id: i64,
    pub chat_id: i64,
    pub item_id: Option<i64>,
    pub change_json: String,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

fn map_proposal(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProposalRow> {
    Ok(ProposalRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        item_id: row.get(2)?,
        change_json: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        decided_at: row.get(6)?,
    })
}

#[derive(Debug, Clone)]
pub struct GroupRow {
    pub id: String,
    pub chat_id: i64,
    pub name: String,
    pub turn_mode: String,
    pub moderator_provider: String,
    pub moderator_model: String,
    pub max_speakers: i64,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

fn map_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupRow> {
    Ok(GroupRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        name: row.get(2)?,
        turn_mode: row.get(3)?,
        moderator_provider: row.get(4)?,
        moderator_model: row.get(5)?,
        max_speakers: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        archived_at: row.get(9)?,
    })
}

#[derive(Debug, Clone)]
pub struct GroupPatch {
    pub name: Option<String>,
    pub turn_mode: Option<String>,
    pub moderator_provider: Option<String>,
    pub moderator_model: Option<String>,
    pub max_speakers: Option<i64>,
    pub archive: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct GroupMember {
    pub persona_id: i64,
    pub name: String,
    pub role_note: String,
    pub position: i64,
}

#[derive(Debug, Clone)]
pub struct GroupSessionRow {
    pub id: String,
    pub group_id: String,
    pub title: String,
    pub summary: String,
    pub summary_upto_id: Option<String>,
    pub created_at: String,
    pub archived_at: Option<String>,
}

fn map_group_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupSessionRow> {
    Ok(GroupSessionRow {
        id: row.get(0)?,
        group_id: row.get(1)?,
        title: row.get(2)?,
        summary: row.get(3)?,
        summary_upto_id: row.get(4)?,
        created_at: row.get(5)?,
        archived_at: row.get(6)?,
    })
}

#[derive(Debug, Clone)]
pub struct GroupMessage {
    pub id: String,
    pub group_id: String,
    pub session_id: String,
    pub speaker_persona_id: Option<i64>,
    pub content: String,
    pub timestamp: String,
    pub run_key: Option<String>,
}

fn map_group_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupMessage> {
    Ok(GroupMessage {
        id: row.get(0)?,
        group_id: row.get(1)?,
        session_id: row.get(2)?,
        speaker_persona_id: row.get(3)?,
        content: row.get(4)?,
        timestamp: row.get(5)?,
        run_key: row.get(6)?,
    })
}

fn normalize_turn_mode(mode: &str) -> Result<&str, FinallyAValueBotError> {
    match mode.trim() {
        "moderator" => Ok("moderator"),
        "roundtable" => Ok("roundtable"),
        other => Err(FinallyAValueBotError::ToolExecution(format!(
            "turn_mode must be moderator or roundtable (got '{other}')"
        ))),
    }
}
