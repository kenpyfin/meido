use chrono::{DateTime, Utc};
use cron::Schedule;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use crate::error::FinallyAValueBotError;

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
}

pub async fn call_blocking<T, F>(
    db: std::sync::Arc<Database>,
    f: F,
) -> Result<T, FinallyAValueBotError>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> Result<T, FinallyAValueBotError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(db.as_ref()))
        .await
        .map_err(|e| FinallyAValueBotError::ToolExecution(format!("DB task join error: {e}")))?
}

pub const MESSAGE_ORIGIN_INTERACTIVE: &str = "interactive";
pub const MESSAGE_ORIGIN_SCHEDULED: &str = "scheduled";
pub const MESSAGE_ORIGIN_GUEST: &str = "guest";
pub const MESSAGE_ORIGIN_ATTACHED_RUN: &str = "attached_run";
pub const MESSAGE_ORIGIN_ATTACHED_TASK: &str = "attached_task";

pub fn message_origin_interactive() -> String {
    MESSAGE_ORIGIN_INTERACTIVE.to_string()
}

pub fn message_origin_scheduled() -> String {
    MESSAGE_ORIGIN_SCHEDULED.to_string()
}

#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub session_id: Option<String>,
    pub sender_name: String,
    pub content: String,
    pub is_from_bot: bool,
    pub timestamp: String,
    /// `interactive` (default), `scheduled`, `guest`, `attached_run`, or `attached_task`.
    pub origin: String,
    /// Set when another persona spoke in this chat (guest reply).
    pub speaker_persona_id: Option<i64>,
}

/// Queued recall ingestion row. Content stays in SQLite or ORIGIN; this row stores only the hash.
#[derive(Debug, Clone)]
pub struct RecallIngestRow {
    pub source_kind: String,
    pub source_id: String,
    pub chat_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub content_hash: String,
    pub attempt_count: i64,
}

fn collect_statement_messages(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
    let mapped = stmt.query_map(params, stored_message_from_row)?;
    Ok(mapped.collect::<Result<Vec<_>, _>>()?)
}

fn stored_message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    Ok(StoredMessage {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        session_id: row.get(3)?,
        sender_name: row.get(4)?,
        content: row.get(5)?,
        is_from_bot: row.get::<_, i32>(6)? != 0,
        timestamp: row.get(7)?,
        origin: row
            .get::<_, Option<String>>(8)?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(message_origin_interactive),
        speaker_persona_id: row.get(9)?,
    })
}

const MESSAGE_SELECT_COLS: &str = "id, chat_id, persona_id, session_id, sender_name, content, is_from_bot, timestamp, origin, speaker_persona_id";

/// Main chat timeline: legacy rows plus focused sessions opted into main chat.
const MAIN_CHAT_MESSAGE_VISIBILITY: &str = "(
    session_id IS NULL
    OR EXISTS (
        SELECT 1 FROM chat_sessions cs
        WHERE cs.id = session_id AND cs.mirror_main_chat = 1
    )
)";

const PERSONA_SELECT_COLS: &str = "id, chat_id, name, model_override, recent_history_min_user, recent_history_min_assistant, operator_memo, dense_delivery_enabled, dense_delivery_messaging_max_chars, dense_delivery_web_max_chars, dense_delivery_summary_chars, agent_engine_override, gemini_adk_topology, cursor_sdk_model, cursor_sdk_model_params, llm_provider_override, llm_base_url_override";

fn persona_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Persona> {
    Ok(Persona {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        name: row.get(2)?,
        model_override: row.get(3)?,
        recent_history_min_user: row.get(4)?,
        recent_history_min_assistant: row.get(5)?,
        operator_memo: row.get(6)?,
        dense_delivery_enabled: row.get::<_, i64>(7)? != 0,
        dense_delivery_messaging_max_chars: row.get(8)?,
        dense_delivery_web_max_chars: row.get(9)?,
        dense_delivery_summary_chars: row.get(10)?,
        agent_engine_override: row.get(11)?,
        gemini_adk_topology: row.get(12)?,
        cursor_sdk_model: row.get(13)?,
        cursor_sdk_model_params: row.get(14)?,
        llm_provider_override: row.get(15)?,
        llm_base_url_override: row.get(16)?,
    })
}

const CHAT_SESSION_SELECT_COLS: &str = "id, chat_id, persona_id, title, intent, status, created_at, last_active_at, archived_at, ttl_hours, bootstrap_context_json, mirror_main_chat, seed_context, source_topic_id";

fn chat_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatSession> {
    Ok(ChatSession {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        title: row.get(3)?,
        intent: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        last_active_at: row.get(7)?,
        archived_at: row.get(8)?,
        ttl_hours: row.get(9)?,
        bootstrap_context_json: row.get(10)?,
        mirror_main_chat: row.get::<_, i64>(11)? != 0,
        seed_context: row.get(12)?,
        source_topic_id: row.get(13)?,
    })
}

#[derive(Debug, Clone)]
pub struct ChatSession {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub title: String,
    pub intent: String,
    pub status: String,
    pub created_at: String,
    pub last_active_at: String,
    pub archived_at: Option<String>,
    pub ttl_hours: i64,
    pub bootstrap_context_json: Option<String>,
    /// When true, session messages also appear on the persona main chat timeline.
    pub mirror_main_chat: bool,
    /// Bounded prior-topic context injected with the session. Not a copy of the transcript.
    pub seed_context: Option<String>,
    /// Topic the seed was built from, when this session was started from topic view.
    pub source_topic_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Persona {
    pub id: i64,
    pub chat_id: i64,
    pub name: String,
    pub model_override: Option<String>,
    /// When set, overrides global default for `trim_to_recent_balanced` minimum user messages.
    pub recent_history_min_user: Option<i64>,
    /// When set, overrides global default for `trim_to_recent_balanced` minimum assistant messages.
    pub recent_history_min_assistant: Option<i64>,
    /// Operator-authored steering note injected into the system prompt (web cockpit).
    pub operator_memo: Option<String>,
    /// When true, over-limit replies spill to PDF + public URL before channel send.
    pub dense_delivery_enabled: bool,
    /// Messaging-channel char cap override (NULL → 2000).
    pub dense_delivery_messaging_max_chars: Option<i64>,
    /// Web/other char cap override (NULL → 1000).
    pub dense_delivery_web_max_chars: Option<i64>,
    /// Summary excerpt cap override (NULL → 800).
    pub dense_delivery_summary_chars: Option<i64>,
    /// Per-persona agent engine (`classic` / `classic_cost_routing` / `cursor` / `gemini_adk`).
    /// New personas default to `classic`. NULL/empty falls back to Classic (no inherit UI).
    pub agent_engine_override: Option<String>,
    /// Optional per-persona Gemini ADK topology JSON. NULL inherits global `GEMINI_ADK_TOPOLOGY_CONFIG`.
    pub gemini_adk_topology: Option<String>,
    /// Per-persona Cursor SDK model id. NULL falls back to global `CURSOR_SDK_MODEL`.
    pub cursor_sdk_model: Option<String>,
    /// Per-persona Cursor SDK model params JSON array. NULL falls back to global params.
    pub cursor_sdk_model_params: Option<String>,
    /// Per-persona classic strategy LLM provider id. NULL falls back to global `LLM_PROVIDER`.
    pub llm_provider_override: Option<String>,
    /// Per-persona local OpenAI-compatible base URL. NULL falls back to global `LLM_BASE_URL`.
    pub llm_base_url_override: Option<String>,
}

/// Maximum `operator_memo` length (characters) for storage and prompt injection.
pub const OPERATOR_MEMO_MAX_CHARS: usize = 4000;

#[derive(Debug, Clone)]
pub struct ChatSummary {
    pub chat_id: i64,
    pub chat_title: Option<String>,
    pub chat_type: String,
    pub last_message_time: String,
    pub last_message_preview: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TaskRunLog {
    pub id: i64,
    pub task_id: i64,
    pub chat_id: i64,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: i64,
    pub success: bool,
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SocialOAuthToken {
    pub platform: String,
    pub chat_id: i64,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScheduledTask {
    pub id: i64,
    pub chat_id: i64,
    pub persona_id: i64,
    pub prompt: String,
    pub schedule_type: String,  // "cron" or "once"
    pub schedule_value: String, // cron expression or ISO timestamp
    pub next_run: String,       // ISO timestamp
    pub last_run: Option<String>,
    pub status: String, // "active", "running", "paused", "completed", "cancelled"
    pub created_at: String,
}

/// Read a column as text, tolerating rows whose value was stored with BLOB affinity
/// (e.g. a prompt hand-edited via the sqlite CLI). SQLite is dynamically typed, so a
/// single BLOB in a TEXT column otherwise makes `row.get::<String>` fail and aborts the
/// entire query (which previously took down the scheduler and Schedules API).
fn row_text(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<String> {
    use rusqlite::types::ValueRef;
    match row.get_ref(idx)? {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Text(t) => Ok(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        ValueRef::Integer(i) => Ok(i.to_string()),
        ValueRef::Real(f) => Ok(f.to_string()),
    }
}

/// Optional variant of [`row_text`]; `NULL` maps to `None`.
fn row_text_opt(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<String>> {
    use rusqlite::types::ValueRef;
    match row.get_ref(idx)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(t) => Ok(Some(String::from_utf8_lossy(t).into_owned())),
        ValueRef::Blob(b) => Ok(Some(String::from_utf8_lossy(b).into_owned())),
        ValueRef::Integer(i) => Ok(Some(i.to_string())),
        ValueRef::Real(f) => Ok(Some(f.to_string())),
    }
}

/// Shared, blob-tolerant row mapping for `scheduled_tasks` queries. All list/lookup queries
/// must select columns in this order: id, chat_id, persona_id, prompt, schedule_type,
/// schedule_value, next_run, last_run, status, created_at.
fn map_scheduled_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    Ok(ScheduledTask {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        prompt: row_text(row, 3)?,
        schedule_type: row_text(row, 4)?,
        schedule_value: row_text(row, 5)?,
        next_run: row_text(row, 6)?,
        last_run: row_text_opt(row, 7)?,
        status: row_text(row, 8)?,
        created_at: row_text(row, 9)?,
    })
}

/// External channel identity for delivery and persona policy. `0` = web (no bot token).
pub const BOT_INSTANCE_WEB: i64 = 0;
pub const BOT_INSTANCE_TELEGRAM_PRIMARY: i64 = 1;
pub const BOT_INSTANCE_DISCORD_PRIMARY: i64 = 2;
pub const BOT_INSTANCE_WHATSAPP_PRIMARY: i64 = 3;
pub const BOT_INSTANCE_WECOM_PRIMARY: i64 = 4;

/// Stable positive i64 for string platform handles (WeCom userid/chatid).
pub fn stable_canonical_chat_id_for_handle(platform: &str, handle: &str) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(platform.as_bytes());
    hasher.update([0u8]);
    hasher.update(handle.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let mut id = i64::from_be_bytes(bytes) & i64::MAX;
    if id <= BOT_INSTANCE_WECOM_PRIMARY || id == 997894126 {
        id = 1_000_000 + (id % 900_000_000);
    }
    if id <= BOT_INSTANCE_WECOM_PRIMARY || id == 997894126 {
        id = 1_000_010;
    }
    id
}

#[derive(Debug, Clone)]
pub struct ChannelBotInstance {
    pub id: i64,
    pub platform: String,
    pub label: String,
    pub token: String,
    pub bot_username: String,
    pub allowed_groups: Vec<i64>,
    pub discord_allowed_channels: Vec<u64>,
    pub whatsapp_phone_number_id: String,
    pub whatsapp_verify_token: String,
    pub whatsapp_webhook_port: u16,
    pub wecom_corp_id: String,
    pub wecom_agent_id: i64,
    pub wecom_callback_token: String,
    pub wecom_encoding_aes_key: String,
    pub wecom_webhook_port: u16,
    pub wecom_allowed_chats: String,
    pub wecom_aibot_id: String,
    pub wecom_mode: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChannelBinding {
    pub canonical_chat_id: i64,
    pub bot_instance_id: i64,
    pub channel_type: String,
    pub channel_handle: String,
}

/// Merged bot instance + per-contact binding/policy for Settings → Channels.
#[derive(Debug, Clone)]
pub struct ContactChannelIntegrationRow {
    pub bot_instance_id: i64,
    pub platform: String,
    pub label: String,
    pub channel_handle: Option<String>,
    pub linked: bool,
    pub persona_mode: ChannelPersonaMode,
    pub persona_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AppSetting {
    pub key: String,
    pub value: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct HookDefinitionRecord {
    pub id: i64,
    pub name: String,
    pub event_name: String,
    pub matcher: Option<String>,
    pub action_type: String,
    pub action_payload_json: String,
    /// `None` means global hook scope; `Some(vec![...])` means explicit persona allowlist.
    pub scoped_persona_ids: Option<Vec<i64>>,
    pub enabled: bool,
    pub updated_at: String,
}

impl HookDefinitionRecord {
    pub fn scoped_for_persona(&self, persona_id: i64) -> bool {
        self.scoped_persona_ids
            .as_ref()
            .map_or(true, |ids| ids.contains(&persona_id))
    }

    pub fn persona_status(
        &self,
        db: &Database,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<(bool, bool, bool), FinallyAValueBotError> {
        let scoped_for_persona = self.scoped_for_persona(persona_id);
        let allowed_for_persona = db.is_hook_allowed_for_persona(chat_id, persona_id, self.id)?;
        let active_for_persona = self.enabled && scoped_for_persona && allowed_for_persona;
        Ok((scoped_for_persona, allowed_for_persona, active_for_persona))
    }
}

#[derive(Debug, Clone)]
pub struct PersonaHookSkillPolicy {
    pub chat_id: i64,
    pub persona_id: i64,
    /// `None` means default allow-all. `Some(vec![])` means explicit allow-none.
    pub allowed_hook_ids: Option<Vec<i64>>,
    /// `None` means default allow-all. `Some(vec![])` means explicit allow-none.
    pub allowed_skill_names: Option<Vec<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPersonaMode {
    All,
    Single,
}

impl ChannelPersonaMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Single => "single",
        }
    }
}

impl TryFrom<&str> for ChannelPersonaMode {
    type Error = FinallyAValueBotError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "all" => Ok(Self::All),
            "single" => Ok(Self::Single),
            other => Err(FinallyAValueBotError::ToolExecution(format!(
                "Invalid channel persona mode: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelPersonaPolicy {
    pub canonical_chat_id: i64,
    pub bot_instance_id: i64,
    pub mode: ChannelPersonaMode,
    pub persona_id: Option<i64>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct BackgroundJob {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub prompt: String,
    pub status: String, // "pending", "running", "completed_raw", "main_agent_processing", "done", "failed", "cancelled"
    pub trigger_reason: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub result_text: Option<String>,
    pub error_text: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub last_progress_at: Option<String>,
    pub last_stage: Option<String>,
    /// `agent` (default) or `shell`.
    pub job_kind: String,
    pub shell_command: Option<String>,
    pub workdir: Option<String>,
    pub tmux_session: Option<String>,
    pub output_path: Option<String>,
    pub exit_code: Option<i32>,
    pub label: Option<String>,
    /// Focused web session this job belongs to (`None` = main chat).
    pub session_id: Option<String>,
}

const BG_JOB_SELECT: &str = "SELECT id, chat_id, persona_id, prompt, status, trigger_reason, created_at, started_at, finished_at, result_text, error_text, lease_owner, lease_expires_at, last_progress_at, last_stage, job_kind, shell_command, workdir, tmux_session, output_path, exit_code, label, session_id";

fn map_background_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackgroundJob> {
    Ok(BackgroundJob {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        prompt: row.get(3)?,
        status: row.get(4)?,
        trigger_reason: row.get(5)?,
        created_at: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        result_text: row.get(9)?,
        error_text: row.get(10)?,
        lease_owner: row.get(11)?,
        lease_expires_at: row.get(12)?,
        last_progress_at: row.get(13)?,
        last_stage: row.get(14)?,
        job_kind: row
            .get::<_, Option<String>>(15)?
            .unwrap_or_else(|| "agent".into()),
        shell_command: row.get(16)?,
        workdir: row.get(17)?,
        tmux_session: row.get(18)?,
        output_path: row.get(19)?,
        exit_code: row.get(20)?,
        label: row.get(21)?,
        session_id: row.get(22)?,
    })
}

#[derive(Debug, Clone)]
pub struct JobHeartbeat {
    pub run_key: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub job_type: String,
    pub stage: String,
    pub message: String,
    pub active: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub id: i64,
    pub owner_chat_id: i64,
    pub title: String,
    pub project_type: String,
    pub status: String,
    pub canonical_path: Option<String>,
    pub metadata_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct RunTimelineEvent {
    pub id: i64,
    pub run_key: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PersonaBulletinEvent {
    pub id: i64,
    pub chat_id: i64,
    pub persona_id: i64,
    pub run_key: Option<String>,
    pub event_type: String,
    pub title: String,
    pub detail: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PersonaBulletinFocus {
    pub chat_id: i64,
    pub persona_id: i64,
    pub title: Option<String>,
    pub content: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct PersonaMessageBookmark {
    pub chat_id: i64,
    pub persona_id: i64,
    pub message_id: String,
    pub role: String,
    pub content_preview: String,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Session of the underlying message when it still exists; `None` for main chat or missing.
    pub session_id: Option<String>,
    /// Timestamp of the underlying message when it still exists.
    pub message_timestamp: Option<String>,
    /// True when the bookmarked message row was deleted.
    pub missing: bool,
}

/// One replace-in-place page of a topic dossier. The message log stays canonical.
#[derive(Debug, Clone)]
pub struct TopicDossierWindow {
    pub messages: Vec<StoredMessage>,
    pub has_older: bool,
    pub has_newer: bool,
    pub bookmarks: Vec<PersonaMessageBookmark>,
}

/// Which page of a topic to load. Cursor ids that are outside the topic resolve to no window.
#[derive(Debug, Clone)]
pub enum DossierPageRequest {
    Latest,
    Before(String),
    After(String),
    Around(String),
}

/// Paginated message window with cursor flags for older/newer pages.
#[derive(Debug, Clone)]
pub struct MessagePage {
    pub messages: Vec<StoredMessage>,
    pub has_older: bool,
    pub has_newer: bool,
}

/// Side chat (sub-thread) anchored to an assistant message; turns are not main timeline.
#[derive(Debug, Clone)]
pub struct SideChat {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    /// Empty string = main chat; otherwise focused session id.
    pub session_id: String,
    pub anchor_message_id: String,
    pub title: String,
    pub anchor_snippet: String,
    pub draft_text: String,
    pub turn_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SideChatTurn {
    pub id: String,
    pub side_chat_id: String,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

pub const SIDE_CHAT_MAX_TURNS: usize = 40;
pub const TOPIC_DOSSIER_DEFAULT_LIMIT: usize = 20;
pub const TOPIC_DOSSIER_MAX_LIMIT: usize = 40;
pub const TOPIC_DOSSIER_BOOKMARK_LIMIT: usize = 20;
pub const TOPIC_DOSSIER_ARTIFACT_LIMIT: usize = 24;

/// One titled span of a chat. Messages stay linear; this row is the outline node.
#[derive(Debug, Clone)]
pub struct ConversationTopic {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    /// `None` is the main chat. A focused session stores its id.
    pub session_id: Option<String>,
    pub parent_id: Option<String>,
    pub continues_topic_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub start_message_id: String,
    pub end_message_id: String,
    pub start_ts: String,
    pub end_ts: String,
    pub message_count: i64,
    /// `open` (the tail that may still grow) or `closed`.
    pub status: String,
    /// `auto` (segmenter) or `user` (rename, pin, merge, split). User rows are frozen.
    pub source: String,
    pub pinned: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Replacement rows written by the segmenter. `source` is always `auto`.
#[derive(Debug, Clone)]
pub struct ConversationTopicDraft {
    pub title: String,
    pub summary: String,
    pub start_message_id: String,
    pub end_message_id: String,
    pub start_ts: String,
    pub end_ts: String,
    pub message_count: i64,
    pub status: String,
}

/// Chat/persona/session the topic tree is scoped to. Empty session means main chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicScope {
    pub chat_id: i64,
    pub persona_id: i64,
    pub session_id: Option<String>,
}

/// Where a segmentation window starts inside a scope.
#[derive(Debug, Clone)]
pub enum TopicWindowCursor {
    Beginning,
    Inclusive { timestamp: String, id: String },
    Exclusive { timestamp: String, id: String },
}

#[derive(Debug, Clone)]
pub struct TopicWindowMessage {
    pub id: String,
    pub timestamp: String,
    pub is_from_bot: bool,
    pub content: String,
}

/// Side chat row plus the anchor message time, for attaching a branch under a topic.
#[derive(Debug, Clone)]
pub struct SideChatBranch {
    pub id: String,
    pub session_id: String,
    pub anchor_message_id: String,
    pub title: String,
    pub turn_count: i64,
    pub anchor_timestamp: Option<String>,
}

const TOPIC_SELECT_COLS: &str = "id, chat_id, persona_id, session_id, parent_id, continues_topic_id, title, summary, start_message_id, end_message_id, start_ts, end_ts, message_count, status, source, pinned, created_at, updated_at";

fn conversation_topic_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationTopic> {
    Ok(ConversationTopic {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        session_id: row.get(3)?,
        parent_id: row.get(4)?,
        continues_topic_id: row.get(5)?,
        title: row.get(6)?,
        summary: row.get(7)?,
        start_message_id: row.get(8)?,
        end_message_id: row.get(9)?,
        start_ts: row.get(10)?,
        end_ts: row.get(11)?,
        message_count: row.get(12)?,
        status: row.get(13)?,
        source: row.get(14)?,
        pinned: row.get::<_, i64>(15)? != 0,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

pub fn normalize_topic_session(session_id: Option<&str>) -> Option<String> {
    session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Operator action item created by the agent (web Inbox); not Tier 3 memory.
#[derive(Debug, Clone)]
pub struct PersonaTodo {
    pub id: i64,
    pub chat_id: i64,
    pub persona_id: i64,
    pub title: String,
    pub status: String,
    pub source_hint: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub description: Option<String>,
    pub owner_persona_id: Option<i64>,
    pub priority: i64,
    pub due_at: Option<String>,
    pub blocked_reason: Option<String>,
    pub position: f64,
    pub source_kind: String,
    pub source_ref: Option<String>,
}

pub(crate) const PERSONA_TODO_COLS: &str = "id, chat_id, persona_id, title, status, source_hint, created_at, updated_at, completed_at, description, owner_persona_id, priority, due_at, blocked_reason, position, source_kind, source_ref";

pub(crate) fn map_persona_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonaTodo> {
    Ok(PersonaTodo {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        persona_id: row.get(2)?,
        title: row.get(3)?,
        status: row.get(4)?,
        source_hint: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        completed_at: row.get(8)?,
        description: row.get(9)?,
        owner_persona_id: row.get(10)?,
        priority: row.get(11).unwrap_or(0),
        due_at: row.get(12)?,
        blocked_reason: row.get(13)?,
        position: row.get(14).unwrap_or(0.0),
        source_kind: row
            .get::<_, Option<String>>(15)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "manual".into()),
        source_ref: row.get(16)?,
    })
}

pub(crate) fn normalize_board_status(status: &str) -> Result<&str, FinallyAValueBotError> {
    match status.trim() {
        "open" | "todo" => Ok("todo"),
        "doing" => Ok("doing"),
        "blocked" => Ok("blocked"),
        "done" => Ok("done"),
        other => Err(FinallyAValueBotError::ToolExecution(format!(
            "Todo status must be open, todo, doing, blocked, or done (got '{other}')"
        ))),
    }
}

/// Latest bot message metadata for inbox / unread indicators.
#[derive(Debug, Clone)]
pub struct PersonaLastBotInfo {
    pub persona_id: i64,
    pub last_bot_message_at: String,
    pub session_id: Option<String>,
    pub session_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CursorAgentRun {
    pub id: i64,
    pub chat_id: i64,
    pub channel: String,
    pub prompt_preview: String,
    pub workdir: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output_preview: Option<String>,
    pub output_path: Option<String>,
    /// When set, run was spawned in tmux (detach=true); session may still be running.
    pub tmux_session: Option<String>,
}

/// Later of two RFC3339 timestamps. Unparseable values lose to a parseable one.
fn later_rfc3339(left: &str, right: &str) -> String {
    let left_dt = DateTime::parse_from_rfc3339(left).ok();
    let right_dt = DateTime::parse_from_rfc3339(right).ok();
    match (left_dt, right_dt) {
        (Some(a), Some(b)) => {
            if b >= a {
                right.to_string()
            } else {
                left.to_string()
            }
        }
        (None, Some(_)) => right.to_string(),
        _ => left.to_string(),
    }
}

/// SQLite file name under the runtime data directory.
pub const DB_FILE_NAME: &str = "meido.db";
/// File name used before the product rename; adopted on first open, never written.
const LEGACY_DB_FILE_NAME: &str = "finally_a_value_bot.db";

/// Move a pre-rename database (and its WAL/SHM sidecars) to `DB_FILE_NAME`.
///
/// Runs only when the current file is absent and the legacy one exists, so a fresh
/// install and an already-migrated install are both no-ops.
fn adopt_legacy_db_file(data_dir: &Path) -> std::io::Result<()> {
    if data_dir.join(DB_FILE_NAME).exists() || !data_dir.join(LEGACY_DB_FILE_NAME).exists() {
        return Ok(());
    }
    for suffix in ["", "-wal", "-shm"] {
        let from = data_dir.join(format!("{LEGACY_DB_FILE_NAME}{suffix}"));
        if from.exists() {
            std::fs::rename(&from, data_dir.join(format!("{DB_FILE_NAME}{suffix}")))?;
        }
    }
    tracing::info!(
        "Renamed database {LEGACY_DB_FILE_NAME} to {DB_FILE_NAME} in {}",
        data_dir.display()
    );
    Ok(())
}

impl Database {
    pub fn new(data_dir: &str) -> Result<Self, FinallyAValueBotError> {
        let data_root = Path::new(data_dir);
        std::fs::create_dir_all(data_root)?;
        adopt_legacy_db_file(data_root)?;
        let db_path = data_root.join(DB_FILE_NAME);

        let conn = Connection::open(db_path)?;
        // PRAGMA journal_mode returns a row; use query_row to consume it (execute_batch fails with extra_check)
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        // Overwrite freed pages so replaced secret text does not linger in the database file.
        let _: i64 = conn.query_row("PRAGMA secure_delete=ON", [], |r| r.get(0))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chats (
                chat_id INTEGER PRIMARY KEY,
                chat_title TEXT,
                chat_type TEXT NOT NULL DEFAULT 'private',
                last_message_time TEXT NOT NULL,
                active_persona_id INTEGER
            );

            CREATE TABLE IF NOT EXISTS personas (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                model_override TEXT,
                UNIQUE(chat_id, name)
            );

            CREATE INDEX IF NOT EXISTS idx_personas_chat_id
                ON personas(chat_id);

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                sender_name TEXT NOT NULL,
                content TEXT NOT NULL,
                is_from_bot INTEGER NOT NULL DEFAULT 0,
                timestamp TEXT NOT NULL,
                origin TEXT NOT NULL DEFAULT 'interactive',
                PRIMARY KEY (id, chat_id, persona_id)
            );

            CREATE INDEX IF NOT EXISTS idx_messages_chat_timestamp
                ON messages(chat_id, persona_id, timestamp);

            CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                prompt TEXT NOT NULL,
                schedule_type TEXT NOT NULL DEFAULT 'cron',
                schedule_value TEXT NOT NULL,
                next_run TEXT NOT NULL,
                last_run TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_status_next
                ON scheduled_tasks(status, next_run);

            CREATE TABLE IF NOT EXISTS task_run_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL,
                chat_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                success INTEGER NOT NULL DEFAULT 1,
                result_summary TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_task_run_logs_task_id
                ON task_run_logs(task_id);

            CREATE TABLE IF NOT EXISTS sessions (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                messages_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );

            CREATE TABLE IF NOT EXISTS social_oauth_tokens (
                platform TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                access_token TEXT NOT NULL,
                refresh_token TEXT,
                expires_at TEXT,
                PRIMARY KEY (platform, chat_id)
            );

            CREATE TABLE IF NOT EXISTS oauth_pending_states (
                state_token TEXT PRIMARY KEY,
                platform TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                expires_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cursor_agent_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                channel TEXT NOT NULL,
                prompt_preview TEXT NOT NULL,
                workdir TEXT,
                started_at TEXT NOT NULL,
                finished_at TEXT NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER,
                output_preview TEXT,
                output_path TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_cursor_agent_runs_chat_id
                ON cursor_agent_runs(chat_id);
            CREATE INDEX IF NOT EXISTS idx_cursor_agent_runs_finished_at
                ON cursor_agent_runs(finished_at DESC);

            CREATE TABLE IF NOT EXISTS background_jobs (
                id TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                trigger_reason TEXT NOT NULL DEFAULT 'timeout',
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                result_text TEXT,
                error_text TEXT,
                lease_owner TEXT,
                lease_expires_at TEXT,
                last_progress_at TEXT,
                last_stage TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_background_jobs_chat_id
                ON background_jobs(chat_id);
            CREATE INDEX IF NOT EXISTS idx_background_jobs_status
                ON background_jobs(status);

            CREATE TABLE IF NOT EXISTS job_heartbeats (
                run_key TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                job_type TEXT NOT NULL,
                stage TEXT NOT NULL,
                message TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_job_heartbeats_chat_id
                ON job_heartbeats(chat_id);
            CREATE INDEX IF NOT EXISTS idx_job_heartbeats_updated
                ON job_heartbeats(updated_at DESC);

            CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_chat_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                project_type TEXT NOT NULL DEFAULT 'general',
                status TEXT NOT NULL DEFAULT 'active',
                canonical_path TEXT,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                updated_at TEXT NOT NULL,
                UNIQUE(owner_chat_id, title)
            );
            CREATE INDEX IF NOT EXISTS idx_projects_owner_updated
                ON projects(owner_chat_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS project_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                run_key TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(project_id, run_key)
            );
            CREATE INDEX IF NOT EXISTS idx_project_runs_run_key
                ON project_runs(run_key);

            CREATE TABLE IF NOT EXISTS workflows (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_chat_id INTEGER NOT NULL,
                intent_signature TEXT NOT NULL,
                steps_json TEXT NOT NULL,
                step_trace_json TEXT NOT NULL DEFAULT '[]',
                approach_summary TEXT NOT NULL DEFAULT '',
                last_outcome TEXT NOT NULL DEFAULT 'unknown',
                failure_reason TEXT,
                evidence_json TEXT NOT NULL DEFAULT '[]',
                confidence REAL NOT NULL DEFAULT 0.0,
                version INTEGER NOT NULL DEFAULT 1,
                success_count INTEGER NOT NULL DEFAULT 0,
                failure_count INTEGER NOT NULL DEFAULT 0,
                last_used_at TEXT,
                updated_at TEXT NOT NULL,
                UNIQUE(owner_chat_id, intent_signature)
            );
            CREATE INDEX IF NOT EXISTS idx_workflows_owner_conf
                ON workflows(owner_chat_id, confidence DESC, updated_at DESC);

            CREATE TABLE IF NOT EXISTS workflow_executions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_id INTEGER NOT NULL,
                run_key TEXT NOT NULL,
                outcome TEXT NOT NULL,
                score REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_workflow_executions_workflow
                ON workflow_executions(workflow_id, created_at DESC);

            CREATE TABLE IF NOT EXISTS run_timeline_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_key TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_run_timeline_events_run_key
                ON run_timeline_events(run_key, id ASC);
            CREATE INDEX IF NOT EXISTS idx_run_timeline_events_chat
                ON run_timeline_events(chat_id, created_at DESC);

            CREATE TABLE IF NOT EXISTS channel_bindings (
                canonical_chat_id INTEGER NOT NULL,
                channel_type TEXT NOT NULL,
                channel_handle TEXT NOT NULL,
                PRIMARY KEY (channel_type, channel_handle),
                FOREIGN KEY (canonical_chat_id) REFERENCES chats(chat_id)
            );
            CREATE INDEX IF NOT EXISTS idx_channel_bindings_canonical
                ON channel_bindings(canonical_chat_id);

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS hook_definitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                event_name TEXT NOT NULL,
                matcher TEXT,
                action_type TEXT NOT NULL,
                action_payload_json TEXT NOT NULL DEFAULT '{}',
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hook_definitions_event_enabled
                ON hook_definitions(event_name, enabled, id);

            CREATE TABLE IF NOT EXISTS persona_hook_skill_policy (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                allowed_hook_ids_json TEXT,
                allowed_skill_names_json TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );
            CREATE INDEX IF NOT EXISTS idx_persona_hook_skill_policy_chat
                ON persona_hook_skill_policy(chat_id);

            CREATE TABLE IF NOT EXISTS channel_persona_policy (
                canonical_chat_id INTEGER NOT NULL,
                channel_type TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'all',
                persona_id INTEGER,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (canonical_chat_id, channel_type)
            );
            CREATE INDEX IF NOT EXISTS idx_channel_persona_policy_chat
                ON channel_persona_policy(canonical_chat_id);",
        )?;

        Self::migrate_persona_schema(&conn)?;
        Self::migrate_scheduled_tasks_persona_schema(&conn)?;
        Self::migrate_channel_bindings(&conn)?;
        Self::migrate_channel_bot_instances_and_policy(&conn)?;
        Self::migrate_fts(&conn)?;
        Self::migrate_cursor_agent_runs_tmux(&conn)?;
        Self::migrate_drop_project_artifacts(&conn)?;
        Self::migrate_persona_bulletin_and_bookmarks(&conn)?;
        Self::migrate_side_chats_schema(&conn)?;
        Self::migrate_persona_todos(&conn)?;
        Self::migrate_persona_read_cursors(&conn)?;
        Self::migrate_workflow_learning_schema(&conn)?;
        Self::migrate_background_jobs_lease_schema(&conn)?;
        Self::migrate_background_jobs_shell_schema(&conn)?;
        Self::migrate_background_jobs_session_schema(&conn)?;
        Self::migrate_personas_prompt_context(&conn)?;
        Self::migrate_personas_dense_delivery_and_engine(&conn)?;
        Self::migrate_gemini_adk_and_retire_deterministic(&conn)?;
        Self::migrate_persona_explicit_engine_and_cursor_model(&conn)?;
        Self::migrate_persona_llm_selection(&conn)?;
        Self::migrate_hook_policy_schema(&conn)?;
        Self::ensure_builtin_hook_definitions(&conn)?;
        Self::migrate_chat_sessions_schema(&conn)?;
        Self::migrate_messages_origin(&conn)?;
        Self::migrate_cursor_engine_agents(&conn)?;
        Self::migrate_conversation_topics(&conn)?;
        Self::migrate_recall_ingest(&conn)?;
        crate::db_collab::migrate(&conn)?;

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    fn column_exists(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let pragma = format!("PRAGMA table_info({table})");
        let mut stmt = conn
            .prepare(&pragma)
            .map_err(|e| FinallyAValueBotError::ToolExecution(format!("pragma {table}: {e}")))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for r in rows {
            if r? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn parse_csv_i64(raw: &str) -> Vec<i64> {
        raw.split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect()
    }

    fn parse_csv_u64(raw: &str) -> Vec<u64> {
        raw.split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect()
    }

    fn format_csv_i64(ids: &[i64]) -> String {
        ids.iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn format_csv_u64(ids: &[u64]) -> String {
        ids.iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn setting_value_conn(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM app_settings WHERE lower(key) = lower(?1)",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .map(|v| v.trim().to_string())
    }

    fn channel_bot_instance_select() -> &'static str {
        "SELECT id, platform, label, token, bot_username, allowed_groups, discord_allowed_channels,
                whatsapp_phone_number_id, whatsapp_verify_token, whatsapp_webhook_port,
                wecom_corp_id, wecom_agent_id, wecom_callback_token, wecom_encoding_aes_key,
                wecom_webhook_port, wecom_allowed_chats, wecom_aibot_id, wecom_mode, created_at
         FROM channel_bot_instances"
    }

    fn map_channel_bot_instance_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<ChannelBotInstance> {
        let whatsapp_port_raw: i64 = row.get(9)?;
        let whatsapp_webhook_port = u16::try_from(whatsapp_port_raw)
            .ok()
            .filter(|p| *p > 0)
            .unwrap_or(8080);
        let wecom_port_raw: i64 = row.get(14)?;
        let wecom_webhook_port = u16::try_from(wecom_port_raw)
            .ok()
            .filter(|p| *p > 0)
            .unwrap_or(8081);
        let allowed_groups_raw: String = row.get(5)?;
        let discord_allowed_raw: String = row.get(6)?;
        Ok(ChannelBotInstance {
            id: row.get(0)?,
            platform: row.get(1)?,
            label: row.get(2)?,
            token: row.get(3)?,
            bot_username: row.get(4)?,
            allowed_groups: Self::parse_csv_i64(&allowed_groups_raw),
            discord_allowed_channels: Self::parse_csv_u64(&discord_allowed_raw),
            whatsapp_phone_number_id: row.get(7)?,
            whatsapp_verify_token: row.get(8)?,
            whatsapp_webhook_port,
            wecom_corp_id: row.get(10)?,
            wecom_agent_id: row.get(11)?,
            wecom_callback_token: row.get(12)?,
            wecom_encoding_aes_key: row.get(13)?,
            wecom_webhook_port,
            wecom_allowed_chats: row.get(15)?,
            wecom_aibot_id: row.get(16)?,
            wecom_mode: row.get(17)?,
            created_at: row.get(18)?,
        })
    }

    fn migrate_channel_bot_instances_and_policy(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_bot_instances (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                platform TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                token TEXT NOT NULL,
                bot_username TEXT NOT NULL DEFAULT '',
                allowed_groups TEXT NOT NULL DEFAULT '',
                discord_allowed_channels TEXT NOT NULL DEFAULT '',
                whatsapp_phone_number_id TEXT NOT NULL DEFAULT '',
                whatsapp_verify_token TEXT NOT NULL DEFAULT '',
                whatsapp_webhook_port INTEGER NOT NULL DEFAULT 8080,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_channel_bot_instances_platform
                ON channel_bot_instances(platform);",
        )?;
        for (column, definition) in [
            ("bot_username", "TEXT NOT NULL DEFAULT ''"),
            ("allowed_groups", "TEXT NOT NULL DEFAULT ''"),
            ("discord_allowed_channels", "TEXT NOT NULL DEFAULT ''"),
            ("whatsapp_phone_number_id", "TEXT NOT NULL DEFAULT ''"),
            ("whatsapp_verify_token", "TEXT NOT NULL DEFAULT ''"),
            ("whatsapp_webhook_port", "INTEGER NOT NULL DEFAULT 8080"),
            ("wecom_corp_id", "TEXT NOT NULL DEFAULT ''"),
            ("wecom_agent_id", "INTEGER NOT NULL DEFAULT 0"),
            ("wecom_callback_token", "TEXT NOT NULL DEFAULT ''"),
            ("wecom_encoding_aes_key", "TEXT NOT NULL DEFAULT ''"),
            ("wecom_webhook_port", "INTEGER NOT NULL DEFAULT 8081"),
            ("wecom_allowed_chats", "TEXT NOT NULL DEFAULT ''"),
            ("wecom_aibot_id", "TEXT NOT NULL DEFAULT ''"),
            ("wecom_mode", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !Self::column_exists(conn, "channel_bot_instances", column)? {
                conn.execute(
                    &format!("ALTER TABLE channel_bot_instances ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }

        Self::backfill_channel_instance_options_from_app_settings(conn)?;

        let has_bi = Self::column_exists(conn, "channel_bindings", "bot_instance_id")?;
        if !has_bi {
            conn.execute_batch(
                "CREATE TABLE channel_bindings_new (
                    bot_instance_id INTEGER NOT NULL,
                    canonical_chat_id INTEGER NOT NULL,
                    channel_type TEXT NOT NULL,
                    channel_handle TEXT NOT NULL,
                    PRIMARY KEY (bot_instance_id, channel_type, channel_handle)
                );
                CREATE INDEX IF NOT EXISTS idx_channel_bindings_new_canonical
                    ON channel_bindings_new(canonical_chat_id);",
            )?;
            conn.execute(
                "INSERT INTO channel_bindings_new (bot_instance_id, canonical_chat_id, channel_type, channel_handle)
                 SELECT
                    CASE channel_type
                        WHEN 'web' THEN 0
                        WHEN 'telegram' THEN 1
                        WHEN 'discord' THEN 2
                        WHEN 'whatsapp' THEN 3
                        ELSE 1
                    END,
                    canonical_chat_id, channel_type, channel_handle
                 FROM channel_bindings",
                [],
            )?;
            conn.execute_batch(
                "DROP TABLE channel_bindings;
                 ALTER TABLE channel_bindings_new RENAME TO channel_bindings;
                 CREATE INDEX IF NOT EXISTS idx_channel_bindings_canonical
                     ON channel_bindings(canonical_chat_id);",
            )?;
        }

        let policy_has_bot =
            Self::column_exists(conn, "channel_persona_policy", "bot_instance_id")?;
        if !policy_has_bot {
            let has_channel_type =
                Self::column_exists(conn, "channel_persona_policy", "channel_type")?;
            if has_channel_type {
                conn.execute_batch(
                    "CREATE TABLE channel_persona_policy_new (
                        canonical_chat_id INTEGER NOT NULL,
                        bot_instance_id INTEGER NOT NULL,
                        mode TEXT NOT NULL DEFAULT 'all',
                        persona_id INTEGER,
                        updated_at TEXT NOT NULL,
                        PRIMARY KEY (canonical_chat_id, bot_instance_id)
                    );
                    CREATE INDEX IF NOT EXISTS idx_channel_persona_policy_new_chat
                        ON channel_persona_policy_new(canonical_chat_id);",
                )?;
                conn.execute(
                    "INSERT INTO channel_persona_policy_new (canonical_chat_id, bot_instance_id, mode, persona_id, updated_at)
                     SELECT canonical_chat_id,
                        CASE channel_type
                            WHEN 'telegram' THEN 1
                            WHEN 'discord' THEN 2
                            WHEN 'whatsapp' THEN 3
                            ELSE 0
                        END,
                        mode, persona_id, updated_at
                     FROM channel_persona_policy
                     WHERE channel_type IN ('telegram', 'discord', 'whatsapp')",
                    [],
                )?;
                conn.execute_batch(
                    "DROP TABLE channel_persona_policy;
                     ALTER TABLE channel_persona_policy_new RENAME TO channel_persona_policy;
                     CREATE INDEX IF NOT EXISTS idx_channel_persona_policy_chat
                         ON channel_persona_policy(canonical_chat_id);",
                )?;
            }
        }

        Self::migrate_misplaced_channel_bot_instance_ids(conn)?;

        Ok(())
    }

    fn backfill_channel_instance_options_from_app_settings(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if let Some(v) = Self::setting_value_conn(conn, "BOT_USERNAME").filter(|v| !v.is_empty()) {
            conn.execute(
                "UPDATE channel_bot_instances
                 SET bot_username = ?1
                 WHERE id = ?2 AND platform = 'telegram' AND bot_username = ''",
                params![v, BOT_INSTANCE_TELEGRAM_PRIMARY],
            )?;
        }
        if let Some(v) = Self::setting_value_conn(conn, "ALLOWED_GROUPS") {
            conn.execute(
                "UPDATE channel_bot_instances
                 SET allowed_groups = ?1
                 WHERE id = ?2 AND platform = 'telegram' AND allowed_groups = ''",
                params![v, BOT_INSTANCE_TELEGRAM_PRIMARY],
            )?;
        }
        if let Some(v) = Self::setting_value_conn(conn, "DISCORD_ALLOWED_CHANNELS") {
            conn.execute(
                "UPDATE channel_bot_instances
                 SET discord_allowed_channels = ?1
                 WHERE id = ?2 AND platform = 'discord' AND discord_allowed_channels = ''",
                params![v, BOT_INSTANCE_DISCORD_PRIMARY],
            )?;
        }
        if let Some(v) =
            Self::setting_value_conn(conn, "WHATSAPP_PHONE_NUMBER_ID").filter(|v| !v.is_empty())
        {
            conn.execute(
                "UPDATE channel_bot_instances
                 SET whatsapp_phone_number_id = ?1
                 WHERE id = ?2 AND platform = 'whatsapp' AND whatsapp_phone_number_id = ''",
                params![v, BOT_INSTANCE_WHATSAPP_PRIMARY],
            )?;
        }
        if let Some(v) =
            Self::setting_value_conn(conn, "WHATSAPP_VERIFY_TOKEN").filter(|v| !v.is_empty())
        {
            conn.execute(
                "UPDATE channel_bot_instances
                 SET whatsapp_verify_token = ?1
                 WHERE id = ?2 AND platform = 'whatsapp' AND whatsapp_verify_token = ''",
                params![v, BOT_INSTANCE_WHATSAPP_PRIMARY],
            )?;
        }
        if let Some(v) =
            Self::setting_value_conn(conn, "WHATSAPP_WEBHOOK_PORT").filter(|v| !v.is_empty())
        {
            if let Ok(port) = v.parse::<u16>() {
                if port > 0 {
                    conn.execute(
                        "UPDATE channel_bot_instances
                         SET whatsapp_webhook_port = ?1
                         WHERE id = ?2 AND platform = 'whatsapp' AND whatsapp_webhook_port = 8080",
                        params![port, BOT_INSTANCE_WHATSAPP_PRIMARY],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Reassign extra bot rows that occupy reserved primary ids 2–4.
    fn migrate_misplaced_channel_bot_instance_ids(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        let expected: [(i64, &str); 3] = [
            (BOT_INSTANCE_DISCORD_PRIMARY, "discord"),
            (BOT_INSTANCE_WHATSAPP_PRIMARY, "whatsapp"),
            (BOT_INSTANCE_WECOM_PRIMARY, "wecom"),
        ];
        for (reserved_id, expected_platform) in expected {
            let platform: Option<String> = conn
                .query_row(
                    "SELECT platform FROM channel_bot_instances WHERE id = ?1",
                    params![reserved_id],
                    |row| row.get(0),
                )
                .ok();
            let Some(actual) = platform else {
                continue;
            };
            if actual == expected_platform {
                continue;
            }
            let new_id = Self::next_extra_bot_instance_id_conn(conn)?;
            Self::reassign_channel_bot_instance_id_conn(conn, reserved_id, new_id)?;
            tracing::info!(
                "Migrated channel_bot_instances id {} ({}) -> id {} (reserved for {})",
                reserved_id,
                actual,
                new_id,
                expected_platform
            );
        }
        Ok(())
    }

    fn next_extra_bot_instance_id_conn(conn: &Connection) -> Result<i64, FinallyAValueBotError> {
        let max_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM channel_bot_instances",
            [],
            |row| row.get(0),
        )?;
        Ok(max_id.max(BOT_INSTANCE_WECOM_PRIMARY) + 1)
    }

    fn reassign_channel_bot_instance_id_conn(
        conn: &Connection,
        old_id: i64,
        new_id: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let inst = conn.query_row(
            &format!("{} WHERE id = ?1", Self::channel_bot_instance_select()),
            params![old_id],
            Self::map_channel_bot_instance_row,
        )?;
        conn.execute(
            "INSERT INTO channel_bot_instances (
                id, platform, label, token, bot_username, allowed_groups, discord_allowed_channels,
                whatsapp_phone_number_id, whatsapp_verify_token, whatsapp_webhook_port,
                wecom_corp_id, wecom_agent_id, wecom_callback_token, wecom_encoding_aes_key,
                wecom_webhook_port, wecom_allowed_chats, wecom_aibot_id, wecom_mode, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                new_id,
                inst.platform,
                inst.label,
                inst.token,
                inst.bot_username,
                Self::format_csv_i64(&inst.allowed_groups),
                Self::format_csv_u64(&inst.discord_allowed_channels),
                inst.whatsapp_phone_number_id,
                inst.whatsapp_verify_token,
                inst.whatsapp_webhook_port as i64,
                inst.wecom_corp_id,
                inst.wecom_agent_id,
                inst.wecom_callback_token,
                inst.wecom_encoding_aes_key,
                inst.wecom_webhook_port as i64,
                inst.wecom_allowed_chats,
                inst.wecom_aibot_id,
                inst.wecom_mode,
                inst.created_at
            ],
        )?;
        conn.execute(
            "UPDATE channel_bindings SET bot_instance_id = ?1 WHERE bot_instance_id = ?2",
            params![new_id, old_id],
        )?;
        conn.execute(
            "UPDATE channel_persona_policy SET bot_instance_id = ?1 WHERE bot_instance_id = ?2",
            params![new_id, old_id],
        )?;
        conn.execute(
            "DELETE FROM channel_bot_instances WHERE id = ?1",
            params![old_id],
        )?;
        Ok(())
    }

    fn migrate_persona_schema(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        // Check if messages has persona_id (new schema)
        let has_persona = conn
            .prepare("PRAGMA table_info(messages)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
                Ok(rows.filter_map(|r| r.ok()).any(|c| c == "persona_id"))
            })
            .unwrap_or(false);

        if has_persona {
            return Ok(());
        }

        // Add active_persona_id to chats if missing
        let has_active = conn
            .prepare("PRAGMA table_info(chats)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
                Ok(rows
                    .filter_map(|r| r.ok())
                    .any(|c| c == "active_persona_id"))
            })
            .unwrap_or(false);
        if !has_active {
            conn.execute("ALTER TABLE chats ADD COLUMN active_persona_id INTEGER", [])?;
        }

        // Create personas table if not exists (might not exist in very old DB)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS personas (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                model_override TEXT,
                UNIQUE(chat_id, name)
            );
            CREATE INDEX IF NOT EXISTS idx_personas_chat_id ON personas(chat_id);",
        )?;

        // Collect all chat_ids
        let chat_ids: Vec<i64> = {
            let mut out = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT chat_id FROM chats UNION SELECT chat_id FROM sessions UNION SELECT chat_id FROM messages",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            for r in rows {
                if let Ok(id) = r {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
            out
        };

        // Create default persona for each chat, set active
        let now = chrono::Utc::now().to_rfc3339();
        for cid in &chat_ids {
            conn.execute(
                "INSERT OR IGNORE INTO chats (chat_id, chat_title, chat_type, last_message_time, active_persona_id)
                 VALUES (?1, NULL, 'private', ?2, NULL)",
                params![cid, now],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO personas (chat_id, name, model_override) VALUES (?1, 'default', NULL)",
                params![cid],
            )?;
            let persona_id: i64 = conn.query_row(
                "SELECT id FROM personas WHERE chat_id = ?1 AND name = 'default'",
                params![cid],
                |row| row.get(0),
            )?;
            conn.execute(
                "UPDATE chats SET active_persona_id = ?1 WHERE chat_id = ?2",
                params![persona_id, cid],
            )?;
        }

        // Migrate sessions
        conn.execute_batch(
            "CREATE TABLE sessions_new (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                messages_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );
            INSERT INTO sessions_new (chat_id, persona_id, messages_json, updated_at)
            SELECT s.chat_id, p.id, s.messages_json, s.updated_at
            FROM sessions s
            JOIN personas p ON p.chat_id = s.chat_id AND p.name = 'default';
            DROP TABLE sessions;
            ALTER TABLE sessions_new RENAME TO sessions;",
        )?;

        // Migrate messages
        conn.execute_batch(
            "CREATE TABLE messages_new (
                id TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                sender_name TEXT NOT NULL,
                content TEXT NOT NULL,
                is_from_bot INTEGER NOT NULL DEFAULT 0,
                timestamp TEXT NOT NULL,
                PRIMARY KEY (id, chat_id, persona_id)
            );
            CREATE INDEX idx_messages_new_chat_ts ON messages_new(chat_id, persona_id, timestamp);
            INSERT INTO messages_new SELECT m.id, m.chat_id, p.id, m.sender_name, m.content, m.is_from_bot, m.timestamp
            FROM messages m
            JOIN personas p ON p.chat_id = m.chat_id AND p.name = 'default';
            DROP TABLE messages;
            ALTER TABLE messages_new RENAME TO messages;
            CREATE INDEX IF NOT EXISTS idx_messages_chat_timestamp ON messages(chat_id, persona_id, timestamp);",
        )?;

        Ok(())
    }

    fn migrate_scheduled_tasks_persona_schema(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        let has_persona_id = conn
            .prepare("PRAGMA table_info(scheduled_tasks)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
                Ok(rows.filter_map(|r| r.ok()).any(|c| c == "persona_id"))
            })
            .unwrap_or(false);
        if has_persona_id {
            return Ok(());
        }

        conn.execute(
            "ALTER TABLE scheduled_tasks ADD COLUMN persona_id INTEGER",
            [],
        )?;
        conn.execute_batch(
            "UPDATE scheduled_tasks
             SET persona_id = (
                 SELECT active_persona_id
                 FROM chats
                 WHERE chats.chat_id = scheduled_tasks.chat_id
             )
             WHERE persona_id IS NULL;
             UPDATE scheduled_tasks
             SET persona_id = (
                 SELECT p.id
                 FROM personas p
                 WHERE p.chat_id = scheduled_tasks.chat_id
                 ORDER BY CASE WHEN p.name = 'default' THEN 0 ELSE 1 END, p.id
                 LIMIT 1
             )
             WHERE persona_id IS NULL;",
        )?;

        let mut stmt =
            conn.prepare("SELECT DISTINCT chat_id FROM scheduled_tasks WHERE persona_id IS NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            let chat_id = row?;
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO chats (chat_id, chat_title, chat_type, last_message_time, active_persona_id)
                 VALUES (?1, NULL, 'private', ?2, NULL)
                 ON CONFLICT(chat_id) DO NOTHING",
                params![chat_id, now],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO personas (chat_id, name, model_override) VALUES (?1, 'default', NULL)",
                params![chat_id],
            )?;
            let persona_id: i64 = conn.query_row(
                "SELECT id FROM personas WHERE chat_id = ?1 AND name = 'default'",
                params![chat_id],
                |r| r.get(0),
            )?;
            conn.execute(
                "UPDATE chats SET active_persona_id = COALESCE(active_persona_id, ?1) WHERE chat_id = ?2",
                params![persona_id, chat_id],
            )?;
            conn.execute(
                "UPDATE scheduled_tasks SET persona_id = ?1 WHERE chat_id = ?2 AND persona_id IS NULL",
                params![persona_id, chat_id],
            )?;
        }

        // Enforce NOT NULL via table rebuild (SQLite cannot alter nullability in place).
        conn.execute_batch(
            "CREATE TABLE scheduled_tasks_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                prompt TEXT NOT NULL,
                schedule_type TEXT NOT NULL DEFAULT 'cron',
                schedule_value TEXT NOT NULL,
                next_run TEXT NOT NULL,
                last_run TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL
            );
            INSERT INTO scheduled_tasks_new (id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at)
            SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
            FROM scheduled_tasks;
            DROP TABLE scheduled_tasks;
            ALTER TABLE scheduled_tasks_new RENAME TO scheduled_tasks;
            CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_status_next
                ON scheduled_tasks(status, next_run);",
        )?;

        Ok(())
    }

    fn migrate_fts(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        // Create FTS5 virtual table and triggers (after all table migrations)
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content,
                content='messages',
                content_rowid='rowid'
            );
            CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_fts_bd BEFORE DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
                INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
            END;",
        )?;

        // One-time migration: populate FTS from existing messages if FTS is empty but messages has data
        let fts_count: i64 =
            conn.query_row("SELECT count(*) FROM messages_fts", [], |r| r.get(0))?;
        let msg_count: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
        if fts_count == 0 && msg_count > 0 {
            conn.execute(
                "INSERT INTO messages_fts(rowid, content) SELECT rowid, content FROM messages",
                [],
            )?;
        }

        Ok(())
    }

    fn migrate_cursor_agent_runs_tmux(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        let has_tmux = conn
            .prepare("PRAGMA table_info(cursor_agent_runs)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
                Ok(rows.filter_map(|r| r.ok()).any(|c| c == "tmux_session"))
            })
            .unwrap_or(false);
        if !has_tmux {
            conn.execute(
                "ALTER TABLE cursor_agent_runs ADD COLUMN tmux_session TEXT",
                [],
            )?;
        }
        Ok(())
    }

    /// Removes unused `project_artifacts` (never read by the app; only written).
    fn migrate_drop_project_artifacts(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_project_artifacts_project;
             DROP TABLE IF EXISTS project_artifacts;",
        )?;
        Ok(())
    }

    fn migrate_workflow_learning_schema(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "workflows", "step_trace_json")? {
            conn.execute(
                "ALTER TABLE workflows ADD COLUMN step_trace_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        if !Self::column_exists(conn, "workflows", "approach_summary")? {
            conn.execute(
                "ALTER TABLE workflows ADD COLUMN approach_summary TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !Self::column_exists(conn, "workflows", "last_outcome")? {
            conn.execute(
                "ALTER TABLE workflows ADD COLUMN last_outcome TEXT NOT NULL DEFAULT 'unknown'",
                [],
            )?;
        }
        if !Self::column_exists(conn, "workflows", "failure_reason")? {
            conn.execute("ALTER TABLE workflows ADD COLUMN failure_reason TEXT", [])?;
        }
        if !Self::column_exists(conn, "workflows", "evidence_json")? {
            conn.execute(
                "ALTER TABLE workflows ADD COLUMN evidence_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_personas_prompt_context(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "personas", "recent_history_min_user")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN recent_history_min_user INTEGER",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "recent_history_min_assistant")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN recent_history_min_assistant INTEGER",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "operator_memo")? {
            conn.execute("ALTER TABLE personas ADD COLUMN operator_memo TEXT", [])?;
        }
        Ok(())
    }

    fn migrate_personas_dense_delivery_and_engine(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        let mut added_enabled = false;
        if !Self::column_exists(conn, "personas", "dense_delivery_enabled")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN dense_delivery_enabled INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            added_enabled = true;
        }
        if !Self::column_exists(conn, "personas", "dense_delivery_messaging_max_chars")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN dense_delivery_messaging_max_chars INTEGER",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "dense_delivery_web_max_chars")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN dense_delivery_web_max_chars INTEGER",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "dense_delivery_summary_chars")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN dense_delivery_summary_chars INTEGER",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "agent_engine_override")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN agent_engine_override TEXT",
                [],
            )?;
        }
        if added_enabled {
            // One-time seed for selling_oversea (persona 28 / operator inbox).
            conn.execute(
                "UPDATE personas SET dense_delivery_enabled = 1 WHERE id = 28 AND chat_id = 997894126",
                [],
            )?;
        }
        Ok(())
    }

    /// Retire Deterministic pipeline storage; add Gemini ADK per-persona topology column.
    fn migrate_gemini_adk_and_retire_deterministic(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "personas", "gemini_adk_topology")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN gemini_adk_topology TEXT",
                [],
            )?;
        }
        // Legacy engine ids → classic (Single turn).
        conn.execute(
            "UPDATE personas SET agent_engine_override = 'classic'
             WHERE lower(trim(agent_engine_override)) IN ('deterministic', 'pipeline')",
            [],
        )?;
        conn.execute(
            "UPDATE app_settings SET value = 'classic', updated_at = ?1
             WHERE lower(trim(key)) = 'agent_engine'
               AND lower(trim(value)) IN ('deterministic', 'pipeline')",
            params![Utc::now().to_rfc3339()],
        )?;
        let _ = conn.execute(
            "DELETE FROM app_settings WHERE lower(trim(key)) = 'deterministic_pipeline_config'",
            [],
        );
        Ok(())
    }

    /// Backfill NULL engines to classic; add per-persona Cursor SDK model columns.
    fn migrate_persona_explicit_engine_and_cursor_model(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "personas", "cursor_sdk_model")? {
            conn.execute("ALTER TABLE personas ADD COLUMN cursor_sdk_model TEXT", [])?;
        }
        if !Self::column_exists(conn, "personas", "cursor_sdk_model_params")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN cursor_sdk_model_params TEXT",
                [],
            )?;
        }
        // Personas without an override become Single turn (no inherit UI).
        conn.execute(
            "UPDATE personas SET agent_engine_override = 'classic'
             WHERE agent_engine_override IS NULL
                OR trim(agent_engine_override) = ''",
            [],
        )?;
        Ok(())
    }

    /// Per-persona classic strategy LLM provider / local base URL (model uses existing `model_override`).
    fn migrate_persona_llm_selection(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "personas", "llm_provider_override")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN llm_provider_override TEXT",
                [],
            )?;
        }
        if !Self::column_exists(conn, "personas", "llm_base_url_override")? {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN llm_base_url_override TEXT",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_hook_policy_schema(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS hook_definitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                event_name TEXT NOT NULL,
                matcher TEXT,
                action_type TEXT NOT NULL,
                action_payload_json TEXT NOT NULL DEFAULT '{}',
                scoped_persona_ids_json TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hook_definitions_event_enabled
                ON hook_definitions(event_name, enabled, id);
            CREATE TABLE IF NOT EXISTS persona_hook_skill_policy (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                allowed_hook_ids_json TEXT,
                allowed_skill_names_json TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );
            CREATE INDEX IF NOT EXISTS idx_persona_hook_skill_policy_chat
                ON persona_hook_skill_policy(chat_id);",
        )?;
        if !Self::column_exists(conn, "hook_definitions", "scoped_persona_ids_json")? {
            conn.execute(
                "ALTER TABLE hook_definitions ADD COLUMN scoped_persona_ids_json TEXT",
                [],
            )?;
        }
        Ok(())
    }

    /// Sync shipped hooks from repository `builtin_hooks/*.hook.json` into SQLite.
    ///
    /// PZ and other optional command hooks are not shipped; operators add those under
    /// `{WORKSPACE_DIR}/hooks/`.
    fn ensure_builtin_hook_definitions(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute(
            "DELETE FROM hook_definitions WHERE name LIKE 'template-%'",
            [],
        )?;
        // Legacy unfinished PreDelivery PDF guard (module never shipped); drop any seeded row.
        conn.execute(
            "DELETE FROM hook_definitions WHERE name = 'predelivery-char-limit-pdf-guard'",
            [],
        )?;
        let dir = crate::builtin_hooks::resolve_builtin_hooks_dir_fallback().ok_or_else(|| {
            FinallyAValueBotError::ToolExecution(
                "builtin_hooks catalog not found: expected repository builtin_hooks/ with *.hook.json manifests".into(),
            )
        })?;
        let synced = crate::builtin_hooks::sync_shipped_hook_definitions(conn, &dir)?;
        if synced == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "no shipped hook manifests in '{}'",
                dir.display()
            )));
        }
        Ok(())
    }

    fn migrate_chat_sessions_schema(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_sessions (
                id TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                intent TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL,
                last_active_at TEXT NOT NULL,
                archived_at TEXT,
                ttl_hours INTEGER NOT NULL DEFAULT 0,
                bootstrap_context_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_persona
                ON chat_sessions(chat_id, persona_id, status);
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_last_active
                ON chat_sessions(last_active_at DESC);",
        )?;
        if !Self::column_exists(conn, "messages", "session_id")? {
            conn.execute("ALTER TABLE messages ADD COLUMN session_id TEXT", [])?;
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_messages_session
                    ON messages(session_id, timestamp ASC);",
            )?;
        }
        if !Self::column_exists(conn, "chat_sessions", "mirror_main_chat")? {
            conn.execute(
                "ALTER TABLE chat_sessions ADD COLUMN mirror_main_chat INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !Self::column_exists(conn, "chat_sessions", "seed_context")? {
            conn.execute("ALTER TABLE chat_sessions ADD COLUMN seed_context TEXT", [])?;
        }
        if !Self::column_exists(conn, "chat_sessions", "source_topic_id")? {
            conn.execute(
                "ALTER TABLE chat_sessions ADD COLUMN source_topic_id TEXT",
                [],
            )?;
        }
        // Session archive/TTL is deprecated: restore any previously archived sessions.
        conn.execute(
            "UPDATE chat_sessions SET status = 'active', archived_at = NULL WHERE status = 'archived'",
            [],
        )?;
        Ok(())
    }

    fn migrate_messages_origin(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "messages", "origin")? {
            conn.execute(
                "ALTER TABLE messages ADD COLUMN origin TEXT NOT NULL DEFAULT 'interactive'",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_cursor_engine_agents(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cursor_engine_agents (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                session_scope TEXT NOT NULL DEFAULT '',
                agent_id TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id, session_scope)
            );
            CREATE INDEX IF NOT EXISTS idx_cursor_engine_agents_updated
                ON cursor_engine_agents(updated_at DESC);",
        )?;
        Ok(())
    }

    fn migrate_background_jobs_lease_schema(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "background_jobs", "lease_owner")? {
            conn.execute(
                "ALTER TABLE background_jobs ADD COLUMN lease_owner TEXT",
                [],
            )?;
        }
        if !Self::column_exists(conn, "background_jobs", "lease_expires_at")? {
            conn.execute(
                "ALTER TABLE background_jobs ADD COLUMN lease_expires_at TEXT",
                [],
            )?;
        }
        if !Self::column_exists(conn, "background_jobs", "last_progress_at")? {
            conn.execute(
                "ALTER TABLE background_jobs ADD COLUMN last_progress_at TEXT",
                [],
            )?;
        }
        if !Self::column_exists(conn, "background_jobs", "last_stage")? {
            conn.execute("ALTER TABLE background_jobs ADD COLUMN last_stage TEXT", [])?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_background_jobs_lease_expires_at
             ON background_jobs(lease_expires_at)",
            [],
        )?;
        Ok(())
    }

    fn migrate_background_jobs_shell_schema(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "background_jobs", "job_kind")? {
            conn.execute(
                "ALTER TABLE background_jobs ADD COLUMN job_kind TEXT NOT NULL DEFAULT 'agent'",
                [],
            )?;
        }
        for col in [
            "shell_command",
            "workdir",
            "tmux_session",
            "output_path",
            "label",
        ] {
            if !Self::column_exists(conn, "background_jobs", col)? {
                conn.execute(
                    &format!("ALTER TABLE background_jobs ADD COLUMN {col} TEXT"),
                    [],
                )?;
            }
        }
        if !Self::column_exists(conn, "background_jobs", "exit_code")? {
            conn.execute(
                "ALTER TABLE background_jobs ADD COLUMN exit_code INTEGER",
                [],
            )?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_background_jobs_job_kind_status
             ON background_jobs(job_kind, status)",
            [],
        )?;
        Ok(())
    }

    fn migrate_background_jobs_session_schema(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        if !Self::column_exists(conn, "background_jobs", "session_id")? {
            conn.execute("ALTER TABLE background_jobs ADD COLUMN session_id TEXT", [])?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_background_jobs_session_id
             ON background_jobs(session_id)",
            [],
        )?;
        Ok(())
    }

    fn migrate_persona_bulletin_and_bookmarks(
        conn: &Connection,
    ) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS persona_bulletin_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                run_key TEXT,
                event_type TEXT NOT NULL,
                title TEXT NOT NULL,
                detail TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_persona_bulletin_events_persona_time
                ON persona_bulletin_events(chat_id, persona_id, created_at DESC);

            CREATE TABLE IF NOT EXISTS persona_bulletin_focus (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                title TEXT,
                content TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );

            CREATE TABLE IF NOT EXISTS persona_message_bookmarks (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content_preview TEXT NOT NULL,
                note TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id, message_id)
            );
            CREATE INDEX IF NOT EXISTS idx_persona_message_bookmarks_persona_time
                ON persona_message_bookmarks(chat_id, persona_id, updated_at DESC);",
        )?;
        Ok(())
    }

    fn migrate_side_chats_schema(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS side_chats (
                id TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                session_id TEXT NOT NULL DEFAULT '',
                anchor_message_id TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                anchor_snippet TEXT NOT NULL DEFAULT '',
                draft_text TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (chat_id, persona_id, session_id, anchor_message_id)
            );
            CREATE INDEX IF NOT EXISTS idx_side_chats_persona_time
                ON side_chats(chat_id, persona_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS side_chat_turns (
                id TEXT PRIMARY KEY,
                side_chat_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE (side_chat_id, seq)
            );
            CREATE INDEX IF NOT EXISTS idx_side_chat_turns_chat_seq
                ON side_chat_turns(side_chat_id, seq ASC);",
        )?;
        Ok(())
    }

    fn migrate_conversation_topics(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversation_topics (
                id TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                session_id TEXT,
                parent_id TEXT,
                continues_topic_id TEXT,
                title TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                start_message_id TEXT NOT NULL,
                end_message_id TEXT NOT NULL,
                start_ts TEXT NOT NULL,
                end_ts TEXT NOT NULL,
                message_count INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'open',
                source TEXT NOT NULL DEFAULT 'auto',
                pinned INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                CHECK (status IN ('open', 'closed')),
                CHECK (source IN ('auto', 'user'))
            );
            CREATE INDEX IF NOT EXISTS idx_conversation_topics_scope
                ON conversation_topics(chat_id, persona_id, session_id, start_ts);",
        )?;
        Ok(())
    }

    fn migrate_persona_todos(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS persona_todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                source_hint TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_persona_todos_chat_status_time
                ON persona_todos(chat_id, status, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_persona_todos_persona_status
                ON persona_todos(chat_id, persona_id, status);",
        )?;
        Ok(())
    }

    fn migrate_persona_read_cursors(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS persona_read_cursors (
                chat_id INTEGER NOT NULL,
                persona_id INTEGER NOT NULL,
                last_read_at TEXT NOT NULL,
                PRIMARY KEY (chat_id, persona_id)
            );
            CREATE INDEX IF NOT EXISTS idx_persona_read_cursors_chat
                ON persona_read_cursors(chat_id);",
        )?;
        Ok(())
    }

    fn migrate_channel_bindings(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_bindings (
                canonical_chat_id INTEGER NOT NULL,
                channel_type TEXT NOT NULL,
                channel_handle TEXT NOT NULL,
                PRIMARY KEY (channel_type, channel_handle)
            );
            CREATE INDEX IF NOT EXISTS idx_channel_bindings_canonical
                ON channel_bindings(canonical_chat_id);",
        )?;
        // Backfill: each existing chat gets one binding (canonical = chat_id)
        let mut stmt = conn.prepare("SELECT chat_id, chat_type, chat_title FROM chats")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (chat_id, chat_type, chat_title) = row?;
            let (ch_type, handle) = match chat_type.as_str() {
                "telegram" => ("telegram", chat_id.to_string()),
                "discord" => ("discord", chat_id.to_string()),
                "web" => ("web", chat_title.unwrap_or_else(|| chat_id.to_string())),
                _ => continue,
            };
            conn.execute(
                "INSERT OR IGNORE INTO channel_bindings (canonical_chat_id, channel_type, channel_handle) VALUES (?1, ?2, ?3)",
                params![chat_id, ch_type, handle],
            )?;
        }
        Ok(())
    }

    pub fn upsert_chat(
        &self,
        chat_id: i64,
        chat_title: Option<&str>,
        chat_type: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO chats (chat_id, chat_title, chat_type, last_message_time)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id) DO UPDATE SET
                chat_title = COALESCE(?2, chat_title),
                chat_type = ?3,
                last_message_time = ?4",
            params![chat_id, chat_title, chat_type, now],
        )?;
        Ok(())
    }

    pub fn store_message(&self, msg: &StoredMessage) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let origin = if msg.origin.trim().is_empty() {
            MESSAGE_ORIGIN_INTERACTIVE
        } else {
            msg.origin.as_str()
        };
        conn.execute(
            "INSERT OR REPLACE INTO messages (id, chat_id, persona_id, session_id, sender_name, content, is_from_bot, timestamp, origin, speaker_persona_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                msg.id,
                msg.chat_id,
                msg.persona_id,
                msg.session_id,
                msg.sender_name,
                msg.content,
                msg.is_from_bot as i32,
                msg.timestamp,
                origin,
                msg.speaker_persona_id,
            ],
        )?;
        Ok(())
    }

    pub fn delete_message(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let prior_ts: Option<String> = match conn.query_row(
            "SELECT timestamp FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3",
            params![chat_id, persona_id, message_id],
            |row| row.get(0),
        ) {
            Ok(ts) => Some(ts),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM persona_message_bookmarks
             WHERE chat_id = ?1 AND persona_id = ?2 AND message_id = ?3",
            params![chat_id, persona_id, message_id],
        )?;
        let rows = tx.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3",
            params![chat_id, persona_id, message_id],
        )?;
        if let Some(ts) = prior_ts.as_deref() {
            Self::repair_topics_containing_timestamp(&tx, chat_id, persona_id, ts)?;
        }
        tx.commit()?;
        Ok(rows > 0)
    }

    /// Update message body in place. Also refreshes bookmark preview when bookmarked.
    pub fn update_message_content(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        content: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE messages SET content = ?1
             WHERE chat_id = ?2 AND persona_id = ?3 AND id = ?4",
            params![content, chat_id, persona_id, message_id],
        )?;
        if rows == 0 {
            return Ok(false);
        }
        let preview: String = content.chars().take(280).collect();
        let now = chrono::Utc::now().to_rfc3339();
        let _ = conn.execute(
            "UPDATE persona_message_bookmarks
             SET content_preview = ?1, updated_at = ?2
             WHERE chat_id = ?3 AND persona_id = ?4 AND message_id = ?5",
            params![preview, now, chat_id, persona_id, message_id],
        )?;
        Ok(true)
    }

    /// Replace `needle` inside this chat's messages and bookmark previews.
    /// When `token_boundary` is set, the needle must not sit inside a longer token.
    pub fn replace_literal_in_chat_messages(
        &self,
        chat_id: i64,
        needle: &str,
        replacement: &str,
        token_boundary: bool,
    ) -> Result<usize, FinallyAValueBotError> {
        if needle.is_empty() || needle.chars().count() < 6 {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, persona_id, content FROM messages
             WHERE chat_id = ?1 AND instr(content, ?2) > 0",
        )?;
        let rows: Vec<(String, i64, String)> = stmt
            .query_map(params![chat_id, needle], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut updated = 0usize;
        for (id, persona_id, content) in rows {
            let next = crate::secret_vault::replace_secret_text(
                &content,
                needle,
                replacement,
                token_boundary,
            );
            if next == content {
                continue;
            }
            conn.execute(
                "UPDATE messages SET content = ?1
                 WHERE chat_id = ?2 AND persona_id = ?3 AND id = ?4",
                params![next, chat_id, persona_id, id],
            )?;
            updated += 1;
        }
        let mut preview_stmt = conn.prepare(
            "SELECT persona_id, message_id, content_preview FROM persona_message_bookmarks
             WHERE chat_id = ?1 AND instr(content_preview, ?2) > 0",
        )?;
        let previews: Vec<(i64, String, String)> = preview_stmt
            .query_map(params![chat_id, needle], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(preview_stmt);
        let now = chrono::Utc::now().to_rfc3339();
        for (persona_id, message_id, preview) in previews {
            let next = crate::secret_vault::replace_secret_text(
                &preview,
                needle,
                replacement,
                token_boundary,
            );
            if next == preview {
                continue;
            }
            let clipped: String = next.chars().take(280).collect();
            conn.execute(
                "UPDATE persona_message_bookmarks
                 SET content_preview = ?1, updated_at = ?2
                 WHERE chat_id = ?3 AND persona_id = ?4 AND message_id = ?5",
                params![clipped, now, chat_id, persona_id, message_id],
            )?;
        }
        Ok(updated)
    }

    /// True when the **latest** row for this chat is a bot message with the same body as `content`
    /// and a recent timestamp. That usually means `send_message` already posted this text and the
    /// main agent is about to deliver the same final reply again.
    pub fn should_skip_duplicate_final_delivery(
        &self,
        chat_id: i64,
        content: &str,
        max_age_secs: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        use rusqlite::OptionalExtension;

        let conn = self.conn.lock().unwrap();
        let last: Option<(bool, String, String)> = conn
            .query_row(
                "SELECT is_from_bot, content, timestamp FROM messages
                 WHERE chat_id = ?1
                 ORDER BY timestamp DESC
                 LIMIT 1",
                params![chat_id],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)? != 0,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let Some((is_bot, last_content, ts)) = last else {
            return Ok(false);
        };
        if !is_bot || last_content != content {
            return Ok(false);
        }
        let Ok(parsed) = DateTime::parse_from_rfc3339(&ts) else {
            return Ok(false);
        };
        let parsed = parsed.with_timezone(&Utc);
        let age = Utc::now().signed_duration_since(parsed);
        Ok(age.num_seconds() >= 0 && age.num_seconds() <= max_age_secs)
    }

    pub fn get_recent_messages(
        &self,
        chat_id: i64,
        persona_id: i64,
        limit: usize,
        exclude_scheduled: bool,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut messages = if exclude_scheduled {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND origin != ?4
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                 ORDER BY timestamp DESC
                 LIMIT ?3"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, limit as i64, MESSAGE_ORIGIN_SCHEDULED],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                 ORDER BY timestamp DESC
                 LIMIT ?3"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, limit as i64],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        // Reverse so oldest first
        messages.reverse();
        Ok(messages)
    }

    pub fn get_all_messages(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2
               AND {MAIN_CHAT_MESSAGE_VISIBILITY}
             ORDER BY timestamp ASC"
        ))?;
        let messages = stmt
            .query_map(params![chat_id, persona_id], stored_message_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn get_messages_for_date_range(
        &self,
        chat_id: i64,
        persona_id: i64,
        from_date: Option<&str>,
        to_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2
               AND {MAIN_CHAT_MESSAGE_VISIBILITY}
               AND (?3 IS NULL OR timestamp >= ?3)
               AND (?4 IS NULL OR timestamp <= ?4)
             ORDER BY timestamp ASC
             LIMIT ?5"
        ))?;
        let messages = stmt
            .query_map(
                params![chat_id, persona_id, from_date, to_date, limit as i64],
                stored_message_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn get_message_days(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT date(timestamp) AS d FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2
               AND {MAIN_CHAT_MESSAGE_VISIBILITY}
             ORDER BY d DESC",
        ))?;
        let days: Vec<String> = stmt
            .query_map(params![chat_id, persona_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(days)
    }

    pub fn get_chats_by_type(
        &self,
        chat_type: &str,
        limit: usize,
    ) -> Result<Vec<ChatSummary>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                c.chat_id,
                c.chat_title,
                c.chat_type,
                c.last_message_time,
                (
                    SELECT m.content
                    FROM messages m
                    WHERE m.chat_id = c.chat_id
                    ORDER BY m.timestamp DESC
                    LIMIT 1
                ) AS last_message_preview
             FROM chats c
             WHERE c.chat_type = ?1
             ORDER BY c.last_message_time DESC
             LIMIT ?2",
        )?;
        let chats = stmt
            .query_map(params![chat_type, limit as i64], |row| {
                Ok(ChatSummary {
                    chat_id: row.get(0)?,
                    chat_title: row.get(1)?,
                    chat_type: row.get(2)?,
                    last_message_time: row.get(3)?,
                    last_message_preview: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(chats)
    }

    pub fn get_recent_chats(
        &self,
        limit: usize,
    ) -> Result<Vec<ChatSummary>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                c.chat_id,
                c.chat_title,
                c.chat_type,
                c.last_message_time,
                (
                    SELECT m.content
                    FROM messages m
                    WHERE m.chat_id = c.chat_id
                    ORDER BY m.timestamp DESC
                    LIMIT 1
                ) AS last_message_preview
             FROM chats c
             ORDER BY c.last_message_time DESC
             LIMIT ?1",
        )?;
        let chats = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ChatSummary {
                    chat_id: row.get(0)?,
                    chat_title: row.get(1)?,
                    chat_type: row.get(2)?,
                    last_message_time: row.get(3)?,
                    last_message_preview: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(chats)
    }

    pub fn get_chat_type(&self, chat_id: i64) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT chat_type FROM chats WHERE chat_id = ?1",
            params![chat_id],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // --- Channel bindings (unified contact) ---

    /// Lookup an existing binding without creating one.
    pub fn lookup_canonical_chat_id(
        &self,
        bot_instance_id: i64,
        channel_type: &str,
        channel_handle: &str,
    ) -> Result<Option<i64>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT canonical_chat_id FROM channel_bindings WHERE bot_instance_id = ?1 AND channel_type = ?2 AND channel_handle = ?3",
            params![bot_instance_id, channel_type, channel_handle],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve (`bot_instance_id`, `channel_type`, `channel_handle`) to canonical_chat_id. If no binding exists, creates one:
    /// - telegram/discord: use handle (as i64) as canonical_chat_id, ensure chat exists, insert binding.
    /// - web: use create_with_canonical_id as the new canonical (caller provides e.g. hash-based id), ensure chat exists, insert binding.
    pub fn resolve_canonical_chat_id(
        &self,
        bot_instance_id: i64,
        channel_type: &str,
        channel_handle: &str,
        create_with_canonical_id: Option<i64>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        if let Some(canonical) = conn
            .query_row(
                "SELECT canonical_chat_id FROM channel_bindings WHERE bot_instance_id = ?1 AND channel_type = ?2 AND channel_handle = ?3",
                params![bot_instance_id, channel_type, channel_handle],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        {
            return Ok(canonical);
        }
        let canonical = match channel_type {
            "telegram" | "discord" => channel_handle.parse::<i64>().map_err(|_| {
                FinallyAValueBotError::ToolExecution(format!(
                    "invalid handle for {}: {}",
                    channel_type, channel_handle
                ))
            })?,
            "web" => create_with_canonical_id.ok_or_else(|| {
                FinallyAValueBotError::ToolExecution(
                    "web resolve requires create_with_canonical_id".into(),
                )
            })?,
            "wecom" => create_with_canonical_id
                .unwrap_or_else(|| stable_canonical_chat_id_for_handle("wecom", channel_handle)),
            _ => {
                return Err(FinallyAValueBotError::ToolExecution(format!(
                    "unknown channel_type: {}",
                    channel_type
                )));
            }
        };
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO chats (chat_id, chat_title, chat_type, last_message_time) VALUES (?1, NULL, ?2, ?3)",
            params![canonical, channel_type, now],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO channel_bindings (bot_instance_id, canonical_chat_id, channel_type, channel_handle) VALUES (?1, ?2, ?3, ?4)",
            params![bot_instance_id, canonical, channel_type, channel_handle],
        )?;
        Ok(canonical)
    }

    /// Add a binding from (`bot_instance_id`, `channel_type`, `channel_handle`) to canonical_chat_id.
    pub fn link_channel(
        &self,
        canonical_chat_id: i64,
        bot_instance_id: i64,
        channel_type: &str,
        channel_handle: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO channel_bindings (bot_instance_id, canonical_chat_id, channel_type, channel_handle) VALUES (?1, ?2, ?3, ?4)",
            params![bot_instance_id, canonical_chat_id, channel_type, channel_handle],
        )?;
        Ok(())
    }

    /// Remove the binding for (`bot_instance_id`, `channel_type`, `channel_handle`).
    pub fn unlink_channel(
        &self,
        bot_instance_id: i64,
        channel_type: &str,
        channel_handle: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM channel_bindings WHERE bot_instance_id = ?1 AND channel_type = ?2 AND channel_handle = ?3",
            params![bot_instance_id, channel_type, channel_handle],
        )?;
        Ok(rows > 0)
    }

    /// List all channel bindings for this contact (canonical_chat_id).
    pub fn list_bindings_for_contact(
        &self,
        canonical_chat_id: i64,
    ) -> Result<Vec<ChannelBinding>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT canonical_chat_id, bot_instance_id, channel_type, channel_handle FROM channel_bindings WHERE canonical_chat_id = ?1",
        )?;
        let rows = stmt.query_map(params![canonical_chat_id], |row| {
            Ok(ChannelBinding {
                canonical_chat_id: row.get(0)?,
                bot_instance_id: row.get(1)?,
                channel_type: row.get(2)?,
                channel_handle: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Deprecated: env no longer overwrites DB. Prefer
    /// [`crate::channel_integration_config::migrate_from_env_if_empty`].
    #[deprecated(
        note = "use channel_integration_config::migrate_from_env_if_empty — DB is source of truth"
    )]
    pub fn sync_channel_bot_instances_from_config(
        &self,
        config: &crate::config::Config,
    ) -> Result<(), FinallyAValueBotError> {
        crate::channel_integration_config::migrate_from_env_if_empty(self, config)
    }

    pub fn list_channel_bot_instances_by_platform(
        &self,
        platform: &str,
    ) -> Result<Vec<ChannelBotInstance>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{} WHERE platform = ?1 ORDER BY id ASC",
            Self::channel_bot_instance_select()
        ))?;
        let rows = stmt.query_map(params![platform], |row| {
            Self::map_channel_bot_instance_row(row)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_channel_bot_instance(
        &self,
        id: i64,
    ) -> Result<Option<ChannelBotInstance>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            &format!("{} WHERE id = ?1", Self::channel_bot_instance_select()),
            params![id],
            Self::map_channel_bot_instance_row,
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// All bot instances (Telegram, Discord, WhatsApp, etc.), ordered by id.
    pub fn list_all_channel_bot_instances(
        &self,
    ) -> Result<Vec<ChannelBotInstance>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{} ORDER BY id ASC",
            Self::channel_bot_instance_select()
        ))?;
        let rows = stmt.query_map([], Self::map_channel_bot_instance_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Upsert a primary bot row (ids 1–4) or any fixed id used as the platform primary.
    pub fn upsert_primary_channel_bot_instance(
        &self,
        id: i64,
        platform: &str,
        label: &str,
        token: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let p = platform.trim().to_ascii_lowercase();
        if !matches!(p.as_str(), "telegram" | "discord" | "whatsapp" | "wecom") {
            return Err(FinallyAValueBotError::ToolExecution(
                "platform must be telegram, discord, whatsapp, or wecom".into(),
            ));
        }
        if token.trim().is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "token cannot be empty".into(),
            ));
        }
        let expected_id = match p.as_str() {
            "telegram" => BOT_INSTANCE_TELEGRAM_PRIMARY,
            "discord" => BOT_INSTANCE_DISCORD_PRIMARY,
            "whatsapp" => BOT_INSTANCE_WHATSAPP_PRIMARY,
            "wecom" => BOT_INSTANCE_WECOM_PRIMARY,
            _ => id,
        };
        if id != expected_id {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "primary {p} instance must use id {expected_id}"
            )));
        }
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO channel_bot_instances (id, platform, label, token, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                platform=excluded.platform,
                label=excluded.label,
                token=excluded.token",
            params![id, p, label.trim(), token.trim(), now],
        )?;
        Ok(())
    }

    pub fn update_channel_bot_instance_options(
        &self,
        id: i64,
        bot_username: Option<&str>,
        allowed_groups: Option<&[i64]>,
        discord_allowed_channels: Option<&[u64]>,
        whatsapp_phone_number_id: Option<&str>,
        whatsapp_verify_token: Option<&str>,
        whatsapp_webhook_port: Option<u16>,
    ) -> Result<bool, FinallyAValueBotError> {
        if matches!(whatsapp_webhook_port, Some(0)) {
            return Err(FinallyAValueBotError::ToolExecution(
                "whatsapp_webhook_port must be between 1 and 65535".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let current = conn.query_row(
            "SELECT bot_username, allowed_groups, discord_allowed_channels,
                    whatsapp_phone_number_id, whatsapp_verify_token, whatsapp_webhook_port
             FROM channel_bot_instances WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        );
        let Ok(current) = current else {
            return Ok(false);
        };
        let port = whatsapp_webhook_port
            .map(i64::from)
            .unwrap_or(current.5)
            .max(1);
        let allowed_groups_csv = allowed_groups
            .map(Self::format_csv_i64)
            .unwrap_or(current.1);
        let discord_allowed_csv = discord_allowed_channels
            .map(Self::format_csv_u64)
            .unwrap_or(current.2);
        conn.execute(
            "UPDATE channel_bot_instances
             SET bot_username = ?1,
                 allowed_groups = ?2,
                 discord_allowed_channels = ?3,
                 whatsapp_phone_number_id = ?4,
                 whatsapp_verify_token = ?5,
                 whatsapp_webhook_port = ?6
             WHERE id = ?7",
            params![
                bot_username.map(str::trim).unwrap_or(current.0.as_str()),
                allowed_groups_csv,
                discord_allowed_csv,
                whatsapp_phone_number_id
                    .map(str::trim)
                    .unwrap_or(current.3.as_str()),
                whatsapp_verify_token
                    .map(str::trim)
                    .unwrap_or(current.4.as_str()),
                port,
                id
            ],
        )?;
        Ok(true)
    }

    pub fn update_channel_bot_instance_wecom_options(
        &self,
        id: i64,
        wecom_corp_id: Option<&str>,
        wecom_agent_id: Option<i64>,
        wecom_callback_token: Option<&str>,
        wecom_encoding_aes_key: Option<&str>,
        wecom_webhook_port: Option<u16>,
        wecom_allowed_chats: Option<&str>,
        wecom_aibot_id: Option<&str>,
        wecom_mode: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        if matches!(wecom_webhook_port, Some(0)) {
            return Err(FinallyAValueBotError::ToolExecution(
                "wecom_webhook_port must be between 1 and 65535".into(),
            ));
        }
        if matches!(wecom_agent_id, Some(v) if v < 0) {
            return Err(FinallyAValueBotError::ToolExecution(
                "wecom_agent_id must be >= 0".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let current = conn.query_row(
            "SELECT wecom_corp_id, wecom_agent_id, wecom_callback_token, wecom_encoding_aes_key,
                    wecom_webhook_port, wecom_allowed_chats, wecom_aibot_id, wecom_mode
             FROM channel_bot_instances WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        );
        let Ok(current) = current else {
            return Ok(false);
        };
        let port = wecom_webhook_port
            .map(i64::from)
            .unwrap_or(current.4)
            .max(1);
        conn.execute(
            "UPDATE channel_bot_instances
             SET wecom_corp_id = ?1,
                 wecom_agent_id = ?2,
                 wecom_callback_token = ?3,
                 wecom_encoding_aes_key = ?4,
                 wecom_webhook_port = ?5,
                 wecom_allowed_chats = ?6,
                 wecom_aibot_id = ?7,
                 wecom_mode = ?8
             WHERE id = ?9",
            params![
                wecom_corp_id.map(str::trim).unwrap_or(current.0.as_str()),
                wecom_agent_id.unwrap_or(current.1),
                wecom_callback_token
                    .map(str::trim)
                    .unwrap_or(current.2.as_str()),
                wecom_encoding_aes_key
                    .map(str::trim)
                    .unwrap_or(current.3.as_str()),
                port,
                wecom_allowed_chats
                    .map(str::trim)
                    .unwrap_or(current.5.as_str()),
                wecom_aibot_id.map(str::trim).unwrap_or(current.6.as_str()),
                wecom_mode.map(str::trim).unwrap_or(current.7.as_str()),
                id
            ],
        )?;
        Ok(true)
    }

    /// Insert a non-primary bot instance. Ids 1–4 are reserved for primaries; extras use id >= 5.
    pub fn create_channel_bot_instance(
        &self,
        platform: &str,
        label: &str,
        token: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let p = platform.trim().to_ascii_lowercase();
        if !matches!(p.as_str(), "telegram" | "discord" | "whatsapp" | "wecom") {
            return Err(FinallyAValueBotError::ToolExecution(
                "platform must be telegram, discord, whatsapp, or wecom".into(),
            ));
        }
        if token.trim().is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "token cannot be empty".into(),
            ));
        }
        if p == "whatsapp" {
            if !self
                .list_channel_bot_instances_by_platform("whatsapp")?
                .is_empty()
            {
                return Err(FinallyAValueBotError::ToolExecution(
                    "Only one WhatsApp Cloud API number is supported. Edit or delete the existing WhatsApp instance.".into(),
                ));
            }
            self.upsert_primary_channel_bot_instance(
                BOT_INSTANCE_WHATSAPP_PRIMARY,
                "whatsapp",
                label,
                token,
            )?;
            return Ok(BOT_INSTANCE_WHATSAPP_PRIMARY);
        }
        if p == "wecom" {
            if !self
                .list_channel_bot_instances_by_platform("wecom")?
                .is_empty()
            {
                return Err(FinallyAValueBotError::ToolExecution(
                    "Only one WeCom app is supported. Edit or delete the existing WeCom instance."
                        .into(),
                ));
            }
            self.upsert_primary_channel_bot_instance(
                BOT_INSTANCE_WECOM_PRIMARY,
                "wecom",
                label,
                token,
            )?;
            return Ok(BOT_INSTANCE_WECOM_PRIMARY);
        }
        let conn = self.conn.lock().unwrap();
        let new_id = Self::next_extra_bot_instance_id_conn(&conn)?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO channel_bot_instances (id, platform, label, token, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![new_id, p, label.trim(), token.trim(), now],
        )?;
        Ok(new_id)
    }

    pub fn get_channel_bot_instance_platform(
        &self,
        bot_instance_id: i64,
    ) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT platform FROM channel_bot_instances WHERE id = ?1",
            params![bot_instance_id],
            |r| r.get::<_, String>(0),
        );
        match row {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Copy channel handles from sibling bindings on the same platform to `new_bot_instance_id`.
    pub fn provision_bindings_for_instance(
        &self,
        platform: &str,
        new_bot_instance_id: i64,
    ) -> Result<u32, FinallyAValueBotError> {
        let platform = platform.trim().to_ascii_lowercase();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT canonical_chat_id, channel_handle
             FROM channel_bindings
             WHERE channel_type = ?1 AND bot_instance_id != ?2",
        )?;
        let rows: Vec<(i64, String)> = stmt
            .query_map(params![platform, new_bot_instance_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut linked = 0u32;
        for (canonical_chat_id, handle) in rows {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM channel_bindings
                     WHERE bot_instance_id = ?1 AND channel_type = ?2 AND channel_handle = ?3",
                    params![new_bot_instance_id, platform, handle],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if exists {
                continue;
            }
            conn.execute(
                "INSERT OR REPLACE INTO channel_bindings (bot_instance_id, canonical_chat_id, channel_type, channel_handle)
                 VALUES (?1, ?2, ?3, ?4)",
                params![new_bot_instance_id, canonical_chat_id, platform, handle],
            )?;
            linked += 1;
        }
        Ok(linked)
    }

    /// Idempotent: ensure every bot instance has sibling bindings for contacts already linked on that platform.
    pub fn provision_all_missing_sibling_bindings(&self) -> Result<u32, FinallyAValueBotError> {
        let instances = self.list_all_channel_bot_instances()?;
        let mut total = 0u32;
        for inst in instances {
            if !matches!(
                inst.platform.as_str(),
                "telegram" | "discord" | "whatsapp" | "wecom"
            ) {
                continue;
            }
            total += self.provision_bindings_for_instance(&inst.platform, inst.id)?;
        }
        Ok(total)
    }

    /// Rows for Settings → Channels: every external chat instance plus binding/policy state for a contact.
    pub fn list_contact_channel_integration_rows(
        &self,
        canonical_chat_id: i64,
    ) -> Result<Vec<ContactChannelIntegrationRow>, FinallyAValueBotError> {
        let bindings = self.list_bindings_for_contact(canonical_chat_id)?;
        let policies = self.list_channel_persona_policies(canonical_chat_id)?;
        let mut policy_by_instance: HashMap<i64, ChannelPersonaPolicy> = HashMap::new();
        for p in policies {
            policy_by_instance.insert(p.bot_instance_id, p);
        }
        let mut binding_by_instance: HashMap<i64, ChannelBinding> = HashMap::new();
        for b in bindings {
            if b.channel_type == "web" {
                continue;
            }
            binding_by_instance.insert(b.bot_instance_id, b);
        }
        let mut rows = Vec::new();
        for platform in ["telegram", "discord", "whatsapp", "wecom"] {
            for inst in self.list_channel_bot_instances_by_platform(platform)? {
                let binding = binding_by_instance.get(&inst.id);
                let (persona_mode, persona_id) = policy_by_instance
                    .get(&inst.id)
                    .map(|p| (p.mode, p.persona_id))
                    .unwrap_or((ChannelPersonaMode::All, None));
                rows.push(ContactChannelIntegrationRow {
                    bot_instance_id: inst.id,
                    platform: inst.platform.clone(),
                    label: inst.label.clone(),
                    channel_handle: binding.map(|b| b.channel_handle.clone()),
                    linked: binding.is_some(),
                    persona_mode,
                    persona_id,
                });
            }
        }
        Ok(rows)
    }

    fn another_all_persona_bot_on_platform(
        &self,
        canonical_chat_id: i64,
        platform: &str,
        exclude_bot_instance_id: i64,
    ) -> Result<Option<i64>, FinallyAValueBotError> {
        let instances = self.list_channel_bot_instances_by_platform(platform)?;
        for inst in instances {
            if inst.id == exclude_bot_instance_id {
                continue;
            }
            let effective_all = match self.get_channel_persona_policy(canonical_chat_id, inst.id)? {
                None => true,
                Some(p) => p.mode == ChannelPersonaMode::All,
            };
            if effective_all {
                return Ok(Some(inst.id));
            }
        }
        Ok(None)
    }

    fn validate_all_persona_slot(
        &self,
        canonical_chat_id: i64,
        bot_instance_id: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let platform = self
            .get_channel_bot_instance_platform(bot_instance_id)?
            .ok_or_else(|| {
                FinallyAValueBotError::ToolExecution(format!(
                    "unknown bot_instance_id {bot_instance_id}"
                ))
            })?;
        if let Some(other) =
            self.another_all_persona_bot_on_platform(canonical_chat_id, &platform, bot_instance_id)?
        {
            let label = self
                .get_channel_bot_instance(other)?
                .map(|i| i.label)
                .unwrap_or_else(|| format!("#{other}"));
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "Only one {platform} bot can use 'all personas' for this contact (already set on {label}). Lock the other bot to a single persona first."
            )));
        }
        Ok(())
    }

    pub fn update_channel_bot_instance(
        &self,
        id: i64,
        label: &str,
        token: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        if token.trim().is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "token cannot be empty".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE channel_bot_instances SET label = ?1, token = ?2 WHERE id = ?3",
            params![label.trim(), token.trim(), id],
        )?;
        Ok(rows > 0)
    }

    /// Deletes a bot instance. Primary ids 1–4 may be deleted to disable that platform.
    pub fn delete_channel_bot_instance(&self, id: i64) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        // Drop related persona policy rows for this instance (bindings stay for contact history).
        let _ = conn.execute(
            "DELETE FROM channel_persona_policy WHERE bot_instance_id = ?1",
            params![id],
        );
        let _ = conn.execute(
            "DELETE FROM channel_bindings WHERE bot_instance_id = ?1",
            params![id],
        );
        let rows = conn.execute(
            "DELETE FROM channel_bot_instances WHERE id = ?1",
            params![id],
        )?;
        Ok(rows > 0)
    }

    pub fn list_app_settings(&self) -> Result<Vec<AppSetting>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key, value, updated_at FROM app_settings ORDER BY key COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AppSetting {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_app_setting(&self, key: &str) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM app_settings WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub fn set_app_setting(&self, key: &str, value: &str) -> Result<(), FinallyAValueBotError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "App setting key cannot be empty".to_string(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![key, value, now],
        )?;
        Ok(())
    }

    pub fn remove_app_setting(&self, key: &str) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
        Ok(rows > 0)
    }

    fn parse_i64_array_json(
        raw: Option<String>,
    ) -> Result<Option<Vec<i64>>, FinallyAValueBotError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let parsed: serde_json::Value = serde_json::from_str(raw.trim()).map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!("Invalid allowed_hook_ids_json: {e}"))
        })?;
        let arr = parsed.as_array().ok_or_else(|| {
            FinallyAValueBotError::ToolExecution(
                "Invalid allowed_hook_ids_json: expected JSON array".to_string(),
            )
        })?;
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            let Some(id) = v.as_i64() else {
                return Err(FinallyAValueBotError::ToolExecution(
                    "Invalid allowed_hook_ids_json: array must contain integers".to_string(),
                ));
            };
            out.push(id);
        }
        Ok(Some(out))
    }

    fn parse_string_array_json(
        raw: Option<String>,
    ) -> Result<Option<Vec<String>>, FinallyAValueBotError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let parsed: serde_json::Value = serde_json::from_str(raw.trim()).map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!("Invalid allowed_skill_names_json: {e}"))
        })?;
        let arr = parsed.as_array().ok_or_else(|| {
            FinallyAValueBotError::ToolExecution(
                "Invalid allowed_skill_names_json: expected JSON array".to_string(),
            )
        })?;
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            let Some(name) = v.as_str() else {
                return Err(FinallyAValueBotError::ToolExecution(
                    "Invalid allowed_skill_names_json: array must contain strings".to_string(),
                ));
            };
            out.push(name.trim().to_string());
        }
        Ok(Some(out))
    }

    pub fn list_hook_definitions(
        &self,
    ) -> Result<Vec<HookDefinitionRecord>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, event_name, matcher, action_type, action_payload_json, scoped_persona_ids_json, enabled, updated_at
             FROM hook_definitions
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let scoped_persona_ids_raw: Option<String> = row.get(6)?;
            let scoped_persona_ids =
                Self::parse_i64_array_json(scoped_persona_ids_raw).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            Ok(HookDefinitionRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                event_name: row.get(2)?,
                matcher: row.get(3)?,
                action_type: row.get(4)?,
                action_payload_json: row.get(5)?,
                scoped_persona_ids,
                enabled: row.get::<_, i64>(7)? != 0,
                updated_at: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_hook_definition(
        &self,
        id: i64,
    ) -> Result<Option<HookDefinitionRecord>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, event_name, matcher, action_type, action_payload_json, scoped_persona_ids_json, enabled, updated_at
             FROM hook_definitions
             WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], |row| {
            let scoped_persona_ids_raw: Option<String> = row.get(6)?;
            let scoped_persona_ids =
                Self::parse_i64_array_json(scoped_persona_ids_raw).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            Ok(HookDefinitionRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                event_name: row.get(2)?,
                matcher: row.get(3)?,
                action_type: row.get(4)?,
                action_payload_json: row.get(5)?,
                scoped_persona_ids,
                enabled: row.get::<_, i64>(7)? != 0,
                updated_at: row.get(8)?,
            })
        });
        match row {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn upsert_hook_definition(
        &self,
        id: Option<i64>,
        name: &str,
        event_name: &str,
        matcher: Option<&str>,
        action_type: &str,
        action_payload_json: &str,
        scoped_persona_ids: Option<&[i64]>,
        enabled: bool,
    ) -> Result<i64, FinallyAValueBotError> {
        let name = name.trim();
        let event_name = event_name.trim();
        let action_type = action_type.trim().to_ascii_lowercase();
        if name.is_empty() || event_name.is_empty() || action_type.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "name, event_name, and action_type are required".to_string(),
            ));
        }
        let valid_action_type = matches!(
            action_type.as_str(),
            "block"
                | "add_context"
                | "command"
                | "prompt"
                | "builtin_persona_focus_sync"
                | "builtin_scheduler_policy_context"
                | "builtin_turn_skill_gate"
                | "builtin_deferred_commitment_guard"
                | "builtin_loop_guard"
                | "builtin_dense_delivery_guard"
                | "builtin_secret_vault_scrub"
        );
        if !valid_action_type {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "Unsupported action_type '{}'. Expected one of: block, add_context, command, prompt, builtin_persona_focus_sync, builtin_scheduler_policy_context, builtin_turn_skill_gate, builtin_deferred_commitment_guard, builtin_loop_guard, builtin_dense_delivery_guard, builtin_secret_vault_scrub",
                action_type
            )));
        }
        // Validate payload JSON eagerly.
        let _: serde_json::Value = serde_json::from_str(action_payload_json).map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!("Invalid action_payload_json: {e}"))
        })?;
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let enabled_i = if enabled { 1 } else { 0 };
        let matcher_norm = matcher.map(|m| m.trim()).filter(|m| !m.is_empty());
        let scoped_persona_ids_json = if let Some(ids) = scoped_persona_ids {
            let mut normalized: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
            normalized.sort_unstable();
            normalized.dedup();
            Some(serde_json::to_string(&normalized).map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "Invalid scoped_persona_ids payload: {e}"
                ))
            })?)
        } else {
            None
        };
        if let Some(id) = id {
            let rows = conn.execute(
                "UPDATE hook_definitions
                 SET name = ?1,
                     event_name = ?2,
                     matcher = ?3,
                     action_type = ?4,
                     action_payload_json = ?5,
                     scoped_persona_ids_json = ?6,
                     enabled = ?7,
                     updated_at = ?8
                 WHERE id = ?9",
                params![
                    name,
                    event_name,
                    matcher_norm,
                    action_type.as_str(),
                    action_payload_json,
                    scoped_persona_ids_json,
                    enabled_i,
                    now,
                    id
                ],
            )?;
            if rows == 0 {
                return Err(FinallyAValueBotError::ToolExecution(format!(
                    "Hook definition {id} not found"
                )));
            }
            Ok(id)
        } else {
            conn.execute(
                "INSERT INTO hook_definitions
                 (name, event_name, matcher, action_type, action_payload_json, scoped_persona_ids_json, enabled, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    name,
                    event_name,
                    matcher_norm,
                    action_type.as_str(),
                    action_payload_json,
                    scoped_persona_ids_json,
                    enabled_i,
                    now
                ],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }

    pub fn delete_hook_definition(&self, id: i64) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM hook_definitions WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    pub fn get_persona_hook_skill_policy(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Option<PersonaHookSkillPolicy>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT chat_id, persona_id, allowed_hook_ids_json, allowed_skill_names_json, updated_at
             FROM persona_hook_skill_policy
             WHERE chat_id = ?1 AND persona_id = ?2",
        )?;
        let row = stmt.query_row(params![chat_id, persona_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        });
        let (chat_id, persona_id, hooks_raw, skills_raw, updated_at) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let allowed_hook_ids = Self::parse_i64_array_json(hooks_raw)?;
        let allowed_skill_names = Self::parse_string_array_json(skills_raw)?;
        Ok(Some(PersonaHookSkillPolicy {
            chat_id,
            persona_id,
            allowed_hook_ids,
            allowed_skill_names,
            updated_at,
        }))
    }

    pub fn set_persona_hook_skill_policy(
        &self,
        chat_id: i64,
        persona_id: i64,
        allowed_hook_ids: Option<&[i64]>,
        allowed_skill_names: Option<&[String]>,
    ) -> Result<(), FinallyAValueBotError> {
        if !self.persona_exists(chat_id, persona_id)? {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "persona_id {persona_id} does not exist for chat {chat_id}"
            )));
        }
        let hooks_json = allowed_hook_ids
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "Failed to serialize allowed_hook_ids: {e}"
                ))
            })?;
        let skills_json = allowed_skill_names
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "Failed to serialize allowed_skill_names: {e}"
                ))
            })?;
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO persona_hook_skill_policy
             (chat_id, persona_id, allowed_hook_ids_json, allowed_skill_names_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chat_id, persona_id) DO UPDATE SET
               allowed_hook_ids_json = excluded.allowed_hook_ids_json,
               allowed_skill_names_json = excluded.allowed_skill_names_json,
               updated_at = excluded.updated_at",
            params![chat_id, persona_id, hooks_json, skills_json, now],
        )?;
        Ok(())
    }

    pub fn is_skill_allowed_for_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        skill_name: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let Some(policy) = self.get_persona_hook_skill_policy(chat_id, persona_id)? else {
            return Ok(true);
        };
        let Some(allowed) = policy.allowed_skill_names else {
            return Ok(true);
        };
        Ok(allowed.iter().any(|s| s.eq_ignore_ascii_case(skill_name)))
    }

    pub fn is_hook_allowed_for_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        hook_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let Some(policy) = self.get_persona_hook_skill_policy(chat_id, persona_id)? else {
            return Ok(true);
        };
        let Some(allowed) = policy.allowed_hook_ids else {
            return Ok(true);
        };
        Ok(allowed.contains(&hook_id))
    }

    pub fn list_channel_persona_policies(
        &self,
        canonical_chat_id: i64,
    ) -> Result<Vec<ChannelPersonaPolicy>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT canonical_chat_id, bot_instance_id, mode, persona_id, updated_at
             FROM channel_persona_policy
             WHERE canonical_chat_id = ?1
             ORDER BY bot_instance_id ASC",
        )?;
        let rows = stmt.query_map(params![canonical_chat_id], |row| {
            let mode_raw: String = row.get(2)?;
            let mode = ChannelPersonaMode::try_from(mode_raw.as_str()).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(ChannelPersonaPolicy {
                canonical_chat_id: row.get(0)?,
                bot_instance_id: row.get(1)?,
                mode,
                persona_id: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_channel_persona_policy(
        &self,
        canonical_chat_id: i64,
        bot_instance_id: i64,
    ) -> Result<Option<ChannelPersonaPolicy>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT canonical_chat_id, bot_instance_id, mode, persona_id, updated_at
             FROM channel_persona_policy
             WHERE canonical_chat_id = ?1 AND bot_instance_id = ?2",
        )?;
        let row = stmt.query_row(params![canonical_chat_id, bot_instance_id], |row| {
            let mode_raw: String = row.get(2)?;
            let mode = ChannelPersonaMode::try_from(mode_raw.as_str()).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(ChannelPersonaPolicy {
                canonical_chat_id: row.get(0)?,
                bot_instance_id: row.get(1)?,
                mode,
                persona_id: row.get(3)?,
                updated_at: row.get(4)?,
            })
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_channel_persona_policy(
        &self,
        canonical_chat_id: i64,
        bot_instance_id: i64,
        mode: ChannelPersonaMode,
        persona_id: Option<i64>,
    ) -> Result<(), FinallyAValueBotError> {
        if bot_instance_id == BOT_INSTANCE_WEB {
            return Err(FinallyAValueBotError::ToolExecution(
                "Persona scope policy does not apply to web chat; the Web UI selects persona."
                    .into(),
            ));
        }
        if mode == ChannelPersonaMode::All {
            self.validate_all_persona_slot(canonical_chat_id, bot_instance_id)?;
        }
        if mode == ChannelPersonaMode::Single {
            let pid = persona_id.ok_or_else(|| {
                FinallyAValueBotError::ToolExecution(
                    "persona_id is required for single persona mode".to_string(),
                )
            })?;
            if !self.persona_exists(canonical_chat_id, pid)? {
                return Err(FinallyAValueBotError::ToolExecution(format!(
                    "persona_id {pid} does not exist for chat {canonical_chat_id}"
                )));
            }
        }
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let pid = if mode == ChannelPersonaMode::Single {
            persona_id
        } else {
            None
        };
        conn.execute(
            "INSERT INTO channel_persona_policy (canonical_chat_id, bot_instance_id, mode, persona_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(canonical_chat_id, bot_instance_id)
             DO UPDATE SET mode=excluded.mode, persona_id=excluded.persona_id, updated_at=excluded.updated_at",
            params![canonical_chat_id, bot_instance_id, mode.as_str(), pid, now],
        )?;
        Ok(())
    }

    pub fn clear_channel_persona_policy(
        &self,
        canonical_chat_id: i64,
        bot_instance_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        self.validate_all_persona_slot(canonical_chat_id, bot_instance_id)?;
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM channel_persona_policy WHERE canonical_chat_id = ?1 AND bot_instance_id = ?2",
            params![canonical_chat_id, bot_instance_id],
        )?;
        Ok(rows > 0)
    }

    /// Get messages since the bot's last response in this chat/persona.
    /// Falls back to `fallback_limit` most recent messages if bot never responded.
    pub fn get_messages_since_last_bot_response(
        &self,
        chat_id: i64,
        persona_id: i64,
        max: usize,
        fallback: usize,
        exclude_scheduled: bool,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();

        let last_bot_ts: Option<String> = if exclude_scheduled {
            conn.query_row(
                &format!(
                    "SELECT timestamp FROM messages
                     WHERE chat_id = ?1 AND persona_id = ?2 AND is_from_bot = 1
                       AND origin != ?3
                       AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                     ORDER BY timestamp DESC LIMIT 1"
                ),
                params![chat_id, persona_id, MESSAGE_ORIGIN_SCHEDULED],
                |row| row.get(0),
            )
            .ok()
        } else {
            conn.query_row(
                &format!(
                    "SELECT timestamp FROM messages
                     WHERE chat_id = ?1 AND persona_id = ?2 AND is_from_bot = 1
                       AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                     ORDER BY timestamp DESC LIMIT 1"
                ),
                params![chat_id, persona_id],
                |row| row.get(0),
            )
            .ok()
        };

        let origin_filter = if exclude_scheduled {
            " AND origin != ?5"
        } else {
            ""
        };

        let mut messages = if let Some(ts) = last_bot_ts {
            let sql = format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND timestamp >= ?3
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}{origin_filter}
                 ORDER BY timestamp DESC
                 LIMIT ?4"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = if exclude_scheduled {
                stmt.query_map(
                    params![
                        chat_id,
                        persona_id,
                        ts,
                        max as i64,
                        MESSAGE_ORIGIN_SCHEDULED
                    ],
                    stored_message_from_row,
                )?
            } else {
                stmt.query_map(
                    params![chat_id, persona_id, ts, max as i64],
                    stored_message_from_row,
                )?
            };
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}{origin_filter}
                 ORDER BY timestamp DESC
                 LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = if exclude_scheduled {
                stmt.query_map(
                    params![
                        chat_id,
                        persona_id,
                        fallback as i64,
                        MESSAGE_ORIGIN_SCHEDULED
                    ],
                    stored_message_from_row,
                )?
            } else {
                stmt.query_map(
                    params![chat_id, persona_id, fallback as i64],
                    stored_message_from_row,
                )?
            };
            rows.collect::<Result<Vec<_>, _>>()?
        };

        messages.reverse();
        Ok(messages)
    }

    // --- Scheduled tasks ---

    pub fn create_scheduled_task(
        &self,
        chat_id: i64,
        prompt: &str,
        schedule_type: &str,
        schedule_value: &str,
        next_run: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let persona_id = self.get_current_persona_id(chat_id)?;
        self.create_scheduled_task_for_persona(
            chat_id,
            persona_id,
            prompt,
            schedule_type,
            schedule_value,
            next_run,
        )
    }

    pub fn create_scheduled_task_for_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        schedule_type: &str,
        schedule_value: &str,
        next_run: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO scheduled_tasks (chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7)",
            params![
                chat_id,
                persona_id,
                prompt,
                schedule_type,
                schedule_value,
                next_run,
                now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn next_cron_run_utc(cron_expr: &str, tz_name: &str) -> Result<String, FinallyAValueBotError> {
        let tz_name = if tz_name.trim().is_empty() {
            "UTC"
        } else {
            tz_name.trim()
        };
        let tz: chrono_tz::Tz = tz_name
            .parse()
            .map_err(|_| FinallyAValueBotError::Config(format!("Invalid timezone: {tz_name}")))?;
        let schedule = Schedule::from_str(cron_expr)
            .map_err(|e| FinallyAValueBotError::Config(format!("Invalid cron expression: {e}")))?;
        let next = schedule.upcoming(tz).next().ok_or_else(|| {
            FinallyAValueBotError::Config(
                "No upcoming run found for this cron expression".to_string(),
            )
        })?;
        Ok(next.with_timezone(&Utc).to_rfc3339())
    }

    fn ensure_unique_cron_task_by_prompt_prefix(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt_prefix: &str,
        prompt: &str,
        cron_expr: &str,
        timezone: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let next_run = Self::next_cron_run_utc(cron_expr, timezone)?;
        let conn = self.conn.lock().unwrap();
        let like_pattern = format!("{prompt_prefix}%");
        let mut stmt = conn.prepare(
            "SELECT id, schedule_value, status FROM scheduled_tasks
             WHERE chat_id = ?1
               AND status IN ('active', 'paused')
               AND schedule_type = 'cron'
               AND prompt LIKE ?2
             ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, id ASC",
        )?;
        let existing: Vec<(i64, String, String)> = stmt
            .query_map(params![chat_id, like_pattern], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let now = chrono::Utc::now().to_rfc3339();
        let primary = existing
            .first()
            .map(|(id, cron, _status)| (*id, cron.clone()));
        if let Some((primary_id, current_cron)) = primary {
            // Uniqueness is for the whole chat, not one persona. The oldest active
            // row (or the oldest paused row, if none are active) becomes the
            // stable maintenance owner. Other personas' copies are deactivated.
            // The original built-in pair shared `0 0 */6 * * *`. Move only that
            // collision onto the target cron. Any other expression is an operator edit.
            let legacy_shared_cron = "0 0 */6 * * *";
            if current_cron != cron_expr && current_cron == legacy_shared_cron {
                conn.execute(
                    "UPDATE scheduled_tasks
                     SET prompt = ?1, schedule_value = ?2, next_run = ?3, persona_id = ?4
                     WHERE id = ?5",
                    params![prompt, cron_expr, next_run, persona_id, primary_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE scheduled_tasks SET prompt = ?1, persona_id = ?2 WHERE id = ?3",
                    params![prompt, persona_id, primary_id],
                )?;
            }
            for (dup_id, _, _) in existing.iter().skip(1) {
                conn.execute(
                    "UPDATE scheduled_tasks SET status = 'inactive' WHERE id = ?1",
                    params![dup_id],
                )?;
            }
        } else {
            conn.execute(
                "INSERT INTO scheduled_tasks (chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, status, created_at)
                 VALUES (?1, ?2, ?3, 'cron', ?4, ?5, 'active', ?6)",
                params![chat_id, persona_id, prompt, cron_expr, next_run, now],
            )?;
        }
        Ok(())
    }

    pub fn ensure_indexing_task(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        cron_expr: &str,
        timezone: &str,
    ) -> Result<(), FinallyAValueBotError> {
        self.ensure_unique_cron_task_by_prompt_prefix(
            chat_id,
            persona_id,
            "Run the vault indexing script:",
            prompt,
            cron_expr,
            timezone,
        )
    }

    pub fn ensure_vault_push_task(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        cron_expr: &str,
        timezone: &str,
    ) -> Result<(), FinallyAValueBotError> {
        self.ensure_unique_cron_task_by_prompt_prefix(
            chat_id,
            persona_id,
            "Sync ORIGIN vault to git remote:",
            prompt,
            cron_expr,
            timezone,
        )
    }

    pub fn ensure_recall_reconcile_task(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        cron_expr: &str,
        timezone: &str,
    ) -> Result<(), FinallyAValueBotError> {
        self.ensure_unique_cron_task_by_prompt_prefix(
            chat_id,
            persona_id,
            crate::recall::RECALL_RECONCILE_PREFIX,
            prompt,
            cron_expr,
            timezone,
        )
    }

    /// Persona named `default` for built-in maintenance jobs. Does not change
    /// `chats.active_persona_id` when the chat already exists.
    pub fn ensure_named_persona(
        &self,
        chat_id: i64,
        name: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        if let Ok(id) = conn.query_row(
            "SELECT id FROM personas WHERE chat_id = ?1 AND name = ?2",
            params![chat_id, name],
            |row| row.get(0),
        ) {
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO personas (chat_id, name, model_override, agent_engine_override) VALUES (?1, ?2, NULL, 'classic')",
            params![chat_id, name],
        )?;
        let id = conn.last_insert_rowid();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO chats (chat_id, chat_title, chat_type, last_message_time, active_persona_id)
             VALUES (?1, NULL, 'private', ?2, ?3)
             ON CONFLICT(chat_id) DO NOTHING",
            params![chat_id, now, id],
        )?;
        Ok(id)
    }

    pub fn list_task_ids_by_prompt_prefix(
        &self,
        chat_id: i64,
        prompt_prefix: &str,
        status: &str,
    ) -> Result<Vec<i64>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let like_pattern = format!("{prompt_prefix}%");
        let mut stmt = conn.prepare(
            "SELECT id FROM scheduled_tasks
             WHERE chat_id = ?1 AND status = ?2 AND prompt LIKE ?3
             ORDER BY id ASC",
        )?;
        let ids = stmt
            .query_map(params![chat_id, status, like_pattern], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn task_status(&self, task_id: i64) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT status FROM scheduled_tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        ) {
            Ok(status) => Ok(Some(status)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn migrate_recall_ingest(conn: &Connection) -> Result<(), FinallyAValueBotError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS recall_ingest_items (
                source_kind TEXT NOT NULL,
                source_id TEXT NOT NULL,
                chat_id INTEGER,
                persona_id INTEGER,
                content_hash TEXT NOT NULL,
                status TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                updated_at TEXT NOT NULL,
                generation INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (source_kind, source_id)
            );
            CREATE TABLE IF NOT EXISTS recall_ingest_checkpoint (
                scope TEXT PRIMARY KEY,
                cursor TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS recall_ingest_lock (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                owner TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    pub fn upsert_recall_ingest(
        &self,
        source_kind: &str,
        source_id: &str,
        chat_id: Option<i64>,
        persona_id: Option<i64>,
        content_hash: &str,
        generation: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO recall_ingest_items (
                source_kind, source_id, chat_id, persona_id, content_hash, status,
                attempt_count, last_error, updated_at, generation
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, NULL, ?6, ?7)
             ON CONFLICT(source_kind, source_id) DO UPDATE SET
                chat_id = excluded.chat_id,
                persona_id = excluded.persona_id,
                generation = excluded.generation,
                content_hash = excluded.content_hash,
                status = CASE
                    WHEN recall_ingest_items.content_hash = excluded.content_hash
                     AND recall_ingest_items.status = 'done' THEN 'done'
                    ELSE 'pending'
                END,
                attempt_count = CASE
                    WHEN recall_ingest_items.content_hash = excluded.content_hash
                     AND recall_ingest_items.status = 'done'
                    THEN recall_ingest_items.attempt_count
                    ELSE 0
                END,
                last_error = CASE
                    WHEN recall_ingest_items.content_hash = excluded.content_hash
                     AND recall_ingest_items.status = 'done'
                    THEN recall_ingest_items.last_error
                    ELSE NULL
                END,
                updated_at = excluded.updated_at",
            params![
                source_kind,
                source_id,
                chat_id,
                persona_id,
                content_hash,
                now,
                generation
            ],
        )?;
        Ok(())
    }

    pub fn claim_recall_ingest_batch(
        &self,
        limit: usize,
    ) -> Result<Vec<RecallIngestRow>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source_kind, source_id, chat_id, persona_id, content_hash, attempt_count
             FROM recall_ingest_items
             WHERE status = 'pending' AND attempt_count < 5
             ORDER BY updated_at ASC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(RecallIngestRow {
                    source_kind: row.get(0)?,
                    source_id: row.get(1)?,
                    chat_id: row.get(2)?,
                    persona_id: row.get(3)?,
                    content_hash: row.get(4)?,
                    attempt_count: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let now = chrono::Utc::now().to_rfc3339();
        for row in &rows {
            conn.execute(
                "UPDATE recall_ingest_items
                 SET attempt_count = attempt_count + 1, updated_at = ?1
                 WHERE source_kind = ?2 AND source_id = ?3",
                params![now, row.source_kind, row.source_id],
            )?;
        }
        Ok(rows)
    }

    pub fn finish_recall_ingest(
        &self,
        source_kind: &str,
        source_id: &str,
        status: &str,
        error_class: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let class = error_class.map(|value| {
            let mut clipped: String = value.chars().take(64).collect();
            clipped.retain(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            clipped
        });
        conn.execute(
            "UPDATE recall_ingest_items
             SET status = ?1, last_error = ?2, updated_at = ?3
             WHERE source_kind = ?4 AND source_id = ?5",
            params![status, class, now, source_kind, source_id],
        )?;
        Ok(())
    }

    pub fn recall_ingest_status(
        &self,
        source_kind: &str,
        source_id: &str,
    ) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT status FROM recall_ingest_items WHERE source_kind = ?1 AND source_id = ?2",
            params![source_kind, source_id],
            |row| row.get(0),
        ) {
            Ok(status) => Ok(Some(status)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn schedule_active_tasks_with_prefix_now(
        &self,
        chat_id: i64,
        prompt_prefix: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let now = chrono::Utc::now().to_rfc3339();
        let like_pattern = format!("{prompt_prefix}%");
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE scheduled_tasks SET next_run = ?1
             WHERE chat_id = ?2 AND status = 'active' AND prompt LIKE ?3",
            params![now, chat_id, like_pattern],
        )?;
        Ok(())
    }

    pub fn recall_checkpoint(&self, scope: &str) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT cursor FROM recall_ingest_checkpoint WHERE scope = ?1",
            params![scope],
            |row| row.get(0),
        ) {
            Ok(cursor) => Ok(Some(cursor)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_recall_checkpoint(
        &self,
        scope: &str,
        cursor: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO recall_ingest_checkpoint (scope, cursor, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET cursor = excluded.cursor, updated_at = excluded.updated_at",
            params![scope, cursor, now],
        )?;
        Ok(())
    }

    pub fn recall_ingest_counts(&self) -> Result<(i64, i64), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM recall_ingest_items WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        let rejected: i64 = conn.query_row(
            "SELECT COUNT(*) FROM recall_ingest_items WHERE status = 'rejected'",
            [],
            |row| row.get(0),
        )?;
        Ok((pending, rejected))
    }

    pub fn stale_origin_source_ids(
        &self,
        generation: i64,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source_id FROM recall_ingest_items
             WHERE source_kind = 'origin' AND generation < ?1",
        )?;
        let ids = stmt
            .query_map(params![generation], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn delete_recall_ingest(
        &self,
        source_kind: &str,
        source_id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM recall_ingest_items WHERE source_kind = ?1 AND source_id = ?2",
            params![source_kind, source_id],
        )?;
        Ok(())
    }

    pub fn try_recall_lock(
        &self,
        owner: &str,
        ttl_secs: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let now = chrono::Utc::now();
        let existing: Option<(String, String)> = match conn.query_row(
            "SELECT owner, expires_at FROM recall_ingest_lock WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(pair) => Some(pair),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e.into());
            }
        };
        let held = existing.as_ref().is_some_and(|(current_owner, expires)| {
            current_owner != owner
                && chrono::DateTime::parse_from_rfc3339(expires)
                    .map(|ts| ts.with_timezone(&chrono::Utc) > now)
                    .unwrap_or(true)
        });
        if held {
            let _ = conn.execute_batch("ROLLBACK;");
            return Ok(false);
        }
        let expires = (now + chrono::Duration::seconds(ttl_secs)).to_rfc3339();
        if let Err(e) = conn.execute(
            "INSERT INTO recall_ingest_lock (id, owner, expires_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, expires_at = excluded.expires_at",
            params![owner, expires],
        ) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e.into());
        }
        conn.execute_batch("COMMIT;")?;
        Ok(true)
    }

    pub fn release_recall_lock(&self, owner: &str) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM recall_ingest_lock WHERE id = 1 AND owner = ?1",
            params![owner],
        )?;
        Ok(())
    }

    pub fn list_messages_after_cursor(
        &self,
        after_timestamp: &str,
        after_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE timestamp > ?1 OR (timestamp = ?1 AND id > ?2)
             ORDER BY timestamp ASC, id ASC
             LIMIT ?3"
        ))?;
        let messages = stmt
            .query_map(
                params![after_timestamp, after_id, limit as i64],
                stored_message_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn get_message_for_recall(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Option<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3"
        ))?;
        let mut rows = stmt.query_map(params![id, chat_id, persona_id], stored_message_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn ensure_onboarding_task(
        &self,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        // Only seed if no messages exist yet (fresh install)
        let message_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

        if message_count == 0 {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM scheduled_tasks WHERE prompt = ?1 AND status = 'active')",
                params![prompt],
                |row| row.get(0),
            )?;

            if !exists {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO scheduled_tasks (chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, status, created_at)
                     VALUES (?1, ?2, ?3, 'once', ?4, ?4, 'active', ?4)",
                    params![chat_id, persona_id, prompt, now],
                )?;
            }
        }
        Ok(())
    }

    pub fn get_due_tasks(&self, now: &str) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE status = 'active' AND next_run <= ?1
             ORDER BY next_run ASC, id ASC",
        )?;
        let tasks = stmt
            .query_map(params![now], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn get_all_active_tasks(&self) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE status IN ('active', 'running', 'paused')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map([], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    /// All scheduled tasks for /schedule and list_scheduled_tasks:
    /// active, running, paused, and completed (all chats/personas).
    pub fn get_all_scheduled_tasks_for_display(
        &self,
    ) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE status IN ('active', 'running', 'paused', 'completed')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map([], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    /// All scheduled tasks for display filtered by chat:
    /// active, running, paused, and completed.
    pub fn get_scheduled_tasks_for_chat_for_display(
        &self,
        chat_id: i64,
    ) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE chat_id = ?1 AND status IN ('active', 'running', 'paused', 'completed')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map(params![chat_id], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    /// Scheduled tasks for display filtered by chat and persona (active, running, paused, completed).
    pub fn get_scheduled_tasks_for_chat_and_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE chat_id = ?1 AND persona_id = ?2 AND status IN ('active', 'running', 'paused', 'completed')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map(params![chat_id, persona_id], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn get_tasks_for_chat(
        &self,
        chat_id: i64,
    ) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE chat_id = ?1 AND status IN ('active', 'running', 'paused')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map(params![chat_id], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    /// Active/running/paused scheduled tasks for one persona in a chat.
    pub fn get_tasks_for_chat_and_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE chat_id = ?1 AND persona_id = ?2 AND status IN ('active', 'running', 'paused')
             ORDER BY id",
        )?;
        let tasks = stmt
            .query_map(params![chat_id, persona_id], map_scheduled_task_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn get_task_by_id(
        &self,
        task_id: i64,
    ) -> Result<Option<ScheduledTask>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, chat_id, persona_id, prompt, schedule_type, schedule_value, next_run, last_run, status, created_at
             FROM scheduled_tasks
             WHERE id = ?1",
            params![task_id],
            map_scheduled_task_row,
        );
        match result {
            Ok(task) => Ok(Some(task)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update_task_status(
        &self,
        task_id: i64,
        status: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE scheduled_tasks SET status = ?1 WHERE id = ?2",
            params![status, task_id],
        )?;
        Ok(rows > 0)
    }

    pub fn update_task_persona(
        &self,
        task_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE scheduled_tasks SET persona_id = ?1 WHERE id = ?2",
            params![persona_id, task_id],
        )?;
        Ok(rows > 0)
    }

    pub fn update_task_prompt(
        &self,
        task_id: i64,
        prompt: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE scheduled_tasks SET prompt = ?1 WHERE id = ?2",
            params![prompt, task_id],
        )?;
        Ok(rows > 0)
    }

    /// Update cron/once schedule fields and next run (after preflight). Does not clear `last_run`.
    pub fn update_task_schedule(
        &self,
        task_id: i64,
        schedule_type: &str,
        schedule_value: &str,
        next_run: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE scheduled_tasks SET schedule_type = ?1, schedule_value = ?2, next_run = ?3 WHERE id = ?4",
            params![schedule_type, schedule_value, next_run, task_id],
        )?;
        Ok(rows > 0)
    }

    pub fn update_task_after_run(
        &self,
        task_id: i64,
        last_run: &str,
        next_run: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match next_run {
            Some(next) => {
                conn.execute(
                    "UPDATE scheduled_tasks SET last_run = ?1, next_run = ?2 WHERE id = ?3",
                    params![last_run, next, task_id],
                )?;
            }
            None => {
                // One-shot task, mark completed
                conn.execute(
                    "UPDATE scheduled_tasks SET last_run = ?1, status = 'completed' WHERE id = ?2",
                    params![last_run, task_id],
                )?;
            }
        }
        Ok(())
    }

    /// Mark a due task as running so it does not get picked again while executing.
    /// For cron tasks, next_run should be precomputed and stored here.
    pub fn mark_task_running(
        &self,
        task_id: i64,
        started_at: &str,
        next_run: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match next_run {
            Some(next) => {
                conn.execute(
                    "UPDATE scheduled_tasks
                     SET last_run = ?1, next_run = ?2, status = 'running'
                     WHERE id = ?3",
                    params![started_at, next, task_id],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE scheduled_tasks
                     SET last_run = ?1, status = 'running'
                     WHERE id = ?2",
                    params![started_at, task_id],
                )?;
            }
        }
        Ok(())
    }

    /// Atomic conditional claim: only marks running if the task is still active and due.
    /// Returns true iff exactly one row was updated. Callers should skip spawn when false.
    pub fn try_mark_task_running(
        &self,
        task_id: i64,
        started_at: &str,
        next_run: Option<&str>,
        now_upper_bound: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = match next_run {
            Some(next) => conn.execute(
                "UPDATE scheduled_tasks
                 SET last_run = ?1, next_run = ?2, status = 'running'
                 WHERE id = ?3 AND status = 'active' AND next_run <= ?4",
                params![started_at, next, task_id, now_upper_bound],
            )?,
            None => conn.execute(
                "UPDATE scheduled_tasks
                 SET last_run = ?1, status = 'running'
                 WHERE id = ?2 AND status = 'active' AND next_run <= ?3",
                params![started_at, task_id, now_upper_bound],
            )?,
        };
        Ok(rows == 1)
    }

    /// Reset tasks stuck in `running` (e.g. process crash or hung agent) back to `active`.
    /// `last_run` holds the claim/start time from `mark_task_running`. Does not change `next_run`.
    /// Returns IDs of reclaimed tasks.
    pub fn reclaim_stale_running_tasks(
        &self,
        now_rfc3339: &str,
        max_age_secs: i64,
    ) -> Result<Vec<i64>, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "reclaim_stale_running_tasks: invalid now timestamp: {e}"
                ))
            })?;

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, last_run FROM scheduled_tasks WHERE status = 'running' AND last_run IS NOT NULL",
        )?;
        let pending: Vec<(i64, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut reclaimed = Vec::new();
        for (id, last_run) in pending {
            let Ok(started) = DateTime::parse_from_rfc3339(&last_run) else {
                continue;
            };
            let started = started.with_timezone(&Utc);
            if now.signed_duration_since(started).num_seconds() > max_age_secs {
                conn.execute(
                    "UPDATE scheduled_tasks SET status = 'active' WHERE id = ?1",
                    params![id],
                )?;
                reclaimed.push(id);
            }
        }
        Ok(reclaimed)
    }

    /// Finalize a running task after execution.
    /// - Cron tasks (Some next_run) return to active with the provided next run.
    /// - One-shot tasks (None) are marked completed.
    pub fn finalize_task_run(
        &self,
        task_id: i64,
        next_run: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        match next_run {
            Some(next) => {
                conn.execute(
                    "UPDATE scheduled_tasks
                     SET next_run = ?1, status = 'active'
                     WHERE id = ?2",
                    params![next, task_id],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE scheduled_tasks SET status = 'completed' WHERE id = ?1",
                    params![task_id],
                )?;
            }
        }
        Ok(())
    }

    // --- Task run logs ---

    #[allow(clippy::too_many_arguments)]
    pub fn log_task_run(
        &self,
        task_id: i64,
        chat_id: i64,
        started_at: &str,
        finished_at: &str,
        duration_ms: i64,
        success: bool,
        result_summary: Option<&str>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO task_run_logs (task_id, chat_id, started_at, finished_at, duration_ms, success, result_summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                task_id,
                chat_id,
                started_at,
                finished_at,
                duration_ms,
                success as i32,
                result_summary,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_task_run_logs(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<TaskRunLog>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, chat_id, started_at, finished_at, duration_ms, success, result_summary
             FROM task_run_logs
             WHERE task_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let logs = stmt
            .query_map(params![task_id, limit as i64], |row| {
                Ok(TaskRunLog {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    chat_id: row.get(2)?,
                    started_at: row.get(3)?,
                    finished_at: row.get(4)?,
                    duration_ms: row.get(5)?,
                    success: row.get::<_, i32>(6)? != 0,
                    result_summary: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(logs)
    }

    // --- Cursor agent runs ---

    pub fn get_cursor_engine_agent_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        session_scope: &str,
    ) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT agent_id FROM cursor_engine_agents
             WHERE chat_id = ?1 AND persona_id = ?2 AND session_scope = ?3",
        )?;
        let mut rows = stmt.query(params![chat_id, persona_id, session_scope])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if id.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(id))
            }
        } else {
            Ok(None)
        }
    }

    pub fn set_cursor_engine_agent_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        session_scope: &str,
        agent_id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let updated_at = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cursor_engine_agents (chat_id, persona_id, session_scope, agent_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chat_id, persona_id, session_scope)
             DO UPDATE SET agent_id = excluded.agent_id, updated_at = excluded.updated_at",
            params![chat_id, persona_id, session_scope, agent_id, updated_at],
        )?;
        Ok(())
    }

    pub fn clear_cursor_engine_agent_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        session_scope: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM cursor_engine_agents
             WHERE chat_id = ?1 AND persona_id = ?2 AND session_scope = ?3",
            params![chat_id, persona_id, session_scope],
        )?;
        Ok(())
    }

    pub fn insert_cursor_agent_run(
        &self,
        chat_id: i64,
        channel: &str,
        prompt_preview: &str,
        workdir: Option<&str>,
        started_at: &str,
        finished_at: &str,
        success: bool,
        exit_code: Option<i32>,
        output_preview: Option<&str>,
        output_path: Option<&str>,
        tmux_session: Option<&str>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cursor_agent_runs (chat_id, channel, prompt_preview, workdir, started_at, finished_at, success, exit_code, output_preview, output_path, tmux_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                chat_id,
                channel,
                prompt_preview,
                workdir,
                started_at,
                finished_at,
                success as i32,
                exit_code,
                output_preview,
                output_path,
                tmux_session,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Get recent cursor-agent runs, optionally filtered by chat_id. Ordered by finished_at DESC.
    pub fn get_cursor_agent_runs(
        &self,
        chat_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<CursorAgentRun>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let runs: Vec<CursorAgentRun> = match chat_id {
            Some(cid) => {
                let mut stmt = conn.prepare(
                    "SELECT id, chat_id, channel, prompt_preview, workdir, started_at, finished_at, success, exit_code, output_preview, output_path, tmux_session
                     FROM cursor_agent_runs WHERE chat_id = ?1 ORDER BY finished_at DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![cid, limit as i64], |row| {
                    Ok(CursorAgentRun {
                        id: row.get(0)?,
                        chat_id: row.get(1)?,
                        channel: row.get(2)?,
                        prompt_preview: row.get(3)?,
                        workdir: row.get(4)?,
                        started_at: row.get(5)?,
                        finished_at: row.get(6)?,
                        success: row.get::<_, i32>(7)? != 0,
                        exit_code: row.get(8)?,
                        output_preview: row.get(9)?,
                        output_path: row.get(10)?,
                        tmux_session: row.get(11)?,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, chat_id, channel, prompt_preview, workdir, started_at, finished_at, success, exit_code, output_preview, output_path, tmux_session
                     FROM cursor_agent_runs ORDER BY finished_at DESC LIMIT ?1",
                )?;
                let rows = stmt.query_map(params![limit as i64], |row| {
                    Ok(CursorAgentRun {
                        id: row.get(0)?,
                        chat_id: row.get(1)?,
                        channel: row.get(2)?,
                        prompt_preview: row.get(3)?,
                        workdir: row.get(4)?,
                        started_at: row.get(5)?,
                        finished_at: row.get(6)?,
                        success: row.get::<_, i32>(7)? != 0,
                        exit_code: row.get(8)?,
                        output_preview: row.get(9)?,
                        output_path: row.get(10)?,
                        tmux_session: row.get(11)?,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
        };
        Ok(runs)
    }

    // --- Projects / workflows / timeline ---

    pub fn upsert_project(
        &self,
        owner_chat_id: i64,
        title: &str,
        project_type: &str,
        status: &str,
        canonical_path: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (owner_chat_id, title, project_type, status, canonical_path, metadata_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(owner_chat_id, title) DO UPDATE SET
               project_type = excluded.project_type,
               status = excluded.status,
               canonical_path = excluded.canonical_path,
               metadata_json = excluded.metadata_json,
               updated_at = excluded.updated_at",
            params![
                owner_chat_id,
                title,
                project_type,
                status,
                canonical_path,
                metadata_json.unwrap_or("{}"),
                now
            ],
        )?;
        let id = conn.query_row(
            "SELECT id FROM projects WHERE owner_chat_id = ?1 AND title = ?2",
            params![owner_chat_id, title],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(id)
    }

    pub fn get_recent_project_for_contact(
        &self,
        owner_chat_id: i64,
    ) -> Result<Option<ProjectRecord>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, owner_chat_id, title, project_type, status, canonical_path, metadata_json, updated_at
             FROM projects
             WHERE owner_chat_id = ?1
             ORDER BY updated_at DESC
             LIMIT 1",
            params![owner_chat_id],
            |row| {
                Ok(ProjectRecord {
                    id: row.get(0)?,
                    owner_chat_id: row.get(1)?,
                    title: row.get(2)?,
                    project_type: row.get(3)?,
                    status: row.get(4)?,
                    canonical_path: row.get(5)?,
                    metadata_json: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        );
        match result {
            Ok(project) => Ok(Some(project)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn touch_project_status(
        &self,
        project_id: i64,
        status: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE projects SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now, project_id],
        )?;
        Ok(())
    }

    pub fn link_project_run(
        &self,
        project_id: i64,
        run_key: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO project_runs (project_id, run_key, created_at) VALUES (?1, ?2, ?3)",
            params![project_id, run_key, now],
        )?;
        Ok(())
    }

    pub fn append_run_timeline_event(
        &self,
        run_key: &str,
        chat_id: i64,
        persona_id: i64,
        event_type: &str,
        payload_json: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO run_timeline_events (run_key, chat_id, persona_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run_key,
                chat_id,
                persona_id,
                event_type,
                payload_json.unwrap_or("{}"),
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_run_timeline_events(
        &self,
        run_key: &str,
        limit: usize,
    ) -> Result<Vec<RunTimelineEvent>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, run_key, chat_id, persona_id, event_type, payload_json, created_at
             FROM run_timeline_events
             WHERE run_key = ?1
             ORDER BY id ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![run_key, limit as i64], |row| {
                Ok(RunTimelineEvent {
                    id: row.get(0)?,
                    run_key: row.get(1)?,
                    chat_id: row.get(2)?,
                    persona_id: row.get(3)?,
                    event_type: row.get(4)?,
                    payload_json: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn append_persona_bulletin_event(
        &self,
        chat_id: i64,
        persona_id: i64,
        run_key: Option<&str>,
        event_type: &str,
        title: &str,
        detail: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO persona_bulletin_events (chat_id, persona_id, run_key, event_type, title, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![chat_id, persona_id, run_key, event_type, title, detail, now],
        )?;
        Ok(())
    }

    pub fn upsert_persona_bulletin_focus(
        &self,
        chat_id: i64,
        persona_id: i64,
        title: Option<&str>,
        content: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO persona_bulletin_focus (chat_id, persona_id, title, content, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chat_id, persona_id) DO UPDATE SET
               title = excluded.title,
               content = excluded.content,
               updated_at = excluded.updated_at",
            params![chat_id, persona_id, title, content, now],
        )?;
        Ok(())
    }

    pub fn get_persona_bulletin_focus(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Option<PersonaBulletinFocus>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT chat_id, persona_id, title, content, updated_at
             FROM persona_bulletin_focus
             WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
            |row| {
                Ok(PersonaBulletinFocus {
                    chat_id: row.get(0)?,
                    persona_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_persona_bulletin_events(
        &self,
        chat_id: i64,
        persona_id: i64,
        limit: usize,
    ) -> Result<Vec<PersonaBulletinEvent>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, persona_id, run_key, event_type, title, detail, created_at
             FROM persona_bulletin_events
             WHERE chat_id = ?1 AND persona_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
        )?;
        let items = stmt
            .query_map(params![chat_id, persona_id, limit as i64], |row| {
                Ok(PersonaBulletinEvent {
                    id: row.get(0)?,
                    chat_id: row.get(1)?,
                    persona_id: row.get(2)?,
                    run_key: row.get(3)?,
                    event_type: row.get(4)?,
                    title: row.get(5)?,
                    detail: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn upsert_persona_message_bookmark(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        role: &str,
        content_preview: &str,
        note: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO persona_message_bookmarks (
                chat_id, persona_id, message_id, role, content_preview, note, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(chat_id, persona_id, message_id) DO UPDATE SET
                role = excluded.role,
                content_preview = excluded.content_preview,
                note = excluded.note,
                updated_at = excluded.updated_at",
            params![
                chat_id,
                persona_id,
                message_id,
                role,
                content_preview,
                note,
                now
            ],
        )?;
        Ok(())
    }

    pub fn delete_persona_message_bookmark(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM persona_message_bookmarks
             WHERE chat_id = ?1 AND persona_id = ?2 AND message_id = ?3",
            params![chat_id, persona_id, message_id],
        )?;
        Ok(rows > 0)
    }

    pub fn list_persona_message_bookmarks(
        &self,
        chat_id: i64,
        persona_id: i64,
        limit: usize,
    ) -> Result<Vec<PersonaMessageBookmark>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT b.chat_id, b.persona_id, b.message_id, b.role, b.content_preview, b.note,
                    b.created_at, b.updated_at, m.session_id, m.timestamp,
                    CASE WHEN m.id IS NULL THEN 1 ELSE 0 END
             FROM persona_message_bookmarks b
             LEFT JOIN messages m
               ON m.chat_id = b.chat_id AND m.persona_id = b.persona_id AND m.id = b.message_id
             WHERE b.chat_id = ?1 AND b.persona_id = ?2
             ORDER BY b.updated_at DESC
             LIMIT ?3",
        )?;
        let items = stmt
            .query_map(params![chat_id, persona_id, limit as i64], |row| {
                let missing_i: i64 = row.get(10)?;
                Ok(PersonaMessageBookmark {
                    chat_id: row.get(0)?,
                    persona_id: row.get(1)?,
                    message_id: row.get(2)?,
                    role: row.get(3)?,
                    content_preview: row.get(4)?,
                    note: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    session_id: row.get(8)?,
                    message_timestamp: row.get(9)?,
                    missing: missing_i != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    fn normalize_side_chat_session_id(session_id: Option<&str>) -> String {
        session_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string()
    }

    fn side_chat_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SideChat> {
        Ok(SideChat {
            id: row.get(0)?,
            chat_id: row.get(1)?,
            persona_id: row.get(2)?,
            session_id: row.get(3)?,
            anchor_message_id: row.get(4)?,
            title: row.get(5)?,
            anchor_snippet: row.get(6)?,
            draft_text: row.get(7)?,
            turn_count: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    }

    /// Ensure a side chat exists for the given anchor; returns the existing or newly created row.
    pub fn ensure_side_chat(
        &self,
        chat_id: i64,
        persona_id: i64,
        session_id: Option<&str>,
        anchor_message_id: &str,
        anchor_snippet: &str,
    ) -> Result<SideChat, FinallyAValueBotError> {
        let session_key = Self::normalize_side_chat_session_id(session_id);
        let anchor_message_id = anchor_message_id.trim();
        if anchor_message_id.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "anchor_message_id is required".into(),
            ));
        }
        let snippet: String = anchor_snippet.chars().take(280).collect();
        let conn = self.conn.lock().unwrap();
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM side_chats
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3 AND anchor_message_id = ?4",
                params![chat_id, persona_id, session_key, anchor_message_id],
                |row| row.get(0),
            )
            .ok();
        let id = if let Some(id) = existing {
            id
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO side_chats (
                    id, chat_id, persona_id, session_id, anchor_message_id,
                    title, anchor_snippet, draft_text, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, '', ?6, '', ?7, ?7)",
                params![
                    id,
                    chat_id,
                    persona_id,
                    session_key,
                    anchor_message_id,
                    snippet,
                    now
                ],
            )?;
            id
        };
        drop(conn);
        self.get_side_chat(chat_id, persona_id, &id)?
            .ok_or_else(|| {
                FinallyAValueBotError::ToolExecution("side chat missing after ensure".into())
            })
    }

    pub fn list_side_chats(
        &self,
        chat_id: i64,
        persona_id: i64,
        limit: usize,
    ) -> Result<Vec<SideChat>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT sc.id, sc.chat_id, sc.persona_id, sc.session_id, sc.anchor_message_id,
                    sc.title, sc.anchor_snippet, sc.draft_text,
                    COALESCE((SELECT COUNT(*) FROM side_chat_turns t WHERE t.side_chat_id = sc.id), 0),
                    sc.created_at, sc.updated_at
             FROM side_chats sc
             WHERE sc.chat_id = ?1 AND sc.persona_id = ?2
             ORDER BY sc.updated_at DESC
             LIMIT ?3",
        )?;
        let items = stmt
            .query_map(
                params![chat_id, persona_id, limit as i64],
                Self::side_chat_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn get_side_chat(
        &self,
        chat_id: i64,
        persona_id: i64,
        side_chat_id: &str,
    ) -> Result<Option<SideChat>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT sc.id, sc.chat_id, sc.persona_id, sc.session_id, sc.anchor_message_id,
                    sc.title, sc.anchor_snippet, sc.draft_text,
                    COALESCE((SELECT COUNT(*) FROM side_chat_turns t WHERE t.side_chat_id = sc.id), 0),
                    sc.created_at, sc.updated_at
             FROM side_chats sc
             WHERE sc.id = ?1 AND sc.chat_id = ?2 AND sc.persona_id = ?3",
            params![side_chat_id, chat_id, persona_id],
            Self::side_chat_from_row,
        );
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_side_chat_turns(
        &self,
        side_chat_id: &str,
    ) -> Result<Vec<SideChatTurn>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, side_chat_id, seq, role, content, created_at
             FROM side_chat_turns
             WHERE side_chat_id = ?1
             ORDER BY seq ASC",
        )?;
        let items = stmt
            .query_map(params![side_chat_id], |row| {
                Ok(SideChatTurn {
                    id: row.get(0)?,
                    side_chat_id: row.get(1)?,
                    seq: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn get_side_chat_turn(
        &self,
        side_chat_id: &str,
        turn_id: &str,
    ) -> Result<Option<SideChatTurn>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, side_chat_id, seq, role, content, created_at
             FROM side_chat_turns
             WHERE side_chat_id = ?1 AND id = ?2
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![side_chat_id, turn_id], |row| {
            Ok(SideChatTurn {
                id: row.get(0)?,
                side_chat_id: row.get(1)?,
                seq: row.get(2)?,
                role: row.get(3)?,
                content: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(Ok(turn)) => Ok(Some(turn)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn patch_side_chat(
        &self,
        chat_id: i64,
        persona_id: i64,
        side_chat_id: &str,
        draft_text: Option<&str>,
        title: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        if draft_text.is_none() && title.is_none() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let rows = match (draft_text, title) {
            (Some(draft), Some(title)) => {
                let title_val: String = title.chars().take(120).collect();
                conn.execute(
                    "UPDATE side_chats SET draft_text = ?1, title = ?2, updated_at = ?3
                     WHERE id = ?4 AND chat_id = ?5 AND persona_id = ?6",
                    params![draft, title_val, now, side_chat_id, chat_id, persona_id],
                )?
            }
            (Some(draft), None) => conn.execute(
                "UPDATE side_chats SET draft_text = ?1, updated_at = ?2
                 WHERE id = ?3 AND chat_id = ?4 AND persona_id = ?5",
                params![draft, now, side_chat_id, chat_id, persona_id],
            )?,
            (None, Some(title)) => {
                let title_val: String = title.chars().take(120).collect();
                conn.execute(
                    "UPDATE side_chats SET title = ?1, updated_at = ?2
                     WHERE id = ?3 AND chat_id = ?4 AND persona_id = ?5",
                    params![title_val, now, side_chat_id, chat_id, persona_id],
                )?
            }
            (None, None) => 0,
        };
        Ok(rows > 0)
    }

    pub fn delete_side_chat(
        &self,
        chat_id: i64,
        persona_id: i64,
        side_chat_id: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let _ = tx.execute(
            "DELETE FROM side_chat_turns WHERE side_chat_id = ?1",
            params![side_chat_id],
        )?;
        let rows = tx.execute(
            "DELETE FROM side_chats WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3",
            params![side_chat_id, chat_id, persona_id],
        )?;
        tx.commit()?;
        Ok(rows > 0)
    }

    /// Append a user + assistant turn pair; caps at SIDE_CHAT_MAX_TURNS (drops oldest).
    pub fn append_side_chat_turns(
        &self,
        chat_id: i64,
        persona_id: i64,
        side_chat_id: &str,
        user_content: &str,
        assistant_content: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let user_content = user_content.trim();
        let assistant_content = assistant_content.trim();
        if user_content.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "user turn must not be empty".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let owned: i64 = conn.query_row(
            "SELECT COUNT(*) FROM side_chats WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3",
            params![side_chat_id, chat_id, persona_id],
            |row| row.get(0),
        )?;
        if owned == 0 {
            return Err(FinallyAValueBotError::ToolExecution(
                "side chat not found".into(),
            ));
        }
        let tx = conn.unchecked_transaction()?;
        let next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM side_chat_turns WHERE side_chat_id = ?1",
                params![side_chat_id],
                |row| row.get(0),
            )
            .unwrap_or(0)
            + 1;
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = uuid::Uuid::new_v4().to_string();
        let asst_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO side_chat_turns (id, side_chat_id, seq, role, content, created_at)
             VALUES (?1, ?2, ?3, 'user', ?4, ?5)",
            params![user_id, side_chat_id, next_seq, user_content, now],
        )?;
        tx.execute(
            "INSERT INTO side_chat_turns (id, side_chat_id, seq, role, content, created_at)
             VALUES (?1, ?2, ?3, 'assistant', ?4, ?5)",
            params![asst_id, side_chat_id, next_seq + 1, assistant_content, now],
        )?;

        // Cap total turns by deleting oldest seq values.
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM side_chat_turns WHERE side_chat_id = ?1",
            params![side_chat_id],
            |row| row.get(0),
        )?;
        if count > SIDE_CHAT_MAX_TURNS as i64 {
            let drop_n = count - SIDE_CHAT_MAX_TURNS as i64;
            tx.execute(
                "DELETE FROM side_chat_turns WHERE id IN (
                    SELECT id FROM side_chat_turns WHERE side_chat_id = ?1
                    ORDER BY seq ASC LIMIT ?2
                 )",
                params![side_chat_id, drop_n],
            )?;
        }

        // Title from first user turn if empty.
        let title: String = tx
            .query_row(
                "SELECT title FROM side_chats WHERE id = ?1",
                params![side_chat_id],
                |row| row.get(0),
            )
            .unwrap_or_default();
        if title.trim().is_empty() {
            let t: String = user_content.chars().take(80).collect();
            tx.execute(
                "UPDATE side_chats SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![t, now, side_chat_id],
            )?;
        } else {
            tx.execute(
                "UPDATE side_chats SET updated_at = ?1 WHERE id = ?2",
                params![now, side_chat_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// How many main-chat (or session) messages are at/after the given message (inclusive), newest-side window size.
    pub fn count_messages_from_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<usize>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let target: Option<(String, Option<String>)> = match conn.query_row(
            "SELECT timestamp, session_id FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3",
            params![chat_id, persona_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let Some((ts, msg_session)) = target else {
            return Ok(None);
        };
        let count = if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
            conn.query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp > ?4 OR (timestamp = ?4 AND id >= ?5))",
                params![chat_id, persona_id, sid, ts, message_id],
                |row| row.get::<_, i64>(0),
            )?
        } else if let Some(sid) = msg_session {
            // Message is in a focused session; count within that session.
            conn.query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp > ?4 OR (timestamp = ?4 AND id >= ?5))",
                params![chat_id, persona_id, sid, ts, message_id],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM messages
                     WHERE chat_id = ?1 AND persona_id = ?2
                       AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                       AND (timestamp > ?3 OR (timestamp = ?3 AND id >= ?4))"
                ),
                params![chat_id, persona_id, ts, message_id],
                |row| row.get::<_, i64>(0),
            )?
        };
        Ok(Some(count as usize))
    }

    fn resolve_message_cursor(
        conn: &rusqlite::Connection,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
    ) -> Result<Option<(String, Option<String>)>, FinallyAValueBotError> {
        match conn.query_row(
            "SELECT timestamp, session_id FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3",
            params![chat_id, persona_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn effective_message_scope(
        session_id: Option<&str>,
        msg_session: Option<String>,
    ) -> Option<String> {
        session_id
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or(msg_session)
    }

    /// Older messages strictly before `message_id` (oldest first). `has_older` when more exist.
    pub fn get_messages_before_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Option<(Vec<StoredMessage>, bool)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let Some((ts, msg_session)) =
            Self::resolve_message_cursor(&conn, chat_id, persona_id, message_id)?
        else {
            return Ok(None);
        };
        let fetch = (limit + 1) as i64;
        let mut older = if let Some(sid) = Self::effective_message_scope(session_id, msg_session) {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp < ?4 OR (timestamp = ?4 AND id < ?5))
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?6"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, sid, ts, message_id, fetch],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                   AND (timestamp < ?3 OR (timestamp = ?3 AND id < ?4))
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?5"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, ts, message_id, fetch],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let has_older = older.len() > limit;
        if has_older {
            older.truncate(limit);
        }
        older.reverse();
        Ok(Some((older, has_older)))
    }

    /// Newer messages strictly after `message_id` (oldest first). `has_newer` when more exist.
    pub fn get_messages_after_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Option<(Vec<StoredMessage>, bool)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let Some((ts, msg_session)) =
            Self::resolve_message_cursor(&conn, chat_id, persona_id, message_id)?
        else {
            return Ok(None);
        };
        let fetch = (limit + 1) as i64;
        let mut newer = if let Some(sid) = Self::effective_message_scope(session_id, msg_session) {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp > ?4 OR (timestamp = ?4 AND id > ?5))
                 ORDER BY timestamp ASC, id ASC
                 LIMIT ?6"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, sid, ts, message_id, fetch],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                   AND (timestamp > ?3 OR (timestamp = ?3 AND id > ?4))
                 ORDER BY timestamp ASC, id ASC
                 LIMIT ?5"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, ts, message_id, fetch],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let has_newer = newer.len() > limit;
        if has_newer {
            newer.truncate(limit);
        }
        Ok(Some((newer, has_newer)))
    }

    /// Window of messages around a target id (older pad + target + newer).
    pub fn get_messages_around_id(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
        before: usize,
        after: usize,
        session_id: Option<&str>,
    ) -> Result<Option<MessagePage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let Some((ts, msg_session)) =
            Self::resolve_message_cursor(&conn, chat_id, persona_id, message_id)?
        else {
            return Ok(None);
        };
        let scope = Self::effective_message_scope(session_id, msg_session);

        let mut older = if let Some(ref sid) = scope {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp < ?4 OR (timestamp = ?4 AND id < ?5))
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?6"
            ))?;
            let rows = stmt.query_map(
                params![
                    chat_id,
                    persona_id,
                    sid,
                    ts,
                    message_id,
                    (before + 1) as i64
                ],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                   AND (timestamp < ?3 OR (timestamp = ?3 AND id < ?4))
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?5"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, ts, message_id, (before + 1) as i64],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let has_older = older.len() > before;
        if has_older {
            older.truncate(before);
        }
        older.reverse();

        let mut newer_incl = if let Some(ref sid) = scope {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND (timestamp > ?4 OR (timestamp = ?4 AND id >= ?5))
                 ORDER BY timestamp ASC, id ASC
                 LIMIT ?6"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, sid, ts, message_id, (after + 2) as i64],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                   AND (timestamp > ?3 OR (timestamp = ?3 AND id >= ?4))
                 ORDER BY timestamp ASC, id ASC
                 LIMIT ?5"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, ts, message_id, (after + 2) as i64],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        // newer_incl includes the target; after+2 means target + after + 1 extra for has_newer.
        let has_newer = newer_incl.len() > after + 1;
        if has_newer {
            newer_incl.truncate(after + 1);
        }

        let mut out = older;
        out.extend(newer_incl);
        Ok(Some(MessagePage {
            messages: out,
            has_older,
            has_newer,
        }))
    }

    /// Recent messages for a focused session (oldest first), with `has_older` from overfetch.
    pub fn get_recent_messages_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<(Vec<StoredMessage>, bool), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE session_id = ?1
             ORDER BY timestamp DESC, id DESC
             LIMIT ?2"
        ))?;
        let mut messages = stmt
            .query_map(
                params![session_id, (limit + 1) as i64],
                stored_message_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_older = messages.len() > limit;
        if has_older {
            messages.truncate(limit);
        }
        messages.reverse();
        Ok((messages, has_older))
    }

    pub fn add_persona_todo(
        &self,
        chat_id: i64,
        persona_id: i64,
        title: &str,
        source_hint: Option<&str>,
    ) -> Result<PersonaTodo, FinallyAValueBotError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "Todo title must not be empty".into(),
            ));
        }
        let clipped_title: String = title.chars().take(500).collect();
        let clipped_hint = source_hint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(1000).collect::<String>());
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO persona_todos (
                chat_id, persona_id, title, status, source_hint, created_at, updated_at, completed_at,
                owner_persona_id, source_kind
             ) VALUES (?1, ?2, ?3, 'todo', ?4, ?5, ?5, NULL, ?2, 'manual')",
            params![chat_id, persona_id, clipped_title, clipped_hint, now],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE persona_todos SET position = ?1 WHERE id = ?2",
            params![id as f64, id],
        )?;
        drop(conn);
        self.get_persona_todo(id)?.ok_or_else(|| {
            FinallyAValueBotError::ToolExecution("todo insert did not return a row".into())
        })
    }

    pub fn list_persona_todos(
        &self,
        chat_id: i64,
        persona_id: i64,
        status_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PersonaTodo>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200) as i64;
        let mut out = Vec::new();
        if let Some(status) = status_filter {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PERSONA_TODO_COLS}
                 FROM persona_todos
                 WHERE chat_id = ?1 AND persona_id = ?2 AND (
                   (?3 = 'open' AND status IN ('open','todo','doing','blocked'))
                   OR (?3 <> 'open' AND status = ?3)
                 )
                 ORDER BY updated_at DESC
                 LIMIT ?4"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, status, limit],
                map_persona_todo,
            )?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PERSONA_TODO_COLS}
                 FROM persona_todos
                 WHERE chat_id = ?1 AND persona_id = ?2
                 ORDER BY
                   CASE WHEN status IN ('open','todo','doing','blocked') THEN 0 ELSE 1 END,
                   position ASC,
                   updated_at DESC
                 LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![chat_id, persona_id, limit], map_persona_todo)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn list_todos_for_chat(
        &self,
        chat_id: i64,
        status_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PersonaTodo>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 500) as i64;
        let mut out = Vec::new();
        if let Some(status) = status_filter {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PERSONA_TODO_COLS}
                 FROM persona_todos
                 WHERE chat_id = ?1 AND (
                   (?2 = 'open' AND status IN ('open','todo','doing','blocked'))
                   OR (?2 <> 'open' AND status = ?2)
                 )
                 ORDER BY position ASC, updated_at DESC
                 LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![chat_id, status, limit], map_persona_todo)?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PERSONA_TODO_COLS}
                 FROM persona_todos
                 WHERE chat_id = ?1
                 ORDER BY
                   CASE WHEN status IN ('open','todo','doing','blocked') THEN 0 ELSE 1 END,
                   position ASC,
                   updated_at DESC
                 LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![chat_id, limit], map_persona_todo)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn get_persona_todo(
        &self,
        todo_id: i64,
    ) -> Result<Option<PersonaTodo>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!("SELECT {PERSONA_TODO_COLS} FROM persona_todos WHERE id = ?1"),
            params![todo_id],
            map_persona_todo,
        );
        match result {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_persona_todo_status(
        &self,
        todo_id: i64,
        status: &str,
    ) -> Result<Option<PersonaTodo>, FinallyAValueBotError> {
        let status = normalize_board_status(status)?;
        let conn = self.conn.lock().unwrap();
        let previous: Option<String> = conn
            .query_row(
                "SELECT status FROM persona_todos WHERE id = ?1",
                params![todo_id],
                |row| row.get(0),
            )
            .ok();
        let now = chrono::Utc::now().to_rfc3339();
        let completed_at: Option<String> = if status == "done" {
            Some(now.clone())
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE persona_todos
             SET status = ?1, updated_at = ?2, completed_at = ?3
             WHERE id = ?4",
            params![status, now, completed_at, todo_id],
        )?;
        if rows > 0 {
            if let Some(from) = previous.as_deref() {
                if from != status {
                    conn.execute(
                        "INSERT INTO board_item_events (chat_id, item_id, from_status, to_status, actor_kind, actor_persona_id, at)
                         SELECT chat_id, id, ?1, ?2, 'persona', persona_id, ?3 FROM persona_todos WHERE id = ?4",
                        params![from, status, now, todo_id],
                    )?;
                }
            }
        }
        if rows == 0 {
            return Ok(None);
        }
        drop(conn);
        self.get_persona_todo(todo_id)
    }

    pub fn message_exists_in_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM messages
                WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3
            )",
            params![chat_id, persona_id, message_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn get_message_for_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        message_id: &str,
    ) -> Result<Option<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!(
                "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3
             LIMIT 1"
            ),
            params![chat_id, persona_id, message_id],
            stored_message_from_row,
        );
        match result {
            Ok(msg) => Ok(Some(msg)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // --- Background jobs ---

    pub fn create_background_job(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        trigger_reason: &str,
        session_id: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
        conn.execute(
            "INSERT INTO background_jobs (id, chat_id, persona_id, prompt, status, trigger_reason, created_at, last_progress_at, last_stage, job_kind, session_id)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?6, 'pending', 'agent', ?7)",
            params![id, chat_id, persona_id, prompt, trigger_reason, now, session_id],
        )?;
        Ok(())
    }

    pub fn create_background_run_optimize_job(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
        history_filename: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let label = "Learn & optimize";
        conn.execute(
            "INSERT INTO background_jobs (id, chat_id, persona_id, prompt, status, trigger_reason, created_at, last_progress_at, last_stage, job_kind, label)
             VALUES (?1, ?2, ?3, ?4, 'pending', 'run_optimize', ?5, ?5, 'pending', 'run_optimize', ?6)",
            params![id, chat_id, persona_id, history_filename, now, label],
        )?;
        Ok(())
    }

    pub fn create_background_shell_job(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
        label: &str,
        shell_command: &str,
        workdir: &str,
        tmux_session: &str,
        output_path: &str,
        trigger_reason: &str,
        session_id: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
        conn.execute(
            "INSERT INTO background_jobs (
                id, chat_id, persona_id, prompt, status, trigger_reason, created_at, last_progress_at, last_stage,
                job_kind, shell_command, workdir, tmux_session, output_path, label, session_id
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?6, 'pending', 'shell', ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                chat_id,
                persona_id,
                label,
                trigger_reason,
                now,
                shell_command,
                workdir,
                tmux_session,
                output_path,
                label,
                session_id,
            ],
        )?;
        Ok(())
    }

    /// External system id only (ComfyUI `prompt_id`, etc.): visible in queue, does not block handoff/shell slots.
    pub fn create_background_tracked_job(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
        prompt: &str,
        label: Option<&str>,
        trigger_reason: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO background_jobs (
                id, chat_id, persona_id, prompt, status, trigger_reason,
                created_at, started_at, last_progress_at, last_stage, job_kind, label
             ) VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?6, ?6, 'external_queue', 'tracked', ?7)",
            params![id, chat_id, persona_id, prompt, trigger_reason, now, label],
        )?;
        Ok(())
    }

    pub fn set_background_shell_paths(
        &self,
        id: &str,
        tmux_session: &str,
        output_path: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE background_jobs SET tmux_session = ?1, output_path = ?2 WHERE id = ?3",
            params![tmux_session, output_path, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "set_background_shell_paths: no job row for id {id}"
            )));
        }
        Ok(())
    }

    /// Record that the user was notified about a terminal shell job (e.g. after silent reconcile).
    pub fn record_background_shell_user_notification(
        &self,
        id: &str,
        result_text: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE background_jobs
             SET result_text = ?1,
                 last_progress_at = ?2,
                 last_stage = 'user_notified'
             WHERE id = ?3",
            params![result_text, now, id],
        )?;
        Ok(())
    }

    /// Agent background jobs enqueued after a successful shell job (`shell_success_followup:{parent_id}:N`).
    pub fn count_shell_success_agent_followups(
        &self,
        parent_shell_job_id: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("shell_success_followup:{parent_shell_job_id}:%");
        let count = conn.query_row(
            "SELECT COUNT(*) FROM background_jobs
             WHERE job_kind = 'agent' AND trigger_reason LIKE ?1",
            params![pattern],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count)
    }

    pub fn mark_background_shell_agent_success_followup_enqueued(
        &self,
        id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE background_jobs
             SET last_progress_at = ?1, last_stage = 'agent_success_followup_enqueued'
             WHERE id = ?2 AND job_kind = 'shell'",
            params![now, id],
        )?;
        Ok(())
    }

    /// Agent background jobs enqueued after a shell failure (`shell_failure_retry:{parent_id}:N`).
    pub fn count_shell_failure_agent_retries(
        &self,
        parent_shell_job_id: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("shell_failure_retry:{parent_shell_job_id}:%");
        let count = conn.query_row(
            "SELECT COUNT(*) FROM background_jobs
             WHERE job_kind = 'agent' AND trigger_reason LIKE ?1",
            params![pattern],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count)
    }

    pub fn mark_background_shell_agent_retry_enqueued(
        &self,
        id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE background_jobs
             SET last_progress_at = ?1, last_stage = 'agent_retry_enqueued'
             WHERE id = ?2 AND job_kind = 'shell'",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn mark_background_shell_finished(
        &self,
        id: &str,
        exit_code: i32,
        result_text: &str,
        success: bool,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        if success {
            conn.execute(
                "UPDATE background_jobs
                 SET status = 'done',
                     finished_at = ?1,
                     result_text = ?2,
                     exit_code = ?3,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     last_progress_at = ?1,
                     last_stage = 'done'
                 WHERE id = ?4",
                params![now, result_text, exit_code, id],
            )?;
        } else {
            conn.execute(
                "UPDATE background_jobs
                 SET status = 'failed',
                     finished_at = ?1,
                     error_text = ?2,
                     result_text = ?2,
                     exit_code = ?3,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     last_progress_at = ?1,
                     last_stage = 'failed'
                 WHERE id = ?4",
                params![now, result_text, exit_code, id],
            )?;
        }
        Ok(())
    }

    pub fn list_shell_jobs_needing_notification(
        &self,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE job_kind = 'shell'
               AND status = 'failed'
               AND (result_text IS NULL OR TRIM(result_text) = '')"
        ))?;
        let jobs = stmt
            .query_map([], map_background_job_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    pub fn list_running_shell_background_jobs(
        &self,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE job_kind = 'shell' AND status = 'running' AND tmux_session IS NOT NULL"
        ))?;
        let jobs = stmt
            .query_map([], map_background_job_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    /// Running shell jobs plus prematurely-failed rows still awaiting user notification.
    pub fn list_shell_jobs_for_monitor(&self) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE job_kind = 'shell'
               AND tmux_session IS NOT NULL
               AND (
                    status = 'running'
                    OR (
                        status = 'failed'
                        AND (result_text IS NULL OR TRIM(result_text) = '')
                    )
               )"
        ))?;
        let jobs = stmt
            .query_map([], map_background_job_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    pub fn list_shell_jobs_with_expired_lease(
        &self,
        now_rfc3339: &str,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE job_kind = 'shell'
               AND status = 'running'
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < ?1"
        ))?;
        let jobs = stmt
            .query_map(params![now_rfc3339], map_background_job_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    pub fn list_active_background_jobs_for_chat(
        &self,
        chat_id: i64,
        now_rfc3339: &str,
        pending_timeout_secs: i64,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "list_active_background_jobs_for_chat: invalid now timestamp: {e}"
                ))
            })?;
        let pending_cutoff =
            (now - chrono::Duration::seconds(pending_timeout_secs.max(1))).to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE chat_id = ?1
               AND (
                    (job_kind IS NULL OR job_kind NOT IN ('tracked'))
                    AND (
                         (status = 'pending' AND created_at >= ?2)
                         OR (
                             status IN ('running', 'completed_raw', 'main_agent_processing')
                             AND COALESCE(lease_expires_at, '9999-12-31T23:59:59+00:00') >= ?3
                         )
                    )
                    OR (job_kind = 'tracked' AND status = 'running')
               )
             ORDER BY created_at ASC"
        ))?;
        let jobs = stmt
            .query_map(
                params![chat_id, pending_cutoff, now_rfc3339],
                map_background_job_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    pub fn claim_background_job_running(
        &self,
        id: &str,
        lease_owner: &str,
        lease_ttl_secs: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();
        let lease_expires =
            (now_dt + chrono::Duration::seconds(lease_ttl_secs.max(1))).to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'running',
                 started_at = COALESCE(started_at, ?1),
                 lease_owner = ?2,
                 lease_expires_at = ?3,
                 last_progress_at = ?1,
                 last_stage = 'running'
             WHERE id = ?4 AND status = 'pending'",
            params![now, lease_owner, lease_expires, id],
        )?;
        Ok(rows == 1)
    }

    pub fn renew_background_job_lease(
        &self,
        id: &str,
        lease_ttl_secs: i64,
        stage: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();
        let lease_expires =
            (now_dt + chrono::Duration::seconds(lease_ttl_secs.max(1))).to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET lease_expires_at = ?1,
                 last_progress_at = ?2,
                 last_stage = ?3
             WHERE id = ?4
               AND status IN ('running', 'completed_raw', 'main_agent_processing')",
            params![lease_expires, now, stage, id],
        )?;
        if rows == 0 {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn mark_background_job_completed_raw(
        &self,
        id: &str,
        result_text: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();
        let lease_expires = (now_dt + chrono::Duration::seconds(120)).to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'completed_raw',
                 finished_at = ?1,
                 result_text = ?2,
                 lease_expires_at = ?3,
                 last_progress_at = ?1,
                 last_stage = 'completed_raw'
             WHERE id = ?4",
            params![now, result_text, lease_expires, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "mark_background_job_completed_raw: no job row for id {id}"
            )));
        }
        Ok(())
    }

    pub fn mark_background_job_main_agent_processing(
        &self,
        id: &str,
        lease_ttl_secs: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();
        let lease_expires =
            (now_dt + chrono::Duration::seconds(lease_ttl_secs.max(1))).to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'main_agent_processing',
                 lease_expires_at = ?1,
                 last_progress_at = ?2,
                 last_stage = 'main_agent_processing'
             WHERE id = ?3",
            params![lease_expires, now, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "mark_background_job_main_agent_processing: no job row for id {id}"
            )));
        }
        Ok(())
    }

    pub fn mark_background_job_done(&self, id: &str) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'done',
                 finished_at = COALESCE(finished_at, ?1),
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_progress_at = ?1,
                 last_stage = 'done'
             WHERE id = ?2",
            params![now, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "mark_background_job_done: no job row for id {id}"
            )));
        }
        Ok(())
    }

    pub fn fail_background_job(
        &self,
        id: &str,
        error_text: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'failed',
                 finished_at = ?1,
                 error_text = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_progress_at = ?1,
                 last_stage = 'failed'
             WHERE id = ?3",
            params![now, error_text, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "fail_background_job: no job row for id {id}"
            )));
        }
        Ok(())
    }

    pub fn mark_background_job_cancelled(
        &self,
        id: &str,
        reason: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE background_jobs
             SET status = 'cancelled',
                 finished_at = ?1,
                 error_text = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_progress_at = ?1,
                 last_stage = 'cancelled'
             WHERE id = ?3",
            params![now, reason, id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "mark_background_job_cancelled: no job row for id {id}"
            )));
        }
        Ok(())
    }

    pub fn count_active_background_jobs_for_chat(
        &self,
        chat_id: i64,
        now_rfc3339: &str,
        pending_timeout_secs: i64,
    ) -> Result<i64, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "count_active_background_jobs_for_chat: invalid now timestamp: {e}"
                ))
            })?;
        let pending_cutoff =
            (now - chrono::Duration::seconds(pending_timeout_secs.max(1))).to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let count = conn.query_row(
            "SELECT COUNT(*) FROM background_jobs
             WHERE chat_id = ?1
               AND (job_kind IS NULL OR job_kind NOT IN ('tracked'))
               AND (
                    (status = 'pending' AND created_at >= ?2)
                    OR (
                        status IN ('running', 'completed_raw', 'main_agent_processing')
                        AND COALESCE(lease_expires_at, '9999-12-31T23:59:59+00:00') >= ?3
                    )
               )",
            params![chat_id, pending_cutoff, now_rfc3339],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count)
    }

    /// True when this persona still has unfinished background work (shell, agent, or tracked).
    pub fn has_unfinished_background_jobs_for_chat_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM background_jobs
             WHERE chat_id = ?1 AND persona_id = ?2
               AND finished_at IS NULL
               AND status IN ('pending', 'running', 'completed_raw', 'main_agent_processing')",
            params![chat_id, persona_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_background_jobs_for_chat(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{BG_JOB_SELECT}
             FROM background_jobs
             WHERE chat_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
        ))?;
        let jobs = stmt
            .query_map(params![chat_id, limit as i64], map_background_job_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    pub fn list_artifact_jobs(
        &self,
        chat_id: i64,
        persona_id: i64,
        session_id: Option<&str>,
    ) -> Result<Vec<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt=conn.prepare(&format!("{BG_JOB_SELECT} FROM background_jobs WHERE chat_id=?1 AND persona_id=?2 AND session_id IS ?3 ORDER BY created_at DESC LIMIT 10"))?;
        let rows = stmt
            .query_map(
                params![chat_id, persona_id, session_id],
                map_background_job_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_background_job(
        &self,
        id: &str,
    ) -> Result<Option<BackgroundJob>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!("{BG_JOB_SELECT} FROM background_jobs WHERE id = ?1"),
            params![id],
            map_background_job_row,
        );
        match result {
            Ok(job) => Ok(Some(job)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn upsert_job_heartbeat(
        &self,
        run_key: &str,
        chat_id: i64,
        persona_id: i64,
        job_type: &str,
        stage: &str,
        message: &str,
        active: bool,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO job_heartbeats (run_key, chat_id, persona_id, job_type, stage, message, active, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(run_key) DO UPDATE SET
               stage = excluded.stage,
               message = excluded.message,
               active = excluded.active,
               updated_at = excluded.updated_at",
            params![
                run_key,
                chat_id,
                persona_id,
                job_type,
                stage,
                message,
                if active { 1 } else { 0 },
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_job_heartbeat(
        &self,
        run_key: &str,
    ) -> Result<Option<JobHeartbeat>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT run_key, chat_id, persona_id, job_type, stage, message, active, updated_at
             FROM job_heartbeats
             WHERE run_key = ?1",
            params![run_key],
            |row| {
                Ok(JobHeartbeat {
                    run_key: row.get(0)?,
                    chat_id: row.get(1)?,
                    persona_id: row.get(2)?,
                    job_type: row.get(3)?,
                    stage: row.get(4)?,
                    message: row.get(5)?,
                    active: row.get::<_, i32>(6)? != 0,
                    updated_at: row.get(7)?,
                })
            },
        );
        match result {
            Ok(h) => Ok(Some(h)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Active heartbeats for a chat (e.g. operator visibility, dashboards).
    pub fn list_active_job_heartbeats_for_chat(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<JobHeartbeat>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let lim = limit.max(1).min(100) as i64;
        let mut stmt = conn.prepare(
            "SELECT run_key, chat_id, persona_id, job_type, stage, message, active, updated_at
             FROM job_heartbeats
             WHERE chat_id = ?1 AND active = 1
             ORDER BY updated_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![chat_id, lim], |row| {
                Ok(JobHeartbeat {
                    run_key: row.get(0)?,
                    chat_id: row.get(1)?,
                    persona_id: row.get(2)?,
                    job_type: row.get(3)?,
                    stage: row.get(4)?,
                    message: row.get(5)?,
                    active: row.get::<_, i32>(6)? != 0,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Recent heartbeats for a chat (including completed), for merging into job lists.
    pub fn list_job_heartbeats_for_chat(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<JobHeartbeat>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let lim = limit.max(1).min(200) as i64;
        let mut stmt = conn.prepare(
            "SELECT run_key, chat_id, persona_id, job_type, stage, message, active, updated_at
             FROM job_heartbeats
             WHERE chat_id = ?1
             ORDER BY updated_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![chat_id, lim], |row| {
                Ok(JobHeartbeat {
                    run_key: row.get(0)?,
                    chat_id: row.get(1)?,
                    persona_id: row.get(2)?,
                    job_type: row.get(3)?,
                    stage: row.get(4)?,
                    message: row.get(5)?,
                    active: row.get::<_, i32>(6)? != 0,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark active heartbeats stale if `updated_at` is older than `max_age_secs`, append timeline
    /// events, and fail matching `manual_background` rows in `background_jobs`.
    pub fn reconcile_stale_active_job_heartbeats(
        &self,
        now_rfc3339: &str,
        max_age_secs: i64,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "reconcile_stale_active_job_heartbeats: invalid now timestamp: {e}"
                ))
            })?;

        let stale_msg = "stale — no recent heartbeat (process may have exited)";
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_key, chat_id, persona_id, job_type, updated_at
             FROM job_heartbeats
             WHERE active = 1",
        )?;
        let rows: Vec<(String, i64, i64, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut reconciled = Vec::new();
        let now_str = now.to_rfc3339();

        for (run_key, chat_id, persona_id, job_type, updated_at) in rows {
            let Ok(updated) = DateTime::parse_from_rfc3339(&updated_at) else {
                continue;
            };
            let updated = updated.with_timezone(&Utc);
            if now.signed_duration_since(updated).num_seconds() <= max_age_secs {
                continue;
            }

            conn.execute(
                "UPDATE job_heartbeats
                 SET stage = 'failed', message = ?1, active = 0, updated_at = ?2
                 WHERE run_key = ?3 AND active = 1",
                params![stale_msg, now_str, run_key],
            )?;

            let payload = format!(
                r#"{{"stage":"failed","message":"{}","reason":"stale_reconcile"}}"#,
                stale_msg.replace('"', "'")
            );
            conn.execute(
                "INSERT INTO run_timeline_events (run_key, chat_id, persona_id, event_type, payload_json, created_at)
                 VALUES (?1, ?2, ?3, 'heartbeat', ?4, ?5)",
                params![run_key, chat_id, persona_id, payload, now_str],
            )?;

            if job_type == "manual_background" {
                let _ = conn.execute(
                    "UPDATE background_jobs
                     SET status = 'failed',
                         finished_at = ?1,
                         error_text = ?2,
                         lease_owner = NULL,
                         lease_expires_at = NULL,
                         last_progress_at = ?1,
                         last_stage = 'failed'
                     WHERE id = ?3
                       AND status IN ('pending', 'running', 'completed_raw', 'main_agent_processing')",
                    params![now_str, stale_msg, run_key],
                );
            }
            reconciled.push(run_key);
        }

        Ok(reconciled)
    }

    /// Fail web background jobs that are still pending and never started within the allowed age.
    pub fn reconcile_stale_pending_background_jobs(
        &self,
        now_rfc3339: &str,
        max_pending_secs: i64,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "reconcile_stale_pending_background_jobs: invalid now timestamp: {e}"
                ))
            })?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, created_at
             FROM background_jobs
             WHERE status = 'pending'
               AND started_at IS NULL",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let stale_msg = "stale pending job — worker never confirmed start";
        let now_str = now.to_rfc3339();
        let mut out = Vec::new();

        for (id, created_at) in rows {
            let Ok(created) = DateTime::parse_from_rfc3339(&created_at) else {
                continue;
            };
            let created = created.with_timezone(&Utc);
            if now.signed_duration_since(created).num_seconds() <= max_pending_secs {
                continue;
            }
            let n = conn.execute(
                "UPDATE background_jobs
                 SET status = 'failed',
                     finished_at = ?1,
                     error_text = ?2,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     last_progress_at = ?1,
                     last_stage = 'failed'
                 WHERE id = ?3
                   AND status = 'pending'
                   AND started_at IS NULL",
                params![now_str, stale_msg, id],
            )?;
            if n > 0 {
                out.push(id);
            }
        }

        Ok(out)
    }

    /// Fail active web background jobs with expired leases.
    pub fn reconcile_expired_background_job_leases(
        &self,
        now_rfc3339: &str,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let stale_msg = "stale lease expired — worker heartbeat not renewed";
        let mut stmt = conn.prepare(
            "SELECT id
             FROM background_jobs
             WHERE status IN ('running', 'completed_raw', 'main_agent_processing')
               AND COALESCE(job_kind, 'agent') != 'shell'
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < ?1",
        )?;
        let rows: Vec<String> = stmt
            .query_map(params![now_rfc3339], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut out = Vec::new();
        for id in rows {
            let n = conn.execute(
                "UPDATE background_jobs
                 SET status = 'failed',
                     finished_at = ?1,
                     error_text = ?2,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     last_progress_at = ?1,
                     last_stage = 'failed'
                 WHERE id = ?3
                   AND status IN ('running', 'completed_raw', 'main_agent_processing')
                   AND COALESCE(job_kind, 'agent') != 'shell'",
                params![now_rfc3339, stale_msg, id],
            )?;
            if n > 0 {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Fail web `background_jobs` rows that never got a heartbeat row but stayed active too long.
    pub fn reconcile_orphan_stale_background_jobs(
        &self,
        now_rfc3339: &str,
        max_age_secs: i64,
    ) -> Result<Vec<String>, FinallyAValueBotError> {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(now_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "reconcile_orphan_stale_background_jobs: invalid now timestamp: {e}"
                ))
            })?;

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT b.id, b.chat_id, b.persona_id, b.started_at
             FROM background_jobs b
             LEFT JOIN job_heartbeats h ON h.run_key = b.id
             WHERE b.status IN ('pending', 'running', 'completed_raw', 'main_agent_processing')
               AND b.started_at IS NOT NULL
               AND h.run_key IS NULL",
        )?;
        let rows: Vec<(String, i64, i64, String)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let stale_msg = "stale — no heartbeat record (worker may have crashed before registration)";
        let mut out = Vec::new();
        let now_str = now.to_rfc3339();

        for (id, _chat_id, _persona_id, started_at) in rows {
            let Ok(started) = DateTime::parse_from_rfc3339(&started_at) else {
                continue;
            };
            let started = started.with_timezone(&Utc);
            if now.signed_duration_since(started).num_seconds() <= max_age_secs {
                continue;
            }
            let n = conn.execute(
                "UPDATE background_jobs
                 SET status = 'failed',
                     finished_at = ?1,
                     error_text = ?2,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     last_progress_at = ?1,
                     last_stage = 'failed'
                 WHERE id = ?3
                   AND status IN ('pending', 'running', 'completed_raw', 'main_agent_processing')",
                params![now_str, stale_msg, id],
            )?;
            if n > 0 {
                out.push(id);
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn delete_task(&self, task_id: i64) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM scheduled_tasks WHERE id = ?1",
            params![task_id],
        )?;
        Ok(rows > 0)
    }

    // --- Sessions ---

    pub fn save_session(
        &self,
        chat_id: i64,
        persona_id: i64,
        messages_json: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sessions (chat_id, persona_id, messages_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id, persona_id) DO UPDATE SET
                messages_json = ?3,
                updated_at = ?4",
            params![chat_id, persona_id, messages_json, now],
        )?;
        Ok(())
    }

    pub fn load_session(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Option<(String, String)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT messages_json, updated_at FROM sessions WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        match result {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_session(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM sessions WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        Ok(rows > 0)
    }

    pub fn delete_chat_data(&self, chat_id: i64) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut affected = 0usize;

        affected += tx.execute(
            "UPDATE chats SET active_persona_id = NULL WHERE chat_id = ?1",
            params![chat_id],
        )?;
        affected += tx.execute("DELETE FROM sessions WHERE chat_id = ?1", params![chat_id])?;
        affected += tx.execute("DELETE FROM messages WHERE chat_id = ?1", params![chat_id])?;
        affected += tx.execute(
            "DELETE FROM conversation_topics WHERE chat_id = ?1",
            params![chat_id],
        )?;
        affected += tx.execute("DELETE FROM personas WHERE chat_id = ?1", params![chat_id])?;
        affected += tx.execute(
            "DELETE FROM scheduled_tasks WHERE chat_id = ?1",
            params![chat_id],
        )?;
        affected += tx.execute(
            "DELETE FROM social_oauth_tokens WHERE chat_id = ?1",
            params![chat_id],
        )?;
        affected += tx.execute(
            "DELETE FROM channel_bindings WHERE canonical_chat_id = ?1",
            params![chat_id],
        )?;
        affected += tx.execute("DELETE FROM chats WHERE chat_id = ?1", params![chat_id])?;

        tx.commit()?;
        Ok(affected > 0)
    }

    // --- Social OAuth tokens ---

    pub fn upsert_social_token(
        &self,
        platform: &str,
        chat_id: i64,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO social_oauth_tokens (platform, chat_id, access_token, refresh_token, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(platform, chat_id) DO UPDATE SET
                access_token = ?3,
                refresh_token = ?4,
                expires_at = ?5",
            params![platform, chat_id, access_token, refresh_token, expires_at],
        )?;
        Ok(())
    }

    pub fn get_social_token(
        &self,
        platform: &str,
        chat_id: i64,
    ) -> Result<Option<SocialOAuthToken>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT platform, chat_id, access_token, refresh_token, expires_at
             FROM social_oauth_tokens
             WHERE platform = ?1 AND chat_id = ?2",
            params![platform, chat_id],
            |row| {
                Ok(SocialOAuthToken {
                    platform: row.get(0)?,
                    chat_id: row.get(1)?,
                    access_token: row.get(2)?,
                    refresh_token: row.get(3)?,
                    expires_at: row.get(4)?,
                })
            },
        );
        match result {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_social_token(
        &self,
        platform: &str,
        chat_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM social_oauth_tokens WHERE platform = ?1 AND chat_id = ?2",
            params![platform, chat_id],
        )?;
        Ok(rows > 0)
    }

    // --- OAuth pending states (short-lived mapping from state param to chat_id) ---

    pub fn create_oauth_pending_state(
        &self,
        state_token: &str,
        platform: &str,
        chat_id: i64,
        expires_at: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO oauth_pending_states (state_token, platform, chat_id, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![state_token, platform, chat_id, expires_at],
        )?;
        Ok(())
    }

    pub fn consume_oauth_pending_state(
        &self,
        state_token: &str,
    ) -> Result<Option<(String, i64)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT platform, chat_id FROM oauth_pending_states
             WHERE state_token = ?1 AND expires_at > datetime('now')",
            params![state_token],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        );
        let pair = match result {
            Ok(p) => p,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        conn.execute(
            "DELETE FROM oauth_pending_states WHERE state_token = ?1",
            params![state_token],
        )?;
        Ok(Some(pair))
    }

    pub fn get_new_user_messages_since(
        &self,
        chat_id: i64,
        persona_id: i64,
        since: &str,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND timestamp > ?3 AND is_from_bot = 0
             ORDER BY timestamp ASC"
        ))?;
        let messages = stmt
            .query_map(params![chat_id, persona_id, since], stored_message_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    // --- Personas ---

    pub fn get_or_create_default_persona(
        &self,
        chat_id: i64,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result: Option<i64> = conn
            .query_row(
                "SELECT active_persona_id FROM chats WHERE chat_id = ?1",
                params![chat_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        if let Some(pid) = result {
            if pid > 0 {
                return Ok(pid);
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO personas (chat_id, name, model_override, agent_engine_override) VALUES (?1, 'default', NULL, 'classic')",
            params![chat_id],
        )?;
        let persona_id: i64 = conn.query_row(
            "SELECT id FROM personas WHERE chat_id = ?1 AND name = 'default'",
            params![chat_id],
            |row| row.get(0),
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO chats (chat_id, chat_title, chat_type, last_message_time, active_persona_id)
             VALUES (?1, NULL, 'private', ?2, ?3)
             ON CONFLICT(chat_id) DO UPDATE SET active_persona_id = ?3",
            params![chat_id, now, persona_id],
        )?;
        Ok(persona_id)
    }

    pub fn get_active_persona_id(
        &self,
        chat_id: i64,
    ) -> Result<Option<i64>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT active_persona_id FROM chats WHERE chat_id = ?1",
            params![chat_id],
            |row| row.get::<_, Option<i64>>(0),
        );
        match result {
            Ok(Some(pid)) if pid > 0 => Ok(Some(pid)),
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve the persona to use for this run: active when set, else create/set default.
    pub fn get_current_persona_id(&self, chat_id: i64) -> Result<i64, FinallyAValueBotError> {
        if let Ok(Some(pid)) = self.get_active_persona_id(chat_id) {
            return Ok(pid);
        }
        self.get_or_create_default_persona(chat_id)
    }

    pub fn persona_exists(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM personas
                WHERE chat_id = ?1 AND id = ?2
            )",
            params![chat_id, persona_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn set_active_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE chats SET active_persona_id = ?1 WHERE chat_id = ?2",
            params![persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    /// Existing artifact migration scopes, newest active personas first.
    pub fn artifact_scopes(&self) -> Result<Vec<(i64, i64)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt=conn.prepare("SELECT p.chat_id,p.id FROM personas p LEFT JOIN messages m ON m.chat_id=p.chat_id AND m.persona_id=p.id GROUP BY p.chat_id,p.id ORDER BY max(m.timestamp) DESC")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_personas(&self, chat_id: i64) -> Result<Vec<Persona>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PERSONA_SELECT_COLS} FROM personas WHERE chat_id = ?1 ORDER BY id"
        ))?;
        let personas = stmt
            .query_map(params![chat_id], persona_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(personas)
    }

    /// Latest persisted user or assistant message, including sessions and side chats.
    pub fn list_persona_last_message_at(
        &self,
        chat_id: i64,
    ) -> Result<Vec<(i64, String)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "WITH activity AS (
                SELECT persona_id, timestamp FROM messages WHERE chat_id = ?1
                UNION ALL
                SELECT sc.persona_id, t.created_at AS timestamp
                FROM side_chats sc JOIN side_chat_turns t ON t.side_chat_id = sc.id
                WHERE sc.chat_id = ?1 AND t.role IN ('user', 'assistant')
             ), ranked AS (
                SELECT persona_id, timestamp,
                       ROW_NUMBER() OVER (
                           PARTITION BY persona_id
                           ORDER BY julianday(timestamp) DESC, timestamp DESC
                       ) AS rank
                FROM activity
             )
             SELECT persona_id, timestamp FROM ranked WHERE rank = 1",
        )?;
        let rows = stmt
            .query_map(params![chat_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Latest bot message per persona in a chat: timestamp plus the session that message belongs to
    /// (`session_id` / title are `None` for main-chat messages).
    pub fn list_persona_last_bot_message_at(
        &self,
        chat_id: i64,
    ) -> Result<Vec<PersonaLastBotInfo>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.persona_id, m.timestamp, m.session_id, cs.title
             FROM messages m
             LEFT JOIN chat_sessions cs ON cs.id = m.session_id
             WHERE m.chat_id = ?1
               AND m.is_from_bot = 1
               AND m.rowid = (
                 SELECT m2.rowid
                 FROM messages m2
                 WHERE m2.chat_id = m.chat_id
                   AND m2.persona_id = m.persona_id
                   AND m2.is_from_bot = 1
                 ORDER BY m2.timestamp DESC, m2.rowid DESC
                 LIMIT 1
               )",
        )?;
        let rows = stmt
            .query_map(params![chat_id], |row| {
                Ok(PersonaLastBotInfo {
                    persona_id: row.get(0)?,
                    last_bot_message_at: row.get(1)?,
                    session_id: row.get(2)?,
                    session_title: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_persona_read_cursors(
        &self,
        chat_id: i64,
    ) -> Result<Vec<(i64, String)>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT persona_id, last_read_at
             FROM persona_read_cursors
             WHERE chat_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![chat_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Writes read cursors for sidebar unread dots.
    /// `only_if_absent` keeps an existing row (first-visit baseline).
    /// Otherwise the stored time moves forward to the later of the two timestamps.
    /// Returns `Err(persona_id)` when that persona is not in the chat; the transaction rolls back.
    pub fn apply_persona_read_cursors(
        &self,
        chat_id: i64,
        only_if_absent: bool,
        updates: &[(i64, String)],
    ) -> Result<Result<Vec<(i64, String)>, i64>, FinallyAValueBotError> {
        use rusqlite::OptionalExtension;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut stored = Vec::with_capacity(updates.len());
        for (persona_id, last_read_at) in updates {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM personas WHERE chat_id = ?1 AND id = ?2)",
                params![chat_id, persona_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(Err(*persona_id));
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT last_read_at FROM persona_read_cursors WHERE chat_id = ?1 AND persona_id = ?2",
                    params![chat_id, persona_id],
                    |row| row.get(0),
                )
                .optional()?;
            let next = match existing {
                Some(prev) if only_if_absent => prev,
                Some(prev) => later_rfc3339(&prev, last_read_at),
                None => last_read_at.clone(),
            };
            tx.execute(
                "INSERT INTO persona_read_cursors (chat_id, persona_id, last_read_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(chat_id, persona_id) DO UPDATE SET last_read_at = excluded.last_read_at",
                params![chat_id, persona_id, next],
            )?;
            stored.push((*persona_id, next));
        }
        tx.commit()?;
        Ok(Ok(stored))
    }

    pub fn create_persona(
        &self,
        chat_id: i64,
        name: &str,
        model_override: Option<&str>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO personas (chat_id, name, model_override, agent_engine_override) VALUES (?1, ?2, ?3, 'classic')",
            params![chat_id, name, model_override],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Renames a persona within a chat. Rejects empty names, the reserved `default` persona,
    /// and collisions with an existing name in the same chat.
    pub fn rename_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
        new_name: &str,
    ) -> Result<(), FinallyAValueBotError> {
        use rusqlite::OptionalExtension;
        let name = new_name.trim();
        if name.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "Persona name cannot be empty".into(),
            ));
        }
        if name == "default" {
            return Err(FinallyAValueBotError::ToolExecution(
                "Cannot rename a persona to the reserved name 'default'".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let current: String = conn
            .query_row(
                "SELECT name FROM personas WHERE id = ?1 AND chat_id = ?2",
                params![persona_id, chat_id],
                |row| row.get(0),
            )
            .map_err(|_| FinallyAValueBotError::ToolExecution("Persona not found".into()))?;
        if current == "default" {
            return Err(FinallyAValueBotError::ToolExecution(
                "Cannot rename the default persona".into(),
            ));
        }
        if current == name {
            return Ok(());
        }
        let conflict: Option<i64> = conn
            .query_row(
                "SELECT id FROM personas WHERE chat_id = ?1 AND name = ?2",
                params![chat_id, name],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "A persona named '{name}' already exists"
            )));
        }
        let rows = conn.execute(
            "UPDATE personas SET name = ?1 WHERE id = ?2 AND chat_id = ?3",
            params![name, persona_id, chat_id],
        )?;
        if rows == 0 {
            return Err(FinallyAValueBotError::ToolExecution(
                "Persona not found".into(),
            ));
        }
        Ok(())
    }

    pub fn get_persona_by_name(
        &self,
        chat_id: i64,
        name: &str,
    ) -> Result<Option<Persona>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!("SELECT {PERSONA_SELECT_COLS} FROM personas WHERE chat_id = ?1 AND name = ?2"),
            params![chat_id, name],
            persona_from_row,
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_persona(&self, id: i64) -> Result<Option<Persona>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!("SELECT {PERSONA_SELECT_COLS} FROM personas WHERE id = ?1"),
            params![id],
            persona_from_row,
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_persona(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM personas WHERE id = ?1 AND chat_id = ?2",
                params![persona_id, chat_id],
                |row| row.get(0),
            )
            .map_err(|_| FinallyAValueBotError::ToolExecution("Persona not found".into()))?;
        if name == "default" {
            return Err(FinallyAValueBotError::ToolExecution(
                "Cannot delete the default persona".into(),
            ));
        }
        let tx = conn.unchecked_transaction()?;
        let _ = tx.execute(
            "DELETE FROM sessions WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM persona_bulletin_events WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM persona_bulletin_focus WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM persona_message_bookmarks WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let side_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT id FROM side_chats WHERE chat_id = ?1 AND persona_id = ?2")?;
            let ids = stmt
                .query_map(params![chat_id, persona_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        for sid in &side_ids {
            let _ = tx.execute(
                "DELETE FROM side_chat_turns WHERE side_chat_id = ?1",
                params![sid],
            )?;
        }
        let _ = tx.execute(
            "DELETE FROM side_chats WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM persona_todos WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let _ = tx.execute(
            "DELETE FROM conversation_topics WHERE chat_id = ?1 AND persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        let rows = tx.execute(
            "DELETE FROM personas WHERE id = ?1 AND chat_id = ?2",
            params![persona_id, chat_id],
        )?;
        tx.execute(
            "UPDATE chats SET active_persona_id = (SELECT id FROM personas WHERE chat_id = ?1 AND name = 'default' LIMIT 1) WHERE chat_id = ?1 AND active_persona_id = ?2",
            params![chat_id, persona_id],
        )?;
        tx.commit()?;
        Ok(rows > 0)
    }

    pub fn update_persona_model(
        &self,
        chat_id: i64,
        persona_id: i64,
        model_override: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE personas SET model_override = ?1 WHERE id = ?2 AND chat_id = ?3",
            params![model_override, persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    /// Sets classic strategy LLM selection for a persona. Empty strings clear to NULL (inherit global).
    pub fn set_persona_llm_selection(
        &self,
        chat_id: i64,
        persona_id: i64,
        provider_override: Option<&str>,
        model_override: Option<&str>,
        base_url_override: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let provider_v = provider_override.map(str::trim).filter(|s| !s.is_empty());
        let model_v = model_override.map(str::trim).filter(|s| !s.is_empty());
        let base_v = base_url_override.map(str::trim).filter(|s| !s.is_empty());
        let rows = conn.execute(
            "UPDATE personas SET llm_provider_override = ?1, model_override = ?2, llm_base_url_override = ?3
             WHERE id = ?4 AND chat_id = ?5",
            params![provider_v, model_v, base_v, persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    /// Sets per-persona prompt controls. `None` for min fields writes SQL NULL (use server defaults).
    /// `operator_memo` `None` clears memo to NULL; `Some("")` also clears.
    pub fn set_persona_prompt_overrides(
        &self,
        chat_id: i64,
        persona_id: i64,
        recent_history_min_user: Option<i64>,
        recent_history_min_assistant: Option<i64>,
        operator_memo: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let memo_db: Option<String> = match operator_memo {
            None => None,
            Some(s) if s.trim().is_empty() => None,
            Some(s) => Some(s.to_string()),
        };
        let rows = conn.execute(
            "UPDATE personas SET recent_history_min_user = ?1, recent_history_min_assistant = ?2, operator_memo = ?3 WHERE id = ?4 AND chat_id = ?5",
            params![
                recent_history_min_user,
                recent_history_min_assistant,
                memo_db,
                persona_id,
                chat_id
            ],
        )?;
        Ok(rows > 0)
    }

    pub fn set_persona_delivery_policy(
        &self,
        chat_id: i64,
        persona_id: i64,
        dense_delivery_enabled: bool,
        messaging_max_chars: Option<i64>,
        web_max_chars: Option<i64>,
        summary_chars: Option<i64>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE personas SET dense_delivery_enabled = ?1, dense_delivery_messaging_max_chars = ?2, dense_delivery_web_max_chars = ?3, dense_delivery_summary_chars = ?4 WHERE id = ?5 AND chat_id = ?6",
            params![
                if dense_delivery_enabled { 1 } else { 0 },
                messaging_max_chars,
                web_max_chars,
                summary_chars,
                persona_id,
                chat_id
            ],
        )?;
        Ok(rows > 0)
    }

    pub fn set_persona_engine_override(
        &self,
        chat_id: i64,
        persona_id: i64,
        agent_engine_override: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let value = agent_engine_override
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let rows = conn.execute(
            "UPDATE personas SET agent_engine_override = ?1 WHERE id = ?2 AND chat_id = ?3",
            params![value, persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    pub fn set_persona_gemini_adk_topology(
        &self,
        chat_id: i64,
        persona_id: i64,
        topology_json: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let value = topology_json.map(str::trim).filter(|s| !s.is_empty());
        let rows = conn.execute(
            "UPDATE personas SET gemini_adk_topology = ?1 WHERE id = ?2 AND chat_id = ?3",
            params![value, persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    pub fn set_persona_cursor_sdk_model(
        &self,
        chat_id: i64,
        persona_id: i64,
        model: Option<&str>,
        model_params_json: Option<&str>,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let model_v = model.map(str::trim).filter(|s| !s.is_empty());
        let params_v = model_params_json.map(str::trim).filter(|s| !s.is_empty());
        let rows = conn.execute(
            "UPDATE personas SET cursor_sdk_model = ?1, cursor_sdk_model_params = ?2
             WHERE id = ?3 AND chat_id = ?4",
            params![model_v, params_v, persona_id, chat_id],
        )?;
        Ok(rows > 0)
    }

    /// Returns messages ranked by relevance (FTS5 rank).
    pub fn search_messages(
        &self,
        chat_id: i64,
        persona_id: i64,
        query: &str,
        limit: usize,
        from_date: Option<&str>,
        to_date: Option<&str>,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.chat_id, m.persona_id, m.session_id, m.sender_name, m.content, m.is_from_bot, m.timestamp, m.origin
             FROM messages_fts
             JOIN messages m ON m.rowid = messages_fts.rowid
             WHERE messages_fts MATCH ?1
               AND m.chat_id = ?2
               AND m.persona_id = ?3
               AND (?4 IS NULL OR m.timestamp >= ?4)
               AND (?5 IS NULL OR m.timestamp <= ?5)
             ORDER BY messages_fts.rank
             LIMIT ?6",
        )?;
        let messages = stmt
            .query_map(
                params![query, chat_id, persona_id, from_date, to_date, limit as i64],
                stored_message_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    // --- Chat Sessions ---

    pub fn create_chat_session(
        &self,
        id: &str,
        chat_id: i64,
        persona_id: i64,
        title: &str,
        intent: &str,
        ttl_hours: i64,
        mirror_main_chat: bool,
        seed_context: Option<&str>,
        source_topic_id: Option<&str>,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let seed = seed_context.map(str::trim).filter(|text| !text.is_empty());
        let topic_id = source_topic_id
            .map(str::trim)
            .filter(|text| !text.is_empty());
        conn.execute(
            "INSERT INTO chat_sessions (id, chat_id, persona_id, title, intent, status, created_at, last_active_at, ttl_hours, mirror_main_chat, seed_context, source_topic_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                chat_id,
                persona_id,
                title,
                intent,
                now,
                ttl_hours,
                i64::from(mirror_main_chat),
                seed,
                topic_id
            ],
        )?;
        Ok(())
    }

    pub fn get_chat_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ChatSession>, FinallyAValueBotError> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                &format!(
                    "SELECT {CHAT_SESSION_SELECT_COLS}
                 FROM chat_sessions WHERE id = ?1"
                ),
                params![session_id],
                chat_session_from_row,
            )
            .optional()?;
        Ok(result)
    }

    pub fn list_chat_sessions(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<ChatSession>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let query = format!(
            "SELECT {CHAT_SESSION_SELECT_COLS}
             FROM chat_sessions WHERE chat_id = ?1 AND persona_id = ?2
             ORDER BY last_active_at DESC"
        );
        let mut stmt = conn.prepare(&query)?;
        let sessions = stmt
            .query_map(params![chat_id, persona_id], chat_session_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    pub fn update_chat_session_last_active(
        &self,
        session_id: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE chat_sessions SET last_active_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )?;
        Ok(())
    }

    pub fn update_chat_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE chat_sessions SET title = ?1 WHERE id = ?2",
            params![title, session_id],
        )?;
        Ok(rows > 0)
    }

    pub fn update_chat_session_ttl(
        &self,
        session_id: &str,
        ttl_hours: i64,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_sessions SET ttl_hours = ?1 WHERE id = ?2",
            params![ttl_hours, session_id],
        )?;
        Ok(())
    }

    pub fn update_chat_session_mirror_main_chat(
        &self,
        session_id: &str,
        mirror_main_chat: bool,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_sessions SET mirror_main_chat = ?1 WHERE id = ?2",
            params![i64::from(mirror_main_chat), session_id],
        )?;
        Ok(())
    }

    pub fn update_chat_session_bootstrap_context(
        &self,
        session_id: &str,
        bootstrap_context_json: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_sessions SET bootstrap_context_json = ?1 WHERE id = ?2",
            params![bootstrap_context_json, session_id],
        )?;
        Ok(())
    }

    pub fn delete_chat_session(&self, session_id: &str) -> Result<bool, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM conversation_topics WHERE session_id = ?1",
            params![session_id],
        )?;
        let rows = tx.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(rows > 0)
    }

    pub fn get_all_messages_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT_COLS}
             FROM messages
             WHERE session_id = ?1
             ORDER BY timestamp ASC"
        ))?;
        let messages = stmt
            .query_map(params![session_id], stored_message_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    /// Messages at or before `max_timestamp`, scoped to main chat or a focused session.
    /// Used to seed ephemeral sub-thread context around an anchor message.
    pub fn get_messages_up_to(
        &self,
        chat_id: i64,
        persona_id: i64,
        max_timestamp: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<StoredMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
        let messages = if let Some(sid) = session_id {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2 AND session_id = ?3
                   AND timestamp <= ?4
                 ORDER BY timestamp ASC"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, sid, max_timestamp],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND {MAIN_CHAT_MESSAGE_VISIBILITY}
                   AND timestamp <= ?3
                 ORDER BY timestamp ASC"
            ))?;
            let rows = stmt.query_map(
                params![chat_id, persona_id, max_timestamp],
                stored_message_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        Ok(messages)
    }

    pub fn list_recent_topic_scopes(
        &self,
        since_ts: &str,
        limit: i64,
    ) -> Result<Vec<TopicScope>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT chat_id, persona_id, session_id
             FROM messages
             WHERE timestamp >= ?1
             GROUP BY chat_id, persona_id, session_id
             ORDER BY MAX(timestamp) DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![since_ts, limit], |row| {
                Ok(TopicScope {
                    chat_id: row.get(0)?,
                    persona_id: row.get(1)?,
                    session_id: normalize_topic_session(
                        row.get::<_, Option<String>>(2)?.as_deref(),
                    ),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_conversation_topics(
        &self,
        scope: &TopicScope,
    ) -> Result<Vec<ConversationTopic>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        Self::list_conversation_topics_conn(&conn, scope)
    }

    fn list_conversation_topics_conn(
        conn: &Connection,
        scope: &TopicScope,
    ) -> Result<Vec<ConversationTopic>, FinallyAValueBotError> {
        let bind = topic_scope_bind(scope);
        let mut stmt = conn.prepare(&format!(
            "SELECT {TOPIC_SELECT_COLS}
             FROM conversation_topics
             WHERE chat_id = ?1 AND persona_id = ?2
               AND ((?3 = 1 AND session_id IS NULL) OR (?3 = 0 AND session_id = ?4))
             ORDER BY start_ts ASC, id ASC"
        ))?;
        let rows = stmt
            .query_map(
                params![
                    scope.chat_id,
                    scope.persona_id,
                    bind.is_main,
                    bind.session_id
                ],
                conversation_topic_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_conversation_topic(
        &self,
        chat_id: i64,
        persona_id: i64,
        topic_id: &str,
    ) -> Result<Option<ConversationTopic>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            &format!(
                "SELECT {TOPIC_SELECT_COLS}
                 FROM conversation_topics
                 WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3"
            ),
            params![topic_id, chat_id, persona_id],
            conversation_topic_from_row,
        );
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// One page of messages in the topic, plus bookmarks whose messages are in the topic range.
    /// An open topic includes the unsegmented tail after its stored end.
    /// `Before`, `After`, and `Around` return `Ok(None)` when the cursor message is outside the topic.
    pub fn topic_dossier_window(
        &self,
        topic: &ConversationTopic,
        limit: usize,
        page: DossierPageRequest,
    ) -> Result<Option<TopicDossierWindow>, FinallyAValueBotError> {
        let limit = limit.clamp(1, TOPIC_DOSSIER_MAX_LIMIT);
        let conn = self.conn.lock().unwrap();
        let scope = TopicScope {
            chat_id: topic.chat_id,
            persona_id: topic.persona_id,
            session_id: topic.session_id.clone(),
        };
        let bind = topic_scope_bind(&scope);
        let open_tail = i64::from(topic.status == "open");
        let fetch = (limit + 1) as i64;
        let bounds = format!(
            "{TOPIC_MESSAGE_SCOPE_SQL}
             AND (timestamp > ?5 OR (timestamp = ?5 AND id >= ?6))
             AND (?7 = 1 OR timestamp < ?8 OR (timestamp = ?8 AND id <= ?9))"
        );
        let cursor_id = match &page {
            DossierPageRequest::Latest => None,
            DossierPageRequest::Before(id)
            | DossierPageRequest::After(id)
            | DossierPageRequest::Around(id) => Some(id.as_str()),
        };
        let anchor = if let Some(message_id) = cursor_id {
            let sql = format!(
                "SELECT {MESSAGE_SELECT_COLS}
                 FROM messages
                 WHERE id = ?10 AND {bounds}
                 LIMIT 1"
            );
            match conn.query_row(
                &sql,
                params![
                    topic.chat_id,
                    topic.persona_id,
                    bind.is_main,
                    bind.session_id,
                    topic.start_ts,
                    topic.start_message_id,
                    open_tail,
                    topic.end_ts,
                    topic.end_message_id,
                    message_id
                ],
                stored_message_from_row,
            ) {
                Ok(message) => Some(message),
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        let (messages, has_older, has_newer) = match &page {
            DossierPageRequest::Latest => {
                let sql = format!(
                    "SELECT {MESSAGE_SELECT_COLS}
                     FROM messages
                     WHERE {bounds}
                     ORDER BY timestamp DESC, id DESC
                     LIMIT ?10"
                );
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt
                    .query_map(
                        params![
                            topic.chat_id,
                            topic.persona_id,
                            bind.is_main,
                            bind.session_id,
                            topic.start_ts,
                            topic.start_message_id,
                            open_tail,
                            topic.end_ts,
                            topic.end_message_id,
                            fetch
                        ],
                        stored_message_from_row,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                let has_older = rows.len() > limit;
                if has_older {
                    rows.truncate(limit);
                }
                rows.reverse();
                (rows, has_older, false)
            }
            DossierPageRequest::Before(_) => {
                let anchor = anchor.as_ref().expect("before cursor resolved");
                let sql = format!(
                    "SELECT {MESSAGE_SELECT_COLS}
                     FROM messages
                     WHERE {bounds}
                       AND (timestamp < ?10 OR (timestamp = ?10 AND id < ?11))
                     ORDER BY timestamp DESC, id DESC
                     LIMIT ?12"
                );
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt
                    .query_map(
                        params![
                            topic.chat_id,
                            topic.persona_id,
                            bind.is_main,
                            bind.session_id,
                            topic.start_ts,
                            topic.start_message_id,
                            open_tail,
                            topic.end_ts,
                            topic.end_message_id,
                            anchor.timestamp,
                            anchor.id,
                            fetch
                        ],
                        stored_message_from_row,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                let has_older = rows.len() > limit;
                if has_older {
                    rows.truncate(limit);
                }
                rows.reverse();
                (rows, has_older, true)
            }
            DossierPageRequest::After(_) => {
                let anchor = anchor.as_ref().expect("after cursor resolved");
                let sql = format!(
                    "SELECT {MESSAGE_SELECT_COLS}
                     FROM messages
                     WHERE {bounds}
                       AND (timestamp > ?10 OR (timestamp = ?10 AND id > ?11))
                     ORDER BY timestamp ASC, id ASC
                     LIMIT ?12"
                );
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt
                    .query_map(
                        params![
                            topic.chat_id,
                            topic.persona_id,
                            bind.is_main,
                            bind.session_id,
                            topic.start_ts,
                            topic.start_message_id,
                            open_tail,
                            topic.end_ts,
                            topic.end_message_id,
                            anchor.timestamp,
                            anchor.id,
                            fetch
                        ],
                        stored_message_from_row,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                let has_newer = rows.len() > limit;
                if has_newer {
                    rows.truncate(limit);
                }
                (rows, true, has_newer)
            }
            DossierPageRequest::Around(_) => {
                let anchor_message = anchor.clone().expect("around cursor resolved");
                let older_sql = format!(
                    "SELECT {MESSAGE_SELECT_COLS}
                     FROM messages
                     WHERE {bounds}
                       AND (timestamp < ?10 OR (timestamp = ?10 AND id < ?11))
                     ORDER BY timestamp DESC, id DESC
                     LIMIT ?12"
                );
                let newer_sql = format!(
                    "SELECT {MESSAGE_SELECT_COLS}
                     FROM messages
                     WHERE {bounds}
                       AND (timestamp > ?10 OR (timestamp = ?10 AND id > ?11))
                     ORDER BY timestamp ASC, id ASC
                     LIMIT ?12"
                );
                let mut older = {
                    let mut older_stmt = conn.prepare(&older_sql)?;
                    collect_statement_messages(
                        &mut older_stmt,
                        params![
                            topic.chat_id,
                            topic.persona_id,
                            bind.is_main,
                            bind.session_id,
                            topic.start_ts,
                            topic.start_message_id,
                            open_tail,
                            topic.end_ts,
                            topic.end_message_id,
                            anchor_message.timestamp,
                            anchor_message.id,
                            fetch
                        ],
                    )?
                };
                let older_has_more = older.len() > limit;
                if older_has_more {
                    older.truncate(limit);
                }
                let mut newer = {
                    let mut newer_stmt = conn.prepare(&newer_sql)?;
                    collect_statement_messages(
                        &mut newer_stmt,
                        params![
                            topic.chat_id,
                            topic.persona_id,
                            bind.is_main,
                            bind.session_id,
                            topic.start_ts,
                            topic.start_message_id,
                            open_tail,
                            topic.end_ts,
                            topic.end_message_id,
                            anchor_message.timestamp,
                            anchor_message.id,
                            fetch
                        ],
                    )?
                };
                let newer_has_more = newer.len() > limit;
                if newer_has_more {
                    newer.truncate(limit);
                }
                let mut before_take = (limit / 2).min(older.len());
                let mut after_take = limit.saturating_sub(1 + before_take).min(newer.len());
                let mut slack = limit.saturating_sub(1 + before_take + after_take);
                if slack > 0 {
                    let extra = slack.min(older.len().saturating_sub(before_take));
                    before_take += extra;
                    slack -= extra;
                }
                if slack > 0 {
                    after_take += slack.min(newer.len().saturating_sub(after_take));
                }
                let mut rows = Vec::with_capacity(before_take + 1 + after_take);
                let mut older_page: Vec<StoredMessage> =
                    older.iter().take(before_take).cloned().collect();
                older_page.reverse();
                rows.append(&mut older_page);
                rows.push(anchor_message);
                rows.extend(newer.iter().take(after_take).cloned());
                let has_older = older_has_more || before_take < older.len();
                let has_newer = newer_has_more || after_take < newer.len();
                (rows, has_older, has_newer)
            }
        };

        let mut bookmark_stmt = conn.prepare(
            "SELECT b.chat_id, b.persona_id, b.message_id, b.role, b.content_preview, b.note,
                    b.created_at, b.updated_at, m.session_id, m.timestamp, 0
             FROM persona_message_bookmarks b
             INNER JOIN messages m
               ON m.chat_id = b.chat_id AND m.persona_id = b.persona_id AND m.id = b.message_id
             WHERE m.chat_id = ?1 AND m.persona_id = ?2
               AND ((?3 = 1 AND (
                    m.session_id IS NULL
                    OR EXISTS (
                        SELECT 1 FROM chat_sessions cs
                        WHERE cs.id = m.session_id AND cs.mirror_main_chat = 1
                    )
               )) OR (?3 = 0 AND m.session_id = ?4))
               AND (m.timestamp > ?5 OR (m.timestamp = ?5 AND m.id >= ?6))
               AND (?7 = 1 OR m.timestamp < ?8 OR (m.timestamp = ?8 AND m.id <= ?9))
             ORDER BY m.timestamp DESC, m.id DESC
             LIMIT ?10",
        )?;
        let bookmarks = bookmark_stmt
            .query_map(
                params![
                    topic.chat_id,
                    topic.persona_id,
                    bind.is_main,
                    bind.session_id,
                    topic.start_ts,
                    topic.start_message_id,
                    open_tail,
                    topic.end_ts,
                    topic.end_message_id,
                    TOPIC_DOSSIER_BOOKMARK_LIMIT as i64
                ],
                |row| {
                    Ok(PersonaMessageBookmark {
                        chat_id: row.get(0)?,
                        persona_id: row.get(1)?,
                        message_id: row.get(2)?,
                        role: row.get(3)?,
                        content_preview: row.get(4)?,
                        note: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        session_id: row.get(8)?,
                        message_timestamp: row.get(9)?,
                        missing: false,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(TopicDossierWindow {
            messages,
            has_older,
            has_newer,
            bookmarks,
        }))
    }

    pub fn list_side_chat_branches(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Result<Vec<SideChatBranch>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT sc.id, sc.session_id, sc.anchor_message_id, sc.title,
                    COALESCE((SELECT COUNT(*) FROM side_chat_turns t WHERE t.side_chat_id = sc.id), 0),
                    m.timestamp
             FROM side_chats sc
             LEFT JOIN messages m
               ON m.chat_id = sc.chat_id AND m.persona_id = sc.persona_id AND m.id = sc.anchor_message_id
             WHERE sc.chat_id = ?1 AND sc.persona_id = ?2
             ORDER BY sc.updated_at DESC
             LIMIT 200",
        )?;
        let rows = stmt
            .query_map(params![chat_id, persona_id], |row| {
                Ok(SideChatBranch {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    anchor_message_id: row.get(2)?,
                    title: row.get(3)?,
                    turn_count: row.get(4)?,
                    anchor_timestamp: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn latest_message_timestamp_in_topic_scope(
        &self,
        scope: &TopicScope,
    ) -> Result<Option<String>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let bind = topic_scope_bind(scope);
        let sql = format!(
            "SELECT timestamp FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
             ORDER BY timestamp DESC, id DESC
             LIMIT 1"
        );
        match conn.query_row(
            &sql,
            params![
                scope.chat_id,
                scope.persona_id,
                bind.is_main,
                bind.session_id
            ],
            |row| row.get::<_, String>(0),
        ) {
            Ok(ts) => Ok(Some(ts)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn count_messages_in_topic_scope(
        &self,
        scope: &TopicScope,
        after: Option<(&str, &str)>,
    ) -> Result<i64, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let bind = topic_scope_bind(scope);
        let (ts, id) = after.unwrap_or(("", ""));
        let sql = format!(
            "SELECT COUNT(*) FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND {TOPIC_MESSAGE_CURSOR_SQL}"
        );
        let count: i64 = conn.query_row(
            &sql,
            params![
                scope.chat_id,
                scope.persona_id,
                bind.is_main,
                bind.session_id,
                ts,
                id,
                0_i64
            ],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn list_topic_window_messages(
        &self,
        scope: &TopicScope,
        cursor: &TopicWindowCursor,
        limit: usize,
    ) -> Result<Vec<TopicWindowMessage>, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        Self::list_topic_window_messages_conn(&conn, scope, cursor, limit)
    }

    fn list_topic_window_messages_conn(
        conn: &Connection,
        scope: &TopicScope,
        cursor: &TopicWindowCursor,
        limit: usize,
    ) -> Result<Vec<TopicWindowMessage>, FinallyAValueBotError> {
        let bind = topic_scope_bind(scope);
        let (ts, id, inclusive) = match cursor {
            TopicWindowCursor::Beginning => (String::new(), String::new(), 1_i64),
            TopicWindowCursor::Inclusive { timestamp, id } => (timestamp.clone(), id.clone(), 1),
            TopicWindowCursor::Exclusive { timestamp, id } => (timestamp.clone(), id.clone(), 0),
        };
        let sql = format!(
            "SELECT id, timestamp, is_from_bot, substr(content, 1, 500)
             FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND {TOPIC_MESSAGE_CURSOR_SQL}
             ORDER BY timestamp ASC, id ASC
             LIMIT ?8"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                params![
                    scope.chat_id,
                    scope.persona_id,
                    bind.is_main,
                    bind.session_id,
                    ts,
                    id,
                    inclusive,
                    limit.max(1) as i64
                ],
                |row| {
                    Ok(TopicWindowMessage {
                        id: row.get(0)?,
                        timestamp: row.get(1)?,
                        is_from_bot: row.get::<_, i32>(2)? != 0,
                        content: row.get(3)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Replace the open auto tail (when `replace_topic_id` is set) with `drafts`.
    /// Refuses to write if that row was edited or closed, or if a draft overlaps a frozen topic.
    pub fn commit_auto_segmentation(
        &self,
        scope: &TopicScope,
        replace_topic_id: Option<&str>,
        drafts: &[ConversationTopicDraft],
    ) -> Result<(), FinallyAValueBotError> {
        if drafts.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic segmentation produced no segments".into(),
            ));
        }
        let open_count = drafts.iter().filter(|d| d.status == "open").count();
        if open_count > 1
            || drafts
                .iter()
                .any(|d| d.status != "open" && d.status != "closed")
        {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic segmentation statuses are invalid".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let existing = Self::list_conversation_topics_conn(&tx, scope)?;
        if let Some(id) = replace_topic_id {
            let current = existing.iter().find(|t| t.id == id);
            match current {
                Some(topic) if topic.source == "auto" && topic.status == "open" => {
                    tx.execute("DELETE FROM conversation_topics WHERE id = ?1", params![id])?;
                }
                _ => {
                    return Err(FinallyAValueBotError::ToolExecution(
                        "topic changed before segmentation could be saved".into(),
                    ));
                }
            }
        }
        let frozen: Vec<&ConversationTopic> = existing
            .iter()
            .filter(|t| replace_topic_id != Some(t.id.as_str()))
            .collect();
        for draft in drafts {
            if draft.message_count <= 0
                || draft.start_message_id.is_empty()
                || draft.end_message_id.is_empty()
            {
                return Err(FinallyAValueBotError::ToolExecution(
                    "topic segmentation draft is empty".into(),
                ));
            }
            for topic in &frozen {
                if topic_ranges_overlap(
                    &draft.start_ts,
                    &draft.start_message_id,
                    &draft.end_ts,
                    &draft.end_message_id,
                    &topic.start_ts,
                    &topic.start_message_id,
                    &topic.end_ts,
                    &topic.end_message_id,
                ) {
                    return Err(FinallyAValueBotError::ToolExecution(
                        "topic segmentation overlapped a frozen topic".into(),
                    ));
                }
            }
        }
        let session = normalize_topic_session(scope.session_id.as_deref());
        let now = chrono::Utc::now().to_rfc3339();
        for draft in drafts {
            let id = uuid::Uuid::new_v4().to_string();
            let title = clamp_topic_title(&draft.title);
            let summary = clamp_topic_summary(&draft.summary);
            tx.execute(
                "INSERT INTO conversation_topics (
                    id, chat_id, persona_id, session_id, parent_id, continues_topic_id,
                    title, summary, start_message_id, end_message_id, start_ts, end_ts,
                    message_count, status, source, pinned, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, NULL, NULL,
                    ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, 'auto', 0, ?13, ?13
                 )",
                params![
                    id,
                    scope.chat_id,
                    scope.persona_id,
                    session,
                    title,
                    summary,
                    draft.start_message_id,
                    draft.end_message_id,
                    draft.start_ts,
                    draft.end_ts,
                    draft.message_count,
                    draft.status,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn patch_conversation_topic(
        &self,
        chat_id: i64,
        persona_id: i64,
        topic_id: &str,
        title: Option<&str>,
        pinned: Option<bool>,
    ) -> Result<Option<ConversationTopic>, FinallyAValueBotError> {
        if title.is_none() && pinned.is_none() {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic patch is empty".into(),
            ));
        }
        if let Some(title) = title {
            if clamp_topic_title(title) == "Untitled" && title.trim().is_empty() {
                return Err(FinallyAValueBotError::ToolExecution(
                    "topic title is empty".into(),
                ));
            }
        }
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let rows = match (title, pinned) {
            (Some(title), Some(pinned)) => {
                let title = clamp_topic_title(title);
                conn.execute(
                    "UPDATE conversation_topics
                     SET title = ?1, pinned = ?2, source = 'user', updated_at = ?3
                     WHERE id = ?4 AND chat_id = ?5 AND persona_id = ?6",
                    params![title, i64::from(pinned), now, topic_id, chat_id, persona_id],
                )?
            }
            (Some(title), None) => {
                let title = clamp_topic_title(title);
                conn.execute(
                    "UPDATE conversation_topics
                     SET title = ?1, source = 'user', updated_at = ?2
                     WHERE id = ?3 AND chat_id = ?4 AND persona_id = ?5",
                    params![title, now, topic_id, chat_id, persona_id],
                )?
            }
            (None, Some(pinned)) => conn.execute(
                "UPDATE conversation_topics
                 SET pinned = ?1, source = 'user', updated_at = ?2
                 WHERE id = ?3 AND chat_id = ?4 AND persona_id = ?5",
                params![i64::from(pinned), now, topic_id, chat_id, persona_id],
            )?,
            (None, None) => 0,
        };
        if rows == 0 {
            return Ok(None);
        }
        drop(conn);
        self.get_conversation_topic(chat_id, persona_id, topic_id)
    }

    pub fn merge_conversation_topic_with_previous(
        &self,
        chat_id: i64,
        persona_id: i64,
        topic_id: &str,
    ) -> Result<ConversationTopic, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let topic = match tx.query_row(
            &format!(
                "SELECT {TOPIC_SELECT_COLS} FROM conversation_topics
                 WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3"
            ),
            params![topic_id, chat_id, persona_id],
            conversation_topic_from_row,
        ) {
            Ok(topic) => topic,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(FinallyAValueBotError::ToolExecution(
                    "topic not found".into(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let scope = TopicScope {
            chat_id,
            persona_id,
            session_id: topic.session_id.clone(),
        };
        let topics = Self::list_conversation_topics_conn(&tx, &scope)?;
        let index = topics
            .iter()
            .position(|t| t.id == topic.id)
            .ok_or_else(|| FinallyAValueBotError::ToolExecution("topic not found".into()))?;
        if index == 0 {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic has no previous topic to merge".into(),
            ));
        }
        let prev = &topics[index - 1];
        for other in topics.iter() {
            if other.id == prev.id || other.id == topic.id {
                continue;
            }
            if topic_ranges_overlap(
                &prev.start_ts,
                &prev.start_message_id,
                &topic.end_ts,
                &topic.end_message_id,
                &other.start_ts,
                &other.start_message_id,
                &other.end_ts,
                &other.end_message_id,
            ) {
                return Err(FinallyAValueBotError::ToolExecution(
                    "topic merge would overlap another topic".into(),
                ));
            }
        }
        let messages = Self::message_ids_between(
            &tx,
            chat_id,
            persona_id,
            topic.session_id.as_deref(),
            &prev.start_ts,
            &prev.start_message_id,
            &topic.end_ts,
            &topic.end_message_id,
        )?;
        if messages.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic merge found no messages".into(),
            ));
        }
        let (start_id, start_ts) = &messages[0];
        let (end_id, end_ts) = &messages[messages.len() - 1];
        let status = if index + 1 == topics.len() && topic.status == "open" {
            "open"
        } else {
            "closed"
        };
        let summary = combine_topic_summary(&prev.summary, &topic.summary);
        let now = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE conversation_topics
             SET start_message_id = ?1, end_message_id = ?2, start_ts = ?3, end_ts = ?4,
                 message_count = ?5, summary = ?6, status = ?7, source = 'user', updated_at = ?8
             WHERE id = ?9",
            params![
                start_id,
                end_id,
                start_ts,
                end_ts,
                messages.len() as i64,
                summary,
                status,
                now,
                prev.id
            ],
        )?;
        tx.execute(
            "UPDATE conversation_topics SET parent_id = ?1 WHERE parent_id = ?2",
            params![prev.id, topic.id],
        )?;
        tx.execute(
            "UPDATE conversation_topics SET continues_topic_id = ?1 WHERE continues_topic_id = ?2",
            params![prev.id, topic.id],
        )?;
        tx.execute(
            "DELETE FROM conversation_topics WHERE id = ?1",
            params![topic.id],
        )?;
        let merged = tx.query_row(
            &format!("SELECT {TOPIC_SELECT_COLS} FROM conversation_topics WHERE id = ?1"),
            params![prev.id],
            conversation_topic_from_row,
        )?;
        tx.commit()?;
        Ok(merged)
    }

    /// Delete a topic and the chat messages that fall inside its range.
    /// Bookmarks and side chats anchored on those messages go with them.
    /// Returns how many messages were removed.
    pub fn delete_conversation_topic(
        &self,
        chat_id: i64,
        persona_id: i64,
        topic_id: &str,
    ) -> Result<usize, FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let topic = match tx.query_row(
            &format!(
                "SELECT {TOPIC_SELECT_COLS} FROM conversation_topics
                 WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3"
            ),
            params![topic_id, chat_id, persona_id],
            conversation_topic_from_row,
        ) {
            Ok(topic) => topic,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(FinallyAValueBotError::ToolExecution(
                    "topic not found".into(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let messages = Self::message_ids_between(
            &tx,
            chat_id,
            persona_id,
            topic.session_id.as_deref(),
            &topic.start_ts,
            &topic.start_message_id,
            &topic.end_ts,
            &topic.end_message_id,
        )?;
        let scope = TopicScope {
            chat_id,
            persona_id,
            session_id: topic.session_id.clone(),
        };
        let bind = topic_scope_bind(&scope);
        let range_sql = format!(
            "SELECT id FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND (timestamp > ?5 OR (timestamp = ?5 AND id >= ?6))
               AND (timestamp < ?7 OR (timestamp = ?7 AND id <= ?8))"
        );
        let range_params = params![
            chat_id,
            persona_id,
            bind.is_main,
            bind.session_id,
            topic.start_ts,
            topic.start_message_id,
            topic.end_ts,
            topic.end_message_id,
        ];
        tx.execute(
            &format!(
                "DELETE FROM side_chat_turns WHERE side_chat_id IN (
                    SELECT sc.id FROM side_chats sc
                    WHERE sc.chat_id = ?1 AND sc.persona_id = ?2
                      AND sc.anchor_message_id IN ({range_sql})
                )"
            ),
            range_params,
        )?;
        tx.execute(
            &format!(
                "DELETE FROM side_chats
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND anchor_message_id IN ({range_sql})"
            ),
            range_params,
        )?;
        tx.execute(
            &format!(
                "DELETE FROM persona_message_bookmarks
                 WHERE chat_id = ?1 AND persona_id = ?2
                   AND message_id IN ({range_sql})"
            ),
            range_params,
        )?;
        tx.execute(
            &format!(
                "DELETE FROM messages
                 WHERE {TOPIC_MESSAGE_SCOPE_SQL}
                   AND (timestamp > ?5 OR (timestamp = ?5 AND id >= ?6))
                   AND (timestamp < ?7 OR (timestamp = ?7 AND id <= ?8))"
            ),
            range_params,
        )?;
        tx.execute(
            "UPDATE conversation_topics SET parent_id = NULL WHERE parent_id = ?1",
            params![topic.id],
        )?;
        tx.execute(
            "UPDATE conversation_topics SET continues_topic_id = NULL WHERE continues_topic_id = ?1",
            params![topic.id],
        )?;
        tx.execute(
            "DELETE FROM conversation_topics WHERE id = ?1 AND chat_id = ?2 AND persona_id = ?3",
            params![topic.id, chat_id, persona_id],
        )?;
        tx.commit()?;
        Ok(messages.len())
    }

    pub fn split_conversation_topic(
        &self,
        scope: &TopicScope,
        message_id: &str,
    ) -> Result<(ConversationTopic, ConversationTopic), FinallyAValueBotError> {
        let conn = self.conn.lock().unwrap();
        let message = match conn.query_row(
            "SELECT id, timestamp, session_id FROM messages
             WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3",
            params![scope.chat_id, scope.persona_id, message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(FinallyAValueBotError::ToolExecution(
                    "message not found".into(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let (_, timestamp, message_session) = &message;
        let view_main = normalize_topic_session(scope.session_id.as_deref()).is_none();
        if view_main {
            let visible: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM messages
                     WHERE chat_id = ?1 AND persona_id = ?2 AND id = ?3
                       AND {MAIN_CHAT_MESSAGE_VISIBILITY}"
                ),
                params![scope.chat_id, scope.persona_id, message_id],
                |row| row.get(0),
            )?;
            if visible == 0 {
                return Err(FinallyAValueBotError::ToolExecution(
                    "message not found".into(),
                ));
            }
        } else if normalize_topic_session(message_session.as_deref())
            != normalize_topic_session(scope.session_id.as_deref())
        {
            return Err(FinallyAValueBotError::ToolExecution(
                "message not found".into(),
            ));
        }
        let topics = Self::list_conversation_topics_conn(&conn, scope)?;
        let topic = topics
            .iter()
            .rev()
            .find(|topic| topic_contains_message(topic, message_id, timestamp))
            .cloned()
            .ok_or_else(|| {
                FinallyAValueBotError::ToolExecution("message is not in a topic".into())
            })?;
        if topic.start_message_id == message_id {
            return Err(FinallyAValueBotError::ToolExecution(
                "cannot split a topic at its first message".into(),
            ));
        }
        let tx = conn.unchecked_transaction()?;
        let pred_sql = format!(
            "SELECT id, timestamp FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND (timestamp < ?5 OR (timestamp = ?5 AND id < ?6))
               AND (timestamp > ?7 OR (timestamp = ?7 AND id >= ?8))
             ORDER BY timestamp DESC, id DESC
             LIMIT 1"
        );
        let bind = topic_scope_bind(scope);
        let predecessor: Option<(String, String)> = match tx.query_row(
            &pred_sql,
            params![
                scope.chat_id,
                scope.persona_id,
                bind.is_main,
                bind.session_id,
                timestamp,
                message_id,
                topic.start_ts,
                topic.start_message_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(row) => Some(row),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let Some((pred_id, pred_ts)) = predecessor else {
            return Err(FinallyAValueBotError::ToolExecution(
                "cannot split a topic at its first message".into(),
            ));
        };
        let earlier = Self::message_ids_between(
            &tx,
            scope.chat_id,
            scope.persona_id,
            scope.session_id.as_deref(),
            &topic.start_ts,
            &topic.start_message_id,
            &pred_ts,
            &pred_id,
        )?;
        let later = Self::message_ids_between(
            &tx,
            scope.chat_id,
            scope.persona_id,
            scope.session_id.as_deref(),
            timestamp,
            message_id,
            &topic.end_ts,
            &topic.end_message_id,
        )?;
        if earlier.is_empty() || later.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "topic split found no messages".into(),
            ));
        }
        let now = chrono::Utc::now().to_rfc3339();
        let (earlier_start_id, earlier_start_ts) = &earlier[0];
        let (earlier_end_id, earlier_end_ts) = &earlier[earlier.len() - 1];
        tx.execute(
            "UPDATE conversation_topics
             SET start_message_id = ?1, end_message_id = ?2, start_ts = ?3, end_ts = ?4,
                 message_count = ?5, status = 'closed', source = 'user', updated_at = ?6
             WHERE id = ?7",
            params![
                earlier_start_id,
                earlier_end_id,
                earlier_start_ts,
                earlier_end_ts,
                earlier.len() as i64,
                now,
                topic.id
            ],
        )?;
        let new_id = uuid::Uuid::new_v4().to_string();
        let (later_start_id, later_start_ts) = &later[0];
        let (later_end_id, later_end_ts) = &later[later.len() - 1];
        let new_title = clamp_topic_title(&format!("{} continued", topic.title));
        let new_status = if topic.status == "open" {
            "open"
        } else {
            "closed"
        };
        tx.execute(
            "INSERT INTO conversation_topics (
                id, chat_id, persona_id, session_id, parent_id, continues_topic_id,
                title, summary, start_message_id, end_message_id, start_ts, end_ts,
                message_count, status, source, pinned, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, NULL,
                ?6, '', ?7, ?8, ?9, ?10,
                ?11, ?12, 'user', 0, ?13, ?13
             )",
            params![
                new_id,
                scope.chat_id,
                scope.persona_id,
                normalize_topic_session(scope.session_id.as_deref()),
                topic.parent_id,
                new_title,
                later_start_id,
                later_end_id,
                later_start_ts,
                later_end_ts,
                later.len() as i64,
                new_status,
                now
            ],
        )?;
        let earlier_topic = tx.query_row(
            &format!("SELECT {TOPIC_SELECT_COLS} FROM conversation_topics WHERE id = ?1"),
            params![topic.id],
            conversation_topic_from_row,
        )?;
        let later_topic = tx.query_row(
            &format!("SELECT {TOPIC_SELECT_COLS} FROM conversation_topics WHERE id = ?1"),
            params![new_id],
            conversation_topic_from_row,
        )?;
        tx.commit()?;
        Ok((earlier_topic, later_topic))
    }

    fn repair_topics_containing_timestamp(
        conn: &Connection,
        chat_id: i64,
        persona_id: i64,
        timestamp: &str,
    ) -> Result<(), FinallyAValueBotError> {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, start_message_id, end_message_id, start_ts, end_ts
             FROM conversation_topics
             WHERE chat_id = ?1 AND persona_id = ?2 AND start_ts <= ?3 AND end_ts >= ?3",
        )?;
        let rows = stmt
            .query_map(params![chat_id, persona_id, timestamp], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (id, session_id, start_id, end_id, start_ts, end_ts) in rows {
            let messages = Self::message_ids_between(
                conn,
                chat_id,
                persona_id,
                session_id.as_deref(),
                &start_ts,
                &start_id,
                &end_ts,
                &end_id,
            )?;
            if messages.is_empty() {
                conn.execute("DELETE FROM conversation_topics WHERE id = ?1", params![id])?;
                continue;
            }
            let (start_id, new_start_ts) = &messages[0];
            let (end_id, new_end_ts) = &messages[messages.len() - 1];
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE conversation_topics
                 SET start_message_id = ?1, end_message_id = ?2, start_ts = ?3, end_ts = ?4,
                     message_count = ?5, updated_at = ?6
                 WHERE id = ?7",
                params![
                    start_id,
                    end_id,
                    new_start_ts,
                    new_end_ts,
                    messages.len() as i64,
                    now,
                    id
                ],
            )?;
        }
        Ok(())
    }

    /// Close the latest topic when it is still open, and remember `title` for the next section.
    /// Messages already waiting after that topic are counted so the caller can segment them.
    pub fn start_user_section(
        &self,
        scope: &TopicScope,
        topic_id: &str,
        title: &str,
    ) -> Result<i64, FinallyAValueBotError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(
                "section title is empty".into(),
            ));
        }
        let title = clamp_topic_title(trimmed);
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let topics = Self::list_conversation_topics_conn(&tx, scope)?;
        let topic = topics
            .iter()
            .find(|topic| topic.id == topic_id)
            .cloned()
            .ok_or_else(|| FinallyAValueBotError::ToolExecution("topic not found".into()))?;
        let latest = topics
            .last()
            .ok_or_else(|| FinallyAValueBotError::ToolExecution("topic not found".into()))?;
        if latest.id != topic.id {
            return Err(FinallyAValueBotError::ToolExecution(
                "a new section starts after the latest topic".into(),
            ));
        }
        let now = chrono::Utc::now().to_rfc3339();
        if topic.status == "open" {
            tx.execute(
                "UPDATE conversation_topics
                 SET status = 'closed', source = 'user', updated_at = ?1
                 WHERE id = ?2 AND chat_id = ?3 AND persona_id = ?4",
                params![now, topic.id, scope.chat_id, scope.persona_id],
            )?;
        }
        let bind = topic_scope_bind(scope);
        let count_sql = format!(
            "SELECT COUNT(*) FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND (timestamp > ?5 OR (timestamp = ?5 AND id > ?6))"
        );
        let unsegmented: i64 = tx.query_row(
            &count_sql,
            params![
                scope.chat_id,
                scope.persona_id,
                bind.is_main,
                bind.session_id,
                topic.end_ts,
                topic.end_message_id
            ],
            |row| row.get(0),
        )?;
        let key = pending_section_setting_key(scope);
        tx.execute(
            "INSERT INTO app_settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, title, now],
        )?;
        tx.commit()?;
        Ok(unsegmented)
    }

    fn message_ids_between(
        conn: &Connection,
        chat_id: i64,
        persona_id: i64,
        session_id: Option<&str>,
        start_ts: &str,
        start_id: &str,
        end_ts: &str,
        end_id: &str,
    ) -> Result<Vec<(String, String)>, FinallyAValueBotError> {
        let scope = TopicScope {
            chat_id,
            persona_id,
            session_id: normalize_topic_session(session_id),
        };
        let bind = topic_scope_bind(&scope);
        let sql = format!(
            "SELECT id, timestamp FROM messages
             WHERE {TOPIC_MESSAGE_SCOPE_SQL}
               AND (timestamp > ?5 OR (timestamp = ?5 AND id >= ?6))
               AND (timestamp < ?7 OR (timestamp = ?7 AND id <= ?8))
             ORDER BY timestamp ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                params![
                    chat_id,
                    persona_id,
                    bind.is_main,
                    bind.session_id,
                    start_ts,
                    start_id,
                    end_ts,
                    end_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

const TOPIC_MESSAGE_SCOPE_SQL: &str = "chat_id = ?1 AND persona_id = ?2
    AND ((?3 = 1 AND (
        session_id IS NULL
        OR EXISTS (
            SELECT 1 FROM chat_sessions cs
            WHERE cs.id = session_id AND cs.mirror_main_chat = 1
        )
    )) OR (?3 = 0 AND session_id = ?4))";

const TOPIC_MESSAGE_CURSOR_SQL: &str = "(
    ?5 = ''
    OR timestamp > ?5
    OR (timestamp = ?5 AND ((?7 = 1 AND id >= ?6) OR (?7 = 0 AND id > ?6)))
)";

struct TopicScopeBind {
    is_main: i64,
    session_id: String,
}

pub fn pending_section_setting_key(scope: &TopicScope) -> String {
    let session = scope.session_id.as_deref().unwrap_or("");
    format!(
        "topic_pending_section:{}:{}:{session}",
        scope.chat_id, scope.persona_id
    )
}

fn topic_scope_bind(scope: &TopicScope) -> TopicScopeBind {
    match normalize_topic_session(scope.session_id.as_deref()) {
        Some(session_id) => TopicScopeBind {
            is_main: 0,
            session_id,
        },
        None => TopicScopeBind {
            is_main: 1,
            session_id: String::new(),
        },
    }
}

fn clamp_topic_title(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = collapsed.chars().take(60).collect();
    if title.is_empty() {
        "Untitled".to_string()
    } else {
        title
    }
}

fn clamp_topic_summary(raw: &str) -> String {
    raw.trim().chars().take(280).collect()
}

fn combine_topic_summary(earlier: &str, later: &str) -> String {
    let earlier = earlier.trim();
    let later = later.trim();
    let joined = match (earlier.is_empty(), later.is_empty()) {
        (true, true) => String::new(),
        (false, true) => earlier.to_string(),
        (true, false) => later.to_string(),
        (false, false) if earlier == later => earlier.to_string(),
        (false, false) => format!("{earlier}\n{later}"),
    };
    clamp_topic_summary(&joined)
}

fn topic_contains_message(topic: &ConversationTopic, message_id: &str, timestamp: &str) -> bool {
    if topic.start_message_id == message_id || topic.end_message_id == message_id {
        return true;
    }
    timestamp >= topic.start_ts.as_str() && timestamp <= topic.end_ts.as_str()
}

fn topic_ranges_overlap(
    a_start_ts: &str,
    a_start_id: &str,
    a_end_ts: &str,
    a_end_id: &str,
    b_start_ts: &str,
    b_start_id: &str,
    b_end_ts: &str,
    b_end_id: &str,
) -> bool {
    !topic_cursor_before(a_end_ts, a_end_id, b_start_ts, b_start_id)
        && !topic_cursor_before(b_end_ts, b_end_id, a_start_ts, a_start_id)
}

fn topic_cursor_before(end_ts: &str, end_id: &str, start_ts: &str, start_id: &str) -> bool {
    end_ts < start_ts || (end_ts == start_ts && end_id < start_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (Database, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("meido_test_{}", uuid::Uuid::new_v4()));
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn test_persona(db: &Database, chat_id: i64) -> i64 {
        db.upsert_chat(chat_id, None, "private").unwrap();
        db.get_or_create_default_persona(chat_id).unwrap()
    }

    #[test]
    fn delete_conversation_topic_removes_messages_bookmarks_and_anchored_side_chats() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        let other = test_persona(&db, 200);
        for (id, chat, persona, timestamp, content) in [
            ("m1", 100, pid, "2026-02-01T00:00:00Z", "inside start"),
            ("m2", 100, pid, "2026-02-01T00:01:00Z", "inside end"),
            ("m3", 100, pid, "2026-02-01T00:02:00Z", "outside"),
            ("other", 200, other, "2026-02-01T00:00:30Z", "other chat"),
        ] {
            db.store_message(&StoredMessage {
                id: id.into(),
                chat_id: chat,
                persona_id: persona,
                session_id: None,
                sender_name: "test".into(),
                content: content.into(),
                is_from_bot: false,
                timestamp: timestamp.into(),
                origin: message_origin_interactive(),
                speaker_persona_id: None,
            })
            .unwrap();
        }
        let now = "2026-02-01T00:03:00Z";
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO conversation_topics (
                    id, chat_id, persona_id, session_id, title, summary,
                    start_message_id, end_message_id, start_ts, end_ts,
                    message_count, status, source, pinned, created_at, updated_at
                 ) VALUES (
                    'topic-a', 100, ?1, NULL, 'Alpha', '',
                    'm1', 'm2', '2026-02-01T00:00:00Z', '2026-02-01T00:01:00Z',
                    2, 'closed', 'user', 0, ?2, ?2
                 )",
                params![pid, now],
            )
            .unwrap();
        db.upsert_persona_message_bookmark(100, pid, "m1", "user", "inside start", None)
            .unwrap();
        db.upsert_persona_message_bookmark(100, pid, "m3", "user", "outside", None)
            .unwrap();
        let inside = db
            .ensure_side_chat(100, pid, None, "m1", "inside start")
            .unwrap();
        db.append_side_chat_turns(100, pid, &inside.id, "branch question", "branch answer")
            .unwrap();
        let outside = db
            .ensure_side_chat(100, pid, None, "m3", "outside")
            .unwrap();

        let removed = db.delete_conversation_topic(100, pid, "topic-a").unwrap();
        assert_eq!(removed, 2);
        let remaining = db.get_recent_messages(100, pid, 10, false).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "m3");
        let other_msgs = db.get_recent_messages(200, other, 10, false).unwrap();
        assert_eq!(other_msgs.len(), 1);
        assert!(db.get_side_chat(100, pid, &inside.id).unwrap().is_none());
        assert!(db.get_side_chat(100, pid, &outside.id).unwrap().is_some());
        let bookmarks: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM persona_message_bookmarks WHERE chat_id = 100 AND persona_id = ?1",
                params![pid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bookmarks, 1);
        let topics = db
            .list_conversation_topics(&TopicScope {
                chat_id: 100,
                persona_id: pid,
                session_id: None,
            })
            .unwrap();
        assert!(topics.is_empty());
        let missing = db
            .delete_conversation_topic(100, pid, "topic-a")
            .unwrap_err();
        assert!(missing.to_string().contains("not found"));
        cleanup(&dir);
    }

    #[test]
    fn test_persona_activity_includes_all_messages_but_not_drafts() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        assert!(db.list_persona_last_message_at(100).unwrap().is_empty());
        for (id, bot, timestamp, session) in [
            ("bot", true, "2026-01-01T00:00:00Z", None),
            ("user", false, "2026-01-02T00:00:00Z", None),
            ("session", false, "2026-01-03T00:00:00Z", Some("session")),
        ] {
            db.store_message(&StoredMessage {
                id: id.into(),
                chat_id: 100,
                persona_id: pid,
                session_id: session.map(str::to_owned),
                sender_name: "test".into(),
                content: "message".into(),
                is_from_bot: bot,
                timestamp: timestamp.into(),
                origin: message_origin_interactive(),
                speaker_persona_id: None,
            })
            .unwrap();
            assert_eq!(
                db.list_persona_last_message_at(100).unwrap(),
                vec![(pid, timestamp.to_owned())]
            );
        }
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO side_chats
                 (id, chat_id, persona_id, anchor_message_id, draft_text, created_at, updated_at)
                 VALUES ('side', 100, ?1, 'user', 'unsent draft', '2026-01-04T00:00:00Z', '2026-01-09T00:00:00Z')",
                params![pid],
            ).unwrap();
        }
        assert_eq!(
            db.list_persona_last_message_at(100).unwrap(),
            vec![(pid, "2026-01-03T00:00:00Z".into())]
        );
        for (seq, role, timestamp) in [
            (1, "user", "2026-01-05T00:00:00Z"),
            (2, "assistant", "2026-01-06T00:00:00Z"),
        ] {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO side_chat_turns (id, side_chat_id, seq, role, content, created_at)
                 VALUES (?1, 'side', ?2, ?3, 'message', ?4)",
                    params![format!("turn-{seq}"), seq, role, timestamp],
                )
                .unwrap();
            assert_eq!(
                db.list_persona_last_message_at(100).unwrap(),
                vec![(pid, timestamp.to_owned())]
            );
        }
        assert!(db.list_persona_last_message_at(200).unwrap().is_empty());
        assert_eq!(
            db.list_persona_last_bot_message_at(100).unwrap()[0].last_bot_message_at,
            "2026-01-01T00:00:00Z"
        );
        cleanup(&dir);
    }

    #[test]
    fn test_new_database_creates_tables() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 1);
        let msgs = db.get_recent_messages(1, pid, 10, false).unwrap();
        assert!(msgs.is_empty());
        let tasks = db.get_due_tasks("2099-01-01T00:00:00Z").unwrap();
        assert!(tasks.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_upsert_chat_insert_and_update() {
        let (db, dir) = test_db();
        db.upsert_chat(100, Some("Test Chat"), "group").unwrap();
        // Update title
        db.upsert_chat(100, Some("New Title"), "group").unwrap();
        // Insert without title
        db.upsert_chat(200, None, "private").unwrap();
        cleanup(&dir);
    }

    #[test]
    fn test_store_and_retrieve_message() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        let msg = StoredMessage {
            id: "msg1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "hello".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:00Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        };
        db.store_message(&msg).unwrap();

        let messages = db.get_recent_messages(100, pid, 10, false).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "msg1");
        assert_eq!(messages[0].sender_name, "alice");
        assert_eq!(messages[0].content, "hello");
        assert!(!messages[0].is_from_bot);
        cleanup(&dir);
    }

    #[test]
    fn test_store_message_upsert() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        let msg = StoredMessage {
            id: "msg1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "original".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:00Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        };
        db.store_message(&msg).unwrap();

        // Store same id again with different content (INSERT OR REPLACE)
        let msg2 = StoredMessage {
            id: "msg1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "updated".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        };
        db.store_message(&msg2).unwrap();

        let messages = db.get_recent_messages(100, pid, 10, false).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "updated");
        cleanup(&dir);
    }

    #[test]
    fn test_get_recent_messages_ordering_and_limit() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        for i in 0..5 {
            let msg = StoredMessage {
                id: format!("msg{i}"),
                chat_id: 100,
                persona_id: pid,
                session_id: None,
                sender_name: "alice".into(),
                content: format!("message {i}"),
                is_from_bot: false,
                timestamp: format!("2024-01-01T00:00:0{i}Z"),
                origin: crate::db::message_origin_interactive(),
                speaker_persona_id: None,
            };
            db.store_message(&msg).unwrap();
        }

        // Limit to 3 - should get the 3 most recent, but reversed to oldest-first
        let messages = db.get_recent_messages(100, pid, 3, false).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "message 2"); // oldest of the 3 most recent
        assert_eq!(messages[1].content, "message 3");
        assert_eq!(messages[2].content, "message 4"); // most recent

        // Different chat_id should be empty
        let pid2 = test_persona(&db, 200);
        let messages = db.get_recent_messages(200, pid2, 10, false).unwrap();
        assert!(messages.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_get_messages_since_last_bot_response_with_bot_msg() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);

        // User message 1
        db.store_message(&StoredMessage {
            id: "m1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "hi".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // Bot response
        db.store_message(&StoredMessage {
            id: "m2".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bot".into(),
            content: "hello!".into(),
            is_from_bot: true,
            timestamp: "2024-01-01T00:00:02Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // User message 2 (after bot response)
        db.store_message(&StoredMessage {
            id: "m3".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "how are you?".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:03Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // User message 3
        db.store_message(&StoredMessage {
            id: "m4".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bob".into(),
            content: "me too".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:04Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        let messages = db
            .get_messages_since_last_bot_response(100, pid, 50, 10, false)
            .unwrap();
        // Should include the bot message and everything after it
        assert!(messages.len() >= 2);
        // First should be the bot msg or after it
        assert_eq!(messages[0].id, "m2"); // the bot message (timestamp >= bot's timestamp)
        assert_eq!(messages[1].id, "m3");
        assert_eq!(messages[2].id, "m4");
        cleanup(&dir);
    }

    #[test]
    fn test_get_messages_since_last_bot_response_no_bot_msg() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);

        for i in 0..5 {
            db.store_message(&StoredMessage {
                id: format!("m{i}"),
                chat_id: 100,
                persona_id: pid,
                session_id: None,
                sender_name: "alice".into(),
                content: format!("msg {i}"),
                is_from_bot: false,
                timestamp: format!("2024-01-01T00:00:0{i}Z"),
                origin: crate::db::message_origin_interactive(),
                speaker_persona_id: None,
            })
            .unwrap();
        }

        // Fallback to last 3
        let messages = db
            .get_messages_since_last_bot_response(100, pid, 50, 3, false)
            .unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "msg 2");
        assert_eq!(messages[2].content, "msg 4");
        cleanup(&dir);
    }

    #[test]
    fn test_create_and_get_scheduled_task() {
        let (db, dir) = test_db();
        let persona_id = test_persona(&db, 100);
        let id = db
            .create_scheduled_task(
                100,
                "say hello",
                "cron",
                "0 */5 * * * *",
                "2024-06-01T00:05:00Z",
            )
            .unwrap();
        assert!(id > 0);

        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].persona_id, persona_id);
        assert_eq!(tasks[0].prompt, "say hello");
        assert_eq!(tasks[0].schedule_type, "cron");
        assert_eq!(tasks[0].status, "active");
        cleanup(&dir);
    }

    #[test]
    fn test_create_scheduled_task_for_persona_binds_explicit_persona() {
        let (db, dir) = test_db();
        let default_pid = test_persona(&db, 100);
        let alt_pid = db.create_persona(100, "alt", None).unwrap();
        db.set_active_persona(100, default_pid).unwrap();

        let id = db
            .create_scheduled_task_for_persona(
                100,
                alt_pid,
                "run as alt persona",
                "once",
                "2099-12-31T00:00:00Z",
                "2099-12-31T00:00:00Z",
            )
            .unwrap();

        let task = db.get_task_by_id(id).unwrap().unwrap();
        assert_eq!(task.persona_id, alt_pid);
        cleanup(&dir);
    }

    #[test]
    fn test_builtin_vault_tasks_use_distinct_run_times() {
        let (db, dir) = test_db();
        let persona_id = test_persona(&db, 100);
        db.ensure_indexing_task(
            100,
            persona_id,
            "Run the vault indexing script: python3 index.py",
            "0 0 */6 * * *",
            "UTC",
        )
        .unwrap();
        db.ensure_vault_push_task(
            100,
            persona_id,
            "Sync ORIGIN vault to git remote: git push",
            "0 0 3,9,15,21 * * *",
            "UTC",
        )
        .unwrap();

        let mut tasks = db.get_tasks_for_chat(100).unwrap();
        tasks.sort_by_key(|task| task.prompt.clone());
        assert_eq!(tasks.len(), 2);
        assert_ne!(tasks[0].schedule_value, tasks[1].schedule_value);
        assert_ne!(tasks[0].next_run, tasks[1].next_run);
        assert_eq!(tasks[0].persona_id, persona_id);
        assert_eq!(tasks[1].persona_id, persona_id);

        db.ensure_indexing_task(
            100,
            persona_id,
            "Run the vault indexing script: python3 index.py",
            "0 0 */6 * * *",
            "UTC",
        )
        .unwrap();
        assert_eq!(db.get_tasks_for_chat(100).unwrap().len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn test_builtin_vault_tasks_collapse_across_personas() {
        let (db, dir) = test_db();
        let owner = db.ensure_named_persona(100, "default").unwrap();
        let other = db.create_persona(100, "other", None).unwrap();
        db.set_active_persona(100, other).unwrap();
        db.create_scheduled_task_for_persona(
            100,
            other,
            "Run the vault indexing script: old",
            "cron",
            "0 0 */6 * * *",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        db.create_scheduled_task_for_persona(
            100,
            owner,
            "Run the vault indexing script: older-looking",
            "cron",
            "0 0 */6 * * *",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        db.ensure_indexing_task(
            100,
            owner,
            "Run the vault indexing script: python3 index.py",
            "0 0 */6 * * *",
            "UTC",
        )
        .unwrap();
        let active: Vec<_> = db
            .get_tasks_for_chat(100)
            .unwrap()
            .into_iter()
            .filter(|task| task.prompt.starts_with("Run the vault indexing script:"))
            .filter(|task| task.status == "active")
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].persona_id, owner);
        assert_eq!(db.get_active_persona_id(100).unwrap(), Some(other));
        cleanup(&dir);
    }

    #[test]
    fn test_shared_builtin_cron_moves_only_when_still_the_default() {
        let (db, dir) = test_db();
        let persona_id = test_persona(&db, 100);
        let prompt = "Sync ORIGIN vault to git remote: git push";
        db.ensure_vault_push_task(100, persona_id, prompt, "0 0 */6 * * *", "UTC")
            .unwrap();
        db.ensure_vault_push_task(100, persona_id, prompt, "0 0 3,9,15,21 * * *", "UTC")
            .unwrap();
        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].schedule_value, "0 0 3,9,15,21 * * *");

        let other_persona = test_persona(&db, 200);
        db.ensure_vault_push_task(200, other_persona, prompt, "0 15 4 * * *", "UTC")
            .unwrap();
        db.ensure_vault_push_task(200, other_persona, prompt, "0 0 3,9,15,21 * * *", "UTC")
            .unwrap();
        let tasks = db.get_tasks_for_chat(200).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].schedule_value, "0 15 4 * * *");
        cleanup(&dir);
    }

    #[test]
    fn test_get_due_tasks() {
        let (db, dir) = test_db();
        db.create_scheduled_task(100, "task1", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();
        db.create_scheduled_task(
            100,
            "task2",
            "once",
            "2099-12-31T00:00:00Z",
            "2099-12-31T00:00:00Z",
        )
        .unwrap();

        // Only task1 is due
        let due = db.get_due_tasks("2024-06-01T00:00:00Z").unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].prompt, "task1");

        // Both are due in the far future
        let due = db.get_due_tasks("2100-01-01T00:00:00Z").unwrap();
        assert_eq!(due.len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn test_get_tasks_for_chat_filters_status() {
        let (db, dir) = test_db();
        let id1 = db
            .create_scheduled_task(
                100,
                "active task",
                "cron",
                "0 * * * * *",
                "2024-01-01T00:00:00Z",
            )
            .unwrap();
        let id2 = db
            .create_scheduled_task(
                100,
                "to cancel",
                "once",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
            )
            .unwrap();
        db.update_task_status(id2, "cancelled").unwrap();

        // Only active/paused tasks should be returned
        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id1);

        // Pause the active one
        db.update_task_status(id1, "paused").unwrap();
        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "paused");
        cleanup(&dir);
    }

    #[test]
    fn test_update_task_status() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        assert!(db.update_task_status(id, "paused").unwrap());
        assert!(db.update_task_status(id, "active").unwrap());
        assert!(db.update_task_status(id, "cancelled").unwrap());

        // Non-existent task
        assert!(!db.update_task_status(9999, "paused").unwrap());
        cleanup(&dir);
    }

    #[test]
    fn test_update_task_after_run_cron() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        db.update_task_after_run(id, "2024-01-01T00:01:00Z", Some("2024-01-01T00:02:00Z"))
            .unwrap();

        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert_eq!(tasks[0].last_run.as_deref(), Some("2024-01-01T00:01:00Z"));
        assert_eq!(tasks[0].next_run, "2024-01-01T00:02:00Z");
        assert_eq!(tasks[0].status, "active");
        cleanup(&dir);
    }

    #[test]
    fn test_update_task_after_run_one_shot() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(
                100,
                "test",
                "once",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
            )
            .unwrap();

        // One-shot: no next_run, should mark as completed
        db.update_task_after_run(id, "2024-01-01T00:00:00Z", None)
            .unwrap();

        // Should not appear in active/paused list
        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert!(tasks.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_delete_task() {
        let (db, dir) = test_db();
        let id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        assert!(db.delete_task(id).unwrap());
        assert!(!db.delete_task(id).unwrap()); // already deleted

        let tasks = db.get_tasks_for_chat(100).unwrap();
        assert!(tasks.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_get_all_messages() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        for i in 0..5 {
            db.store_message(&StoredMessage {
                id: format!("msg{i}"),
                chat_id: 100,
                persona_id: pid,
                session_id: None,
                sender_name: "alice".into(),
                content: format!("message {i}"),
                is_from_bot: false,
                timestamp: format!("2024-01-01T00:00:0{i}Z"),
                origin: crate::db::message_origin_interactive(),
                speaker_persona_id: None,
            })
            .unwrap();
        }

        let messages = db.get_all_messages(100, pid).unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].content, "message 0");
        assert_eq!(messages[4].content, "message 4");

        // Different chat should be empty
        let pid2 = test_persona(&db, 200);
        assert!(db.get_all_messages(200, pid2).unwrap().is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_log_task_run() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        let log_id = db
            .log_task_run(
                task_id,
                100,
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:05Z",
                5000,
                true,
                Some("Success"),
            )
            .unwrap();
        assert!(log_id > 0);

        let logs = db.get_task_run_logs(task_id, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].task_id, task_id);
        assert_eq!(logs[0].duration_ms, 5000);
        assert!(logs[0].success);
        assert_eq!(logs[0].result_summary.as_deref(), Some("Success"));
        cleanup(&dir);
    }

    #[test]
    fn test_get_task_run_logs_ordering_and_limit() {
        let (db, dir) = test_db();
        let task_id = db
            .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
            .unwrap();

        for i in 0..5 {
            db.log_task_run(
                task_id,
                100,
                &format!("2024-01-01T00:0{i}:00Z"),
                &format!("2024-01-01T00:0{i}:05Z"),
                5000,
                true,
                Some(&format!("Run {i}")),
            )
            .unwrap();
        }

        // Limit to 3, most recent first
        let logs = db.get_task_run_logs(task_id, 3).unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0].result_summary.as_deref(), Some("Run 4")); // most recent
        assert_eq!(logs[2].result_summary.as_deref(), Some("Run 2"));
        cleanup(&dir);
    }

    #[test]
    fn test_save_and_load_session() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        let json = r#"[{"role":"user","content":"hello"}]"#;
        db.save_session(100, pid, json).unwrap();

        let result = db.load_session(100, pid).unwrap();
        assert!(result.is_some());
        let (loaded_json, updated_at) = result.unwrap();
        assert_eq!(loaded_json, json);
        assert!(!updated_at.is_empty());

        // Upsert: save again with different data
        let json2 = r#"[{"role":"user","content":"hello"},{"role":"assistant","content":"hi"}]"#;
        db.save_session(100, pid, json2).unwrap();
        let (loaded_json2, _) = db.load_session(100, pid).unwrap().unwrap();
        assert_eq!(loaded_json2, json2);

        cleanup(&dir);
    }

    #[test]
    fn test_load_session_nonexistent() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 999);
        let result = db.load_session(999, pid).unwrap();
        assert!(result.is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_delete_session() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        db.save_session(100, pid, "[]").unwrap();
        assert!(db.delete_session(100, pid).unwrap());
        assert!(db.load_session(100, pid).unwrap().is_none());
        // Delete again returns false
        assert!(!db.delete_session(100, pid).unwrap());
        cleanup(&dir);
    }

    #[test]
    fn test_get_new_user_messages_since() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);

        // Messages before the cutoff
        db.store_message(&StoredMessage {
            id: "m1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "old msg".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // Bot message at the cutoff
        db.store_message(&StoredMessage {
            id: "m2".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bot".into(),
            content: "response".into(),
            is_from_bot: true,
            timestamp: "2024-01-01T00:00:02Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // User messages after cutoff
        db.store_message(&StoredMessage {
            id: "m3".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "new msg 1".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:03Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        db.store_message(&StoredMessage {
            id: "m4".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bob".into(),
            content: "new msg 2".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:04Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        // Bot message after cutoff (should be excluded - only non-bot)
        db.store_message(&StoredMessage {
            id: "m5".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "bot".into(),
            content: "bot again".into(),
            is_from_bot: true,
            timestamp: "2024-01-01T00:00:05Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();

        let msgs = db
            .get_new_user_messages_since(100, pid, "2024-01-01T00:00:02Z")
            .unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "new msg 1");
        assert_eq!(msgs[1].content, "new msg 2");

        cleanup(&dir);
    }

    #[test]
    fn test_persona_bulletin_events_latest_first() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        db.append_persona_bulletin_event(
            100,
            pid,
            Some("run:1"),
            "memory_update",
            "Updated memory",
            Some("Tiered memory written"),
        )
        .unwrap();
        db.append_persona_bulletin_event(
            100,
            pid,
            Some("run:2"),
            "file_update",
            "Updated file",
            Some("Edited src/main.rs"),
        )
        .unwrap();

        let items = db.list_persona_bulletin_events(100, pid, 1).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].event_type, "file_update");
        assert_eq!(items[0].title, "Updated file");
        cleanup(&dir);
    }

    #[test]
    fn test_persona_bulletin_focus_upsert_and_get() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        db.upsert_persona_bulletin_focus(
            100,
            pid,
            Some("Current focus"),
            "Wing: origin\nRooms: 19",
        )
        .unwrap();
        db.upsert_persona_bulletin_focus(100, pid, None, "Updated focus content")
            .unwrap();
        let focus = db.get_persona_bulletin_focus(100, pid).unwrap().unwrap();
        assert_eq!(focus.chat_id, 100);
        assert_eq!(focus.persona_id, pid);
        assert_eq!(focus.title, None);
        assert_eq!(focus.content, "Updated focus content");
        assert!(!focus.updated_at.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_persona_message_bookmark_crud() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 100);
        db.store_message(&StoredMessage {
            id: "m-bookmark-1".into(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "bookmark me".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
        assert!(db
            .message_exists_in_persona(100, pid, "m-bookmark-1")
            .unwrap());
        assert!(!db.message_exists_in_persona(100, pid, "missing").unwrap());

        db.upsert_persona_message_bookmark(
            100,
            pid,
            "m-bookmark-1",
            "user",
            "bookmark me",
            Some("important"),
        )
        .unwrap();
        db.upsert_persona_message_bookmark(
            100,
            pid,
            "m-bookmark-1",
            "user",
            "bookmark me updated",
            Some("still important"),
        )
        .unwrap();

        let items = db.list_persona_message_bookmarks(100, pid, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message_id, "m-bookmark-1");
        assert_eq!(items[0].content_preview, "bookmark me updated");
        assert_eq!(items[0].note.as_deref(), Some("still important"));
        assert!(!items[0].missing);
        assert_eq!(
            items[0].message_timestamp.as_deref(),
            Some("2024-01-01T00:00:01Z")
        );

        assert!(db
            .delete_persona_message_bookmark(100, pid, "m-bookmark-1")
            .unwrap());
        assert!(!db
            .delete_persona_message_bookmark(100, pid, "m-bookmark-1")
            .unwrap());
        cleanup(&dir);
    }

    #[test]
    fn test_message_cursor_pagination_before_after_around() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 200);
        for i in 1..=10 {
            db.store_message(&StoredMessage {
                id: format!("m{i}"),
                chat_id: 200,
                persona_id: pid,
                session_id: None,
                sender_name: "alice".into(),
                content: format!("msg {i}"),
                is_from_bot: false,
                timestamp: format!("2024-01-01T00:00:{i:02}Z"),
                origin: crate::db::message_origin_interactive(),
                speaker_persona_id: None,
            })
            .unwrap();
        }

        let (before, has_older) = db
            .get_messages_before_id(200, pid, "m5", 2, None)
            .unwrap()
            .expect("cursor exists");
        assert_eq!(
            before.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["m3", "m4"]
        );
        assert!(has_older);

        let (after, has_newer) = db
            .get_messages_after_id(200, pid, "m5", 2, None)
            .unwrap()
            .expect("cursor exists");
        assert_eq!(
            after.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["m6", "m7"]
        );
        assert!(has_newer);

        let page = db
            .get_messages_around_id(200, pid, "m5", 2, 2, None)
            .unwrap()
            .expect("around exists");
        assert_eq!(
            page.messages
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m3", "m4", "m5", "m6", "m7"]
        );
        assert!(page.has_older);
        assert!(page.has_newer);

        let edge = db
            .get_messages_around_id(200, pid, "m1", 2, 2, None)
            .unwrap()
            .expect("around m1");
        assert!(!edge.has_older);
        assert!(edge.has_newer);
        assert_eq!(edge.messages[0].id, "m1");

        let tip = db
            .get_messages_around_id(200, pid, "m10", 2, 2, None)
            .unwrap()
            .expect("around m10");
        assert!(tip.has_older);
        assert!(!tip.has_newer);

        assert!(db
            .get_messages_before_id(200, pid, "missing", 2, None)
            .unwrap()
            .is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_persona_message_bookmark_missing_flag_when_message_deleted() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 101);
        db.store_message(&StoredMessage {
            id: "m-gone".into(),
            chat_id: 101,
            persona_id: pid,
            session_id: None,
            sender_name: "alice".into(),
            content: "soon gone".into(),
            is_from_bot: false,
            timestamp: "2024-01-01T00:00:01Z".into(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
        db.upsert_persona_message_bookmark(101, pid, "m-gone", "user", "soon gone", None)
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("DELETE FROM messages WHERE id = ?1", params!["m-gone"])
                .unwrap();
        }
        let items = db.list_persona_message_bookmarks(101, pid, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].missing);
        assert!(items[0].message_timestamp.is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_builtin_postdelivery_focus_sync_hook_seeded() {
        let (db, dir) = test_db();
        let hooks = db.list_hook_definitions().unwrap();
        let focus_hook = hooks
            .iter()
            .find(|h| h.name == "postdelivery-persona-focus-sync")
            .expect("focus sync hook must be seeded");
        assert_eq!(focus_hook.event_name, "PostDelivery");
        assert_eq!(focus_hook.action_type, "builtin_persona_focus_sync");
        assert!(focus_hook.enabled);
        cleanup(&dir);
    }

    #[test]
    fn test_shipped_hooks_loaded_from_builtin_hooks_catalog() {
        let (db, dir) = test_db();
        let hooks = db.list_hook_definitions().unwrap();
        assert!(
            !hooks.iter().any(|h| h.name.starts_with("template-")),
            "template example hooks must not be seeded"
        );
        assert!(
            !hooks
                .iter()
                .any(|h| h.name == "posttool-pz-terminal-cleanup"),
            "PZ hook must not be shipped in builtin catalog"
        );

        let builtin_names = [
            "postdelivery-persona-focus-sync",
            "beforeturn-scheduler-policy-context",
            "pretool-turn-skill-gate",
            "prestop-deferred-commitment-guard",
            "postbatch-loop-guard",
            "predelivery-dense-delivery-guard",
        ];
        for name in builtin_names {
            let hook = hooks
                .iter()
                .find(|h| h.name == name)
                .unwrap_or_else(|| panic!("missing shipped builtin hook {name}"));
            assert!(
                hook.action_type.starts_with("builtin_"),
                "{} must use builtin_* action_type, got {}",
                name,
                hook.action_type
            );
            assert!(hook.enabled, "{name} should be enabled by default");
            assert!(
                hook.scoped_persona_ids.is_none(),
                "{name} should be globally scoped"
            );
        }
        assert_eq!(
            hooks.len(),
            builtin_names.len(),
            "fresh DB should contain only the six shipped catalog hooks: {:?}",
            hooks.iter().map(|h| &h.name).collect::<Vec<_>>()
        );
        cleanup(&dir);
    }

    #[test]
    fn test_builtin_predelivery_dense_delivery_hook_seeded() {
        let (db, dir) = test_db();
        let hooks = db.list_hook_definitions().unwrap();
        let hook = hooks
            .iter()
            .find(|h| h.name == "predelivery-dense-delivery-guard")
            .expect("dense delivery hook must be seeded");
        assert_eq!(hook.event_name, "PreDelivery");
        assert_eq!(hook.action_type, "builtin_dense_delivery_guard");
        assert!(hook.enabled);
        assert!(hook.scoped_persona_ids.is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_persona_delivery_policy_and_engine_override() {
        let (db, dir) = test_db();
        let chat_id = 42;
        let pid = test_persona(&db, chat_id);
        let p = db.get_persona(pid).unwrap().unwrap();
        assert!(!p.dense_delivery_enabled);
        assert!(p.agent_engine_override.is_none());

        db.set_persona_delivery_policy(chat_id, pid, true, Some(1800), None, Some(600))
            .unwrap();
        db.set_persona_engine_override(chat_id, pid, Some("cursor"))
            .unwrap();
        let p = db.get_persona(pid).unwrap().unwrap();
        assert!(p.dense_delivery_enabled);
        assert_eq!(p.dense_delivery_messaging_max_chars, Some(1800));
        assert_eq!(p.dense_delivery_web_max_chars, None);
        assert_eq!(p.dense_delivery_summary_chars, Some(600));
        assert_eq!(p.agent_engine_override.as_deref(), Some("cursor"));

        db.set_persona_engine_override(chat_id, pid, Some("  "))
            .unwrap();
        let p = db.get_persona(pid).unwrap().unwrap();
        assert!(p.agent_engine_override.is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_hook_persona_status_fields() {
        let (db, dir) = test_db();
        let chat_id = 9007;
        let owner_persona_id = db
            .create_persona(chat_id, "owner", None)
            .expect("create owner persona");
        let other_persona_id = db
            .create_persona(chat_id, "other", None)
            .expect("create other persona");
        let global_hook_id = db
            .upsert_hook_definition(
                None,
                "global-hook",
                "BeforeTurn",
                None,
                "add_context",
                r#"{"additional_context":"global"}"#,
                None,
                true,
            )
            .expect("upsert global hook");
        let scoped_hook_id = db
            .upsert_hook_definition(
                None,
                "scoped-hook",
                "BeforeTurn",
                None,
                "add_context",
                r#"{"additional_context":"scoped"}"#,
                Some(&[owner_persona_id]),
                true,
            )
            .expect("upsert scoped hook");
        let disabled_hook_id = db
            .upsert_hook_definition(
                None,
                "disabled-hook",
                "BeforeTurn",
                None,
                "add_context",
                r#"{"additional_context":"disabled"}"#,
                None,
                false,
            )
            .expect("upsert disabled hook");

        let hooks = db.list_hook_definitions().expect("list hooks");
        let global_hook = hooks
            .iter()
            .find(|h| h.id == global_hook_id)
            .expect("global hook");
        let scoped_hook = hooks
            .iter()
            .find(|h| h.id == scoped_hook_id)
            .expect("scoped hook");
        let disabled_hook = hooks
            .iter()
            .find(|h| h.id == disabled_hook_id)
            .expect("disabled hook");

        let (scoped, allowed, active) = global_hook
            .persona_status(&db, chat_id, owner_persona_id)
            .expect("global owner status");
        assert!(scoped);
        assert!(allowed);
        assert!(active);

        let (scoped, allowed, active) = scoped_hook
            .persona_status(&db, chat_id, owner_persona_id)
            .expect("scoped owner status");
        assert!(scoped);
        assert!(allowed);
        assert!(active);

        let (scoped, allowed, active) = scoped_hook
            .persona_status(&db, chat_id, other_persona_id)
            .expect("scoped other status");
        assert!(!scoped);
        assert!(allowed);
        assert!(!active);

        db.set_persona_hook_skill_policy(chat_id, owner_persona_id, Some(&[]), None)
            .expect("block all hooks for owner");
        let (_, allowed, active) = global_hook
            .persona_status(&db, chat_id, owner_persona_id)
            .expect("global owner blocked status");
        assert!(!allowed);
        assert!(!active);

        let (_, _, active) = disabled_hook
            .persona_status(&db, chat_id, other_persona_id)
            .expect("disabled status");
        assert!(!active);

        cleanup(&dir);
    }

    #[test]
    fn test_background_shell_job_persists_session_id() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 42);
        db.create_background_shell_job(
            "job-sess",
            42,
            pid,
            "label",
            "echo hi",
            "/tmp",
            "tmux-sess",
            "/tmp/out.log",
            "tool",
            Some("focused-session-1"),
        )
        .unwrap();
        let job = db.get_background_job("job-sess").unwrap().unwrap();
        assert_eq!(job.session_id.as_deref(), Some("focused-session-1"));
        db.create_background_job("job-main", 42, pid, "prompt", "timeout", None)
            .unwrap();
        let main = db.get_background_job("job-main").unwrap().unwrap();
        assert!(main.session_id.is_none());
        cleanup(&dir);
    }

    fn store_topic_message(db: &Database, chat_id: i64, persona_id: i64, id: &str, ts: &str) {
        db.store_message(&StoredMessage {
            id: id.into(),
            chat_id,
            persona_id,
            session_id: None,
            sender_name: "user".into(),
            content: format!("body {id}"),
            is_from_bot: false,
            timestamp: ts.into(),
            origin: message_origin_interactive(),
            speaker_persona_id: None,
        })
        .unwrap();
    }

    #[test]
    fn test_conversation_topics_commit_merge_split_and_freeze() {
        let (db, dir) = test_db();
        let pid = test_persona(&db, 7);
        for (id, ts) in [
            ("m1", "2026-04-01T00:00:00Z"),
            ("m2", "2026-04-01T00:01:00Z"),
            ("m3", "2026-04-01T00:02:00Z"),
            ("m4", "2026-04-01T00:03:00Z"),
        ] {
            store_topic_message(&db, 7, pid, id, ts);
        }
        let scope = TopicScope {
            chat_id: 7,
            persona_id: pid,
            session_id: None,
        };
        db.commit_auto_segmentation(
            &scope,
            None,
            &[ConversationTopicDraft {
                title: "Planning".into(),
                summary: String::new(),
                start_message_id: "m1".into(),
                end_message_id: "m4".into(),
                start_ts: "2026-04-01T00:00:00Z".into(),
                end_ts: "2026-04-01T00:03:00Z".into(),
                message_count: 4,
                status: "open".into(),
            }],
        )
        .unwrap();
        let topics = db.list_conversation_topics(&scope).unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].source, "auto");
        let open_id = topics[0].id.clone();

        let renamed = db
            .patch_conversation_topic(7, pid, &open_id, Some("Keep this"), None)
            .unwrap()
            .unwrap();
        assert_eq!(renamed.source, "user");
        assert_eq!(renamed.title, "Keep this");
        let refused = db.commit_auto_segmentation(
            &scope,
            Some(&open_id),
            &[ConversationTopicDraft {
                title: "Overwrite".into(),
                summary: String::new(),
                start_message_id: "m1".into(),
                end_message_id: "m4".into(),
                start_ts: "2026-04-01T00:00:00Z".into(),
                end_ts: "2026-04-01T00:03:00Z".into(),
                message_count: 4,
                status: "open".into(),
            }],
        );
        assert!(refused.is_err());
        assert_eq!(
            db.get_conversation_topic(7, pid, &open_id)
                .unwrap()
                .unwrap()
                .title,
            "Keep this"
        );

        let (_earlier, later) = db.split_conversation_topic(&scope, "m3").unwrap();
        assert_eq!(later.start_message_id, "m3");
        assert_eq!(later.source, "user");
        let merged = db
            .merge_conversation_topic_with_previous(7, pid, &later.id)
            .unwrap();
        assert_eq!(merged.start_message_id, "m1");
        assert_eq!(merged.end_message_id, "m4");
        assert_eq!(db.list_conversation_topics(&scope).unwrap().len(), 1);

        assert!(db.delete_message(7, pid, "m2").unwrap());
        let after = db.list_conversation_topics(&scope).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message_count, 3);
        cleanup(&dir);
    }

    #[test]
    fn test_topic_ranges_do_not_overlap_when_one_ends_before_the_next() {
        assert!(!topic_ranges_overlap(
            "2026-01-01T00:00:00Z",
            "a",
            "2026-01-01T00:01:00Z",
            "b",
            "2026-01-01T00:02:00Z",
            "c",
            "2026-01-01T00:03:00Z",
            "d",
        ));
        assert!(topic_ranges_overlap(
            "2026-01-01T00:00:00Z",
            "a",
            "2026-01-01T00:02:00Z",
            "c",
            "2026-01-01T00:02:00Z",
            "c",
            "2026-01-01T00:03:00Z",
            "d",
        ));
    }
}
