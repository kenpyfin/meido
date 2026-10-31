mod artifacts_api;
mod env_config;
use std::collections::{HashMap, VecDeque};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::WebSocket;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use base64::Engine;
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, warn};

use crate::agent::{
    archive_conversation, history_to_claude_messages, process_with_agent,
    process_with_agent_with_events, trim_to_recent_balanced, AgentEvent, AgentRequestContext,
    AppState, InboundImages,
};
use crate::background_jobs::{
    await_handoff_startup_ack, handoff_trigger_for_db, is_background_handoff_response,
    try_enqueue_background_handoff, HandoffEnqueueOutcome,
};
use crate::channel::{deliver_agent_final_to_contact, deliver_to_contact, DeliveryScope};
use crate::chat_queue::{QueueEnqueueMeta, QueueRemoveOutcome, QueueSource};
use crate::claude::{Message, MessageContent};
use crate::config::Config;
use crate::db::{
    call_blocking, ChannelBotInstance, ChannelPersonaMode, JobHeartbeat, Persona,
    PersonaMessageBookmark, StoredMessage, BOT_INSTANCE_WEB,
};
use crate::final_delivery_dedupe::{ensure_visible_turn_text, failed_turn_notice};
use crate::hook_executor::validate_command_payload;
use crate::slash_commands::{parse as parse_slash_command, SlashCommand};
use crate::social_oauth;
use crate::web_terminal::{self, TerminalHub};
use std::time::SystemTime;

static WEB_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

#[derive(Clone)]
pub(crate) struct WebState {
    pub(crate) app_state: Arc<AppState>,
    pub(crate) auth_token: Option<String>,
    run_hub: RunHub,
    request_hub: RequestHub,
    terminal_hub: TerminalHub,
    limits: WebLimits,
    /// Cache for `ensure_web_binding_for_universal` to avoid repeated DB writes on every
    /// `/api/history` poll while this server process is alive.
    web_binding_universal_done: Arc<Mutex<Option<i64>>>,
}

#[derive(Clone, Debug)]
struct RunEvent {
    id: u64,
    event: String,
    data: String,
}

#[derive(Clone, Default)]
struct RunHub {
    channels: Arc<Mutex<HashMap<String, RunChannel>>>,
}

#[derive(Clone, Debug)]
struct WebLimits {
    max_inflight_per_session: usize,
    max_requests_per_window: usize,
    rate_window: Duration,
    run_history_limit: usize,
    session_idle_ttl: Duration,
}

impl Default for WebLimits {
    fn default() -> Self {
        Self {
            max_inflight_per_session: 2,
            max_requests_per_window: 8,
            rate_window: Duration::from_secs(10),
            run_history_limit: 512,
            session_idle_ttl: Duration::from_secs(300),
        }
    }
}

impl WebLimits {
    fn from_config(cfg: &Config) -> Self {
        Self {
            max_inflight_per_session: cfg.web_max_inflight_per_session,
            max_requests_per_window: cfg.web_max_requests_per_window,
            rate_window: Duration::from_secs(cfg.web_rate_window_seconds),
            run_history_limit: cfg.web_run_history_limit,
            session_idle_ttl: Duration::from_secs(cfg.web_session_idle_ttl_seconds),
        }
    }
}

#[derive(Clone, Default)]
struct RequestHub {
    sessions: Arc<Mutex<HashMap<String, SessionQuota>>>,
}

struct SessionQuota {
    inflight: usize,
    recent: VecDeque<Instant>,
    last_touch: Instant,
}

impl Default for SessionQuota {
    fn default() -> Self {
        Self {
            inflight: 0,
            recent: VecDeque::new(),
            last_touch: Instant::now(),
        }
    }
}

#[derive(Clone)]
struct RunChannel {
    sender: broadcast::Sender<RunEvent>,
    history: VecDeque<RunEvent>,
    next_id: u64,
    done: bool,
}

impl RunHub {
    async fn create(&self, run_id: &str) {
        let (tx, _) = broadcast::channel(512);
        let mut guard = self.channels.lock().await;
        guard.insert(
            run_id.to_string(),
            RunChannel {
                sender: tx,
                history: VecDeque::new(),
                next_id: 1,
                done: false,
            },
        );
    }

    async fn publish(&self, run_id: &str, event: &str, data: String, history_limit: usize) {
        let mut guard = self.channels.lock().await;
        let Some(channel) = guard.get_mut(run_id) else {
            return;
        };

        let evt = RunEvent {
            id: channel.next_id,
            event: event.to_string(),
            data,
        };
        channel.next_id = channel.next_id.saturating_add(1);
        if channel.history.len() >= history_limit {
            let _ = channel.history.pop_front();
        }
        channel.history.push_back(evt.clone());
        if evt.event == "done" || evt.event == "error" {
            channel.done = true;
        }
        let _ = channel.sender.send(evt);
    }

    async fn subscribe_with_replay(
        &self,
        run_id: &str,
        last_event_id: Option<u64>,
    ) -> Option<(
        broadcast::Receiver<RunEvent>,
        Vec<RunEvent>,
        bool,
        bool,
        Option<u64>,
    )> {
        let guard = self.channels.lock().await;
        let channel = guard.get(run_id)?;
        let oldest_event_id = channel.history.front().map(|e| e.id);
        let replay_truncated = matches!(
            (last_event_id, oldest_event_id),
            (Some(last), Some(oldest)) if last.saturating_add(1) < oldest
        );
        let replay = channel
            .history
            .iter()
            .filter(|e| last_event_id.is_none_or(|id| e.id > id))
            .cloned()
            .collect::<Vec<_>>();
        Some((
            channel.sender.subscribe(),
            replay,
            channel.done,
            replay_truncated,
            oldest_event_id,
        ))
    }

    async fn status(&self, run_id: &str) -> Option<(bool, u64)> {
        let guard = self.channels.lock().await;
        let channel = guard.get(run_id)?;
        Some((channel.done, channel.next_id.saturating_sub(1)))
    }

    async fn remove_later(&self, run_id: String, after_seconds: u64) {
        let channels = self.channels.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(after_seconds)).await;
            let mut guard = channels.lock().await;
            guard.remove(&run_id);
        });
    }
}

impl RequestHub {
    async fn begin(
        &self,
        session_key: &str,
        limits: &WebLimits,
    ) -> Result<(), (StatusCode, String)> {
        let now = Instant::now();
        let mut guard = self.sessions.lock().await;
        let quota = guard.entry(session_key.to_string()).or_default();
        quota.last_touch = now;

        while let Some(ts) = quota.recent.front() {
            if now.duration_since(*ts) > limits.rate_window {
                let _ = quota.recent.pop_front();
            } else {
                break;
            }
        }

        if quota.inflight >= limits.max_inflight_per_session {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "too many concurrent requests for session".into(),
            ));
        }
        if quota.recent.len() >= limits.max_requests_per_window {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded for session".into(),
            ));
        }

        quota.inflight += 1;
        quota.recent.push_back(now);
        Ok(())
    }

    async fn end_with_limits(&self, session_key: &str, limits: &WebLimits) {
        let now = Instant::now();
        let mut guard = self.sessions.lock().await;
        if let Some(quota) = guard.get_mut(session_key) {
            while let Some(ts) = quota.recent.front() {
                if now.duration_since(*ts) > limits.rate_window {
                    let _ = quota.recent.pop_front();
                } else {
                    break;
                }
            }
            quota.inflight = quota.inflight.saturating_sub(1);
            quota.last_touch = now;
            if quota.inflight == 0 && quota.recent.is_empty() {
                guard.remove(session_key);
            }
        }
        guard.retain(|_, quota| {
            !(quota.inflight == 0 && now.duration_since(quota.last_touch) > limits.session_idle_ttl)
        });
    }
}

fn auth_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn require_auth(
    headers: &HeaderMap,
    expected_token: Option<&str>,
) -> Result<(), (StatusCode, String)> {
    let Some(expected) = expected_token else {
        return Ok(());
    };

    let provided = auth_token_from_headers(headers).unwrap_or_default();

    if provided == expected {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "unauthorized".into()))
    }
}

#[derive(Debug, Serialize)]
struct HistoryItem {
    artifacts: Vec<crate::artifacts::Artifact>,
    id: String,
    sender_name: String,
    content: String,
    is_from_bot: bool,
    timestamp: String,
    origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    limit: Option<usize>,
    day: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    /// When set, return a window of messages around this id (ignores day/limit paging).
    #[serde(default)]
    around_id: Option<String>,
    /// When "1" / "true", omit scheduler messages already stored in the transcript.
    #[serde(default)]
    hide_scheduled: Option<String>,
    /// Older messages to include with around_id (default 15).
    #[serde(default)]
    around_before: Option<usize>,
    /// Newer messages to include with around_id (default 15).
    #[serde(default)]
    around_after: Option<usize>,
    /// Cursor: return messages strictly older than this id (ignores day).
    #[serde(default)]
    before_id: Option<String>,
    /// Cursor: return messages strictly newer than this id (ignores day).
    #[serde(default)]
    after_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    sender_name: Option<String>,
    message: String,
    #[serde(default)]
    session_id: Option<String>,
    /// When set, another persona answers this turn inside the host chat.
    /// Kept for older clients. `guest_persona_ids` is the list the web UI sends.
    #[serde(default)]
    guest_persona_id: Option<i64>,
    /// Tagged personas, in the order they should answer. The host does not answer.
    #[serde(default)]
    guest_persona_ids: Vec<i64>,
    #[serde(default)]
    attachments: Vec<SendAttachmentRequest>,
}

#[derive(Debug, Deserialize)]
struct PatchPersonaMessageBody {
    content: String,
}

#[derive(Debug, Deserialize)]
struct SubthreadHistoryTurn {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct SubthreadStreamRequest {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    anchor_message_id: String,
    message: String,
    #[serde(default)]
    session_id: Option<String>,
    /// Prior ephemeral turns in this side chat (not including the current message).
    #[serde(default)]
    history: Vec<SubthreadHistoryTurn>,
    /// When set, persist the completed user+assistant turns onto this side chat.
    #[serde(default)]
    side_chat_id: Option<String>,
}

#[derive(Deserialize)]
struct PersonaSideChatPathParams {
    persona_id: i64,
}

#[derive(Deserialize)]
struct PersonaSideChatIdPathParams {
    persona_id: i64,
    side_chat_id: String,
}

#[derive(Debug, Deserialize)]
struct SideChatEnsureBody {
    anchor_message_id: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    anchor_snippet: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SideChatPatchBody {
    #[serde(default)]
    draft_text: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopicsListQuery {
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopicIdPathParams {
    persona_id: i64,
    topic_id: String,
}

#[derive(Debug, Deserialize)]
struct TopicDossierQuery {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_id: Option<String>,
    #[serde(default)]
    after_id: Option<String>,
    #[serde(default)]
    around_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopicPatchBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TopicSplitBody {
    message_id: String,
}

#[derive(Debug, Deserialize)]
struct TopicSectionBody {
    title: String,
}

#[derive(Debug, Deserialize)]
struct SideChatPromoteTurnBody {
    turn_id: String,
}

#[derive(Debug, Deserialize)]
struct SendAttachmentRequest {
    filename: Option<String>,
    media_type: Option<String>,
    /// Inline base64 payload (legacy). Prefer `tool_path` + `url` from `POST /api/uploads`.
    #[serde(default)]
    data_base64: Option<String>,
    /// Relative path under `shared/upload/` (e.g. `upload/web/{chat_id}/{persona_id}/{file}`).
    #[serde(default)]
    tool_path: Option<String>,
    /// Accepted for old clients; server derives the canonical URL from the verified file.
    #[serde(default, rename = "url")]
    _url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadQueryParams {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct UploadResponse {
    filename: String,
    media_type: String,
    bytes: u64,
    tool_path: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    run_id: String,
    last_event_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ResetRequest {
    chat_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PersonasQuery {
    chat_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PersonasSwitchRequest {
    chat_id: Option<i64>,
    persona_name: String,
}

#[derive(Debug, Deserialize)]
struct PersonaCreateRequest {
    chat_id: Option<i64>,
    name: String,
    agent_engine: String,
    #[serde(default)]
    llm_provider: Option<String>,
    #[serde(default)]
    llm_model: Option<String>,
    #[serde(default)]
    llm_base_url: Option<String>,
    #[serde(default)]
    local_base_url: Option<String>,
    #[serde(default)]
    local_model: Option<String>,
    #[serde(default)]
    cursor_sdk_model: Option<String>,
    #[serde(default)]
    gemini_default_model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PersonaDeleteRequest {
    chat_id: Option<i64>,
    persona_id: i64,
}

#[derive(Debug, Deserialize)]
struct PersonaRenameRequest {
    chat_id: Option<i64>,
    persona_id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ContactsBindRequest {
    #[allow(dead_code)]
    chat_id: Option<i64>,
    /// Canonical chat_id of the contact to bind web to (e.g. from Telegram).
    contact_chat_id: i64,
}

#[derive(Debug, Deserialize)]
struct ContactsUnlinkRequest {
    #[allow(dead_code)]
    chat_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SchedulesQuery {
    chat_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TodosQuery {
    chat_id: Option<i64>,
    /// open (default) | done | all
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TodoPatchRequest {
    /// todo | plan | build | verify, plus legacy names normalized by the server
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    owner_persona_id: Option<i64>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    blocked_reason: Option<String>,
    #[serde(default)]
    position: Option<f64>,
    #[serde(default)]
    before_position: Option<f64>,
    #[serde(default)]
    after_position: Option<f64>,
    #[serde(default)]
    auto_advance: Option<bool>,
    /// When true, sets completed_at without moving the column.
    #[serde(default)]
    completed: Option<bool>,
    #[serde(default)]
    deploy_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct TodoRunRequest {
    #[serde(default)]
    deploy_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TodoPathParams {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TodoCreateRequest {
    chat_id: Option<i64>,
    title: String,
    persona_id: Option<i64>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScheduleCreateRequest {
    chat_id: Option<i64>,
    prompt: String,
    schedule_type: String, // "cron" | "once"
    schedule_value: String,
    timezone: Option<String>,
    persona_id: Option<i64>,
    #[serde(default)]
    output_delivery: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteSessionRequest {
    chat_id: Option<i64>,
    #[allow(dead_code)]
    persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ScheduleUpdateRequest {
    status: Option<String>, // "paused" | "active" | "cancelled"
    persona_id: Option<i64>,
    prompt: Option<String>,
    /// When changing timing, send both `schedule_type` and `schedule_value` (same semantics as create).
    schedule_type: Option<String>,
    schedule_value: Option<String>,
    timezone: Option<String>,
    #[serde(default)]
    output_delivery: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RunStatusQuery {
    run_id: String,
    /// When true, include `timeline_events` count in the response.
    /// Defaults to `false` to keep polling cheap.
    timeline_events: Option<bool>,
}

use crate::artifact_http::ContentOptions as UploadQuery;

#[cfg(test)]
fn artifact_kind_from_filename(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.ends_with(".bmp")
        || lower.ends_with(".svg")
    {
        "image".to_string()
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "markdown".to_string()
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        "html".to_string()
    } else if lower.ends_with(".txt")
        || lower.ends_with(".json")
        || lower.ends_with(".log")
        || lower.ends_with(".csv")
        || lower.ends_with(".yml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".xml")
    {
        "text".to_string()
    } else {
        "other".to_string()
    }
}

#[cfg(test)]
fn extract_upload_urls_from_text(text: &str) -> Vec<String> {
    let Some(re) = regex::Regex::new(r#"/api/(?:uploads|artifacts)/[^\s\)\]\(<>"']+"#).ok() else {
        return Vec::new();
    };
    re.find_iter(text).map(|m| m.as_str().to_string()).collect()
}

fn system_time_to_rfc3339(value: std::time::SystemTime) -> Option<String> {
    let datetime: chrono::DateTime<chrono::Utc> = value.into();
    Some(datetime.to_rfc3339())
}

async fn index() -> impl IntoResponse {
    match WEB_ASSETS.get_file("index.html") {
        Some(file) => Html(String::from_utf8_lossy(file.contents()).to_string()).into_response(),
        None => (StatusCode::NOT_FOUND, "index.html missing").into_response(),
    }
}

async fn api_health(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    Ok(Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "web_enabled": state.app_state.config.web_enabled,
    })))
}

#[derive(Debug, Deserialize)]
struct SecretsForgetQuery {
    name: String,
    #[serde(default)]
    skill: bool,
}

#[derive(Debug, Deserialize)]
struct SecretsListQuery {
    persona_id: Option<i64>,
}

async fn api_secrets_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<SecretsListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let data_dir = state.app_state.config.runtime_data_dir();
    let filter_persona = query.persona_id.filter(|id| *id > 0);
    let items: Vec<serde_json::Value> = crate::secret_vault::list_meta(&data_dir, chat_id)
        .into_iter()
        .filter(|row| match filter_persona {
            Some(persona_id) => row.persona_id == Some(persona_id) || row.persona_id.is_none(),
            None => true,
        })
        .map(|row| {
            let legacy = row.persona_id.is_none();
            let mut item = json!({
                "name": row.name,
                "kind": row.kind,
                "skill": row.skill,
                "created_at": row.created_at,
                "scrubbed": row.scrubbed,
                "persona_id": row.persona_id,
            });
            if filter_persona.is_some() {
                item["legacy"] = json!(legacy);
            }
            item
        })
        .collect();
    Ok(Json(json!({ "secrets": items })))
}

async fn api_secrets_forget(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<SecretsForgetQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let name = query.name.trim().to_ascii_uppercase();
    crate::secret_vault::forget(
        &state.app_state.config.runtime_data_dir(),
        &state.app_state.config.skills_data_dir_absolute(),
        chat_id,
        &name,
        query.skill,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "ok": true, "name": name })))
}

/// Single universal chat; no multi-chat concept. Web always uses this chat.
pub(crate) fn resolve_chat_id_for_web(
    _chat_id: Option<i64>,
    config: &Config,
) -> Result<i64, (StatusCode, String)> {
    Ok(config.operator_inbox_chat_id())
}

/// Ensure web/default always points to the configured universal chat.
/// This allows UNIVERSAL_CHAT_ID changes to take effect on restart.
pub(crate) async fn ensure_web_binding_for_universal(
    state: &WebState,
    chat_id: i64,
) -> Result<(), (StatusCode, String)> {
    // Web always resolves to the same universal chat id; cache to avoid repeated writes.
    let cid = chat_id;
    {
        let guard = state.web_binding_universal_done.lock().await;
        if guard.is_some_and(|id| id == cid) {
            return Ok(());
        }
    }
    call_blocking(state.app_state.db.clone(), move |db| {
        db.upsert_chat(cid, None, "web")?;
        db.link_channel(cid, BOT_INSTANCE_WEB, "web", "default")?;
        Ok(())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    {
        let mut guard = state.web_binding_universal_done.lock().await;
        *guard = Some(cid);
    }
    Ok(())
}

async fn api_chat(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let cid = chat_id;
    let persona_id = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_current_persona_id(cid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": persona_id,
    })))
}

async fn resolve_history_persona_id(
    state: &WebState,
    chat_id: i64,
    persona_id: Option<i64>,
) -> Result<i64, (StatusCode, String)> {
    let pid = match persona_id {
        Some(id) if id > 0 => id,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "persona_id query parameter is required".into(),
            ));
        }
    };
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Persona {pid} does not exist for this chat"),
        ));
    }
    Ok(pid)
}

async fn api_history(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let start = Instant::now();
    let requested_day = query.day.clone();
    let requested_limit = query.limit;
    let hide_scheduled = matches!(
        query
            .hide_scheduled
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    );

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let persona_id = resolve_history_persona_id(&state, chat_id, query.persona_id).await?;
    let cid2 = chat_id;
    let pid = persona_id;
    let page_limit = requested_limit.unwrap_or(10).clamp(1, 100);

    let (messages, has_older, has_newer) = if let Some(ref around_id) = query.around_id {
        let around_id = around_id.trim().to_string();
        if around_id.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "around_id is required".into()));
        }
        let before = query.around_before.unwrap_or(15).clamp(0, 100);
        let after = query.around_after.unwrap_or(15).clamp(0, 100);
        let session_id = query.session_id.clone();
        let page = call_blocking(state.app_state.db.clone(), move |db| {
            db.get_messages_around_id(cid2, pid, &around_id, before, after, session_id.as_deref())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "message not found".into()))?;
        (page.messages, page.has_older, page.has_newer)
    } else if let Some(ref before_id) = query.before_id {
        let before_id = before_id.trim().to_string();
        if before_id.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "before_id is required".into()));
        }
        let session_id = query.session_id.clone();
        let (msgs, older) = call_blocking(state.app_state.db.clone(), move |db| {
            db.get_messages_before_id(cid2, pid, &before_id, page_limit, session_id.as_deref())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "message not found".into()))?;
        // A before_id page is older than the current window, so newer messages exist.
        (msgs, older, true)
    } else if let Some(ref after_id) = query.after_id {
        let after_id = after_id.trim().to_string();
        if after_id.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "after_id is required".into()));
        }
        let session_id = query.session_id.clone();
        let (msgs, newer) = call_blocking(state.app_state.db.clone(), move |db| {
            db.get_messages_after_id(cid2, pid, &after_id, page_limit, session_id.as_deref())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "message not found".into()))?;
        // An after_id page is newer than the current window, so older messages exist.
        (msgs, true, newer)
    } else if let Some(ref sid) = query.session_id {
        let session_id = sid.clone();
        if requested_day.is_some() {
            // Day filter is main-chat only; session path ignores day.
        }
        let (msgs, older) = call_blocking(state.app_state.db.clone(), move |db| {
            db.get_recent_messages_for_session(&session_id, page_limit)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (msgs, older, false)
    } else if let Some(ref day) = requested_day {
        let (from_date, to_date) = day_range(day);
        let msgs = call_blocking(state.app_state.db.clone(), move |db| {
            db.get_messages_for_date_range(
                cid2,
                pid,
                Some(from_date.as_str()),
                Some(to_date.as_str()),
                2000,
            )
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (msgs, false, false)
    } else {
        // Live window: latest N. When no limit is given, return the full timeline (legacy).
        match requested_limit {
            Some(_) => {
                let fetch = page_limit + 1;
                let mut msgs = call_blocking(state.app_state.db.clone(), move |db| {
                    db.get_recent_messages(cid2, pid, fetch, hide_scheduled)
                })
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let older = msgs.len() > page_limit;
                if older {
                    let skip = msgs.len() - page_limit;
                    msgs = msgs.into_iter().skip(skip).collect();
                }
                (msgs, older, false)
            }
            None => {
                let msgs = call_blocking(state.app_state.db.clone(), move |db| {
                    db.get_all_messages(cid2, pid)
                })
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                (msgs, false, false)
            }
        }
    };
    let mut messages = messages;
    if hide_scheduled {
        messages.retain(|m| m.origin != crate::db::MESSAGE_ORIGIN_SCHEDULED);
    }

    let root = state.app_state.config.workspace_root_absolute();
    let items: Vec<HistoryItem> = tokio::task::spawn_blocking(move || {
        let catalog = crate::artifacts::Catalog::open(&root).ok();
        messages
            .into_iter()
            .map(|m| HistoryItem {
                artifacts: catalog
                    .as_ref()
                    .and_then(|c| c.message_artifacts(m.chat_id, m.persona_id, &m.id).ok())
                    .unwrap_or_default(),
                id: m.id,
                sender_name: m.sender_name,
                content: crate::agent_turn_context::strip_stored_dialogue_markup(&m.content),
                is_from_bot: m.is_from_bot,
                timestamp: m.timestamp,
                origin: m.origin,
                speaker_persona_id: m.speaker_persona_id,
            })
            .collect()
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(
        target: "web",
        endpoint = "/api/history",
        chat_id = chat_id,
        persona_id = persona_id,
        day = ?requested_day,
        limit = ?requested_limit,
        returned_messages = items.len(),
        has_older = has_older,
        has_newer = has_newer,
        duration_ms = start.elapsed().as_millis(),
        "History fetched"
    );

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": persona_id,
        "messages": items,
        "has_older": has_older,
        "has_newer": has_newer,
    })))
}

/// Return (from_date, to_date) as ISO strings for a given day (YYYY-MM-DD).
fn day_range(day: &str) -> (String, String) {
    if let Ok(d) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
        let start: chrono::DateTime<chrono::Utc> = chrono::DateTime::from_naive_utc_and_offset(
            d.and_hms_opt(0, 0, 0).unwrap(),
            chrono::Utc,
        );
        let end: chrono::DateTime<chrono::Utc> = chrono::DateTime::from_naive_utc_and_offset(
            d.and_hms_opt(23, 59, 59).unwrap(),
            chrono::Utc,
        );
        return (start.to_rfc3339(), end.to_rfc3339());
    }
    ("".into(), "".into())
}

async fn api_history_days(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let persona_id = resolve_history_persona_id(&state, chat_id, query.persona_id).await?;
    let cid2 = chat_id;
    let pid = persona_id;
    let days = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_message_days(cid2, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": persona_id,
        "days": days,
    })))
}

async fn api_send(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<SendRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    enqueue_persona_turn(state, body, None).await.map(Json)
}

#[derive(Clone, Copy)]
struct BoardStageFollowUp {
    item_id: i64,
    chain_remaining: u8,
}

async fn enqueue_persona_turn(
    state: WebState,
    body: SendRequest,
    follow_up: Option<BoardStageFollowUp>,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let start = Instant::now();
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let key = format!("chat:{}", chat_id);
    if let Err((status, msg)) = state.request_hub.begin(&key, &state.limits).await {
        info!(
            target: "web",
            endpoint = "/api/send",
            chat_id = chat_id,
            status = status.as_u16(),
            reason = %msg,
            "Request rejected by limiter"
        );
        return Err((status, msg));
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    state.run_hub.create(&run_id).await;
    let cid = chat_id;
    let persona_id_queue = if let Some(pid) = body.persona_id {
        pid
    } else {
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let queue_label = body.message.chars().take(120).collect::<String>();
    let abort_hook = {
        let db = state.app_state.db.clone();
        let bot_username = state.app_state.config.bot_username.clone();
        let run_hub = state.run_hub.clone();
        let run_id_abort = run_id.clone();
        let limits = state.limits.clone();
        let base = crate::queue_abort::make_store_only_hard_abort_hook(
            db,
            bot_username,
            chat_id,
            persona_id_queue,
            run_id.clone(),
        );
        std::sync::Arc::new(move |reason: String| {
            let run_hub = run_hub.clone();
            let run_id_abort = run_id_abort.clone();
            let limits = limits.clone();
            let base = base.clone();
            let reason_for_hub = reason.clone();
            Box::pin(async move {
                base(reason).await;
                run_hub
                    .publish(
                        &run_id_abort,
                        "error",
                        serde_json::json!({ "error": reason_for_hub }).to_string(),
                        limits.run_history_limit,
                    )
                    .await;
                run_hub.remove_later(run_id_abort, 300).await;
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    let queue_meta = QueueEnqueueMeta {
        run_id: run_id.clone(),
        persona_id: persona_id_queue,
        source: QueueSource::Web,
        label: queue_label,
        project_id: None,
        workflow_id: None,
        on_hard_abort: Some(abort_hook),
    };
    if let Some(follow) = follow_up {
        let db = state.app_state.db.clone();
        let run_for_mark = run_id.clone();
        let item_id = follow.item_id;
        call_blocking(db, move |db| {
            db.mark_board_stage_running(item_id, &run_for_mark)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let state_for_task = state.clone();
    let run_id_for_task = run_id.clone();
    let limits = state.limits.clone();
    let stage_follow_up = follow_up;
    let (queue_position, _) = state
        .app_state
        .chat_queue
        .enqueue_with_meta(chat_id, queue_meta, |cancel| async move {
            state_for_task
                .run_hub
                .publish(
                    &run_id_for_task,
                    "status",
                    json!({"message": "running"}).to_string(),
                    limits.run_history_limit,
                )
                .await;
            let turn_error = match send_and_store_response_with_events(
                state_for_task.clone(),
                body,
                None,
                Some(&run_id_for_task),
                Some(cancel),
            )
            .await
            {
                Ok(resp) => {
                    let v = resp.0;
                    if let Some(jid) = v.get("background_job_id").and_then(|x| x.as_str()) {
                        if !jid.is_empty() {
                            state_for_task
                                .run_hub
                                .publish(
                                    &run_id_for_task,
                                    "background_job",
                                    json!({
                                        "job_id": jid,
                                        "message": "Task queued in background."
                                    })
                                    .to_string(),
                                    limits.run_history_limit,
                                )
                                .await;
                        }
                    }
                    let response_text = v
                        .get("response")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut done_payload = json!({ "response": response_text });
                    if let Some(jid) = v.get("background_job_id").and_then(|x| x.as_str()) {
                        if !jid.is_empty() {
                            done_payload["background_job_id"] = json!(jid);
                        }
                    }
                    state_for_task
                        .run_hub
                        .publish(
                            &run_id_for_task,
                            "done",
                            done_payload.to_string(),
                            limits.run_history_limit,
                        )
                        .await;
                    None
                }
                Err((_, err_msg)) => {
                    state_for_task
                        .run_hub
                        .publish(
                            &run_id_for_task,
                            "error",
                            json!({"error": err_msg}).to_string(),
                            limits.run_history_limit,
                        )
                        .await;
                    Some(err_msg)
                }
            };
            if let Some(follow) = stage_follow_up {
                settle_board_stage(state_for_task.clone(), follow, turn_error).await;
            }
            state_for_task
                .run_hub
                .remove_later(run_id_for_task, 300)
                .await;
        })
        .await;
    if queue_position == 0 {
        if let Some(follow) = follow_up {
            let db = state.app_state.db.clone();
            let item_id = follow.item_id;
            let _ = call_blocking(db, move |db| db.clear_board_stage_run(item_id)).await;
        }
        state.request_hub.end_with_limits(&key, &state.limits).await;
        return Err((
            StatusCode::BAD_REQUEST,
            "persona_id must be positive".into(),
        ));
    }
    state
        .run_hub
        .publish(
            &run_id,
            "status",
            json!({
                "message": if queue_position > 1 {
                    format!("queued ({} ahead)", queue_position.saturating_sub(1))
                } else {
                    "queued".to_string()
                }
            })
            .to_string(),
            state.limits.run_history_limit,
        )
        .await;
    state.request_hub.end_with_limits(&key, &state.limits).await;
    info!(
        target: "web",
        endpoint = "/api/send",
        chat_id = chat_id,
        run_id = %run_id,
        queue_position = queue_position,
        latency_ms = start.elapsed().as_millis(),
        "Accepted queued request"
    );
    Ok(json!({
        "ok": true,
        "chat_id": chat_id,
        "run_id": run_id,
        "state": "queued",
        "queue_position": queue_position,
    }))
}

async fn settle_board_stage(
    state: WebState,
    follow: BoardStageFollowUp,
    turn_error: Option<String>,
) {
    let item_id = follow.item_id;
    if let Some(err) = turn_error {
        let db = state.app_state.db.clone();
        let _ = call_blocking(db, move |db| db.mark_board_stage_failed(item_id, &err)).await;
        return;
    }
    let loaded = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_todo(item_id)
    })
    .await;
    let Ok(Some(todo)) = loaded else {
        return;
    };
    let outcome = todo.stage_outcome.clone().unwrap_or_default();
    if outcome.is_empty() || outcome == "running" {
        let db = state.app_state.db.clone();
        let _ = call_blocking(db, move |db| {
            db.mark_board_stage_failed(item_id, "Stage ended without finish_pipeline_stage")
        })
        .await;
        return;
    }
    if outcome == "passed" && todo.auto_advance != 0 && follow.chain_remaining > 0 {
        if let Some(next) = crate::board_pipeline::next_stage(&todo.status) {
            if next != todo.status {
                let chat_id = todo.chat_id;
                let mut advanced = todo.clone();
                advanced.status = next.to_string();
                advanced.blocked_reason = None;
                advanced.stage_outcome = None;
                advanced.stage_run_id = None;
                let patch = crate::db_collab::BoardPatch {
                    status: Some(next.to_string()),
                    title: None,
                    description: None,
                    owner_persona_id: None,
                    priority: None,
                    due_at: None,
                    blocked_reason: Some(String::new()),
                    position: None,
                    before_position: None,
                    after_position: None,
                    actor_kind: "pipeline",
                    actor_persona_id: todo.owner_persona_id,
                    auto_advance: None,
                    completed: None,
                    deploy_prompt: None,
                };
                let moved = call_blocking(state.app_state.db.clone(), move |db| {
                    db.apply_board_patch(chat_id, item_id, &patch)
                })
                .await;
                if moved.ok().flatten().is_none() {
                    return;
                }
                let remaining = follow.chain_remaining.saturating_sub(1);
                if let Err((_, message)) =
                    start_board_stage(state.clone(), &advanced, remaining).await
                {
                    let db = state.app_state.db.clone();
                    let _ =
                        call_blocking(db, move |db| db.mark_board_stage_failed(item_id, &message))
                            .await;
                }
                return;
            }
        }
    }
    let db = state.app_state.db.clone();
    let _ = call_blocking(db, move |db| db.clear_board_stage_run(item_id)).await;
}

type BoardStageStart = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<serde_json::Value, (StatusCode, String)>> + Send>,
>;

fn start_board_stage(
    state: WebState,
    todo: &crate::db::PersonaTodo,
    chain_remaining: u8,
) -> BoardStageStart {
    let todo = todo.clone();
    Box::pin(async move { start_board_stage_inner(state, &todo, chain_remaining).await })
}

async fn start_board_stage_inner(
    state: WebState,
    todo: &crate::db::PersonaTodo,
    chain_remaining: u8,
) -> Result<serde_json::Value, (StatusCode, String)> {
    if !crate::board_pipeline::is_runnable_stage(&todo.status) {
        return Err((
            StatusCode::BAD_REQUEST,
            "This column does not run a stage".into(),
        ));
    }
    let owner = todo.owner_persona_id.filter(|id| *id > 0).ok_or((
        StatusCode::BAD_REQUEST,
        "Assign an owner before running this stage".into(),
    ))?;
    if todo.stage_outcome.as_deref() == Some("running") {
        return Err((StatusCode::CONFLICT, "This stage is already running".into()));
    }
    let chat_id = todo.chat_id;
    let sessions = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_chat_sessions(chat_id, owner)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let session_id = sessions.into_iter().next().map(|session| session.id);
    let body = SendRequest {
        chat_id: Some(chat_id),
        persona_id: Some(owner),
        sender_name: Some("Board".to_string()),
        message: crate::board_pipeline::build_stage_prompt(todo),
        session_id,
        guest_persona_id: None,
        guest_persona_ids: Vec::new(),
        attachments: Vec::new(),
    };
    enqueue_persona_turn(
        state,
        body,
        Some(BoardStageFollowUp {
            item_id: todo.id,
            chain_remaining,
        }),
    )
    .await
}

async fn api_send_stream(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<SendRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let start = Instant::now();

    let text = body.message.trim().to_string();
    if text.is_empty() && body.attachments.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message is required".into()));
    }

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let key = format!("chat:{}", chat_id);
    if let Err((status, msg)) = state.request_hub.begin(&key, &state.limits).await {
        info!(
            target: "web",
            endpoint = "/api/send_stream",
            chat_id = chat_id,
            status = status.as_u16(),
            reason = %msg,
            "Request rejected by limiter"
        );
        return Err((status, msg));
    }

    let run_id = uuid::Uuid::new_v4().to_string();
    state.run_hub.create(&run_id).await;
    let cid = chat_id;
    let persona_id_queue = if let Some(pid) = body.persona_id {
        pid
    } else {
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let guest_ids =
        crate::collab::guest::ordered_guest_ids(body.guest_persona_id, &body.guest_persona_ids);
    if !guest_ids.is_empty() {
        let db = state.app_state.db.clone();
        let host_id = persona_id_queue;
        let chat_for_guests = chat_id;
        let checked = call_blocking(db, move |db| {
            Ok(crate::collab::guest::validate_guest_ids(
                db,
                chat_for_guests,
                host_id,
                &guest_ids,
            ))
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Err(msg) = checked {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            state.run_hub.remove_later(run_id, 0).await;
            let status = if msg.starts_with("Database error") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_REQUEST
            };
            return Err((status, msg));
        }
    }
    let queue_label = text.chars().take(120).collect::<String>();
    let abort_hook = {
        let db = state.app_state.db.clone();
        let bot_username = state.app_state.config.bot_username.clone();
        let run_hub = state.run_hub.clone();
        let run_id_abort = run_id.clone();
        let limits = state.limits.clone();
        let base = crate::queue_abort::make_store_only_hard_abort_hook(
            db,
            bot_username,
            chat_id,
            persona_id_queue,
            run_id.clone(),
        );
        std::sync::Arc::new(move |reason: String| {
            let run_hub = run_hub.clone();
            let run_id_abort = run_id_abort.clone();
            let limits = limits.clone();
            let base = base.clone();
            let reason_for_hub = reason.clone();
            Box::pin(async move {
                base(reason).await;
                run_hub
                    .publish(
                        &run_id_abort,
                        "error",
                        serde_json::json!({ "error": reason_for_hub }).to_string(),
                        limits.run_history_limit,
                    )
                    .await;
                run_hub.remove_later(run_id_abort, 300).await;
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    let queue_meta = QueueEnqueueMeta {
        run_id: run_id.clone(),
        persona_id: persona_id_queue,
        source: QueueSource::Web,
        label: queue_label,
        project_id: None,
        workflow_id: None,
        on_hard_abort: Some(abort_hook),
    };
    let state_for_task = state.clone();
    let run_id_for_task = run_id.clone();
    let limits = state.limits.clone();
    let (queue_position, _) = state
        .app_state
        .chat_queue
        .enqueue_with_meta(chat_id, queue_meta, |cancel| async move {
            let run_start = Instant::now();
            state_for_task
                .run_hub
                .publish(
                    &run_id_for_task,
                    "status",
                    json!({"message": "running"}).to_string(),
                    limits.run_history_limit,
                )
                .await;

            let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            let run_hub = state_for_task.run_hub.clone();
            let run_id_for_events = run_id_for_task.clone();
            let run_history_limit = limits.run_history_limit;
            let forward = tokio::spawn(async move {
                while let Some(evt) = evt_rx.recv().await {
                    match evt {
                        AgentEvent::Iteration { iteration } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "status",
                                    json!({"message": format!("iteration {iteration}")})
                                        .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::ToolStart {
                            tool_use_id,
                            name,
                            input,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "tool_start",
                                    json!({
                                        "tool_use_id": tool_use_id,
                                        "name": name,
                                        "input": input
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::ToolResult {
                            tool_use_id,
                            name,
                            is_error,
                            output,
                            duration_ms,
                            status_code,
                            bytes,
                            error_type,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "tool_result",
                                    json!({
                                        "tool_use_id": tool_use_id,
                                        "name": name,
                                        "is_error": is_error,
                                        "output": output,
                                        "duration_ms": duration_ms,
                                        "status_code": status_code,
                                        "bytes": bytes,
                                        "error_type": error_type
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::TextDelta { delta } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "delta",
                                    json!({"delta": delta}).to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::Hook {
                            event_name,
                            tool_name,
                            matched_hook_ids,
                            blocked_reason,
                            additional_context_count,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "hook",
                                    json!({
                                        "event_name": event_name,
                                        "tool_name": tool_name,
                                        "matched_hook_ids": matched_hook_ids,
                                        "blocked_reason": blocked_reason,
                                        "additional_context_count": additional_context_count,
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::UserMessageRewritten {
                            message_id,
                            original,
                            rewritten,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "message_rewritten",
                                    json!({
                                        "message_id": message_id,
                                        "original": original,
                                        "rewritten": rewritten,
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::FinalResponse { text } => {
                            if !text.is_empty() {
                                run_hub
                                    .publish(
                                        &run_id_for_events,
                                        "delta",
                                        json!({"delta": text}).to_string(),
                                        run_history_limit,
                                    )
                                    .await;
                            }
                        }
                    }
                }
            });

            let send_result = send_and_store_response_with_events(
                state_for_task.clone(),
                body,
                Some(&evt_tx),
                Some(&run_id_for_task),
                Some(cancel),
            )
            .await;
            // Tool events sit on this channel until `forward` drains them.
            // Publish `done` only after that, or the stream closes first and drops them.
            drop(evt_tx);
            let _ = forward.await;
            match send_result {
                Ok(resp) => {
                    let v = resp.0;
                    if let Some(err) = v.get("guest_error") {
                        let name = err
                            .get("persona_name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("A tagged persona");
                        let reason = err
                            .get("error")
                            .and_then(|value| value.as_str())
                            .unwrap_or("the guest turn failed");
                        let reason = reason.trim().trim_end_matches('.');
                        state_for_task
                            .run_hub
                            .publish(
                                &run_id_for_task,
                                "error",
                                json!({
                                    "error": format!("{name} could not answer: {reason}. Nothing else ran."),
                                    "kind": "guest",
                                    "persona_id": err.get("persona_id").cloned().unwrap_or(serde_json::Value::Null),
                                    "persona_name": name,
                                })
                                .to_string(),
                                limits.run_history_limit,
                            )
                            .await;
                    } else {
                    if let Some(jid) = v.get("background_job_id").and_then(|x| x.as_str()) {
                        if !jid.is_empty() {
                            state_for_task
                                .run_hub
                                .publish(
                                    &run_id_for_task,
                                    "background_job",
                                    json!({
                                        "job_id": jid,
                                        "message": "Task queued in background."
                                    })
                                    .to_string(),
                                    limits.run_history_limit,
                                )
                                .await;
                        }
                    }
                    let response_text = v
                        .get("response")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut done_payload = json!({ "response": response_text });
                    if let Some(jid) = v.get("background_job_id").and_then(|x| x.as_str()) {
                        if !jid.is_empty() {
                            done_payload["background_job_id"] = json!(jid);
                        }
                    }
                    state_for_task
                        .run_hub
                        .publish(
                            &run_id_for_task,
                            "done",
                            done_payload.to_string(),
                            limits.run_history_limit,
                        )
                        .await;
                    }
                }
                Err((_, err_msg)) => {
                    state_for_task
                        .run_hub
                        .publish(
                            &run_id_for_task,
                            "error",
                            json!({"error": err_msg}).to_string(),
                            limits.run_history_limit,
                        )
                        .await;
                }
            }
            info!(
                target: "web",
                endpoint = "/api/send_stream",
                chat_id = chat_id,
                run_id = %run_id_for_task,
                latency_ms = run_start.elapsed().as_millis(),
                "Stream run finished"
            );

            state_for_task
                .run_hub
                .remove_later(run_id_for_task, 300)
                .await;
        })
        .await;

    state
        .run_hub
        .publish(
            &run_id,
            "status",
            json!({
                "message": if queue_position > 1 {
                    format!("queued ({} ahead)", queue_position.saturating_sub(1))
                } else {
                    "queued".to_string()
                }
            })
            .to_string(),
            limits.run_history_limit,
        )
        .await;
    state.request_hub.end_with_limits(&key, &state.limits).await;
    info!(
        target: "web",
        endpoint = "/api/send_stream",
        chat_id = chat_id,
        run_id = %run_id,
        queue_position = queue_position,
        latency_ms = start.elapsed().as_millis(),
        "Accepted stream run"
    );

    Ok(Json(json!({
        "ok": true,
        "run_id": run_id,
        "state": "queued",
        "queue_position": queue_position,
    })))
}

/// Side-chat stream: seeds context from the anchor bot message + cockpit window,
/// runs the shared agent without writing into main history, and optionally persists
/// turns onto a `side_chats` row when `side_chat_id` is provided.
async fn api_subthread_stream(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<SubthreadStreamRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let start = Instant::now();

    let text = body.message.trim().to_string();
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message is required".into()));
    }
    if text.chars().count() > 100_000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "message exceeds 100000 character limit".into(),
        ));
    }
    if body.history.len() > 40 {
        return Err((StatusCode::BAD_REQUEST, "history exceeds 40 turns".into()));
    }

    let anchor_message_id = body.anchor_message_id.trim().to_string();
    if anchor_message_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "anchor_message_id is required".into(),
        ));
    }

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let key = format!("chat:{}", chat_id);
    if let Err((status, msg)) = state.request_hub.begin(&key, &state.limits).await {
        return Err((status, msg));
    }

    let cid = chat_id;
    let persona_id = if let Some(pid) = body.persona_id {
        pid
    } else {
        match call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        {
            Ok(pid) => pid,
            Err(e) => {
                state.request_hub.end_with_limits(&key, &state.limits).await;
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
            }
        }
    };

    let pid_check = persona_id;
    let exists = match call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid_check)
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    };
    if !exists {
        state.request_hub.end_with_limits(&key, &state.limits).await;
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let pid_lookup = persona_id;
    let anchor = match call_blocking(state.app_state.db.clone(), {
        let anchor_message_id = anchor_message_id.clone();
        move |db| db.get_message_for_persona(chat_id, pid_lookup, &anchor_message_id)
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    };
    let Some(anchor) = anchor else {
        state.request_hub.end_with_limits(&key, &state.limits).await;
        return Err((StatusCode::NOT_FOUND, "anchor message not found".into()));
    };
    if !anchor.is_from_bot {
        state.request_hub.end_with_limits(&key, &state.limits).await;
        return Err((
            StatusCode::BAD_REQUEST,
            "anchor must be a bot response".into(),
        ));
    }

    let session_id = body
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Prefer the anchor's own session scope when present.
    let session_for_seed = session_id.clone().or_else(|| anchor.session_id.clone());

    let pid_hist = persona_id;
    let max_ts = anchor.timestamp.clone();
    let session_for_query = session_for_seed.clone();
    let prior = match call_blocking(state.app_state.db.clone(), move |db| {
        db.get_messages_up_to(chat_id, pid_hist, &max_ts, session_for_query.as_deref())
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    };

    let pid_persona = persona_id;
    let persona_row = match call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona(pid_persona)
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    };

    let min_user = persona_row
        .as_ref()
        .and_then(|p| p.recent_history_min_user)
        .map(|n| (n as usize).clamp(1, 25))
        .unwrap_or_else(|| {
            state
                .app_state
                .config
                .recent_history_min_user_messages
                .clamp(1, 25)
        });
    let min_asst = persona_row
        .as_ref()
        .and_then(|p| p.recent_history_min_assistant)
        .map(|n| (n as usize).clamp(1, 25))
        .unwrap_or_else(|| {
            state
                .app_state
                .config
                .recent_history_min_assistant_messages
                .clamp(1, 25)
        });

    let mut seeded = history_to_claude_messages(&prior, &state.app_state.config.bot_username, true);
    seeded = trim_to_recent_balanced(seeded, min_user, min_asst);

    for turn in &body.history {
        let role = turn.role.trim().to_ascii_lowercase();
        let content = turn.content.trim();
        if content.is_empty() {
            continue;
        }
        let role = match role.as_str() {
            "assistant" | "bot" => "assistant",
            _ => "user",
        };
        seeded.push(Message {
            role: role.into(),
            content: MessageContent::Text(content.to_string()),
        });
    }
    seeded.push(Message {
        role: "user".into(),
        content: MessageContent::Text(text.clone()),
    });

    let run_id = uuid::Uuid::new_v4().to_string();
    state.run_hub.create(&run_id).await;
    let queue_label = format!("subthread: {}", text.chars().take(100).collect::<String>());
    let abort_hook = {
        let run_hub = state.run_hub.clone();
        let run_id_abort = run_id.clone();
        let limits = state.limits.clone();
        std::sync::Arc::new(move |reason: String| {
            let run_hub = run_hub.clone();
            let run_id_abort = run_id_abort.clone();
            let limits = limits.clone();
            let reason_for_hub = reason.clone();
            Box::pin(async move {
                run_hub
                    .publish(
                        &run_id_abort,
                        "error",
                        serde_json::json!({ "error": reason_for_hub }).to_string(),
                        limits.run_history_limit,
                    )
                    .await;
                run_hub.remove_later(run_id_abort, 300).await;
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    let queue_meta = QueueEnqueueMeta {
        run_id: run_id.clone(),
        persona_id,
        source: QueueSource::Web,
        label: queue_label,
        project_id: None,
        workflow_id: None,
        on_hard_abort: Some(abort_hook),
    };

    let state_for_task = state.clone();
    let run_id_for_task = run_id.clone();
    let limits = state.limits.clone();
    let history_override = seeded;
    let session_id_for_run = session_for_seed;
    let side_chat_id_for_persist = body
        .side_chat_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let user_text_for_persist = text.clone();
    if let Some(ref sid) = side_chat_id_for_persist {
        let sid = sid.clone();
        let pid_sc = persona_id;
        let ok = match call_blocking(state.app_state.db.clone(), move |db| {
            db.get_side_chat(chat_id, pid_sc, &sid)
        })
        .await
        {
            Ok(v) => v.is_some(),
            Err(e) => {
                state.request_hub.end_with_limits(&key, &state.limits).await;
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
            }
        };
        if !ok {
            state.request_hub.end_with_limits(&key, &state.limits).await;
            return Err((StatusCode::NOT_FOUND, "side chat not found".into()));
        }
    }
    let (queue_position, _) = state
        .app_state
        .chat_queue
        .enqueue_with_meta(chat_id, queue_meta, |cancel| async move {
            let run_start = Instant::now();
            state_for_task
                .run_hub
                .publish(
                    &run_id_for_task,
                    "status",
                    json!({"message": "running"}).to_string(),
                    limits.run_history_limit,
                )
                .await;

            let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            let run_hub = state_for_task.run_hub.clone();
            let run_id_for_events = run_id_for_task.clone();
            let run_history_limit = limits.run_history_limit;
            let forward = tokio::spawn(async move {
                while let Some(evt) = evt_rx.recv().await {
                    match evt {
                        AgentEvent::Iteration { iteration } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "status",
                                    json!({"message": format!("iteration {iteration}")})
                                        .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::ToolStart {
                            tool_use_id,
                            name,
                            input,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "tool_start",
                                    json!({
                                        "tool_use_id": tool_use_id,
                                        "name": name,
                                        "input": input
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::ToolResult {
                            tool_use_id,
                            name,
                            is_error,
                            output,
                            duration_ms,
                            status_code,
                            bytes,
                            error_type,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "tool_result",
                                    json!({
                                        "tool_use_id": tool_use_id,
                                        "name": name,
                                        "is_error": is_error,
                                        "output": output,
                                        "duration_ms": duration_ms,
                                        "status_code": status_code,
                                        "bytes": bytes,
                                        "error_type": error_type
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::TextDelta { delta } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "delta",
                                    json!({"delta": delta}).to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::Hook {
                            event_name,
                            tool_name,
                            matched_hook_ids,
                            blocked_reason,
                            additional_context_count,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "hook",
                                    json!({
                                        "event_name": event_name,
                                        "tool_name": tool_name,
                                        "matched_hook_ids": matched_hook_ids,
                                        "blocked_reason": blocked_reason,
                                        "additional_context_count": additional_context_count,
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::UserMessageRewritten {
                            message_id,
                            original,
                            rewritten,
                        } => {
                            run_hub
                                .publish(
                                    &run_id_for_events,
                                    "message_rewritten",
                                    json!({
                                        "message_id": message_id,
                                        "original": original,
                                        "rewritten": rewritten,
                                    })
                                    .to_string(),
                                    run_history_limit,
                                )
                                .await;
                        }
                        AgentEvent::FinalResponse { text } => {
                            if !text.is_empty() {
                                run_hub
                                    .publish(
                                        &run_id_for_events,
                                        "delta",
                                        json!({"delta": text}).to_string(),
                                        run_history_limit,
                                    )
                                    .await;
                            }
                        }
                    }
                }
            });

            let agent_result = process_with_agent_with_events(
                &state_for_task.app_state,
                AgentRequestContext {
                    caller_channel: "web",
                    chat_id,
                    chat_type: "private",
                    persona_id,
                    is_scheduled_task: false,
                    is_background_job: false,
                    run_key: Some(run_id_for_task.clone()),
                    reply_bot_instance_id: None,
                    session_id: session_id_for_run,
                    history_override: Some(history_override),
                    tool_iteration_cap: None,
                    skip_recall: false,
                    inbound_user_message_id: None,
                },
                None,
                InboundImages::new(),
                Some(&evt_tx),
                Some(cancel),
            )
            .await;

            // Persist onto side_chats when requested; never write into main timeline.
            let response = match agent_result {
                Ok(agent_out) => ensure_visible_turn_text(&agent_out.response),
                Err(e) => {
                    warn!(
                        target: "web",
                        chat_id,
                        persona_id,
                        error = %e,
                        "subthread agent run failed"
                    );
                    failed_turn_notice(&e.to_string())
                }
            };

            if let Some(side_chat_id) = side_chat_id_for_persist.as_ref() {
                let side_chat_id = side_chat_id.clone();
                let user_text = user_text_for_persist.clone();
                let assistant_text = response.clone();
                let pid_persist = persona_id;
                if let Err(e) = call_blocking(state_for_task.app_state.db.clone(), move |db| {
                    db.append_side_chat_turns(
                        chat_id,
                        pid_persist,
                        &side_chat_id,
                        &user_text,
                        &assistant_text,
                    )
                })
                .await
                {
                    warn!(
                        target: "web",
                        chat_id,
                        persona_id,
                        error = %e,
                        "failed to persist side chat turns"
                    );
                }
            }

            drop(evt_tx);
            let _ = forward.await;
            state_for_task
                .run_hub
                .publish(
                    &run_id_for_task,
                    "done",
                    json!({
                        "response": response,
                        "persisted": side_chat_id_for_persist.is_some(),
                    })
                    .to_string(),
                    limits.run_history_limit,
                )
                .await;
            info!(
                target: "web",
                endpoint = "/api/subthread_stream",
                chat_id = chat_id,
                run_id = %run_id_for_task,
                latency_ms = run_start.elapsed().as_millis(),
                "Subthread stream run finished"
            );

            state_for_task
                .run_hub
                .remove_later(run_id_for_task, 300)
                .await;
        })
        .await;

    state
        .run_hub
        .publish(
            &run_id,
            "status",
            json!({
                "message": if queue_position > 1 {
                    format!("queued ({} ahead)", queue_position.saturating_sub(1))
                } else {
                    "queued".to_string()
                }
            })
            .to_string(),
            limits.run_history_limit,
        )
        .await;
    state.request_hub.end_with_limits(&key, &state.limits).await;
    info!(
        target: "web",
        endpoint = "/api/subthread_stream",
        chat_id = chat_id,
        run_id = %run_id,
        queue_position = queue_position,
        latency_ms = start.elapsed().as_millis(),
        "Accepted subthread stream run"
    );

    Ok(Json(json!({
        "ok": true,
        "run_id": run_id,
        "state": "queued",
        "queue_position": queue_position,
        "ephemeral": true,
        "context_window": {
            "min_user": min_user,
            "min_assistant": min_asst,
        },
    })))
}

async fn api_stream(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let start = Instant::now();

    let Some((mut rx, replay, done, replay_truncated, oldest_event_id)) = state
        .run_hub
        .subscribe_with_replay(&query.run_id, query.last_event_id)
        .await
    else {
        return Err((StatusCode::NOT_FOUND, "run not found".into()));
    };
    info!(
        target: "web",
        endpoint = "/api/stream",
        run_id = %query.run_id,
        last_event_id = ?query.last_event_id,
        replay_count = replay.len(),
        replay_truncated = replay_truncated,
        oldest_event_id = ?oldest_event_id,
        latency_ms = start.elapsed().as_millis(),
        "Stream subscription established"
    );

    let stream = async_stream::stream! {
        let meta = Event::default().event("replay_meta").data(
            json!({
                "replay_truncated": replay_truncated,
                "oldest_event_id": oldest_event_id,
                "requested_last_event_id": query.last_event_id,
            })
            .to_string()
        );
        yield Ok::<Event, std::convert::Infallible>(meta);

        let mut finished = false;
        for evt in replay {
            let is_done = evt.event == "done" || evt.event == "error";
            let event = Event::default()
                .id(evt.id.to_string())
                .event(evt.event)
                .data(evt.data);
            yield Ok::<Event, std::convert::Infallible>(event);
            if is_done {
                finished = true;
                break;
            }
        }

        if finished || done {
            return;
        }

        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let done = evt.event == "done" || evt.event == "error";
                    let event = Event::default()
                        .id(evt.id.to_string())
                        .event(evt.event)
                        .data(evt.data);
                    yield Ok::<Event, std::convert::Infallible>(event);
                    if done {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keepalive"),
    ))
}

/// Preview for ops poll / background job JSON. Byte-capped on a UTF-8 boundary.
fn background_job_result_preview(text: &str) -> &str {
    if text.len() > 200 {
        &text[..text.floor_char_boundary(200)]
    } else {
        text
    }
}

fn json_background_job(j: &crate::db::BackgroundJob) -> serde_json::Value {
    json!({
        "id": j.id,
        "chat_id": j.chat_id,
        "persona_id": j.persona_id,
        "prompt": j.prompt,
        "status": j.status,
        "trigger_reason": j.trigger_reason,
        "created_at": j.created_at,
        "started_at": j.started_at,
        "finished_at": j.finished_at,
        "result_preview": j.result_text.as_deref().map(background_job_result_preview),
        "error_text": j.error_text,
        "lease_owner": j.lease_owner,
        "lease_expires_at": j.lease_expires_at,
        "last_progress_at": j.last_progress_at,
        "last_stage": j.last_stage,
        "job_kind": j.job_kind,
        "shell_command": j.shell_command,
        "workdir": j.workdir,
        "tmux_session": j.tmux_session,
        "output_path": j.output_path,
        "exit_code": j.exit_code,
        "label": j.label,
        "session_id": j.session_id,
    })
}

fn json_job_heartbeat(h: &JobHeartbeat) -> serde_json::Value {
    json!({
        "run_key": h.run_key,
        "chat_id": h.chat_id,
        "persona_id": h.persona_id,
        "job_type": h.job_type,
        "stage": h.stage,
        "message": h.message,
        "active": h.active,
        "updated_at": h.updated_at,
    })
}

async fn api_run_status(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<RunStatusQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let run_key = query.run_id.clone();
    let include_timeline_events = query.timeline_events.unwrap_or(false);
    let timeline_count = if include_timeline_events {
        call_blocking(state.app_state.db.clone(), {
            let run_key = run_key.clone();
            move |db| Ok(db.get_run_timeline_events(&run_key, 500)?.len() as i64)
        })
        .await
        .unwrap_or(0)
    } else {
        0
    };

    let hb_opt = call_blocking(state.app_state.db.clone(), {
        let run_key = run_key.clone();
        move |db| db.get_job_heartbeat(&run_key)
    })
    .await
    .ok()
    .flatten();

    let bg_json = if hb_opt.as_ref().is_some_and(|h| {
        matches!(
            h.job_type.as_str(),
            "manual_background" | "shell_background"
        )
    }) {
        call_blocking(state.app_state.db.clone(), {
            let run_key = run_key.clone();
            move |db| db.get_background_job(&run_key)
        })
        .await
        .ok()
        .flatten()
        .map(|j| json_background_job(&j))
    } else {
        None
    };

    if let Some((done, last_event_id)) = state.run_hub.status(&query.run_id).await {
        return Ok(Json(json!({
            "ok": true,
            "run_id": query.run_id,
            "done": done,
            "last_event_id": last_event_id,
            "timeline_events": timeline_count,
            "heartbeat": hb_opt.as_ref().map(json_job_heartbeat),
            "background_job": bg_json,
            "source": "run_hub",
        })));
    }

    if let Some(ref hb) = hb_opt {
        let done = !hb.active;
        return Ok(Json(json!({
            "ok": true,
            "run_id": query.run_id,
            "done": done,
            "last_event_id": 0u64,
            "timeline_events": timeline_count,
            "heartbeat": json_job_heartbeat(hb),
            "background_job": bg_json,
            "source": "database",
        })));
    }

    Err((StatusCode::NOT_FOUND, "run not found".into()))
}

async fn api_queue_diagnostics(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let lanes = state.app_state.chat_queue.diagnostics().await;
    let db = state.app_state.db.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state
        .app_state
        .config
        .background_job_pending_start_timeout_secs as i64;
    let mut background_by_chat: HashMap<i64, Vec<serde_json::Value>> = HashMap::new();
    let mut rows = Vec::new();
    for lane in &lanes {
        let chat_id = lane.chat_id;
        let mut items = Vec::new();
        for it in &lane.items {
            let cid = chat_id;
            let pid = it.persona_id;
            let persona_name = call_blocking(db.clone(), move |db| {
                Ok(db
                    .get_persona(pid)?
                    .filter(|p| p.chat_id == cid)
                    .map(|p| p.name)
                    .unwrap_or_else(|| format!("persona #{pid}")))
            })
            .await
            .unwrap_or_else(|_| format!("persona #{pid}"));
            items.push(json!({
                "run_id": it.run_id,
                "persona_id": it.persona_id,
                "persona_name": persona_name,
                "source": it.source,
                "label": it.label,
                "state": it.state,
                "project_id": it.project_id,
                "workflow_id": it.workflow_id,
                "position": it.position,
                "elapsed_ms": it.elapsed_ms,
            }));
        }
        rows.push(json!({
            "chat_id": lane.chat_id,
            "persona_id": lane.persona_id,
            "pending": lane.pending,
            "active_for_ms": lane.active_for_ms,
            "oldest_wait_ms": lane.oldest_wait_ms,
            "last_error": lane.last_error,
            "project_id": lane.project_id,
            "workflow_id": lane.workflow_id,
            "items": items,
        }));

        let cid = lane.chat_id;
        let now_bg = now.clone();
        let timeout = pending_timeout_secs;
        if let Ok(active) = call_blocking(db.clone(), move |database| {
            database.list_active_background_jobs_for_chat(cid, &now_bg, timeout)
        })
        .await
        {
            let entries: Vec<serde_json::Value> = active.iter().map(json_background_job).collect();
            if !entries.is_empty() {
                background_by_chat.insert(lane.chat_id, entries);
            }
        }
    }
    let bg_map: serde_json::Map<String, serde_json::Value> = background_by_chat
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::Array(v)))
        .collect();
    Ok(Json(json!({
        "ok": true,
        "lanes": rows,
        "background_by_chat": bg_map,
    })))
}

#[derive(Debug, Deserialize)]
struct OpsPollQuery {
    chat_id: Option<i64>,
    /// When false/0, skip personas list (sidebar refresh is slower than queue/background).
    include_personas: Option<String>,
    limit: Option<usize>,
}

fn ops_poll_include_personas(raw: Option<&str>) -> bool {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some(s) => !matches!(
            s.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
    }
}

/// Combined queue + background (+ optional personas) snapshot for the web ops poller.
/// Replaces three parallel GETs per tick to cut connection churn under frequent poll.
async fn api_ops_poll(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<OpsPollQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    let include_personas = ops_poll_include_personas(query.include_personas.as_deref());
    if include_personas {
        ensure_web_binding_for_universal(&state, chat_id).await?;
    }

    let all_lanes = state.app_state.chat_queue.diagnostics().await;
    let db = state.app_state.db.clone();
    let mut lane_rows = Vec::new();
    for lane in all_lanes.into_iter().filter(|l| l.chat_id == chat_id) {
        let mut items = Vec::new();
        for it in &lane.items {
            let cid = chat_id;
            let pid = it.persona_id;
            let persona_name = call_blocking(db.clone(), move |database| {
                Ok(database
                    .get_persona(pid)?
                    .filter(|p| p.chat_id == cid)
                    .map(|p| p.name)
                    .unwrap_or_else(|| format!("persona #{pid}")))
            })
            .await
            .unwrap_or_else(|_| format!("persona #{pid}"));
            items.push(json!({
                "run_id": it.run_id,
                "persona_id": it.persona_id,
                "persona_name": persona_name,
                "source": it.source,
                "label": it.label,
                "state": it.state,
                "project_id": it.project_id,
                "workflow_id": it.workflow_id,
                "position": it.position,
                "elapsed_ms": it.elapsed_ms,
            }));
        }
        lane_rows.push(json!({
            "chat_id": lane.chat_id,
            "persona_id": lane.persona_id,
            "pending": lane.pending,
            "active_for_ms": lane.active_for_ms,
            "oldest_wait_ms": lane.oldest_wait_ms,
            "last_error": lane.last_error,
            "project_id": lane.project_id,
            "workflow_id": lane.workflow_id,
            "items": items,
        }));
    }

    let limit = query.limit.unwrap_or(20).min(100);
    let jobs = call_blocking(state.app_state.db.clone(), move |database| {
        database.list_background_jobs_for_chat(chat_id, limit)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let heartbeats = call_blocking(state.app_state.db.clone(), move |database| {
        database.list_job_heartbeats_for_chat(chat_id, 200)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let hb_by_key: HashMap<String, JobHeartbeat> = heartbeats
        .into_iter()
        .map(|h| (h.run_key.clone(), h))
        .collect();
    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state
        .app_state
        .config
        .background_job_pending_start_timeout_secs as i64;
    let active_count = call_blocking(state.app_state.db.clone(), move |database| {
        database.count_active_background_jobs_for_chat(chat_id, &now, pending_timeout_secs)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let job_items: Vec<serde_json::Value> = jobs
        .into_iter()
        .map(|j| {
            let hb = hb_by_key.get(&j.id);
            let mut row = json_background_job(&j);
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "heartbeat".into(),
                    hb.map(json_job_heartbeat)
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            row
        })
        .collect();

    let personas = if include_personas {
        let cid = chat_id;
        let mut persona_rows: Vec<Persona> =
            call_blocking(state.app_state.db.clone(), move |database| {
                database.list_personas(cid)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let activity = order_personas_by_activity(&state, chat_id, &mut persona_rows).await?;
        let cid2 = chat_id;
        let active_id = call_blocking(state.app_state.db.clone(), move |database| {
            database.get_active_persona_id(cid2)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let cid3 = chat_id;
        let last_bot_rows = call_blocking(state.app_state.db.clone(), move |database| {
            database.list_persona_last_bot_message_at(cid3)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let last_bot_by_persona: HashMap<i64, crate::db::PersonaLastBotInfo> = last_bot_rows
            .into_iter()
            .map(|row| (row.persona_id, row))
            .collect();
        let cid4 = chat_id;
        let read_rows = call_blocking(state.app_state.db.clone(), move |database| {
            database.list_persona_read_cursors(cid4)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let read_by_persona: HashMap<i64, String> = read_rows.into_iter().collect();
        Some(
            persona_rows
                .iter()
                .map(|p| {
                    let last = last_bot_by_persona.get(&p.id);
                    json!({
                        "id": p.id,
                        "name": p.name,
                        "is_active": active_id == Some(p.id),
                        "last_message_at": activity.get(&p.id),
                        "last_bot_message_at": last.map(|r| r.last_bot_message_at.clone()),
                        "last_bot_message_session_id": last.and_then(|r| r.session_id.clone()),
                        "last_bot_message_session_title": last.and_then(|r| r.session_title.clone()),
                        "last_read_at": read_by_persona.get(&p.id),
                    })
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "lanes": lane_rows,
        "jobs": job_items,
        "active_count": active_count,
        "personas_included": include_personas,
        "personas": personas.unwrap_or_default(),
        "collab": {
            "unread_runs": call_blocking(state.app_state.db.clone(), move |db| db.count_unread_runs(chat_id)).await.unwrap_or(0),
            "blocked_items": call_blocking(state.app_state.db.clone(), move |db| db.count_blocked_items(chat_id)).await.unwrap_or(0),
        },
    })))
}

#[derive(Deserialize)]
struct QueueCancelRequest {
    run_id: String,
    chat_id: Option<i64>,
}

async fn api_queue_cancel(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<QueueCancelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let run_id = body.run_id.trim();
    if run_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "run_id is required".into()));
    }
    info!(chat_id, run_id, "web queue cancel requested");
    let ok = state
        .app_state
        .chat_queue
        .request_cancel(run_id, chat_id)
        .await;
    if !ok {
        warn!(chat_id, run_id, "web queue cancel target not found");
        return Err((StatusCode::NOT_FOUND, "run not found for this chat".into()));
    }
    info!(chat_id, run_id, "web queue cancel accepted");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct QueueRemoveRequest {
    run_id: String,
    chat_id: Option<i64>,
}

async fn api_queue_remove(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<QueueRemoveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let run_id = body.run_id.trim();
    if run_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "run_id is required".into()));
    }
    info!(chat_id, run_id, "web queue remove requested");
    let outcome = state
        .app_state
        .chat_queue
        .request_remove_queued(run_id, chat_id)
        .await;
    match outcome {
        QueueRemoveOutcome::Removed => {
            info!(chat_id, run_id, "web queue remove accepted");
            Ok(Json(json!({ "ok": true })))
        }
        QueueRemoveOutcome::Running => {
            warn!(
                chat_id,
                run_id, "web queue remove rejected because run is running"
            );
            Err((
                StatusCode::CONFLICT,
                "run is currently running; use Stop instead".into(),
            ))
        }
        QueueRemoveOutcome::NotFound => {
            warn!(chat_id, run_id, "web queue remove target not found");
            Err((StatusCode::NOT_FOUND, "run not found for this chat".into()))
        }
    }
}

fn is_background_job_active_status(status: &str) -> bool {
    matches!(
        status,
        "pending" | "running" | "completed_raw" | "main_agent_processing"
    )
}

#[derive(Deserialize)]
struct BackgroundJobCancelRequest {
    job_id: String,
    chat_id: Option<i64>,
}

async fn api_background_job_cancel(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<BackgroundJobCancelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let job_id = body.job_id.trim().to_string();
    if job_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "job_id is required".into()));
    }

    let job = call_blocking(state.app_state.db.clone(), {
        let job_id = job_id.clone();
        move |db| db.get_background_job(&job_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(job) = job else {
        return Err((StatusCode::NOT_FOUND, "background job not found".into()));
    };
    if job.chat_id != chat_id {
        return Err((StatusCode::NOT_FOUND, "background job not found".into()));
    }
    if !is_background_job_active_status(job.status.as_str()) {
        return Err((
            StatusCode::CONFLICT,
            format!("background job is not active (status: {})", job.status),
        ));
    }

    info!(chat_id, job_id, "web background job cancel requested");

    if job.job_kind == "shell" {
        if let Err(e) = crate::background_shell::cancel_background_shell_job(
            &state.app_state,
            &job,
            "Cancelled by user",
        )
        .await
        {
            return Err((StatusCode::CONFLICT, e));
        }
        return Ok(Json(json!({ "ok": true })));
    }

    let cancelled = state
        .app_state
        .background_job_control
        .request_cancel(&job_id, chat_id)
        .await;
    if !cancelled {
        warn!(
            chat_id,
            job_id, "web background job cancel target not found in runtime registry"
        );
        return Err((
            StatusCode::CONFLICT,
            "background job is no longer cancellable".into(),
        ));
    }

    Ok(Json(json!({ "ok": true })))
}

fn guest_turn_failure(
    chat_id: i64,
    persona_id: i64,
    prompt: &str,
    guest_id: Option<i64>,
    guest_name: &str,
    error: String,
) -> Json<serde_json::Value> {
    Json(json!({
        "ok": false,
        "chat_id": chat_id,
        "persona_id": persona_id,
        "prompt": prompt,
        "response": "",
        "background_job_queued": false,
        "guest_error": {
            "persona_id": guest_id,
            "persona_name": guest_name,
            "error": error,
        }
    }))
}

async fn send_and_store_response_with_events(
    state: WebState,
    body: SendRequest,
    event_tx: Option<&tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    run_key: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let raw_text = body.message.trim().to_string();
    let mut text = raw_text.clone();
    let mut images = InboundImages::new();

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let persona_id = if let Some(pid) = body.persona_id {
        pid
    } else {
        let cid = chat_id;
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let attachment_notes =
        process_web_attachments(&state, chat_id, persona_id, &body.attachments, &mut images)
            .await?;
    if !attachment_notes.is_empty() {
        let note_text = attachment_notes.join("\n");
        if text.trim().is_empty() {
            text = note_text;
        } else {
            text = format!("{}\n\n{}", text.trim(), note_text);
        }
    }

    if text.trim().is_empty() && images.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message is required".into()));
    }

    // Single entry point: parse slash command first. If command, run backend handler and return — never send to LLM.
    if let Some(cmd) = parse_slash_command(&raw_text) {
        ensure_web_binding_for_universal(&state, chat_id).await?;
        call_blocking(state.app_state.db.clone(), move |db| {
            db.upsert_chat(chat_id, Some("default"), "web")
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let cid = chat_id;
        let persona_id = if let Some(pid) = body.persona_id {
            pid
        } else {
            call_blocking(state.app_state.db.clone(), move |db| {
                db.get_current_persona_id(cid)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        };

        let resp = match cmd {
            SlashCommand::Reset => {
                let cid2 = chat_id;
                let _ = call_blocking(state.app_state.db.clone(), move |db| {
                    db.delete_session(cid2, persona_id)
                })
                .await;
                "Conversation cleared. Principles and per-persona memory are unchanged.".into()
            }
            SlashCommand::Skills => state.app_state.skills.list_skills_formatted(),
            SlashCommand::Persona => {
                crate::persona::handle_persona_command(
                    state.app_state.db.clone(),
                    chat_id,
                    text.trim(),
                    Some(&state.app_state.config),
                )
                .await
            }
            SlashCommand::Schedule => {
                let cid_sched = chat_id;
                let pid_sched = persona_id;
                let tasks = if pid_sched > 0 {
                    call_blocking(state.app_state.db.clone(), move |db| {
                        db.get_scheduled_tasks_for_chat_and_persona(cid_sched, pid_sched)
                    })
                    .await
                } else {
                    call_blocking(state.app_state.db.clone(), move |db| {
                        db.get_scheduled_tasks_for_chat_for_display(cid_sched)
                    })
                    .await
                };
                match &tasks {
                    Ok(t) => crate::tools::schedule::format_tasks_list(t),
                    Err(e) => format!("Error listing tasks: {e}"),
                }
            }
            SlashCommand::Secrets => crate::secret_vault::secrets_command_reply(
                &state.app_state.config.runtime_data_dir(),
                &state.app_state.config.skills_data_dir_absolute(),
                chat_id,
                &raw_text,
            ),
            SlashCommand::Archive => {
                let cid2 = chat_id;
                let pid = persona_id;
                let history = call_blocking(state.app_state.db.clone(), move |db| {
                    db.get_recent_messages(cid2, pid, 500, false)
                })
                .await
                .unwrap_or_default();
                let messages: Vec<Message> = history
                    .into_iter()
                    .map(|m| Message {
                        role: if m.is_from_bot { "assistant" } else { "user" }.into(),
                        content: MessageContent::Text(m.content),
                    })
                    .collect();
                if messages.is_empty() {
                    "No conversation to archive.".into()
                } else {
                    archive_conversation(
                        &state.app_state.config.runtime_data_dir(),
                        chat_id,
                        &messages,
                    );
                    format!("Archived {} messages.", messages.len())
                }
            }
        };

        deliver_to_contact(
            state.app_state.db.clone(),
            state.app_state.telegram_bots.as_ref(),
            state.app_state.discord_http.as_ref(),
            state.app_state.wecom.as_deref(),
            &state.app_state.config.bot_username,
            chat_id,
            persona_id,
            &resp,
            Some(state.app_state.config.workspace_root_absolute()),
            DeliveryScope::StoreOnly,
            None,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(json!({
            "ok": true,
            "chat_id": chat_id,
            "response": resp,
        })));
    }

    // Not a slash command: normal flow — resolve, store message, run agent
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let sender_name = body
        .sender_name
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("web-user")
        .to_string();

    call_blocking(state.app_state.db.clone(), move |db| {
        db.upsert_chat(chat_id, Some("default"), "web")
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let full_prompt_for_handoff = text.clone();
    let user_msg = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id,
        persona_id,
        session_id: body.session_id.clone(),
        sender_name: sender_name.clone(),
        content: text.clone(),
        is_from_bot: false,
        timestamp: chrono::Utc::now().to_rfc3339(),
        origin: crate::db::message_origin_interactive(),
        speaker_persona_id: None,
    };
    let inbound_user_message_id = user_msg.id.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.store_message(&user_msg)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let guest_ids =
        crate::collab::guest::ordered_guest_ids(body.guest_persona_id, &body.guest_persona_ids);
    let guests = if guest_ids.is_empty() {
        Vec::new()
    } else {
        let db = state.app_state.db.clone();
        let host_id = persona_id;
        let chat_for_guests = chat_id;
        let checked = call_blocking(db, move |db| {
            Ok(crate::collab::guest::validate_guest_ids(
                db,
                chat_for_guests,
                host_id,
                &guest_ids,
            ))
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        match checked {
            Ok(list) => list,
            Err(msg) => {
                return Ok(guest_turn_failure(
                    chat_id,
                    persona_id,
                    &raw_text,
                    None,
                    "A tagged persona",
                    msg,
                ));
            }
        }
    };

    if let Some(run_id) = run_key {
        state
            .run_hub
            .publish(
                run_id,
                "message_stored",
                json!({
                    "persona_id": persona_id,
                    "message_id": inbound_user_message_id,
                })
                .to_string(),
                state.limits.run_history_limit,
            )
            .await;
    }

    if !guests.is_empty() {
        // Tagged personas answer in order. The host turn does not run.
        let total = guests.len();
        let mut replies = Vec::with_capacity(total);
        for (index, guest) in guests.iter().enumerate() {
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
            {
                return Ok(guest_turn_failure(
                    chat_id,
                    persona_id,
                    &raw_text,
                    Some(guest.id),
                    &guest.name,
                    "Cancelled".into(),
                ));
            }
            let position = index + 1;
            if let Some(run_id) = run_key {
                state
                    .run_hub
                    .publish(
                        run_id,
                        "status",
                        json!({
                            "message": format!(
                                "{} is answering ({position} of {total})",
                                guest.name
                            )
                        })
                        .to_string(),
                        state.limits.run_history_limit,
                    )
                    .await;
            }
            match crate::collab::guest::run_guest_turn(
                state.app_state.clone(),
                chat_id,
                persona_id,
                guest.id,
                body.session_id.clone(),
                &text,
                position,
                total,
                cancel.clone(),
            )
            .await
            {
                Ok(reply) => replies.push(format!("[Guest: {}]\n{reply}", guest.name)),
                Err(error) => {
                    warn!(
                        target: "collab",
                        chat_id,
                        guest_persona_id = guest.id,
                        error = %error,
                        "guest turn failed"
                    );
                    return Ok(guest_turn_failure(
                        chat_id,
                        persona_id,
                        &raw_text,
                        Some(guest.id),
                        &guest.name,
                        error,
                    ));
                }
            }
        }
        return Ok(Json(json!({
            "ok": true,
            "chat_id": chat_id,
            "persona_id": persona_id,
            "prompt": raw_text,
            "response": replies.join("\n\n"),
            "background_job_queued": false,
            "background_job_id": serde_json::Value::Null,
        })));
    }

    let agent_result = process_with_agent_with_events(
        &state.app_state,
        AgentRequestContext {
            caller_channel: "web",
            chat_id,
            chat_type: "private",
            persona_id,
            is_scheduled_task: false,
            is_background_job: false,
            run_key: run_key.map(|s| s.to_string()),
            reply_bot_instance_id: None,
            session_id: body.session_id.clone(),
            history_override: None,
            tool_iteration_cap: None,
            skip_recall: false,
            inbound_user_message_id: Some(inbound_user_message_id),
        },
        None,
        images,
        event_tx,
        cancel,
    )
    .await;

    // User message is already stored. Never leave the turn without a bot row:
    // Cursor/sidecar errors used to return 500 here, so the stream flashed Error
    // and history polling then showed only the user message.
    let mut response = match agent_result {
        Ok(agent_out) => ensure_visible_turn_text(&agent_out.response),
        Err(e) => {
            warn!(
                target: "web",
                chat_id,
                persona_id,
                error = %e,
                "agent run failed after user message stored; delivering failure notice"
            );
            failed_turn_notice(&e.to_string())
        }
    };

    let mut background_job_id: Option<String> = None;
    let mut background_job_queued = false;

    if is_background_handoff_response(&response) {
        let trig = handoff_trigger_for_db(&response).unwrap_or("timeout");
        match try_enqueue_background_handoff(
            state.app_state.clone(),
            chat_id,
            persona_id,
            full_prompt_for_handoff,
            trig,
            "web",
            body.session_id.clone(),
        )
        .await
        {
            HandoffEnqueueOutcome::Queued { job_id, start_ack } => {
                background_job_id = Some(job_id.clone());
                background_job_queued = true;
                response = await_handoff_startup_ack(start_ack).await;
            }
            HandoffEnqueueOutcome::BlockedAlreadyRunning => {
                response = "A background task is already running for this chat. Please wait for it to finish before starting another long-running background task.".into();
            }
            HandoffEnqueueOutcome::ActiveLookupFailed(msg) => {
                response = format!("Failed to check active background jobs: {msg}");
            }
            HandoffEnqueueOutcome::DbCreateFailed(e) => {
                response = format!("Failed to queue background task: {e}");
            }
        }
    } else {
        let delivery = deliver_agent_final_to_contact(
            state.app_state.db.clone(),
            state.app_state.telegram_bots.as_ref(),
            state.app_state.discord_http.as_ref(),
            state.app_state.wecom.as_deref(),
            &state.app_state.config.bot_username,
            chat_id,
            persona_id,
            &response,
            Some(state.app_state.config.workspace_root_absolute()),
            DeliveryScope::StoreOnly,
            body.session_id.clone(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        response = delivery.response_for_client;
    }

    let mut out = json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": persona_id,
        "prompt": raw_text,
        "response": response,
        "background_job_queued": background_job_queued,
    });
    if let Some(id) = background_job_id {
        out["background_job_id"] = json!(id);
    } else {
        out["background_job_id"] = serde_json::Value::Null;
    }
    Ok(Json(out))
}

#[cfg(test)]
fn upload_rel_url_exists(state: &WebState, rel_url: &str) -> bool {
    if let Some(id) = rel_url
        .strip_prefix("/api/artifacts/")
        .and_then(|s| s.strip_suffix("/content"))
    {
        return crate::artifacts::Catalog::open(&state.app_state.config.workspace_root_absolute())
            .and_then(|c| c.get(id))
            .is_ok_and(|a| a.blob_path.is_file());
    }
    let Some(rel) = rel_url.strip_prefix("/api/uploads/") else {
        return true;
    };
    let shared_path = state
        .app_state
        .config
        .workspace_root_absolute()
        .join("shared")
        .join("upload")
        .join(rel);
    if shared_path.is_file() {
        return true;
    }
    let legacy_path = FsPath::new(state.app_state.config.working_dir())
        .join("uploads")
        .join(rel);
    legacy_path.is_file()
}

#[cfg(test)]
async fn materialize_response_file_links(
    state: &WebState,
    chat_id: i64,
    persona_id: i64,
    response: &str,
) -> Result<String, (StatusCode, String)> {
    crate::final_delivery_media::materialize_web_delivery_file_links(
        &state.app_state.config.workspace_root_absolute(),
        Some(state.app_state.config.working_dir()),
        chat_id,
        persona_id,
        response,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

fn web_max_document_bytes(config: &Config) -> u64 {
    config
        .max_document_size_mb
        .saturating_mul(1024)
        .saturating_mul(1024)
}

/// Axum default is 2 MB; allow inline JSON base64 payloads up to ~1.4× file limit + headroom.
fn web_json_body_limit_bytes(config: &Config) -> usize {
    let doc = web_max_document_bytes(config) as f64;
    (doc * 1.45).ceil() as usize + 65_536
}

fn web_multipart_body_limit_bytes(config: &Config) -> usize {
    web_max_document_bytes(config) as usize + 2 * 1024 * 1024
}

fn web_upload_dir(state: &WebState, chat_id: i64) -> PathBuf {
    state
        .app_state
        .config
        .workspace_root_absolute()
        .join("shared")
        .join("upload")
        .join("web")
        .join(chat_id.to_string())
}

fn web_upload_dir_for_persona(state: &WebState, chat_id: i64, persona_id: i64) -> PathBuf {
    web_upload_dir(state, chat_id).join(persona_id.to_string())
}

fn resolve_upload_tool_path_on_disk(state: &WebState, tool_path: &str) -> Option<PathBuf> {
    let clean = tool_path.trim().trim_start_matches('/');
    if clean.is_empty() || clean.contains("..") {
        return None;
    }
    let full = state
        .app_state
        .config
        .workspace_root_absolute()
        .join("shared")
        .join(clean);
    if full.is_file() {
        Some(full)
    } else {
        None
    }
}

fn fast_upload_filename(safe_name: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f");
    format!("{ts}-{safe_name}")
}

fn attachment_notes_for_saved(
    filename: &str,
    bytes_len: u64,
    mime: &str,
    tool_path: &str,
    saved_path: &FsPath,
    rel_url: &str,
) -> Vec<String> {
    let mut notes = vec![format!(
        "[document] filename={} bytes={} mime={} tool_path={} saved_path={} url={}",
        filename,
        bytes_len,
        mime,
        tool_path,
        saved_path.display(),
        rel_url
    )];
    if mime.starts_with("image/") {
        let alt = filename.replace(']', "_");
        notes.push(format!("![{alt}]({rel_url})"));
    } else {
        notes.push(format!("[{filename}]({rel_url})"));
    }
    notes
}

async fn push_image_from_path(
    images: &mut InboundImages,
    path: &FsPath,
    bytes_len: u64,
    mime: &str,
) -> Result<(), (StatusCode, String)> {
    if !images.would_accept(mime, usize::try_from(bytes_len).unwrap_or(usize::MAX)) {
        images.record_dropped(mime);
        return Ok(());
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    images.push_bytes(&bytes, mime);
    Ok(())
}

async fn process_web_attachments(
    state: &WebState,
    chat_id: i64,
    persona_id: i64,
    attachments: &[SendAttachmentRequest],
    images: &mut InboundImages,
) -> Result<Vec<String>, (StatusCode, String)> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let max_bytes = web_max_document_bytes(&state.app_state.config);
    let dir = web_upload_dir_for_persona(state, chat_id, persona_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut notes = Vec::new();
    for (idx, att) in attachments.iter().enumerate() {
        let mime = att
            .media_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename = att
            .filename
            .clone()
            .unwrap_or_else(|| format!("web-attachment-{}.bin", idx + 1));

        if let Some(tool_path) = att.tool_path.as_deref().filter(|s| !s.trim().is_empty()) {
            let expected_prefix = format!("upload/web/{chat_id}/{persona_id}/");
            if !tool_path.starts_with(&expected_prefix) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("attachment path must start with {expected_prefix}"),
                ));
            }
            let disk_path =
                resolve_upload_tool_path_on_disk(state, tool_path).ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("attachment not found: {tool_path}"),
                    )
                })?;
            let meta = tokio::fs::metadata(&disk_path)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let bytes_len = meta.len();
            if bytes_len > max_bytes {
                notes.push(format!(
                    "[document] filename={} bytes={} mime={} skipped=too_large",
                    filename, bytes_len, mime
                ));
                continue;
            }
            let rel_url = crate::artifacts::publish(
                state.app_state.config.workspace_root_absolute(),
                chat_id,
                persona_id,
                disk_path.clone(),
                "attachment",
            )
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            .url;
            push_image_from_path(images, &disk_path, bytes_len, &mime).await?;
            notes.extend(attachment_notes_for_saved(
                &filename, bytes_len, &mime, tool_path, &disk_path, &rel_url,
            ));
            continue;
        }

        let b64 = att.data_base64.as_deref().filter(|s| !s.trim().is_empty());
        let Some(b64) = b64 else {
            return Err((
                StatusCode::BAD_REQUEST,
                "attachment must include tool_path or data_base64".into(),
            ));
        };

        let bytes = decode_base64_payload(b64).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid attachment base64: {e}"),
            )
        })?;

        if (bytes.len() as u64) > max_bytes {
            notes.push(format!(
                "[document] filename={} bytes={} mime={} skipped=too_large",
                filename,
                bytes.len(),
                mime
            ));
            continue;
        }

        let safe_name = sanitize_upload_filename(&filename);
        let stable_name = stable_upload_filename(&safe_name, &bytes, idx + 1);
        let path = dir.join(stable_name);
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let saved_file = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let tool_path = format!("upload/web/{}/{}/{}", chat_id, persona_id, saved_file);
        let rel_url = crate::artifacts::publish(
            state.app_state.config.workspace_root_absolute(),
            chat_id,
            persona_id,
            path.clone(),
            "attachment",
        )
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
        .url;

        images.push_bytes(&bytes, &mime);

        notes.extend(attachment_notes_for_saved(
            &filename,
            bytes.len() as u64,
            &mime,
            &tool_path,
            &path,
            &rel_url,
        ));
    }

    Ok(notes)
}

async fn api_upload(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<UploadQueryParams>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    let persona_id = if let Some(pid) = query.persona_id {
        pid
    } else {
        let cid = chat_id;
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let max_bytes = web_max_document_bytes(&state.app_state.config);
    let dir = web_upload_dir_for_persona(&state, chat_id, persona_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut uploaded: Option<UploadResponse> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid multipart upload: {e}"),
        )
    })? {
        if field.name() != Some("file") {
            continue;
        }
        let original_name = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "web-upload.bin".to_string());
        let mime = field
            .content_type()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let safe_name = sanitize_upload_filename(&original_name);
        let disk_name = fast_upload_filename(&safe_name);
        let path = dir.join(&disk_name);
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut total: u64 = 0;
        while let Some(chunk) = field.chunk().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("failed to read upload chunk: {e}"),
            )
        })? {
            total = total.saturating_add(chunk.len() as u64);
            if total > max_bytes {
                let _ = tokio::fs::remove_file(&path).await;
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "file exceeds maximum size of {} MB (MAX_DOCUMENT_SIZE_MB)",
                        state.app_state.config.max_document_size_mb
                    ),
                ));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let saved_file = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let tool_path = format!("upload/web/{}/{}/{}", chat_id, persona_id, saved_file);
        let rel_url = format!("/api/uploads/web/{chat_id}/{persona_id}/{saved_file}");
        let artifact = crate::artifacts::publish(
            state.app_state.config.workspace_root_absolute(),
            chat_id,
            persona_id,
            path.clone(),
            "attachment",
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let alias_root = state.app_state.config.workspace_root_absolute();
        let alias_id = artifact.id.clone();
        let alias_url = rel_url.clone();
        tokio::task::spawn_blocking(move || {
            crate::artifacts::Catalog::open(&alias_root)?.add_alias(&alias_url, &alias_id)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        uploaded = Some(UploadResponse {
            filename: original_name,
            media_type: mime,
            bytes: total,
            tool_path,
            url: artifact.url,
        });
        break;
    }

    uploaded.map(Json).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "missing file field in upload".into(),
        )
    })
}

fn decode_base64_payload(payload: &str) -> anyhow::Result<Vec<u8>> {
    let raw = payload
        .split_once(',')
        .map(|(_, b64)| b64)
        .unwrap_or(payload)
        .trim();
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|e| anyhow::anyhow!(e))
}

fn sanitize_upload_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "web-upload.bin".to_string()
    } else {
        sanitized
    }
}

fn stable_upload_filename(safe_name: &str, bytes: &[u8], fallback_index: usize) -> String {
    let final_name = if safe_name.is_empty() {
        format!("web-attachment-{}.bin", fallback_index)
    } else {
        safe_name.to_string()
    };
    let digest = Sha256::digest(bytes);
    let hash = digest[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{hash}-{final_name}")
}

/// Clear context: delete only the current persona's session for this contact (per-persona reset).
async fn api_reset(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ResetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let cid = chat_id;
    let pid = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_current_persona_id(cid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let cid2 = chat_id;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_session(cid2, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "deleted": deleted,
        "message": "Conversation cleared. Principles and per-persona memory are unchanged."
    })))
}

async fn api_delete_session(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<DeleteSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_chat_data(chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "deleted": deleted,
        "message": "Conversation cleared. Principles and per-persona memory are unchanged."
    })))
}

/// Both initial load and polling use the same activity ordering.
async fn order_personas_by_activity(
    state: &WebState,
    chat_id: i64,
    personas: &mut [Persona],
) -> Result<HashMap<i64, String>, (StatusCode, String)> {
    let rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_persona_last_message_at(chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let activity: HashMap<i64, String> = rows.into_iter().collect();
    personas.sort_by_cached_key(|p| {
        (
            std::cmp::Reverse(
                activity
                    .get(&p.id)
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()),
            ),
            p.id,
        )
    });
    Ok(activity)
}

async fn api_personas(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<PersonasQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let cid = chat_id;

    let mut personas: Vec<Persona> =
        call_blocking(state.app_state.db.clone(), move |db| db.list_personas(cid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let activity = order_personas_by_activity(&state, chat_id, &mut personas).await?;
    let cid2 = chat_id;
    let active_id = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_active_persona_id(cid2)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cid3 = chat_id;
    let last_bot_rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_persona_last_bot_message_at(cid3)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let last_bot_by_persona: HashMap<i64, crate::db::PersonaLastBotInfo> = last_bot_rows
        .into_iter()
        .map(|row| (row.persona_id, row))
        .collect();

    let cid4 = chat_id;
    let read_rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_persona_read_cursors(cid4)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let read_by_persona: HashMap<i64, String> = read_rows.into_iter().collect();

    let global_engine = state.app_state.runtime_toggles.agent_engine();
    let items: Vec<serde_json::Value> = personas
        .iter()
        .map(|p| {
            let last = last_bot_by_persona.get(&p.id);
            let effective = crate::runtime_toggles::resolve_run_agent_engine(
                p.agent_engine_override.as_deref(),
                global_engine,
            );
            json!({
                "id": p.id,
                "name": p.name,
                "model_override": p.model_override,
                "is_active": active_id == Some(p.id),
                "last_message_at": activity.get(&p.id),
                        "last_bot_message_at": last.map(|r| r.last_bot_message_at.clone()),
                "last_bot_message_session_id": last.and_then(|r| r.session_id.clone()),
                "last_bot_message_session_title": last.and_then(|r| r.session_title.clone()),
                "last_read_at": read_by_persona.get(&p.id),
                "agent_engine_override": p.agent_engine_override,
                "agent_engine_effective": effective.as_str(),
            })
        })
        .collect();

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "personas": items,
    })))
}

#[derive(Deserialize)]
struct PersonaReadCursorItem {
    persona_id: i64,
    last_read_at: Option<String>,
}

#[derive(Deserialize)]
struct PersonaReadCursorsRequest {
    chat_id: Option<i64>,
    /// When true, an existing cursor is left unchanged (first-visit baseline).
    if_absent: Option<bool>,
    items: Vec<PersonaReadCursorItem>,
}

fn normalize_read_cursor(raw: Option<&str>) -> Result<String, (StatusCode, String)> {
    let dt = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => chrono::DateTime::parse_from_rfc3339(value)
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "last_read_at must be an RFC3339 timestamp".into(),
                )
            })?
            .with_timezone(&chrono::Utc),
        None => chrono::Utc::now(),
    };
    Ok(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

async fn api_personas_read(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonaReadCursorsRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    if body.items.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "items is required".into()));
    }
    if body.items.len() > 200 {
        return Err((StatusCode::BAD_REQUEST, "too many read cursors".into()));
    }
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let only_if_absent = body.if_absent.unwrap_or(false);
    let mut updates = Vec::with_capacity(body.items.len());
    for item in &body.items {
        if item.persona_id <= 0 {
            return Err((StatusCode::BAD_REQUEST, "persona_id is required".into()));
        }
        let stamp = normalize_read_cursor(item.last_read_at.as_deref())?;
        updates.push((item.persona_id, stamp));
    }
    let outcome = call_blocking(state.app_state.db.clone(), move |db| {
        db.apply_persona_read_cursors(chat_id, only_if_absent, &updates)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stored = match outcome {
        Ok(rows) => rows,
        Err(persona_id) => {
            return Err((
                StatusCode::NOT_FOUND,
                format!("persona {persona_id} not found"),
            ));
        }
    };
    let cursors: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|(persona_id, last_read_at)| {
            json!({
                "persona_id": persona_id,
                "last_read_at": last_read_at,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "cursors": cursors,
    })))
}

async fn api_personas_switch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonasSwitchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let persona_name = body.persona_name.clone();
    let persona_name_for_msg = persona_name.clone();

    let persona = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_by_name(chat_id, &persona_name)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(persona) = persona else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Persona '{}' not found", persona_name_for_msg),
        ));
    };

    let ok = call_blocking(state.app_state.db.clone(), move |db| {
        db.set_active_persona(chat_id, persona.id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !ok {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to switch persona".into(),
        ));
    }

    Ok(Json(json!({
        "ok": true,
        "message": format!("Switched to {}", persona_name_for_msg),
    })))
}

#[derive(Deserialize)]
struct PersonaMemoryPathParams {
    persona_id: i64,
}

#[derive(Debug, Deserialize, Default)]
struct AgentHistoryOptimizeRequest {
    #[serde(default)]
    operator_notes: Option<String>,
}

#[derive(Deserialize)]
struct PersonaBookmarkPathParams {
    persona_id: i64,
}

#[derive(Deserialize)]
struct PersonaBookmarkDeletePathParams {
    persona_id: i64,
    message_id: String,
}

#[derive(Deserialize)]
struct PersonaMessagePathParams {
    persona_id: i64,
    message_id: String,
}

#[derive(Deserialize)]
struct PersonaPolicyPathParams {
    persona_id: i64,
}

#[derive(Deserialize)]
struct HookDefinitionUpsertBody {
    id: Option<i64>,
    name: String,
    event_name: String,
    matcher: Option<String>,
    action_type: String,
    action_payload_json: Option<String>,
    enabled: Option<bool>,
    #[serde(default)]
    scoped_persona_ids: Option<Vec<i64>>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    creating_persona_id: Option<i64>,
}

#[derive(Deserialize)]
struct HookDefinitionDeletePathParams {
    id: i64,
}

#[derive(Deserialize)]
struct SkillsQuery {
    persona_id: Option<i64>,
}

#[derive(Deserialize)]
struct HooksQuery {
    persona_id: Option<i64>,
}

/// PATCH tri-state: absent = unchanged, JSON `null` = clear, value = set.
/// Plain `Option<Option<T>>` treats JSON `null` as absent; this wrapper maps it to `Some(None)`.
fn deserialize_patch_field<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

#[derive(Debug, Deserialize, Default)]
struct PersonaPolicyPatchBody {
    /// `null` => default allow-all. `[]` => explicit allow-none.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    allowed_hook_ids: Option<Option<Vec<i64>>>,
    /// `null` => default allow-all. `[]` => explicit allow-none.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    allowed_skill_names: Option<Option<Vec<String>>>,
}

#[derive(Deserialize)]
struct PersonaBookmarkUpsertBody {
    message_id: String,
    note: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct PersonaBulletinPatchBody {
    /// `null` clears override (server defaults). Omitted = leave unchanged.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    recent_history_min_user: Option<Option<i64>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    recent_history_min_assistant: Option<Option<i64>>,
    /// `null` clears memo. Omitted = leave unchanged.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    operator_memo: Option<Option<String>>,
    #[serde(default)]
    dense_delivery_enabled: Option<bool>,
    #[serde(default)]
    user_message_rewrite_enabled: Option<bool>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    dense_delivery_messaging_max_chars: Option<Option<i64>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    dense_delivery_web_max_chars: Option<Option<i64>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    dense_delivery_summary_chars: Option<Option<i64>>,
    /// `null`/empty clears override (inherit global). Omitted = leave unchanged.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_patch_field")]
    agent_engine_override: Option<Option<String>>,
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() > max_chars {
        format!("{}...", input.chars().take(max_chars).collect::<String>())
    } else {
        input.to_string()
    }
}

fn bookmark_to_json(b: &PersonaMessageBookmark) -> serde_json::Value {
    json!({
        "message_id": b.message_id,
        "role": b.role,
        "content_preview": b.content_preview,
        "note": b.note,
        "created_at": b.created_at,
        "updated_at": b.updated_at,
        "session_id": b.session_id,
        "message_timestamp": b.message_timestamp,
        "missing": b.missing,
    })
}

fn normalize_persona_scope_ids(ids: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    out.sort_unstable();
    out.dedup();
    out
}

// --- Chat Sessions ---

#[derive(Debug, Deserialize)]
struct ChatSessionsListQuery {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    /// Deprecated: session archive is gone; the list always includes every session.
    #[serde(default)]
    include_archived: Option<bool>,
}

fn chat_session_json(session: &crate::db::ChatSession) -> serde_json::Value {
    serde_json::json!({
        "id": session.id,
        "chat_id": session.chat_id,
        "persona_id": session.persona_id,
        "title": session.title,
        "intent": session.intent,
        "status": session.status,
        "created_at": session.created_at,
        "last_active_at": session.last_active_at,
        "archived_at": session.archived_at,
        "ttl_hours": session.ttl_hours,
        "mirror_main_chat": session.mirror_main_chat,
        "source_topic_id": session.source_topic_id,
    })
}

async fn api_chat_sessions_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<ChatSessionsListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    let cid = chat_id;
    let persona_id = if let Some(pid) = query.persona_id {
        pid
    } else {
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let _ = query.include_archived;
    let sessions = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_chat_sessions(chat_id, persona_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<serde_json::Value> = sessions.iter().map(chat_session_json).collect();
    Ok(Json(serde_json::json!({ "sessions": items })))
}

#[derive(Debug, Deserialize)]
struct CreateChatSessionRequest {
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    #[serde(default)]
    intent: String,
    #[serde(default)]
    ttl_hours: Option<i64>,
    #[serde(default)]
    mirror_main_chat: Option<bool>,
    #[serde(default)]
    source_topic_id: Option<String>,
}

const TOPIC_SESSION_SEED_TURN_CHARS: usize = 600;
const TOPIC_SESSION_SEED_CHARS: usize = 6000;
const TOPIC_SESSION_SEED_TURNS: usize = 8;
const TOPIC_SESSION_SEED_BOOKMARKS: usize = 10;

fn trimmed_cursor_id(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.len() > 128 {
        return Err("id is too long".into());
    }
    Ok(Some(text.to_string()))
}

fn dossier_page_request(
    query: &TopicDossierQuery,
) -> Result<crate::db::DossierPageRequest, (StatusCode, String)> {
    let before = trimmed_cursor_id(query.before_id.as_deref())
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let after = trimmed_cursor_id(query.after_id.as_deref())
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let around = trimmed_cursor_id(query.around_id.as_deref())
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let chosen = usize::from(before.is_some())
        + usize::from(after.is_some())
        + usize::from(around.is_some());
    if chosen > 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "only one of before_id, after_id, around_id is allowed".into(),
        ));
    }
    if let Some(id) = before {
        return Ok(crate::db::DossierPageRequest::Before(id));
    }
    if let Some(id) = after {
        return Ok(crate::db::DossierPageRequest::After(id));
    }
    if let Some(id) = around {
        return Ok(crate::db::DossierPageRequest::Around(id));
    }
    Ok(crate::db::DossierPageRequest::Latest)
}

fn topic_seed_media_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(
            r"(?i)!\[[^\]]*\]\([^)]*\)|<img\b[^>]*>|\[[^\]]*\]\([^)]*/api/(?:uploads|artifacts)/[^)]*\)",
        )
        .expect("topic seed media pattern")
    })
}

fn strip_topic_seed_media(text: &str) -> String {
    topic_seed_media_pattern()
        .replace_all(text, "")
        .trim()
        .to_string()
}

fn clamp_seed_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn build_topic_session_seed(
    title: &str,
    summary: &str,
    bookmarks: &[crate::db::PersonaMessageBookmark],
    messages: &[StoredMessage],
) -> String {
    let title = title.trim();
    let heading = if title.is_empty() {
        "Prior topic".to_string()
    } else {
        format!("Prior topic: {title}")
    };
    let summary = strip_topic_seed_media(summary);
    let summary_block = if summary.is_empty() {
        String::new()
    } else {
        format!("\n\nSummary:\n{}", clamp_seed_chars(&summary, 1200))
    };
    let mut bookmark_lines = Vec::new();
    for bookmark in bookmarks.iter().take(TOPIC_SESSION_SEED_BOOKMARKS) {
        let preview = strip_topic_seed_media(&bookmark.content_preview);
        if preview.is_empty() {
            continue;
        }
        bookmark_lines.push(format!("- {}", clamp_seed_chars(&preview, 200)));
    }
    let bookmark_block = if bookmark_lines.is_empty() {
        String::new()
    } else {
        format!("\n\nBookmarks:\n{}", bookmark_lines.join("\n"))
    };
    let start = messages.len().saturating_sub(TOPIC_SESSION_SEED_TURNS);
    let mut turns = Vec::new();
    for message in &messages[start..] {
        let body = clamp_seed_chars(
            &strip_topic_seed_media(&message.content),
            TOPIC_SESSION_SEED_TURN_CHARS,
        );
        if body.is_empty() {
            continue;
        }
        let role = if message.is_from_bot {
            "Assistant"
        } else {
            "You"
        };
        turns.push(format!("{role}: {body}"));
    }
    let head = format!("{heading}{summary_block}{bookmark_block}");
    let mut kept = turns;
    loop {
        let mut body = head.clone();
        if !kept.is_empty() {
            body.push_str("\n\nRecent turns:\n");
            body.push_str(&kept.join("\n"));
        }
        if body.chars().count() <= TOPIC_SESSION_SEED_CHARS || kept.is_empty() {
            return clamp_seed_chars(&body, TOPIC_SESSION_SEED_CHARS);
        }
        kept.remove(0);
    }
}

struct TopicSessionSeed {
    intent: String,
    seed: String,
}

async fn load_topic_session_seed(
    state: &WebState,
    chat_id: i64,
    persona_id: i64,
    topic_id: &str,
) -> Result<TopicSessionSeed, (StatusCode, String)> {
    let started = Instant::now();
    let lookup = topic_id.to_string();
    let topic = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_conversation_topic(chat_id, persona_id, &lookup)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(topic) = topic else {
        return Err((StatusCode::NOT_FOUND, "topic not found".into()));
    };
    if topic.chat_id != chat_id || topic.persona_id != persona_id {
        return Err((StatusCode::NOT_FOUND, "topic not found".into()));
    }
    let topic_for_window = topic.clone();
    let window = call_blocking(state.app_state.db.clone(), move |db| {
        db.topic_dossier_window(
            &topic_for_window,
            TOPIC_SESSION_SEED_TURNS,
            crate::db::DossierPageRequest::Latest,
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(window) = window else {
        return Err((StatusCode::NOT_FOUND, "topic not found".into()));
    };
    let seed = build_topic_session_seed(
        &topic.title,
        &topic.summary,
        &window.bookmarks,
        &window.messages,
    );
    let title = topic.title.trim();
    let intent = if title.is_empty() {
        "Continue topic".to_string()
    } else {
        format!("Continue: {title}")
    };
    info!(
        target: "web",
        topic_id_len = topic_id.len(),
        seed_chars = seed.chars().count(),
        duration_ms = started.elapsed().as_millis(),
        "topic session seed built"
    );
    Ok(TopicSessionSeed {
        intent: intent.chars().take(500).collect(),
        seed,
    })
}

async fn bootstrap_chat_session_context(
    app_state: Arc<AppState>,
    session_id: String,
    chat_id: i64,
    persona_id: i64,
    intent: String,
) {
    let bootstrap_prompt = format!(
        concat!(
            "You are bootstrapping a new focused session. The user's intent is:\n\n",
            "\"{}\"\n\n",
            "Use recall for ORIGIN notes relevant to this intent. Do not shell out to mempalace. ",
            "Also list which skills from the workspace are relevant.\n\n",
            "Return ONLY a compact JSON object (no markdown fences) with this structure:\n",
            "{{\"relevant_notes\": [\"note title or path\", ...], ",
            "\"selected_skills\": [\"skill_name\", ...], ",
            "\"key_context\": \"2-3 sentence summary of relevant background knowledge\"}}\n\n",
            "If nothing relevant is found, return the JSON with empty arrays and a brief note in key_context. ",
            "Do NOT ask the user anything. Do NOT include explanation outside the JSON."
        ),
        intent
    );

    let bootstrap_result = process_with_agent(
        &app_state,
        AgentRequestContext {
            caller_channel: "web",
            chat_id,
            chat_type: "private",
            persona_id,
            is_scheduled_task: false,
            is_background_job: false,
            run_key: None,
            reply_bot_instance_id: None,
            session_id: Some(session_id.clone()),
            history_override: None,
            tool_iteration_cap: None,
            skip_recall: false,
            inbound_user_message_id: None,
        },
        Some(&bootstrap_prompt),
        InboundImages::new(),
    )
    .await;

    let Ok(response) = bootstrap_result else {
        warn!(
            session_id = %session_id,
            "chat session bootstrap agent run failed"
        );
        return;
    };

    let trimmed = response.trim().to_string();
    let ctx_sid = session_id.clone();
    let ctx_json = trimmed.clone();
    if let Err(e) = call_blocking(app_state.db.clone(), move |db| {
        db.update_chat_session_bootstrap_context(&ctx_sid, &ctx_json)
    })
    .await
    {
        warn!(
            session_id = %session_id,
            error = %e,
            "failed to persist chat session bootstrap context"
        );
    }

    let bot_msg = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id,
        persona_id,
        session_id: Some(session_id),
        sender_name: app_state.config.bot_username.clone(),
        content: format!("Session ready. Context loaded for: {}", intent),
        is_from_bot: true,
        timestamp: chrono::Utc::now().to_rfc3339(),
        origin: crate::db::message_origin_interactive(),
        speaker_persona_id: None,
    };
    if let Err(e) = call_blocking(app_state.db.clone(), move |db| db.store_message(&bot_msg)).await
    {
        warn!(error = %e, "failed to store chat session bootstrap message");
    }
}

async fn api_chat_sessions_create(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<CreateChatSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    let cid = chat_id;
    let persona_id = if let Some(pid) = body.persona_id {
        pid
    } else {
        call_blocking(state.app_state.db.clone(), move |db| {
            db.get_current_persona_id(cid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let source_topic_id = trimmed_cursor_id(body.source_topic_id.as_deref())
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let mut intent = body.intent.trim().to_string();
    let mut mirror_main_chat = body.mirror_main_chat.unwrap_or(false);
    let mut seed_context: Option<String> = None;
    if let Some(topic_id) = source_topic_id.clone() {
        let seeded = load_topic_session_seed(&state, chat_id, persona_id, &topic_id).await?;
        if intent.is_empty() {
            intent = seeded.intent;
        }
        mirror_main_chat = false;
        seed_context = Some(seeded.seed);
    }
    if intent.is_empty() || intent.len() > 500 {
        return Err((
            StatusCode::BAD_REQUEST,
            "intent must be 1-500 characters".into(),
        ));
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let title = intent.chars().take(60).collect::<String>();
    // ttl_hours is accepted for compatibility; session TTL auto-archive is deprecated.
    let ttl_hours = body.ttl_hours.unwrap_or(0).max(0);

    let sid = session_id.clone();
    let t = title.clone();
    let i = intent.clone();
    let seed = seed_context.clone();
    let topic_for_row = source_topic_id.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.create_chat_session(
            &sid,
            chat_id,
            persona_id,
            &t,
            &i,
            ttl_hours,
            mirror_main_chat,
            seed.as_deref(),
            topic_for_row.as_deref(),
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let created = call_blocking(state.app_state.db.clone(), {
        let session_id = session_id.clone();
        move |db| db.get_chat_session(&session_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "session not found".into(),
    ))?;

    let app_state = state.app_state.clone();
    let bootstrap_sid = session_id.clone();
    let bootstrap_intent = intent.clone();
    tokio::spawn(async move {
        bootstrap_chat_session_context(
            app_state,
            bootstrap_sid,
            chat_id,
            persona_id,
            bootstrap_intent,
        )
        .await;
    });

    Ok(Json(serde_json::json!({
        "ok": true,
        "session": chat_session_json(&created),
        "session_id": session_id,
        "bootstrap_pending": true,
    })))
}

async fn api_chat_sessions_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let session = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_chat_session(&session_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match session {
        Some(s) => {
            let mut value = chat_session_json(&s);
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "bootstrap_context_json".into(),
                    serde_json::json!(s.bootstrap_context_json),
                );
            }
            Ok(Json(value))
        }
        None => Err((StatusCode::NOT_FOUND, "session not found".into())),
    }
}

#[derive(Debug, Deserialize)]
struct PatchChatSessionRequest {
    title: Option<String>,
    status: Option<String>,
    ttl_hours: Option<i64>,
    mirror_main_chat: Option<bool>,
}

async fn api_chat_sessions_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(session_id): Path<String>,
    Json(body): Json<PatchChatSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    if let Some(ref status) = body.status {
        match status.as_str() {
            "archived" => {
                return Err((
                    StatusCode::GONE,
                    "session archive is deprecated; delete the session instead".into(),
                ));
            }
            "active" => {
                // no-op: archive/TTL restore is deprecated; sessions stay active
            }
            _ => {
                return Err((StatusCode::BAD_REQUEST, "status must be 'active'".into()));
            }
        }
    }

    if let Some(ref title) = body.title {
        let sid = session_id.clone();
        let t = title.clone();
        call_blocking(state.app_state.db.clone(), move |db| {
            db.update_chat_session_title(&sid, &t)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(ttl) = body.ttl_hours {
        let sid = session_id.clone();
        let ttl_val = ttl.max(0);
        call_blocking(state.app_state.db.clone(), move |db| {
            db.update_chat_session_ttl(&sid, ttl_val)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(mirror_main_chat) = body.mirror_main_chat {
        let sid = session_id.clone();
        call_blocking(state.app_state.db.clone(), move |db| {
            db.update_chat_session_mirror_main_chat(&sid, mirror_main_chat)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_chat_sessions_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_chat_session(&session_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if deleted {
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err((StatusCode::NOT_FOUND, "session not found".into()))
    }
}

// --- end Chat Sessions ---

async fn api_skills_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<SkillsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let persona_skill_policy = if let Some(pid) = query.persona_id {
        let exists = call_blocking(state.app_state.db.clone(), move |db| {
            db.persona_exists(chat_id, pid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !exists {
            return Err((StatusCode::NOT_FOUND, "persona not found".into()));
        }
        Some(
            call_blocking(state.app_state.db.clone(), move |db| {
                db.get_persona_hook_skill_policy(chat_id, pid)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        )
    } else {
        None
    };

    let skill_manager = &state.app_state.skills;
    let all_skills = skill_manager.discover_all_skills();
    let mut remote_count = 0usize;
    let rows: Vec<serde_json::Value> = all_skills
        .into_iter()
        .map(|s| {
            let remote = skill_manager.skill_is_remote(&s);
            if remote {
                remote_count += 1;
            }
            let allowed_for_persona = persona_skill_policy.as_ref().map(|policy| {
                let Some(row) = policy else {
                    return true;
                };
                let Some(allowed) = row.allowed_skill_names.as_ref() else {
                    return true;
                };
                allowed.iter().any(|a| a.eq_ignore_ascii_case(&s.name))
            });
            json!({
                "name": s.name,
                "description": s.description,
                "when_to_use": s.when_to_use,
                "platforms": s.platforms,
                "deps": s.deps,
                "source": s.source,
                "version": s.version,
                "updated_at": s.updated_at,
                "remote": remote,
                "allowed_for_persona": allowed_for_persona,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "skills": rows,
        "total": rows.len(),
        "remote_count": remote_count,
    })))
}

fn hook_definition_to_json(
    h: crate::db::HookDefinitionRecord,
    persona_status: Option<(bool, bool, bool)>,
) -> serde_json::Value {
    let payload = serde_json::from_str::<serde_json::Value>(&h.action_payload_json)
        .unwrap_or_else(|_| json!({}));
    let mut row = json!({
        "id": h.id,
        "name": h.name,
        "event_name": h.event_name,
        "matcher": h.matcher,
        "action_type": h.action_type,
        "action_payload_json": h.action_payload_json,
        "action_payload": payload,
        "scoped_persona_ids": h.scoped_persona_ids,
        "is_global": h.scoped_persona_ids.is_none(),
        "enabled": h.enabled,
        "updated_at": h.updated_at,
    });
    if let Some((scoped_for_persona, allowed_for_persona, active_for_persona)) = persona_status {
        if let Some(obj) = row.as_object_mut() {
            obj.insert("scoped_for_persona".into(), json!(scoped_for_persona));
            obj.insert("allowed_for_persona".into(), json!(allowed_for_persona));
            obj.insert("active_for_persona".into(), json!(active_for_persona));
        }
    }
    row
}

async fn api_hooks_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<HooksQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let persona_id = query.persona_id;
    let chat_id = if persona_id.is_some() {
        let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
        ensure_web_binding_for_universal(&state, chat_id).await?;
        if let Some(pid) = persona_id {
            let exists = call_blocking(state.app_state.db.clone(), move |db| {
                db.persona_exists(chat_id, pid)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if !exists {
                return Err((StatusCode::NOT_FOUND, "persona not found".into()));
            }
        }
        Some(chat_id)
    } else {
        None
    };

    let db = state.app_state.db.clone();
    let hooks_json = call_blocking(db, move |db| {
        let hooks = db.list_hook_definitions()?;
        let rows = if let (Some(chat_id), Some(pid)) = (chat_id, persona_id) {
            hooks
                .into_iter()
                .map(|h| {
                    let status = h.persona_status(db, chat_id, pid)?;
                    Ok(hook_definition_to_json(h, Some(status)))
                })
                .collect::<Result<Vec<_>, crate::error::FinallyAValueBotError>>()?
        } else {
            hooks
                .into_iter()
                .map(|h| hook_definition_to_json(h, None))
                .collect()
        };
        Ok::<Vec<serde_json::Value>, crate::error::FinallyAValueBotError>(rows)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "ok": true, "hooks": hooks_json })))
}

async fn api_hooks_post(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<HookDefinitionUpsertBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let payload_json = body
        .action_payload_json
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("{}")
        .to_string();
    if body.action_type.trim().eq_ignore_ascii_case("command") {
        validate_command_payload(&state.app_state.config, &payload_json)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    let enabled = body.enabled.unwrap_or(true);
    let scope_raw = body.scope.as_deref().map(str::trim).unwrap_or_default();
    let scope_normalized = if scope_raw.is_empty() {
        None
    } else if scope_raw.eq_ignore_ascii_case("global") {
        Some("global")
    } else if scope_raw.eq_ignore_ascii_case("persona") {
        Some("persona")
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "scope must be 'global' or 'persona' when provided".to_string(),
        ));
    };
    let scoped_from_body = body
        .scoped_persona_ids
        .as_ref()
        .map(|ids| normalize_persona_scope_ids(ids));
    let scoped_persona_ids: Option<Vec<i64>> = if let Some(scope) = scope_normalized {
        match scope {
            "global" => None,
            "persona" => match scoped_from_body {
                Some(ids) => Some(ids),
                None => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "scope=persona requires scoped_persona_ids".to_string(),
                    ));
                }
            },
            _ => None,
        }
    } else if body.id.is_some() {
        if let Some(ids) = scoped_from_body {
            Some(ids)
        } else {
            let existing_id = body.id.unwrap_or_default();
            let existing = call_blocking(state.app_state.db.clone(), move |db| {
                db.get_hook_definition(existing_id)
            })
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            let Some(existing) = existing else {
                return Err((StatusCode::NOT_FOUND, "hook not found".to_string()));
            };
            existing.scoped_persona_ids
        }
    } else if let Some(ids) = scoped_from_body {
        Some(ids)
    } else if let Some(creating_persona_id) = body.creating_persona_id {
        if creating_persona_id <= 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                "creating_persona_id must be positive".to_string(),
            ));
        }
        let exists = call_blocking(state.app_state.db.clone(), move |db| {
            db.persona_exists(chat_id, creating_persona_id)
        })
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if !exists {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("creating_persona_id {} not found", creating_persona_id),
            ));
        }
        Some(vec![creating_persona_id])
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "persona scope required: provide scoped_persona_ids or creating_persona_id, or set scope=global"
                .to_string(),
        ));
    };
    let id = call_blocking(state.app_state.db.clone(), move |db| {
        db.upsert_hook_definition(
            body.id,
            &body.name,
            &body.event_name,
            body.matcher.as_deref(),
            &body.action_type,
            &payload_json,
            scoped_persona_ids.as_deref(),
            enabled,
        )
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

async fn api_hooks_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<HookDefinitionDeletePathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_hook_definition(path.id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "deleted": deleted })))
}

async fn api_persona_policy_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaPolicyPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let policy = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_hook_skill_policy(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": path.persona_id,
        "allowed_hook_ids": policy.as_ref().and_then(|p| p.allowed_hook_ids.clone()),
        "allowed_skill_names": policy.as_ref().and_then(|p| p.allowed_skill_names.clone()),
        "uses_default_hooks": policy.as_ref().and_then(|p| p.allowed_hook_ids.as_ref()).is_none(),
        "uses_default_skills": policy.as_ref().and_then(|p| p.allowed_skill_names.as_ref()).is_none(),
        "updated_at": policy.as_ref().map(|p| p.updated_at.clone()),
    })))
}

async fn api_persona_policy_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaPolicyPathParams>,
    Json(body): Json<PersonaPolicyPatchBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let current = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_hook_skill_policy(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut next_hooks = current.as_ref().and_then(|p| p.allowed_hook_ids.clone());
    let mut next_skills = current.as_ref().and_then(|p| p.allowed_skill_names.clone());
    if let Some(v) = body.allowed_hook_ids {
        next_hooks = v;
    }
    if let Some(v) = body.allowed_skill_names {
        next_skills = v;
    }
    if let Some(list) = next_hooks.as_mut() {
        list.retain(|id| *id > 0);
        list.sort_unstable();
        list.dedup();
    }
    if let Some(list) = next_skills.as_mut() {
        let mut norm = Vec::new();
        for item in list.iter() {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                norm.push(trimmed.to_string());
            }
        }
        norm.sort_by_key(|s| s.to_ascii_lowercase());
        norm.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        *list = norm;
    }

    let next_hooks_for_db = next_hooks.clone();
    let next_skills_for_db = next_skills.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.set_persona_hook_skill_policy(
            chat_id,
            pid,
            next_hooks_for_db.as_deref(),
            next_skills_for_db.as_deref(),
        )
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "persona_id": path.persona_id,
        "allowed_hook_ids": next_hooks,
        "allowed_skill_names": next_skills,
    })))
}

async fn api_persona_bulletin_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let pid2 = path.persona_id;
    let focus = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_bulletin_focus(chat_id, pid2)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pid3 = path.persona_id;
    let bookmarks = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_persona_message_bookmarks(chat_id, pid3, 20)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let focus_json = focus.map(|f| {
        json!({
            "title": f.title,
            "content": f.content,
            "updated_at": f.updated_at,
        })
    });
    let bookmarks_json: Vec<serde_json::Value> = bookmarks
        .into_iter()
        .map(|b| bookmark_to_json(&b))
        .collect();

    let pid4 = path.persona_id;
    let persona = call_blocking(state.app_state.db.clone(), move |db| {
        let p = db.get_persona(pid4)?;
        Ok(p.filter(|x| x.chat_id == chat_id))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cfg = &state.app_state.config;
    let def_u = cfg.recent_history_min_user_messages.clamp(1, 25) as i64;
    let def_a = cfg.recent_history_min_assistant_messages.clamp(1, 25) as i64;
    let global_engine = state.app_state.runtime_toggles.agent_engine();
    let (hs_json, delivery_json, engine_json) = if let Some(ref p) = persona {
        let o_u = p.recent_history_min_user;
        let o_a = p.recent_history_min_assistant;
        let e_u = o_u.unwrap_or(def_u).clamp(1, 25);
        let e_a = o_a.unwrap_or(def_a).clamp(1, 25);
        let hs = json!({
            "min_user": {
                "effective": e_u,
                "persona_override": o_u,
                "uses_default": o_u.is_none(),
            },
            "min_assistant": {
                "effective": e_a,
                "persona_override": o_a,
                "uses_default": o_a.is_none(),
            },
            "defaults": { "min_user": def_u, "min_assistant": def_a },
        });
        let msg_max = crate::dense_delivery_guard::effective_max_chars(p, "wecom");
        let web_max = crate::dense_delivery_guard::effective_max_chars(p, "web");
        let delivery = json!({
            "enabled": p.dense_delivery_enabled,
            "messaging_max_chars": msg_max,
            "web_max_chars": web_max,
            "summary_chars": crate::dense_delivery_guard::effective_summary_chars(p),
            "defaults": {
                "messaging_max_chars": crate::dense_delivery_guard::DEFAULT_MESSAGING_MAX_CHARS,
                "web_max_chars": crate::dense_delivery_guard::DEFAULT_WEB_MAX_CHARS,
                "summary_chars": crate::dense_delivery_guard::DEFAULT_SUMMARY_CHARS,
            },
        });
        let effective = crate::runtime_toggles::resolve_run_agent_engine(
            p.agent_engine_override.as_deref(),
            global_engine,
        );
        let engine = json!({
            "override": p.agent_engine_override,
            "global": global_engine.as_str(),
            "effective": effective.as_str(),
            "uses_default": p.agent_engine_override.as_deref().map(str::trim).unwrap_or("").is_empty(),
        });
        (hs, delivery, engine)
    } else {
        (json!({}), json!({}), json!({}))
    };

    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "focus": focus_json,
        "bookmarks": bookmarks_json,
        "history_suffix": hs_json,
        "operator_memo": persona.as_ref().and_then(|p| p.operator_memo.clone()),
        "dense_delivery_enabled": persona.as_ref().map(|p| p.dense_delivery_enabled).unwrap_or(false),
        "user_message_rewrite_enabled": persona.as_ref().map(|p| p.user_message_rewrite_enabled).unwrap_or(false),
        "dense_delivery": delivery_json,
        "agent_engine_override": persona.as_ref().and_then(|p| p.agent_engine_override.clone()),
        "agent_engine_global": global_engine.as_str(),
        "agent_engine_effective": crate::runtime_toggles::resolve_run_agent_engine(
            persona.as_ref().and_then(|p| p.agent_engine_override.as_deref()),
            global_engine,
        ).as_str(),
        "agent_engine": engine_json,
    })))
}

async fn api_persona_bulletin_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
    Json(body): Json<PersonaBulletinPatchBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    if body.recent_history_min_user.is_none()
        && body.recent_history_min_assistant.is_none()
        && body.operator_memo.is_none()
        && body.dense_delivery_enabled.is_none()
        && body.user_message_rewrite_enabled.is_none()
        && body.dense_delivery_messaging_max_chars.is_none()
        && body.dense_delivery_web_max_chars.is_none()
        && body.dense_delivery_summary_chars.is_none()
        && body.agent_engine_override.is_none()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "no fields to update (send recent_history_min_user, recent_history_min_assistant, operator_memo, dense_delivery_enabled, user_message_rewrite_enabled, and/or agent_engine_override)"
                .into(),
        ));
    }

    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let pid2 = path.persona_id;
    let current = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid2))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "persona not found".into()))?;
    if current.chat_id != chat_id {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let patch_prompt = body.recent_history_min_user.is_some()
        || body.recent_history_min_assistant.is_some()
        || body.operator_memo.is_some();
    let mut nu = current.recent_history_min_user;
    let mut na = current.recent_history_min_assistant;
    let mut memo = current.operator_memo.clone();

    if let Some(v) = body.recent_history_min_user {
        if let Some(n) = v {
            if !(1..=25).contains(&n) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "recent_history_min_user must be between 1 and 25".into(),
                ));
            }
            nu = Some(n);
        } else {
            nu = None;
        }
    }
    if let Some(v) = body.recent_history_min_assistant {
        if let Some(n) = v {
            if !(1..=25).contains(&n) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "recent_history_min_assistant must be between 1 and 25".into(),
                ));
            }
            na = Some(n);
        } else {
            na = None;
        }
    }
    if let Some(v) = body.operator_memo {
        memo = match v {
            None => None,
            Some(s) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else if t.chars().count() > crate::db::OPERATOR_MEMO_MAX_CHARS {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!(
                            "operator_memo exceeds {} characters",
                            crate::db::OPERATOR_MEMO_MAX_CHARS
                        ),
                    ));
                } else {
                    Some(s)
                }
            }
        };
    }

    let mut dense_enabled = current.dense_delivery_enabled;
    let mut msg_max = current.dense_delivery_messaging_max_chars;
    let mut web_max = current.dense_delivery_web_max_chars;
    let mut summary_chars = current.dense_delivery_summary_chars;
    let mut engine_ov = current.agent_engine_override.clone();
    let mut patch_delivery = false;
    let mut patch_engine = false;
    let mut patch_rewrite = false;
    let mut rewrite_enabled = current.user_message_rewrite_enabled;

    if let Some(v) = body.dense_delivery_enabled {
        dense_enabled = v;
        patch_delivery = true;
    }
    if let Some(v) = body.user_message_rewrite_enabled {
        rewrite_enabled = v;
        patch_rewrite = true;
    }
    if let Some(v) = body.dense_delivery_messaging_max_chars {
        msg_max = v.filter(|n| *n > 0);
        patch_delivery = true;
    }
    if let Some(v) = body.dense_delivery_web_max_chars {
        web_max = v.filter(|n| *n > 0);
        patch_delivery = true;
    }
    if let Some(v) = body.dense_delivery_summary_chars {
        summary_chars = v.filter(|n| *n > 0);
        patch_delivery = true;
    }
    if let Some(v) = body.agent_engine_override {
        patch_engine = true;
        engine_ov = match v {
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "agent_engine_override cannot be null; set classic, classic_cost_routing, cursor, or gemini_adk".into(),
                ));
            }
            Some(s) => {
                let t = s.trim();
                if t.is_empty() || crate::runtime_toggles::AgentEngine::parse_override(t).is_none()
                {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "agent_engine_override must be classic, classic_cost_routing, cursor, or gemini_adk".into(),
                    ));
                } else {
                    Some(t.to_string())
                }
            }
        };
    }

    let ok = call_blocking(state.app_state.db.clone(), move |db| {
        if patch_prompt {
            db.set_persona_prompt_overrides(chat_id, pid, nu, na, memo.as_deref())?;
        }
        if patch_delivery {
            db.set_persona_delivery_policy(
                chat_id,
                pid,
                dense_enabled,
                msg_max,
                web_max,
                summary_chars,
            )?;
        }
        if patch_rewrite {
            db.set_persona_user_message_rewrite(chat_id, pid, rewrite_enabled)?;
        }
        if patch_engine {
            db.set_persona_engine_override(chat_id, pid, engine_ov.as_deref())?;
        }
        Ok(true)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !ok {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update persona".into(),
        ));
    }

    Ok(Json(json!({ "ok": true, "persona_id": path.persona_id })))
}

async fn api_persona_bookmarks_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let pid2 = path.persona_id;
    let bookmarks = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_persona_message_bookmarks(chat_id, pid2, 50)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let bookmarks_json: Vec<serde_json::Value> = bookmarks
        .into_iter()
        .map(|b| bookmark_to_json(&b))
        .collect();
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "bookmarks": bookmarks_json,
    })))
}

async fn api_persona_bookmarks_post(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
    Json(body): Json<PersonaBookmarkUpsertBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    if body.message_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let message_id = body.message_id.trim().to_string();
    let pid_lookup = path.persona_id;
    let message = call_blocking(state.app_state.db.clone(), {
        let message_id = message_id.clone();
        move |db| db.get_message_for_persona(chat_id, pid_lookup, &message_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(message) = message else {
        return Err((
            StatusCode::NOT_FOUND,
            "message not found for persona".into(),
        ));
    };
    let role = if message.is_from_bot {
        "assistant"
    } else {
        "user"
    };
    let preview = truncate_chars(message.content.trim(), 480);
    let note_clean = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_chars(s, 180));
    let pid_upsert = path.persona_id;
    call_blocking(state.app_state.db.clone(), {
        let note_clean = note_clean.clone();
        let message_id = message_id.clone();
        let preview = preview.clone();
        move |db| {
            db.upsert_persona_message_bookmark(
                chat_id,
                pid_upsert,
                &message_id,
                role,
                &preview,
                note_clean.as_deref(),
            )
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "bookmark": {
            "message_id": message_id,
            "role": role,
            "content_preview": preview,
            "note": note_clean,
            "session_id": message.session_id,
            "message_timestamp": message.timestamp,
            "missing": false,
        }
    })))
}

async fn api_persona_bookmarks_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkDeletePathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let message_id = path.message_id.trim().to_string();
    if message_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }
    let pid_delete = path.persona_id;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_persona_message_bookmark(chat_id, pid_delete, &message_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "message_id": path.message_id,
        "deleted": deleted,
    })))
}

fn topic_scope_from_query(
    chat_id: i64,
    persona_id: i64,
    session_id: Option<&str>,
) -> crate::db::TopicScope {
    crate::db::TopicScope {
        chat_id,
        persona_id,
        session_id: crate::db::normalize_topic_session(session_id),
    }
}

fn topic_user_error(err: crate::error::FinallyAValueBotError) -> (StatusCode, String) {
    let msg = err.to_string();
    if msg.contains("not found") {
        (StatusCode::NOT_FOUND, msg)
    } else if msg.contains("topic")
        || msg.contains("section")
        || msg.contains("split")
        || msg.contains("merge")
    {
        (StatusCode::BAD_REQUEST, msg)
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
}

fn side_chat_in_topic(
    topic: &crate::db::ConversationTopic,
    branch: &crate::db::SideChatBranch,
) -> bool {
    side_chat_in_dossier(topic, branch, false)
}

/// Open topics keep branches that start after the topic, including the unsegmented tail.
fn side_chat_in_dossier(
    topic: &crate::db::ConversationTopic,
    branch: &crate::db::SideChatBranch,
    open_tail: bool,
) -> bool {
    let same_session = match topic.session_id.as_deref() {
        Some(session_id) => branch.session_id == session_id,
        None => branch.session_id.trim().is_empty(),
    };
    if !same_session {
        return false;
    }
    match branch.anchor_timestamp.as_deref() {
        Some(ts) => {
            if ts < topic.start_ts.as_str() {
                return false;
            }
            open_tail || ts <= topic.end_ts.as_str()
        }
        None => {
            branch.anchor_message_id == topic.start_message_id
                || branch.anchor_message_id == topic.end_message_id
        }
    }
}

fn topic_to_json(
    topic: &crate::db::ConversationTopic,
    branches: &[&crate::db::SideChatBranch],
) -> serde_json::Value {
    json!({
        "id": topic.id,
        "session_id": topic.session_id,
        "parent_id": topic.parent_id,
        "continues_topic_id": topic.continues_topic_id,
        "title": topic.title,
        "summary": topic.summary,
        "start_message_id": topic.start_message_id,
        "end_message_id": topic.end_message_id,
        "start_ts": topic.start_ts,
        "end_ts": topic.end_ts,
        "message_count": topic.message_count,
        "status": topic.status,
        "source": topic.source,
        "pinned": topic.pinned,
        "created_at": topic.created_at,
        "updated_at": topic.updated_at,
        "side_chats": branches.iter().map(|branch| json!({
            "id": branch.id,
            "title": branch.title,
            "anchor_message_id": branch.anchor_message_id,
            "turn_count": branch.turn_count,
        })).collect::<Vec<_>>(),
    })
}

async fn ensure_persona(
    state: &WebState,
    headers: &HeaderMap,
    persona_id: i64,
) -> Result<i64, (StatusCode, String)> {
    require_auth(headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(state, chat_id).await?;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, persona_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    Ok(chat_id)
}

async fn api_persona_topics_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatPathParams>,
    Query(query): Query<TopicsListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let scope = topic_scope_from_query(chat_id, path.persona_id, query.session_id.as_deref());
    let scope_for_topics = scope.clone();
    let topics = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_conversation_topics(&scope_for_topics)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pid = path.persona_id;
    let branches = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_side_chat_branches(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let scope_for_count = scope.clone();
    let unsegmented = call_blocking(state.app_state.db.clone(), move |db| {
        let after = db
            .list_conversation_topics(&scope_for_count)?
            .into_iter()
            .last();
        match after {
            Some(topic) => db.count_messages_in_topic_scope(
                &scope_for_count,
                Some((topic.end_ts.as_str(), topic.end_message_id.as_str())),
            ),
            None => db.count_messages_in_topic_scope(&scope_for_count, None),
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let in_flight = crate::topics::scope_in_flight(&scope);
    if unsegmented > 0 && !in_flight {
        let app = state.app_state.clone();
        let backfill = scope.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::topics::segment_scope(&app, &backfill, crate::topics::TopicTrigger::Lazy)
                    .await
            {
                tracing::warn!(
                    chat_id = backfill.chat_id,
                    persona_id = backfill.persona_id,
                    "topic backfill failed: {e}"
                );
            }
        });
    }
    let mut grouped: Vec<Vec<&crate::db::SideChatBranch>> = vec![Vec::new(); topics.len()];
    for branch in &branches {
        if let Some(index) = topics
            .iter()
            .enumerate()
            .rev()
            .find(|(_, topic)| side_chat_in_topic(topic, branch))
            .map(|(index, _)| index)
        {
            grouped[index].push(branch);
        }
    }
    let items: Vec<serde_json::Value> = topics
        .iter()
        .zip(grouped.iter())
        .map(|(topic, branches)| topic_to_json(topic, branches))
        .collect();
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "session_id": scope.session_id,
        "segmenting": in_flight || unsegmented > 0,
        "unsegmented_count": unsegmented,
        "topics": items,
    })))
}

async fn api_persona_topic_dossier(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TopicIdPathParams>,
    Query(query): Query<TopicDossierQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let started = Instant::now();
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let topic_id = path.topic_id.trim().to_string();
    if topic_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "topic_id is required".into()));
    }
    let limit = query
        .limit
        .unwrap_or(crate::db::TOPIC_DOSSIER_DEFAULT_LIMIT)
        .clamp(1, crate::db::TOPIC_DOSSIER_MAX_LIMIT);
    let pid = path.persona_id;
    let topic_lookup = topic_id.clone();
    let topic = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_conversation_topic(chat_id, pid, &topic_lookup)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(topic) = topic else {
        return Err((StatusCode::NOT_FOUND, "topic not found".into()));
    };
    if query.session_id.is_some() {
        let requested = crate::db::normalize_topic_session(query.session_id.as_deref());
        if requested != topic.session_id {
            return Err((StatusCode::NOT_FOUND, "topic not found".into()));
        }
    }
    let page = dossier_page_request(&query)?;
    let open_tail = topic.status == "open";
    let pid = path.persona_id;
    let branches = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_side_chat_branches(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let topic_for_window = topic.clone();
    let window = call_blocking(state.app_state.db.clone(), move |db| {
        db.topic_dossier_window(&topic_for_window, limit, page)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(window) = window else {
        return Err((StatusCode::NOT_FOUND, "message is not in this topic".into()));
    };
    let root = state.app_state.config.workspace_root_absolute();
    let artifact_keys: Vec<(i64, i64, String)> = window
        .messages
        .iter()
        .map(|message| (message.chat_id, message.persona_id, message.id.clone()))
        .collect();
    let artifacts = tokio::task::spawn_blocking(move || {
        let catalog = crate::artifacts::Catalog::open(&root).ok();
        let Some(catalog) = catalog else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (chat_id, persona_id, message_id) in &artifact_keys {
            if out.len() >= crate::db::TOPIC_DOSSIER_ARTIFACT_LIMIT {
                break;
            }
            let rows = catalog
                .message_artifacts(*chat_id, *persona_id, message_id)
                .unwrap_or_default();
            for artifact in rows {
                if artifact.status == "deleted" || !seen.insert(artifact.id.clone()) {
                    continue;
                }
                out.push(json!({
                    "id": artifact.id,
                    "name": artifact.name,
                    "kind": artifact.kind,
                    "size_bytes": artifact.size_bytes,
                    "status": artifact.status,
                    "availability": artifact.availability,
                    "message_id": artifact.message_id,
                    "url": artifact.url,
                    "preview_url": artifact.preview_url,
                }));
                if out.len() >= crate::db::TOPIC_DOSSIER_ARTIFACT_LIMIT {
                    break;
                }
            }
        }
        out
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let messages: Vec<HistoryItem> = window
        .messages
        .iter()
        .map(|message| HistoryItem {
            artifacts: Vec::new(),
            id: message.id.clone(),
            sender_name: message.sender_name.clone(),
            content: crate::agent_turn_context::strip_stored_dialogue_markup(&message.content),
            is_from_bot: message.is_from_bot,
            timestamp: message.timestamp.clone(),
            origin: message.origin.clone(),
            speaker_persona_id: message.speaker_persona_id,
        })
        .collect();
    let older_before_id = if window.has_older {
        messages.first().map(|message| message.id.clone())
    } else {
        None
    };
    let newer_after_id = if window.has_newer {
        messages.last().map(|message| message.id.clone())
    } else {
        None
    };
    let branch_refs: Vec<&crate::db::SideChatBranch> = branches
        .iter()
        .filter(|branch| side_chat_in_dossier(&topic, branch, open_tail))
        .collect();
    let bookmarks: Vec<serde_json::Value> = window
        .bookmarks
        .iter()
        .map(|bookmark| {
            json!({
                "message_id": bookmark.message_id,
                "role": bookmark.role,
                "content_preview": bookmark.content_preview,
                "note": bookmark.note,
                "created_at": bookmark.created_at,
                "updated_at": bookmark.updated_at,
                "session_id": bookmark.session_id,
                "message_timestamp": bookmark.message_timestamp,
                "missing": bookmark.missing,
            })
        })
        .collect();
    let scope = topic_scope_from_query(chat_id, path.persona_id, topic.session_id.as_deref());
    let segmenting = crate::topics::scope_in_flight(&scope);
    info!(
        target: "web",
        endpoint = "/api/personas/:persona_id/topics/:topic_id/dossier",
        chat_id,
        persona_id = path.persona_id,
        messages = messages.len(),
        bookmarks = bookmarks.len(),
        artifacts = artifacts.len(),
        side_chats = branch_refs.len(),
        has_older = window.has_older,
        has_newer = window.has_newer,
        duration_ms = started.elapsed().as_millis(),
        "topic dossier fetched"
    );
    Ok(Json(json!({
        "ok": true,
        "segmenting": segmenting,
        "topic": topic_to_json(&topic, &branch_refs),
        "messages": messages,
        "has_older": window.has_older,
        "has_newer": window.has_newer,
        "bookmarks": bookmarks,
        "artifacts": artifacts,
        "evidence": {
            "start_message_id": topic.start_message_id,
            "older_before_id": older_before_id,
            "newer_after_id": newer_after_id,
        },
    })))
}

async fn api_persona_topics_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TopicIdPathParams>,
    Json(body): Json<TopicPatchBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    if body.title.is_none() && body.pinned.is_none() {
        return Err((StatusCode::BAD_REQUEST, "topic patch is empty".into()));
    }
    let topic_id = path.topic_id.clone();
    let title = body.title.clone();
    let pinned = body.pinned;
    let pid = path.persona_id;
    let updated = call_blocking(state.app_state.db.clone(), move |db| {
        db.patch_conversation_topic(chat_id, pid, &topic_id, title.as_deref(), pinned)
    })
    .await
    .map_err(topic_user_error)?;
    let Some(topic) = updated else {
        return Err((StatusCode::NOT_FOUND, "topic not found".into()));
    };
    Ok(Json(json!({
        "ok": true,
        "topic": topic_to_json(&topic, &[]),
    })))
}

async fn api_persona_topics_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TopicIdPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let topic_id = path.topic_id.clone();
    let pid = path.persona_id;
    let deleted_messages = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_conversation_topic(chat_id, pid, &topic_id)
    })
    .await
    .map_err(topic_user_error)?;
    Ok(Json(json!({
        "ok": true,
        "deleted_messages": deleted_messages,
    })))
}

async fn api_persona_topics_section(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TopicIdPathParams>,
    Query(query): Query<TopicsListQuery>,
    Json(body): Json<TopicSectionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "section title is empty".into()));
    }
    let scope = topic_scope_from_query(chat_id, path.persona_id, query.session_id.as_deref());
    let topic_id = path.topic_id.clone();
    let unsegmented = call_blocking(state.app_state.db.clone(), move |db| {
        db.start_user_section(&scope, &topic_id, &title)
    })
    .await
    .map_err(topic_user_error)?;
    if unsegmented > 0 {
        let app = state.app_state.clone();
        let scope = topic_scope_from_query(chat_id, path.persona_id, query.session_id.as_deref());
        tokio::spawn(async move {
            if let Err(e) =
                crate::topics::segment_scope(&app, &scope, crate::topics::TopicTrigger::Manual)
                    .await
            {
                tracing::warn!(
                    chat_id = scope.chat_id,
                    persona_id = scope.persona_id,
                    "new section segmentation failed: {e}"
                );
            }
        });
    }
    Ok(Json(json!({
        "ok": true,
        "pending": unsegmented == 0,
        "unsegmented": unsegmented,
    })))
}

async fn api_persona_topics_merge(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TopicIdPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let topic_id = path.topic_id.clone();
    let pid = path.persona_id;
    let topic = call_blocking(state.app_state.db.clone(), move |db| {
        db.merge_conversation_topic_with_previous(chat_id, pid, &topic_id)
    })
    .await
    .map_err(topic_user_error)?;
    Ok(Json(json!({
        "ok": true,
        "topic": topic_to_json(&topic, &[]),
    })))
}

async fn api_persona_topics_split(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatPathParams>,
    Query(query): Query<TopicsListQuery>,
    Json(body): Json<TopicSplitBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let message_id = body.message_id.trim().to_string();
    if message_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }
    let scope = topic_scope_from_query(chat_id, path.persona_id, query.session_id.as_deref());
    let (earlier, later) = call_blocking(state.app_state.db.clone(), move |db| {
        db.split_conversation_topic(&scope, &message_id)
    })
    .await
    .map_err(topic_user_error)?;
    Ok(Json(json!({
        "ok": true,
        "earlier": topic_to_json(&earlier, &[]),
        "later": topic_to_json(&later, &[]),
    })))
}

async fn api_persona_topics_refresh(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatPathParams>,
    Query(query): Query<TopicsListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chat_id = ensure_persona(&state, &headers, path.persona_id).await?;
    let scope = topic_scope_from_query(chat_id, path.persona_id, query.session_id.as_deref());
    if crate::topics::scope_in_flight(&scope) {
        return Ok(Json(
            json!({ "ok": true, "started": false, "reason": "in_flight" }),
        ));
    }
    let app = state.app_state.clone();
    tokio::spawn(async move {
        match crate::topics::segment_scope(&app, &scope, crate::topics::TopicTrigger::Manual).await
        {
            Ok(status) => {
                tracing::info!(
                    chat_id = scope.chat_id,
                    persona_id = scope.persona_id,
                    "topic refresh finished: {status:?}"
                );
            }
            Err(e) => {
                tracing::warn!(
                    chat_id = scope.chat_id,
                    persona_id = scope.persona_id,
                    "topic refresh failed: {e}"
                );
            }
        }
    });
    Ok(Json(json!({ "ok": true, "started": true })))
}

fn side_chat_to_json(sc: &crate::db::SideChat) -> serde_json::Value {
    json!({
        "id": sc.id,
        "session_id": if sc.session_id.is_empty() { serde_json::Value::Null } else { json!(sc.session_id) },
        "anchor_message_id": sc.anchor_message_id,
        "title": sc.title,
        "anchor_snippet": sc.anchor_snippet,
        "draft_text": sc.draft_text,
        "turn_count": sc.turn_count,
        "created_at": sc.created_at,
        "updated_at": sc.updated_at,
    })
}

async fn api_persona_side_chats_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let pid2 = path.persona_id;
    let items = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_side_chats(chat_id, pid2, 100)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "side_chats": items.iter().map(side_chat_to_json).collect::<Vec<_>>(),
    })))
}

async fn api_persona_side_chats_ensure(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatPathParams>,
    Json(body): Json<SideChatEnsureBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let anchor_message_id = body.anchor_message_id.trim().to_string();
    if anchor_message_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "anchor_message_id is required".into(),
        ));
    }
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let pid_lookup = path.persona_id;
    let message = call_blocking(state.app_state.db.clone(), {
        let anchor_message_id = anchor_message_id.clone();
        move |db| db.get_message_for_persona(chat_id, pid_lookup, &anchor_message_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(message) = message else {
        return Err((StatusCode::NOT_FOUND, "anchor message not found".into()));
    };
    if !message.is_from_bot {
        return Err((
            StatusCode::BAD_REQUEST,
            "anchor must be a bot response".into(),
        ));
    }
    let snippet = body
        .anchor_snippet
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| truncate_chars(message.content.trim(), 280));
    let session_id = body
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| message.session_id.clone());
    let pid_ensure = path.persona_id;
    let sc = call_blocking(state.app_state.db.clone(), move |db| {
        db.ensure_side_chat(
            chat_id,
            pid_ensure,
            session_id.as_deref(),
            &anchor_message_id,
            &snippet,
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let turns = call_blocking(state.app_state.db.clone(), {
        let id = sc.id.clone();
        move |db| db.list_side_chat_turns(&id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let turns_json: Vec<serde_json::Value> = turns
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "role": t.role,
                "content": t.content,
                "created_at": t.created_at,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "side_chat": side_chat_to_json(&sc),
        "turns": turns_json,
    })))
}

async fn api_persona_side_chat_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatIdPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let side_chat_id = path.side_chat_id.trim().to_string();
    if side_chat_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "side_chat_id is required".into()));
    }
    let pid = path.persona_id;
    let sc = call_blocking(state.app_state.db.clone(), {
        let side_chat_id = side_chat_id.clone();
        move |db| db.get_side_chat(chat_id, pid, &side_chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "side chat not found".into()))?;
    let turns = call_blocking(state.app_state.db.clone(), {
        let id = sc.id.clone();
        move |db| db.list_side_chat_turns(&id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let turns_json: Vec<serde_json::Value> = turns
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "role": t.role,
                "content": t.content,
                "created_at": t.created_at,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "side_chat": side_chat_to_json(&sc),
        "turns": turns_json,
    })))
}

async fn api_persona_side_chat_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatIdPathParams>,
    Json(body): Json<SideChatPatchBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let side_chat_id = path.side_chat_id.trim().to_string();
    if side_chat_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "side_chat_id is required".into()));
    }
    let draft = body.draft_text.as_deref();
    let title = body.title.as_deref();
    if draft.is_none() && title.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "draft_text or title is required".into(),
        ));
    }
    let pid = path.persona_id;
    let updated = call_blocking(state.app_state.db.clone(), {
        let side_chat_id = side_chat_id.clone();
        let draft = draft.map(str::to_string);
        let title = title.map(str::to_string);
        move |db| {
            db.patch_side_chat(
                chat_id,
                pid,
                &side_chat_id,
                draft.as_deref(),
                title.as_deref(),
            )
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !updated {
        return Err((StatusCode::NOT_FOUND, "side chat not found".into()));
    }
    let sc = call_blocking(state.app_state.db.clone(), {
        let side_chat_id = side_chat_id.clone();
        move |db| db.get_side_chat(chat_id, pid, &side_chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "side chat not found".into()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "side_chat": side_chat_to_json(&sc),
    })))
}

async fn api_persona_side_chat_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatIdPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let side_chat_id = path.side_chat_id.trim().to_string();
    if side_chat_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "side_chat_id is required".into()));
    }
    let pid = path.persona_id;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_side_chat(chat_id, pid, &side_chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "deleted": deleted,
    })))
}

/// Deposit one side-chat assistant turn into the main timeline (no agent run).
async fn api_persona_side_chat_promote_turn(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaSideChatIdPathParams>,
    Json(body): Json<SideChatPromoteTurnBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let side_chat_id = path.side_chat_id.trim().to_string();
    if side_chat_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "side_chat_id is required".into()));
    }
    let turn_id = body.turn_id.trim().to_string();
    if turn_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "turn_id is required".into()));
    }

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let sc = call_blocking(state.app_state.db.clone(), {
        let side_chat_id = side_chat_id.clone();
        move |db| db.get_side_chat(chat_id, pid, &side_chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "side chat not found".into()))?;

    let turn = call_blocking(state.app_state.db.clone(), {
        let side_chat_id = sc.id.clone();
        let turn_id = turn_id.clone();
        move |db| db.get_side_chat_turn(&side_chat_id, &turn_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "turn not found".into()))?;

    if !turn.role.eq_ignore_ascii_case("assistant") {
        return Err((
            StatusCode::BAD_REQUEST,
            "only assistant turns can be added to the main chat".into(),
        ));
    }
    let content = turn.content.trim().to_string();
    if content.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "turn content is empty".into()));
    }
    if content.chars().count() > 100_000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "content exceeds 100000 character limit".into(),
        ));
    }

    let message_id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let session_id = if sc.session_id.trim().is_empty() {
        None
    } else {
        Some(sc.session_id.clone())
    };
    let sender_name = state.app_state.config.bot_username.clone();
    let stored = crate::db::StoredMessage {
        id: message_id.clone(),
        chat_id,
        persona_id: path.persona_id,
        session_id: session_id.clone(),
        sender_name: sender_name.clone(),
        content: content.clone(),
        is_from_bot: true,
        timestamp: timestamp.clone(),
        origin: crate::db::message_origin_interactive(),
        speaker_persona_id: None,
    };
    call_blocking(state.app_state.db.clone(), move |db| {
        db.store_message(&stored)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "side_chat_id": sc.id,
        "turn_id": turn.id,
        "message_id": message_id,
        "message": {
            "id": message_id,
            "sender_name": sender_name,
            "content": content,
            "is_from_bot": true,
            "timestamp": timestamp,
            "session_id": session_id,
        },
    })))
}

/// Full text of one message in the persona thread (used by web bookmark reader; bookmarks only store a short preview).
async fn api_persona_message_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMessagePathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let message_id = path.message_id.trim().to_string();
    if message_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }

    let pid_lookup = path.persona_id;
    let cid = chat_id;
    let message = call_blocking(state.app_state.db.clone(), {
        let message_id = message_id.clone();
        move |db| db.get_message_for_persona(cid, pid_lookup, &message_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(m) = message else {
        return Err((StatusCode::NOT_FOUND, "message not found".into()));
    };

    let pid_count = path.persona_id;
    let mid = m.id.clone();
    let messages_from = call_blocking(state.app_state.db.clone(), {
        let mid = mid.clone();
        let sid = m.session_id.clone();
        move |db| db.count_messages_from_id(cid, pid_count, &mid, sid.as_deref())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .unwrap_or(0);

    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "message": {
            "id": m.id,
            "sender_name": m.sender_name,
            "content": m.content,
            "is_from_bot": m.is_from_bot,
            "timestamp": m.timestamp,
            "session_id": m.session_id,
        },
        "messages_from": messages_from,
    })))
}

async fn api_persona_message_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMessagePathParams>,
    Json(body): Json<PatchPersonaMessageBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let message_id = path.message_id.trim().to_string();
    if message_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }

    let content = body.content.trim().to_string();
    if content.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "content is required".into()));
    }
    if content.chars().count() > 100_000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "content exceeds 100000 character limit".into(),
        ));
    }

    let pid_update = path.persona_id;
    let updated = call_blocking(state.app_state.db.clone(), {
        let message_id = message_id.clone();
        let content = content.clone();
        move |db| db.update_message_content(chat_id, pid_update, &message_id, &content)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, "message not found".into()));
    }

    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "message_id": message_id,
        "content": content,
        "updated": true,
    })))
}

async fn api_persona_message_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMessagePathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let message_id = path.message_id.trim().to_string();
    if message_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message_id is required".into()));
    }

    let pid_delete = path.persona_id;
    let deleted = call_blocking(state.app_state.db.clone(), {
        let message_id = message_id.clone();
        move |db| db.delete_message(chat_id, pid_delete, &message_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "message not found".into()));
    }

    Ok(Json(json!({
        "ok": true,
        "persona_id": path.persona_id,
        "message_id": path.message_id,
        "deleted": true,
    })))
}

fn ensure_persona_memory_file_exists_for_web(state: &AppState, chat_id: i64, persona_id: i64) {
    let path = state.memory.persona_memory_state_path(chat_id, persona_id);
    if path.exists() {
        return;
    }
    let display_name = if !state.config.agent_display_name.trim().is_empty() {
        Some(state.config.agent_display_name.as_str())
    } else {
        None
    };
    let _ = state
        .memory
        .ensure_persona_memory_state_exists(chat_id, persona_id, display_name);
}

fn file_mtime_ms(path: &std::path::Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i64)
}

async fn api_persona_memory_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMemoryPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    ensure_persona_memory_file_exists_for_web(&state.app_state, chat_id, pid);
    let mem_path = state
        .app_state
        .memory
        .persona_memory_state_path(chat_id, pid);
    let memory_state = state
        .app_state
        .memory
        .read_or_migrate_persona_memory_state(chat_id, pid)
        .unwrap_or_default();
    let content = serde_json::to_string_pretty(&memory_state).unwrap_or_else(|_| "{}".to_string());
    let mtime_ms = file_mtime_ms(&mem_path).unwrap_or(0);
    Ok(Json(json!({
        "ok": true,
        "persona_id": pid,
        "content": content,
        "mtime_ms": mtime_ms,
        "path": mem_path.to_string_lossy(),
    })))
}

#[derive(Deserialize)]
struct PersonaMemoryPutBody {
    content: String,
    if_match_mtime_ms: Option<i64>,
}

async fn api_persona_memory_put(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMemoryPathParams>,
    Json(body): Json<PersonaMemoryPutBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    if body.content.len() > 256 * 1024 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "memory content too large".into(),
        ));
    }

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    ensure_persona_memory_file_exists_for_web(&state.app_state, chat_id, pid);
    let mem_path = state
        .app_state
        .memory
        .persona_memory_state_path(chat_id, pid);
    let current_mtime = file_mtime_ms(&mem_path).unwrap_or(0);
    if let Some(expected) = body.if_match_mtime_ms {
        if expected != current_mtime {
            return Err((
                StatusCode::CONFLICT,
                "memory was modified; reload and retry".into(),
            ));
        }
    }

    let mut state_payload: crate::memory::PersonaMemoryState = serde_json::from_str(&body.content)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid memory_state JSON: {e}"),
            )
        })?;
    state_payload.normalize();
    state
        .app_state
        .memory
        .validate_memory_state(&state_payload)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    state
        .app_state
        .memory
        .write_persona_memory_state(chat_id, pid, state_payload)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let _ = state.app_state.memory.append_persona_memory_event(
        chat_id,
        pid,
        "web_memory_state_put",
        "user_manual",
        json!({"path": mem_path.to_string_lossy().to_string()}),
    );

    let new_mtime = file_mtime_ms(&mem_path).unwrap_or(0);
    Ok(Json(json!({
        "ok": true,
        "persona_id": pid,
        "mtime_ms": new_mtime,
    })))
}

async fn api_workspace_agents_md_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let path = state.app_state.memory.groups_root_memory_path();
    let content = state
        .app_state
        .memory
        .read_groups_root_memory()
        .unwrap_or_default();
    let mtime_ms = file_mtime_ms(&path).unwrap_or(0);
    Ok(Json(json!({
        "ok": true,
        "content": content,
        "mtime_ms": mtime_ms,
        "path": path.to_string_lossy(),
    })))
}

#[derive(Deserialize)]
struct WorkspaceAgentsMdPutBody {
    content: String,
    if_match_mtime_ms: Option<i64>,
}

async fn api_workspace_agents_md_put(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<WorkspaceAgentsMdPutBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    if body.content.len() > 256 * 1024 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "principles content too large".into(),
        ));
    }

    let path = state.app_state.memory.groups_root_memory_path();
    let current_mtime = file_mtime_ms(&path).unwrap_or(0);
    if let Some(expected) = body.if_match_mtime_ms {
        if expected != current_mtime {
            return Err((
                StatusCode::CONFLICT,
                "principles file was modified; reload and retry".into(),
            ));
        }
    }

    state
        .app_state
        .memory
        .write_groups_root_memory(&body.content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let new_mtime = file_mtime_ms(&path).unwrap_or(0);
    Ok(Json(json!({
        "ok": true,
        "mtime_ms": new_mtime,
    })))
}

async fn api_persona_agent_history_latest(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMemoryPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    let data_dir = state.app_state.config.runtime_data_dir();
    match crate::agent_history::read_latest_agent_history(&data_dir, chat_id, pid) {
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "no agent history for this persona".into(),
        )),
        Ok(Some(r)) => Ok(Json(json!({
            "ok": true,
            "persona_id": pid,
            "filename": r.filename,
            "content": r.content,
            "mtime_ms": r.mtime_ms,
            "path": r.path.to_string_lossy(),
        }))),
        Err(crate::agent_history::ReadLatestAgentHistoryError::FileTooLarge(_)) => Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "agent history file too large".into(),
        )),
        Err(crate::agent_history::ReadLatestAgentHistoryError::Io(e)) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    }
}

async fn api_persona_agent_history_optimize(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaMemoryPathParams>,
    body: Option<Json<AgentHistoryOptimizeRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }

    if crate::topics::system_llm_target(
        crate::topics::load_job_choice(&state.app_state, crate::topics::SystemLlmJob::Learn).await,
    )
    .is_none()
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "System LLM is not configured. Set a provider and model under Settings → System LLM."
                .into(),
        ));
    }

    let data_dir = state.app_state.config.runtime_data_dir();
    let history = match crate::agent_history::read_latest_agent_history(&data_dir, chat_id, pid) {
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                "no agent history for this persona".into(),
            ));
        }
        Ok(Some(r)) => r,
        Err(crate::agent_history::ReadLatestAgentHistoryError::FileTooLarge(_)) => {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "agent history file too large".into(),
            ));
        }
        Err(crate::agent_history::ReadLatestAgentHistoryError::Io(e)) => {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    };

    let operator_notes = body
        .and_then(|Json(req)| req.operator_notes)
        .and_then(|notes| {
            let trimmed = notes.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

    let app_state = state.app_state.clone();
    match crate::run_optimizer::try_enqueue_run_optimize(
        app_state,
        chat_id,
        pid,
        history.filename.clone(),
        history.content,
        operator_notes,
    )
    .await
    {
        crate::run_optimizer::RunOptimizeEnqueueOutcome::Queued { job_id } => Ok(Json(json!({
            "ok": true,
            "job_id": job_id,
            "filename": history.filename,
            "message": "Queued. Track progress in Background jobs.",
        }))),
        crate::run_optimizer::RunOptimizeEnqueueOutcome::BlockedAlreadyRunning => Err((
            StatusCode::CONFLICT,
            "another background job is already active for this chat".into(),
        )),
        crate::run_optimizer::RunOptimizeEnqueueOutcome::ActiveLookupFailed(e) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
        crate::run_optimizer::RunOptimizeEnqueueOutcome::DbCreateFailed(e) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
    }
}

async fn api_personas_create(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonaCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Persona name cannot be empty".into(),
        ));
    }
    let name_owned = name.to_string();
    let engine = crate::runtime_toggles::AgentEngine::parse_override(body.agent_engine.trim())
        .ok_or((
            StatusCode::BAD_REQUEST,
            "agent_engine must be classic, classic_cost_routing, cursor, or gemini_adk".to_string(),
        ))?;
    let provider = body
        .llm_provider
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let model = body
        .llm_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let base_url = body
        .llm_base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let local_url = body
        .local_base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let local_model = body
        .local_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let cursor_model = body
        .cursor_sdk_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let gemini_model = body
        .gemini_default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match engine {
        crate::runtime_toggles::AgentEngine::Classic
        | crate::runtime_toggles::AgentEngine::ClassicCostRouting => {
            if provider.is_none() || model.is_none() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Provider and model are required.".into(),
                ));
            }
            if provider
                .as_deref()
                .is_some_and(crate::llm_catalog::is_local_provider)
                && base_url.is_none()
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "A local provider needs a base URL.".into(),
                ));
            }
            if engine == crate::runtime_toggles::AgentEngine::ClassicCostRouting
                && (local_url.is_none() || local_model.is_none())
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Cost routing needs a local URL and model.".into(),
                ));
            }
        }
        crate::runtime_toggles::AgentEngine::Cursor => {
            if cursor_model.is_none() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "A Cursor model is required.".into(),
                ));
            }
        }
        crate::runtime_toggles::AgentEngine::GeminiAdk => {
            if gemini_model.is_none() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "A Gemini default model is required.".into(),
                ));
            }
        }
    }
    let engine_str = engine.as_str().to_string();
    let gemini_topology = gemini_model.map(|model| {
        let mut profile = crate::gemini_adk::AdkTopologyProfile::default_profile();
        profile.default_model = model;
        serde_json::to_string(&profile).unwrap_or_else(|_| "{}".into())
    });
    let persona_id = call_blocking(state.app_state.db.clone(), {
        let engine_str = engine_str.clone();
        let provider = provider.clone();
        let model = model.clone();
        let base_url = base_url.clone();
        let local_url = local_url.clone();
        let local_model = local_model.clone();
        let cursor_model = cursor_model.clone();
        let gemini_topology = gemini_topology.clone();
        move |db| {
            let persona_id = db.create_persona(chat_id, &name_owned, model.as_deref())?;
            db.set_persona_engine_override(chat_id, persona_id, Some(&engine_str))?;
            if provider.is_some() || model.is_some() || base_url.is_some() {
                db.set_persona_llm_selection(
                    chat_id,
                    persona_id,
                    provider.as_deref(),
                    model.as_deref(),
                    base_url.as_deref(),
                )?;
            }
            if local_url.is_some() || local_model.is_some() {
                db.set_persona_local_delegate(
                    chat_id,
                    persona_id,
                    local_url.as_deref(),
                    local_model.as_deref(),
                    Some(false),
                )?;
            }
            if let Some(cursor_model) = cursor_model.as_deref() {
                db.set_persona_cursor_sdk_model(chat_id, persona_id, Some(cursor_model), None)?;
            }
            if let Some(topology) = gemini_topology.as_deref() {
                db.set_persona_gemini_adk_topology(chat_id, persona_id, Some(topology))?;
            }
            Ok(persona_id)
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::tools::ensure_persona_shared_dir(
        std::path::Path::new(state.app_state.config.working_dir()),
        chat_id,
        persona_id,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "persona_id": persona_id,
        "message": format!("Persona '{}' created", name),
    })))
}

#[derive(Deserialize)]
struct PersonaLocalQuery {
    persona_id: i64,
}

#[derive(Deserialize)]
struct PersonaLocalPatch {
    persona_id: i64,
    base_url: String,
    model: String,
}

async fn api_persona_local_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<PersonaLocalQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let pid = query.persona_id;
    let persona = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter(|persona| persona.chat_id == chat_id)
        .ok_or((StatusCode::NOT_FOUND, "persona not found".to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "base_url": persona.local_delegate_base_url.unwrap_or_default(),
        "model": persona.local_delegate_model.unwrap_or_default(),
        "tools_ok": persona.local_delegate_tools_ok.unwrap_or(false),
    })))
}

async fn api_persona_local_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonaLocalPatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let base_url = crate::local_delegate::normalize_base_url_for_provider(body.base_url.trim(), "");
    let model = body.model.trim().to_string();
    if base_url.is_empty() || model.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Local URL and model are required.".into(),
        ));
    }
    let pid = body.persona_id;
    let base_saved = base_url.clone();
    let model_saved = model.clone();
    let saved = call_blocking(state.app_state.db.clone(), move |db| {
        db.set_persona_local_delegate(
            chat_id,
            pid,
            Some(&base_saved),
            Some(&model_saved),
            Some(false),
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !saved {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    Ok(Json(json!({
        "ok": true,
        "base_url": base_url,
        "model": model,
        "tools_ok": false,
        "message": "Local URL and model saved. Run Test before the next cost-routing turn.",
    })))
}

#[derive(Deserialize)]
struct PersonaExternalPathsBody {
    paths: Vec<String>,
}

fn persona_external_paths_json(
    persona_id: i64,
    paths: &[std::path::PathBuf],
    workspace_root: &std::path::Path,
) -> serde_json::Value {
    let mut automatic_paths = Vec::new();
    if crate::self_repo::workspace_disjoint_from_app(workspace_root) {
        automatic_paths.push(workspace_root.to_string_lossy().to_string());
        if let Some(app) = crate::self_repo::auto_approved_app_directory(workspace_root) {
            let app_text = app.to_string_lossy().to_string();
            if !automatic_paths.iter().any(|p| p == &app_text) {
                automatic_paths.push(app_text);
            }
        }
    }
    json!({
        "persona_id": persona_id,
        "env_key": crate::persona_external_paths::env_key(persona_id),
        "paths": paths.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "automatic_paths": automatic_paths,
    })
}

async fn api_persona_external_paths_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    let paths = state.app_state.persona_external_paths.paths_for(pid);
    let workspace = state.app_state.config.workspace_root_absolute();
    Ok(Json(persona_external_paths_json(pid, &paths, &workspace)))
}

async fn api_persona_external_paths_put(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<PersonaBookmarkPathParams>,
    Json(body): Json<PersonaExternalPathsBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let pid = path.persona_id;
    let exists = call_blocking(state.app_state.db.clone(), move |db| {
        db.persona_exists(chat_id, pid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "persona not found".into()));
    }
    if body.paths.len() > 32 {
        return Err((
            StatusCode::BAD_REQUEST,
            "at most 32 external paths are allowed".into(),
        ));
    }
    let saved = state
        .app_state
        .persona_external_paths
        .set(pid, &body.paths)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let workspace = state.app_state.config.workspace_root_absolute();
    Ok(Json(persona_external_paths_json(pid, &saved, &workspace)))
}

async fn api_personas_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonaDeleteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let persona_id = body.persona_id;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_persona(chat_id, persona_id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if deleted {
        if let Err(err) = state.app_state.persona_external_paths.clear(persona_id) {
            tracing::warn!(persona_id, "Could not clear persona external paths: {err}");
        }
    }

    Ok(Json(json!({
        "ok": true,
        "deleted": deleted,
        "message": if deleted { "Persona deleted" } else { "Persona not found or cannot delete default" },
    })))
}

async fn api_personas_rename(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<PersonaRenameRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let persona_id = body.persona_id;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Persona name cannot be empty".into(),
        ));
    }
    let name_for_msg = name.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.rename_persona(chat_id, persona_id, &name)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "persona_id": persona_id,
        "name": name_for_msg,
        "message": format!("Persona renamed to '{name_for_msg}'"),
    })))
}

async fn api_contacts_bind(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ContactsBindRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let contact_chat_id = body.contact_chat_id;
    call_blocking(state.app_state.db.clone(), move |db| {
        db.link_channel(contact_chat_id, BOT_INSTANCE_WEB, "web", "default")
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "message": "Web bound to contact",
        "contact_chat_id": contact_chat_id,
    })))
}

async fn api_contacts_unlink(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(_body): Json<ContactsUnlinkRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let removed = call_blocking(state.app_state.db.clone(), move |db| {
        db.unlink_channel(BOT_INSTANCE_WEB, "web", "default")
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "removed": removed,
        "message": if removed { "Web unlinked from contact" } else { "No binding found" },
    })))
}

async fn api_schedules_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<SchedulesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let tasks = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_tasks_for_chat(chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<serde_json::Value> = tasks
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "chat_id": t.chat_id,
                "persona_id": t.persona_id,
                "prompt": t.prompt,
                "schedule_type": t.schedule_type,
                "schedule_value": t.schedule_value,
                "next_run": t.next_run,
                "last_run": t.last_run,
                "status": t.status,
                "created_at": t.created_at,
                "output_delivery": t.output_delivery,
            })
        })
        .collect();

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "tasks": items,
    })))
}

fn persona_todo_to_json(t: &crate::db::PersonaTodo) -> serde_json::Value {
    json!({
        "id": t.id,
        "chat_id": t.chat_id,
        "persona_id": t.persona_id,
        "title": t.title,
        "status": t.status,
        "source_hint": t.source_hint,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
        "completed_at": t.completed_at,
        "description": t.description,
        "owner_persona_id": t.owner_persona_id,
        "priority": t.priority,
        "due_at": t.due_at,
        "blocked_reason": t.blocked_reason,
        "position": t.position,
        "source_kind": t.source_kind,
        "source_ref": t.source_ref,
        "auto_advance": t.auto_advance != 0,
        "stage_run_id": t.stage_run_id,
        "stage_outcome": t.stage_outcome,
        "deploy_prompt": t.deploy_prompt,
    })
}

async fn api_todos_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<TodosQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let status_raw = query
        .status
        .as_deref()
        .unwrap_or("open")
        .trim()
        .to_ascii_lowercase();
    let status_filter: Option<String> = match status_raw.as_str() {
        "all" => None,
        "open" | "done" => Some(status_raw.clone()),
        other => match crate::db::normalize_board_status(other) {
            Ok(status) => Some(status.to_string()),
            Err(err) => return Err((StatusCode::BAD_REQUEST, err.to_string())),
        },
    };

    let filter = status_filter.clone();
    let todos = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_todos_for_chat(chat_id, filter.as_deref(), 200)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<serde_json::Value> = todos.iter().map(persona_todo_to_json).collect();

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "status": status_raw,
        "todos": items,
    })))
}

async fn api_todos_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TodoPathParams>,
    Json(body): Json<TodoPatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let todo_id = path.id;
    let patch = crate::db_collab::BoardPatch {
        status: body.status,
        title: body.title,
        description: body.description,
        owner_persona_id: body.owner_persona_id,
        priority: body.priority,
        due_at: body.due_at,
        blocked_reason: body.blocked_reason,
        position: body.position,
        before_position: body.before_position,
        after_position: body.after_position,
        actor_kind: "operator",
        actor_persona_id: None,
        auto_advance: body.auto_advance,
        completed: body.completed,
        deploy_prompt: body.deploy_prompt,
    };
    let updated = call_blocking(state.app_state.db.clone(), move |db| {
        db.apply_board_patch(chat_id, todo_id, &patch)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(todo) = updated else {
        return Err((StatusCode::NOT_FOUND, "todo not found".into()));
    };

    Ok(Json(json!({
        "ok": true,
        "todo": persona_todo_to_json(&todo),
    })))
}

async fn api_todos_create(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<TodoCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "title is required".into()));
    }
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let requested_persona = body.persona_id.filter(|id| *id > 0);
    let description = body
        .description
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let created = call_blocking(state.app_state.db.clone(), move |db| {
        let persona_id = if let Some(pid) = requested_persona {
            if !db.persona_exists(chat_id, pid)? {
                return Err(crate::error::FinallyAValueBotError::ToolExecution(format!(
                    "Persona {pid} does not exist for this chat"
                )));
            }
            pid
        } else {
            db.get_current_persona_id(chat_id)?
        };
        db.add_persona_todo(chat_id, persona_id, &title, Some("board"))
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let todo = if let Some(description) = description {
        let todo_id = created.id;
        let patch = crate::db_collab::BoardPatch {
            status: None,
            title: None,
            description: Some(description),
            owner_persona_id: None,
            priority: None,
            due_at: None,
            blocked_reason: None,
            position: None,
            before_position: None,
            after_position: None,
            actor_kind: "operator",
            actor_persona_id: None,
            auto_advance: None,
            completed: None,
            deploy_prompt: None,
        };
        call_blocking(state.app_state.db.clone(), move |db| {
            db.apply_board_patch(chat_id, todo_id, &patch)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or(created)
    } else {
        created
    };
    Ok(Json(json!({
        "ok": true,
        "todo": persona_todo_to_json(&todo),
    })))
}

async fn api_todos_events(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TodoPathParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let todo_id = path.id;
    let events = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_board_item_events(chat_id, todo_id, 100)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(events) = events else {
        return Err((StatusCode::NOT_FOUND, "todo not found".into()));
    };
    let items: Vec<serde_json::Value> = events
        .iter()
        .map(|event| {
            json!({
                "id": event.id,
                "from_status": event.from_status,
                "to_status": event.to_status,
                "actor_kind": event.actor_kind,
                "actor_persona_id": event.actor_persona_id,
                "at": event.at,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "events": items,
    })))
}

async fn api_todos_run(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(path): Path<TodoPathParams>,
    Json(body): Json<TodoRunRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let todo_id = path.id;
    let todo = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_persona_todo(todo_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(mut todo) = todo.filter(|item| item.chat_id == chat_id) else {
        return Err((StatusCode::NOT_FOUND, "todo not found".into()));
    };
    if let Some(prompt) = body.deploy_prompt {
        let patch = crate::db_collab::BoardPatch {
            status: None,
            title: None,
            description: None,
            owner_persona_id: None,
            priority: None,
            due_at: None,
            blocked_reason: None,
            position: None,
            before_position: None,
            after_position: None,
            actor_kind: "operator",
            actor_persona_id: None,
            auto_advance: None,
            completed: None,
            deploy_prompt: Some(prompt),
        };
        let updated = call_blocking(state.app_state.db.clone(), move |db| {
            db.apply_board_patch(chat_id, todo_id, &patch)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(updated) = updated else {
            return Err((StatusCode::NOT_FOUND, "todo not found".into()));
        };
        todo = updated;
    }
    let queued = start_board_stage(state, &todo, crate::board_pipeline::AUTO_CHAIN_BUDGET).await?;
    Ok(Json(queued))
}

async fn api_schedules_create(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ScheduleCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let effective_tz = body.timezone.as_deref().or_else(|| {
        let default = state.app_state.config.timezone.trim();
        if default.is_empty() {
            None
        } else {
            Some(default)
        }
    });
    let preflight = crate::tools::schedule::preflight_schedule_request(
        &body.schedule_type,
        &body.schedule_value,
        effective_tz,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let output_delivery =
        crate::tools::schedule::normalize_output_delivery(body.output_delivery.as_deref(), true)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let prompt = body.prompt;
    let schedule_type = body.schedule_type;
    let schedule_value = preflight.schedule_value.clone();
    let next_run_for_db = preflight.next_run.clone();
    let requested_persona_id = body.persona_id.filter(|id| *id > 0);
    let id = call_blocking(state.app_state.db.clone(), move |db| {
        let persona_id = if let Some(pid) = requested_persona_id {
            if !db.persona_exists(chat_id, pid)? {
                return Err(crate::error::FinallyAValueBotError::ToolExecution(format!(
                    "Persona {} does not exist for this chat",
                    pid
                )));
            }
            pid
        } else {
            db.get_current_persona_id(chat_id)?
        };
        db.create_scheduled_task_for_persona(
            chat_id,
            persona_id,
            &prompt,
            &schedule_type,
            &schedule_value,
            &next_run_for_db,
            &output_delivery,
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "id": id,
        "message": "Schedule created",
        "next_run": preflight.next_run,
        "timezone": preflight.timezone_used,
        "timezone_assumption": if preflight.timezone_defaulted_to_utc {
            "Timezone not provided. UTC was assumed."
        } else {
            "Timezone provided by request."
        },
    })))
}

async fn api_schedules_update(
    headers: HeaderMap,
    State(state): State<WebState>,
    axum::extract::Path(task_id): axum::extract::Path<i64>,
    Json(body): Json<ScheduleUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;

    let status = match body.status.as_deref() {
        Some("paused") => Some("paused"),
        Some("active") | Some("resumed") => Some("active"),
        Some("cancelled") => Some("cancelled"),
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "status must be paused, active, or cancelled".into(),
            ));
        }
        None => None,
    };
    let persona_id = body.persona_id;
    let prompt_update = body.prompt.as_ref().map(|p| p.trim().to_string());
    let schedule_type_in = body.schedule_type.as_ref().map(|s| s.trim().to_string());
    let schedule_value_in = body.schedule_value.as_ref().map(|s| s.trim().to_string());
    let has_schedule_pair = schedule_type_in.is_some() && schedule_value_in.is_some();
    let schedule_partial = schedule_type_in.is_some() ^ schedule_value_in.is_some();
    if schedule_partial {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide both schedule_type and schedule_value to change the schedule".into(),
        ));
    }
    let output_delivery = match body.output_delivery.as_deref() {
        None => None,
        Some(raw) => Some(
            crate::tools::schedule::normalize_output_delivery(Some(raw), false)
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?,
        ),
    };
    if status.is_none()
        && persona_id.is_none()
        && prompt_update.is_none()
        && output_delivery.is_none()
        && !has_schedule_pair
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide at least one field to update: status, persona_id, prompt, output_delivery, or schedule_type+schedule_value".into(),
        ));
    }
    if let Some(pid) = persona_id {
        if pid <= 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                "persona_id must be a positive integer".into(),
            ));
        }
    }

    let task = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_task_by_id(task_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(task) = task else {
        return Err((StatusCode::NOT_FOUND, "Task not found".into()));
    };
    if task.chat_id != chat_id {
        return Err((StatusCode::NOT_FOUND, "Task not found".into()));
    }

    if let Some(pid) = persona_id {
        let exists = call_blocking(state.app_state.db.clone(), move |db| {
            db.persona_exists(chat_id, pid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !exists {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Persona {} does not exist for this chat", pid),
            ));
        }
    }

    if let Some(ref p) = prompt_update {
        if p.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "prompt must not be empty".into()));
        }
    }

    let schedule_preflight = if has_schedule_pair {
        let st = schedule_type_in.as_ref().unwrap().as_str();
        let sv = schedule_value_in.as_ref().unwrap().as_str();
        let effective_tz = body.timezone.as_deref().or_else(|| {
            let default = state.app_state.config.timezone.trim();
            if default.is_empty() {
                None
            } else {
                Some(default)
            }
        });
        Some(
            crate::tools::schedule::preflight_schedule_request(st, sv, effective_tz)
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?,
        )
    } else {
        None
    };

    if let Some(next_status) = status {
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            db.update_task_status(task_id, next_status)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "Task not found".into()));
        }
    }
    if let Some(pid) = persona_id {
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            db.update_task_persona(task_id, pid)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "Task not found".into()));
        }
    }
    if let Some(p) = prompt_update {
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            db.update_task_prompt(task_id, &p)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "Task not found".into()));
        }
    }
    if let Some(delivery) = output_delivery {
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            db.update_task_output_delivery(task_id, &delivery)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "Task not found".into()));
        }
    }

    if let Some(pref) = &schedule_preflight {
        let schedule_type_owned = schedule_type_in.as_ref().unwrap().clone();
        let schedule_value_owned = pref.schedule_value.clone();
        let next_run_owned = pref.next_run.clone();
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            db.update_task_schedule(
                task_id,
                &schedule_type_owned,
                &schedule_value_owned,
                &next_run_owned,
            )
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "Task not found".into()));
        }
    }

    let mut response = json!({
        "ok": true,
        "message": "Task updated",
    });
    if let Some(pref) = schedule_preflight {
        response["next_run"] = json!(pref.next_run);
        response["timezone"] = json!(pref.timezone_used);
        response["timezone_assumption"] = json!(if pref.timezone_defaulted_to_utc {
            "Timezone not provided. UTC was assumed."
        } else {
            "Timezone provided by request."
        });
    }

    Ok(Json(response))
}

// --- Background jobs API ---

#[derive(Debug, Deserialize)]
struct BackgroundJobsQuery {
    chat_id: Option<i64>,
    limit: Option<usize>,
}

async fn api_background_jobs_list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<BackgroundJobsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    let limit = query.limit.unwrap_or(20).min(100);
    let jobs = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_background_jobs_for_chat(chat_id, limit)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let heartbeats = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_job_heartbeats_for_chat(chat_id, 200)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let hb_by_key: HashMap<String, JobHeartbeat> = heartbeats
        .into_iter()
        .map(|h| (h.run_key.clone(), h))
        .collect();

    let active_heartbeats = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_active_job_heartbeats_for_chat(chat_id, 20)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let now = chrono::Utc::now().to_rfc3339();
    let pending_timeout_secs = state
        .app_state
        .config
        .background_job_pending_start_timeout_secs as i64;
    let active_count = call_blocking(state.app_state.db.clone(), move |db| {
        db.count_active_background_jobs_for_chat(chat_id, &now, pending_timeout_secs)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<serde_json::Value> = jobs
        .into_iter()
        .map(|j| {
            let hb = hb_by_key.get(&j.id);
            let mut row = json_background_job(&j);
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "heartbeat".into(),
                    hb.map(json_job_heartbeat)
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            row
        })
        .collect();

    let active_hb_json: Vec<serde_json::Value> =
        active_heartbeats.iter().map(json_job_heartbeat).collect();

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "jobs": items,
        "active_count": active_count,
        "active_heartbeats": active_hb_json,
    })))
}

async fn api_background_job_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(job_id): Path<String>,
    Query(query): Query<BackgroundJobsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    let job = call_blocking(state.app_state.db.clone(), {
        let job_id = job_id.clone();
        move |db| db.get_background_job(&job_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(job) = job else {
        return Err((StatusCode::NOT_FOUND, "background job not found".into()));
    };
    if job.chat_id != chat_id {
        return Err((StatusCode::NOT_FOUND, "background job not found".into()));
    }

    let hb = call_blocking(state.app_state.db.clone(), {
        let job_id = job_id.clone();
        move |db| db.get_job_heartbeat(&job_id)
    })
    .await
    .ok()
    .flatten();

    let timeline = call_blocking(state.app_state.db.clone(), {
        let job_id = job_id.clone();
        move |db| db.get_run_timeline_events(&job_id, 50)
    })
    .await
    .unwrap_or_default();

    let timeline_json: Vec<serde_json::Value> = timeline
        .into_iter()
        .map(|e| {
            json!({
                "id": e.id,
                "event_type": e.event_type,
                "payload_json": e.payload_json,
                "created_at": e.created_at,
            })
        })
        .collect();

    let mut job_json = json_background_job(&job);
    if let Some(obj) = job_json.as_object_mut() {
        if let Some(rt) = job.result_text {
            obj.insert("result_text".into(), json!(rt));
        }
    }

    Ok(Json(json!({
        "ok": true,
        "job": job_json,
        "heartbeat": hb.as_ref().map(json_job_heartbeat),
        "timeline_events_recent": timeline_json,
    })))
}

#[derive(Debug, Deserialize)]
struct ContactsBindingsQuery {
    chat_id: Option<i64>,
}

/// Accepted for API compatibility; [`api_settings_patch`] always returns 501.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SettingsPatchRequest {
    upsert: Option<HashMap<String, String>>,
    remove: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ChannelPersonaPolicyUpsertRequest {
    chat_id: Option<i64>,
    bot_instance_id: i64,
    mode: String, // "all" | "single"
    persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ChannelPersonaPolicyDeleteRequest {
    chat_id: Option<i64>,
    bot_instance_id: i64,
}

fn setting_is_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("API_KEY")
        || upper.ends_with("_KEY")
        || upper.contains("PASSWORD")
}

fn mask_setting_value(value: &str) -> String {
    if value.len() <= 6 {
        return "***".to_string();
    }
    let prefix = &value[..3];
    let suffix = &value[value.len().saturating_sub(2)..];
    format!("{prefix}***{suffix}")
}

fn is_llm_ready(cfg: &Config) -> bool {
    crate::llm_catalog::is_api_key_configured_for_provider(&cfg.llm_provider)
        || crate::llm_catalog::any_provider_api_key_configured()
}

fn is_channel_ready(db: &crate::db::Database) -> bool {
    crate::channel_integration_config::has_any_messaging_bot_token(db).unwrap_or(false)
}

async fn api_contacts_bindings(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<ContactsBindingsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_contact_channel_integration_rows(chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let (persona_mode, persona_id) = match r.persona_mode {
                ChannelPersonaMode::All => ("all", None),
                ChannelPersonaMode::Single => ("single", r.persona_id),
            };
            json!({
                "bot_instance_id": r.bot_instance_id,
                "platform": r.platform,
                "label": r.label,
                "channel_type": r.platform,
                "channel_handle": r.channel_handle,
                "linked": r.linked,
                "persona_mode": persona_mode,
                "persona_id": persona_id
            })
        })
        .collect();

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "bindings": items,
    })))
}

#[derive(Debug, Deserialize)]
struct LlmModelPatchRequest {
    model: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    custom: bool,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    thinking_enabled: Option<bool>,
    #[serde(default)]
    show_thinking: Option<bool>,
    /// When set, save classic strategy LLM onto this persona (does not change global app_settings).
    #[serde(default)]
    persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LlmGetQuery {
    #[serde(default)]
    persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LlmModelsQuery {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimePatchRequest {
    #[serde(default)]
    tool_output_debug: Option<bool>,
    #[serde(default)]
    post_tool_evaluator_enabled: Option<bool>,
    #[serde(default)]
    response_quality_evaluator_enabled: Option<bool>,
    #[serde(default)]
    agent_engine: Option<String>,
    /// Process-wide Classic tool-loop cap. `None` leaves the current limit.
    #[serde(default)]
    max_tool_iterations: Option<usize>,
    /// Output tokens per model reply (`MAX_TOKENS`). `None` leaves the current limit.
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ConnectionsPatch {
    id: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    url: Option<String>,
    /// Local model id. Only the local-server connection stores this.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    clear: bool,
}

/// Shown in the Cursor field when a key already exists. Not a secret.
const CURSOR_KEY_MASK: &str = "••••••••••••••••";

fn env_var_present(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn cursor_key_mask(data_dir: &str) -> Option<&'static str> {
    let present = env_var_present("CURSOR_API_KEY")
        || crate::engine_connections::secret_is_saved(data_dir, "CURSOR_API_KEY");
    present.then_some(CURSOR_KEY_MASK)
}

fn is_key_mask_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '•')
}

fn connection_secret_from_env(
    def: &crate::engine_connections::ConnectionDef,
    config: &crate::config::Config,
) -> bool {
    match def.id {
        "openai" => {
            config
                .openai_api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || env_var_present("OPENAI_API_KEY")
        }
        "google" => {
            config
                .gemini_api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || env_var_present("GEMINI_API_KEY")
                || env_var_present("GOOGLE_API_KEY")
        }
        "perplexity" => {
            config
                .perplexity_api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || env_var_present("PERPLEXITY_API_KEY")
        }
        _ => {
            if let Some(provider_id) = def.provider_id {
                return crate::llm_catalog::provider_api_key_env_hints(provider_id)
                    .iter()
                    .any(|name| env_var_present(name));
            }
            def.secret_name.is_some_and(env_var_present)
        }
    }
}

fn connections_json(state: &crate::agent::AppState) -> serde_json::Value {
    let data_dir = state.config.runtime_data_dir();
    let local_url = state
        .db
        .get_app_setting("LOCAL_SERVER_URL")
        .ok()
        .flatten()
        .unwrap_or_default();
    let local_model = state
        .db
        .get_app_setting("LOCAL_SERVER_MODEL")
        .ok()
        .flatten()
        .unwrap_or_default();
    let connections: Vec<serde_json::Value> = crate::engine_connections::CONNECTIONS
        .iter()
        .filter(|def| def.supported)
        .map(|def| {
            let (source, url) = match def.kind {
                crate::engine_connections::ConnectionKind::Url => {
                    if local_url.trim().is_empty() {
                        ("missing", None)
                    } else {
                        ("saved", Some(local_url.as_str()))
                    }
                }
                crate::engine_connections::ConnectionKind::Secret => {
                    let from_env = connection_secret_from_env(def, &state.config);
                    let saved = def.secret_name.is_some_and(|name| {
                        crate::engine_connections::secret_is_saved(&data_dir, name)
                    });
                    let source = if from_env {
                        "env"
                    } else if saved {
                        "saved"
                    } else {
                        "missing"
                    };
                    (source, None)
                }
            };
            json!({
                "id": def.id,
                "label": def.label,
                "kind": match def.kind {
                    crate::engine_connections::ConnectionKind::Secret => "secret",
                    crate::engine_connections::ConnectionKind::Url => "url",
                },
                "featured": def.featured,
                "used_by": def.used_by,
                "env_var": def.secret_name,
                "provider_id": def.provider_id,
                "source": source,
                "url": url,
                "model": if matches!(def.kind, crate::engine_connections::ConnectionKind::Url) {
                    Some(local_model.as_str())
                } else {
                    None
                },
                "masked_secret": if def.id == "cursor" {
                    cursor_key_mask(&data_dir)
                } else {
                    None
                },
            })
        })
        .collect();
    json!({
        "ok": true,
        "connections": connections,
    })
}

async fn api_connections_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    // Same disk refresh as GET /api/env-config, so Reload from .env sees hand edits.
    env_config::load_env(&state).await?;
    Ok(Json(connections_json(&state.app_state)))
}

async fn api_connections_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ConnectionsPatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let def = crate::engine_connections::find_connection(body.id.trim())
        .ok_or((StatusCode::BAD_REQUEST, "Unknown connection.".into()))?;
    let data_dir = state.app_state.config.runtime_data_dir();
    match def.kind {
        crate::engine_connections::ConnectionKind::Url => {
            let raw = body.url.unwrap_or_default();
            let url = crate::local_delegate::normalize_base_url_for_provider(raw.trim(), "");
            if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Enter an http or https URL for the local model server.".into(),
                ));
            }
            let model = body
                .model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or((StatusCode::BAD_REQUEST, "Enter a local model name.".into()))?;
            let url_saved = url.clone();
            let model_saved = model.clone();
            call_blocking(state.app_state.db.clone(), move |db| {
                db.set_app_setting("LOCAL_SERVER_URL", &url_saved)?;
                db.set_app_setting("LOCAL_SERVER_MODEL", &model_saved)?;
                Ok(())
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        crate::engine_connections::ConnectionKind::Secret => {
            let name = def.secret_name.ok_or((
                StatusCode::BAD_REQUEST,
                "This connection does not take an API key.".into(),
            ))?;
            if body.clear {
                crate::engine_connections::clear_secret(&data_dir, name)
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
                env_config::persist_pairs(&[(name, None)])
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            } else {
                let secret = body.secret.unwrap_or_default();
                if is_key_mask_placeholder(&secret) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Enter a new API key to replace the current one.".into(),
                    ));
                }
                crate::engine_connections::write_secret(&data_dir, name, &secret)
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
                env_config::persist_pairs(&[(name, Some(secret.trim()))])
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
                state.app_state.env_redactor.register_trusted(secret.trim());
            }
            if def
                .provider_id
                .is_some_and(|id| id.eq_ignore_ascii_case(&state.app_state.llm.current_provider()))
            {
                let provider = state.app_state.llm.current_provider();
                let model = state.app_state.llm.current_model();
                let base = state.app_state.llm.current_base_url();
                state
                    .app_state
                    .llm
                    .apply_selection(provider, model, base)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            }
        }
    }
    let mut payload = connections_json(&state.app_state);
    let message = if def.id == "cursor" && !body.clear {
        "Cursor key saved. It replaces CURSOR_API_KEY in .env. The bridge sidecar uses it the next time it starts."
    } else if body.clear {
        "Saved key removed."
    } else {
        "Connection saved in .env. A key already in .env is the one Meido uses."
    };
    payload["message"] = json!(message);
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
struct TopicsModelJobPatch {
    id: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Deserialize)]
struct TopicsModelPatch {
    #[serde(default)]
    source: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    jobs: Vec<TopicsModelJobPatch>,
}

fn topics_model_json(
    choice: &crate::topics::SystemLlmChoice,
    saved: bool,
    strategy_provider: &str,
    strategy_model: &str,
    local_base_url: &str,
    jobs: Vec<serde_json::Value>,
    message: Option<&str>,
) -> serde_json::Value {
    let (source, provider, model, base_url) = match choice {
        crate::topics::SystemLlmChoice::Local { base_url, model } => (
            "local",
            "llama",
            Some(model.as_str()),
            Some(base_url.as_str()),
        ),
        crate::topics::SystemLlmChoice::Provider { provider_id, model } => (
            provider_id.as_str(),
            provider_id.as_str(),
            Some(model.as_str()),
            None,
        ),
        crate::topics::SystemLlmChoice::Missing => ("missing", "", None, None),
    };
    json!({
        "ok": true,
        "source": source,
        "provider": provider,
        "model": model,
        "base_url": base_url,
        "saved": saved,
        "strategy_provider": strategy_provider,
        "strategy_model": strategy_model,
        "local_base_url": local_base_url,
        "jobs": jobs,
        "message": message,
    })
}

async fn system_llm_job_rows(
    state: &crate::agent::AppState,
) -> Result<Vec<serde_json::Value>, (StatusCode, String)> {
    let hook_fallback = state.config.hook_prompt_model.clone();
    call_blocking(state.db.clone(), move |db| {
        let mut rows = Vec::new();
        for job in crate::topics::SystemLlmJob::ALL {
            let (provider, model) = crate::topics::read_system_llm_job(db, job)?;
            let provider = provider.unwrap_or_default();
            let model = model.unwrap_or_default();
            let using_default = provider.trim().is_empty();
            let legacy_model = if job == crate::topics::SystemLlmJob::PromptHooks && using_default {
                let stored = db.get_app_setting(crate::topics::APP_SETTING_HOOK_PROMPT_MODEL)?;
                crate::topics::effective_hook_prompt_model(stored.as_deref(), &hook_fallback)
            } else {
                None
            };
            rows.push(json!({
                "id": job.id(),
                "label": job.label(),
                "detail": job.detail(),
                "provider": provider,
                "model": model,
                "using_default": using_default && legacy_model.is_none(),
                "legacy_model": legacy_model,
            }));
        }
        Ok(rows)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn read_topics_model(
    state: &crate::agent::AppState,
) -> Result<(crate::topics::SystemLlmChoice, bool, String, String, String), (StatusCode, String)> {
    let db = state.db.clone();
    let saved = call_blocking(db, crate::topics::read_system_llm_settings)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let db = state.db.clone();
    let local_server = call_blocking(db, |db| {
        Ok((
            db.get_app_setting("LOCAL_SERVER_URL")?.unwrap_or_default(),
            db.get_app_setting("LOCAL_SERVER_MODEL")?
                .unwrap_or_default(),
        ))
    })
    .await
    .unwrap_or_default();
    let choice = crate::topics::resolve_system_llm_choice(crate::topics::SystemLlmInputs {
        saved_source: saved.0.as_deref(),
        saved_model: saved.1.as_deref(),
        local_server_url: &local_server.0,
        local_server_model: &local_server.1,
    });
    let local_base_url = local_server.0;
    let saved_choice = saved.0.as_deref().is_some_and(|s| !s.trim().is_empty());
    Ok((
        choice,
        saved_choice,
        crate::llm_catalog::resolve_catalog_provider_id(&state.llm.current_provider()),
        state.llm.current_model(),
        local_base_url,
    ))
}

async fn api_topics_model_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let (choice, saved, provider, model, local_base_url) =
        read_topics_model(&state.app_state).await?;
    let jobs = system_llm_job_rows(&state.app_state).await?;
    Ok(Json(topics_model_json(
        &choice,
        saved,
        &provider,
        &model,
        &local_base_url,
        jobs,
        None,
    )))
}

async fn api_topics_model_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<TopicsModelPatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let source = body
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(body.source.trim())
        .to_ascii_lowercase();
    let model = body.model.unwrap_or_default();
    let model = model.trim().to_string();
    if source.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Choose a provider for the system LLM.".into(),
        ));
    }
    let stored_model = if source == "strategy" {
        String::new()
    } else if crate::llm_catalog::is_local_provider(&source) || source == "local" {
        if model.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Choose a model for the system LLM.".into(),
            ));
        }
        let mm = state.app_state.llm.local_delegate_config();
        if mm.tier2_base_url.trim().is_empty() && mm.local_base_url.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Set a local server URL under Settings → Connections before using a local model for the system LLM.".into(),
            ));
        }
        model
    } else if crate::llm_catalog::find_provider(&source).is_some() {
        if model.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Choose a model for the system LLM.".into(),
            ));
        }
        model
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Unknown system LLM provider.".into(),
        ));
    };
    let mut job_writes = Vec::new();
    let mut hook_model_env: Option<Option<String>> = None;
    for job in &body.jobs {
        let Some(parsed_job) = crate::topics::SystemLlmJob::from_id(&job.id) else {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown system LLM row {}.", job.id.trim()),
            ));
        };
        let write =
            crate::topics::parse_system_llm_job_write(parsed_job, &job.provider, &job.model)
                .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
        if parsed_job == crate::topics::SystemLlmJob::PromptHooks {
            hook_model_env = Some(match &write {
                crate::topics::SystemLlmJobWrite::Clear => None,
                crate::topics::SystemLlmJobWrite::ModelOnly(model) => Some(model.clone()),
                crate::topics::SystemLlmJobWrite::Pair { model, .. } => Some(model.clone()),
            });
        }
        if matches!(
            &write,
            crate::topics::SystemLlmJobWrite::Pair { provider, .. }
                if crate::llm_catalog::is_local_provider(provider) || provider == "local"
        ) {
            let mm = state.app_state.llm.local_delegate_config();
            if mm.tier2_base_url.trim().is_empty() && mm.local_base_url.trim().is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Set a local server URL under Settings → Connections before using a local model for {}.",
                        parsed_job.label()
                    ),
                ));
            }
        }
        job_writes.push((parsed_job, write));
    }
    let stored_source = source.clone();
    let provider_for_env = stored_source.clone();
    let model_for_env = stored_model.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        crate::topics::save_system_llm_settings(db, &stored_source, &stored_model)?;
        for (job, write) in &job_writes {
            crate::topics::apply_system_llm_job_write(db, *job, write)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut env_pairs = vec![
        ("SYSTEM_LLM_PROVIDER", Some(provider_for_env.as_str())),
        ("SYSTEM_LLM_MODEL", Some(model_for_env.as_str())),
    ];
    let hook_env_owned = hook_model_env.clone();
    if hook_model_env.is_some() {
        env_pairs.push((
            "HOOK_PROMPT_MODEL",
            hook_env_owned.as_ref().and_then(|value| value.as_deref()),
        ));
    }
    env_config::persist_pairs(&env_pairs).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let (choice, saved, provider, strategy_model, local_base_url) =
        read_topics_model(&state.app_state).await?;
    let jobs = system_llm_job_rows(&state.app_state).await?;
    Ok(Json(topics_model_json(
        &choice,
        saved,
        &provider,
        &strategy_model,
        &local_base_url,
        jobs,
        Some("System LLM saved. Blank rows use the shared default on the next pass. No restart."),
    )))
}

async fn api_runtime_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let sources = call_blocking(state.app_state.db.clone(), |db| {
        Ok::<_, crate::error::FinallyAValueBotError>((
            crate::runtime_toggles::RuntimeToggles::tool_output_debug_from_app_settings(db)?,
            crate::runtime_toggles::RuntimeToggles::post_tool_evaluator_from_app_settings(db)?,
            crate::runtime_toggles::RuntimeToggles::response_quality_evaluator_from_app_settings(
                db,
            )?,
            crate::runtime_toggles::RuntimeToggles::agent_engine_from_app_settings(db)?,
        ))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let toggles = &state.app_state.runtime_toggles;
    let mm_cfg = state.app_state.llm.local_delegate_config();
    let engine = toggles.agent_engine();
    let local_ready = mm_cfg.local_routable();
    let max_turn_bounds = crate::config::max_tool_iteration_bounds(
        &state.app_state.config.runtime_reliability_profile,
    );
    Ok(Json(json!({
        "ok": true,
        "tool_output_debug": toggles.tool_output_debug(),
        "post_tool_evaluator_enabled": toggles.post_tool_evaluator_enabled(),
        "response_quality_evaluator_enabled": toggles.response_quality_evaluator_enabled(),
        "agent_engine": engine.as_str(),
        "local_delegate_configured": mm_cfg.local_configured(),
        "local_delegate_tools_ok": mm_cfg.local_tools_ok,
        "local_delegate_ready": local_ready,
        "cost_routing_effective": crate::local_delegate::cost_routing_effective(engine, &mm_cfg),
        "terminal": web_terminal::capabilities_json(&state.app_state.config),
        "sources": {
            "tool_output_debug": if sources.0 { "app_settings" } else { "env" },
            "post_tool_evaluator_enabled": if sources.1 { "app_settings" } else { "env" },
            "response_quality_evaluator_enabled": if sources.2 { "app_settings" } else { "env" },
            "agent_engine": if sources.3.is_some() { "app_settings" } else { "env" },
        },
        "max_tool_iterations": toggles.max_tool_iterations(),
        "max_tool_iterations_floor": max_turn_bounds.0,
        "max_tool_iterations_ceiling": max_turn_bounds.1,
        "max_tokens": state.app_state.llm.max_tokens(),
        "max_tokens_min": crate::config::MAX_TOKENS_MIN,
        "max_tokens_max": crate::config::MAX_TOKENS_MAX,
        "description": "When enabled, verbose shell output is shown in chat (including background-job completion). When off, full logs are agent-only.",
    })))
}

async fn api_runtime_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<RuntimePatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    if let Some(requested) = body.max_tool_iterations {
        if !(1..=crate::config::MAX_TOOL_ITERATIONS_HARD_CAP).contains(&requested) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Max turns must be from 1 to {}.",
                    crate::config::MAX_TOOL_ITERATIONS_HARD_CAP
                ),
            ));
        }
    }
    if let Some(requested) = body.max_tokens {
        if !(crate::config::MAX_TOKENS_MIN..=crate::config::MAX_TOKENS_MAX).contains(&requested) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Max tokens must be from {} to {}.",
                    crate::config::MAX_TOKENS_MIN,
                    crate::config::MAX_TOKENS_MAX
                ),
            ));
        }
    }
    let toggles = &state.app_state.runtime_toggles;
    let mut messages: Vec<&str> = Vec::new();
    let mut max_turns_message: Option<String> = None;
    let mut max_tokens_message: Option<String> = None;

    if let Some(enabled) = body.tool_output_debug {
        toggles.set_tool_output_debug(enabled);
        call_blocking(state.app_state.db.clone(), move |db| {
            crate::runtime_toggles::RuntimeToggles::persist_tool_output_debug(db, enabled)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        messages.push(if enabled {
            "Verbose pipeline logging enabled."
        } else {
            "Verbose pipeline logging disabled."
        });
    }
    if let Some(enabled) = body.post_tool_evaluator_enabled {
        toggles.set_post_tool_evaluator_enabled(enabled);
        call_blocking(state.app_state.db.clone(), move |db| {
            crate::runtime_toggles::RuntimeToggles::persist_post_tool_evaluator_enabled(db, enabled)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        messages.push(if enabled {
            "Post-tool evaluator (PTE) enabled."
        } else {
            "Post-tool evaluator (PTE) disabled."
        });
    }
    if let Some(enabled) = body.response_quality_evaluator_enabled {
        toggles.set_response_quality_evaluator_enabled(enabled);
        call_blocking(state.app_state.db.clone(), move |db| {
            crate::runtime_toggles::RuntimeToggles::persist_response_quality_evaluator_enabled(
                db, enabled,
            )
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        messages.push(if enabled {
            "Pre-delivery quality evaluator (PDQE) enabled."
        } else {
            "Pre-delivery quality evaluator (PDQE) disabled."
        });
    }
    if let Some(ref engine_raw) = body.agent_engine {
        let engine = crate::runtime_toggles::AgentEngine::parse(engine_raw);
        toggles.set_agent_engine(engine);
        call_blocking(state.app_state.db.clone(), move |db| {
            crate::runtime_toggles::RuntimeToggles::persist_agent_engine(db, engine)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let routing_for_engine = engine == crate::runtime_toggles::AgentEngine::ClassicCostRouting;
        let mut mm_cfg = state.app_state.llm.local_delegate_config();
        if mm_cfg.routing_enabled != routing_for_engine {
            mm_cfg.routing_enabled = routing_for_engine;
            mm_cfg = mm_cfg.normalize();
            call_blocking(state.app_state.db.clone(), {
                let cfg_db = mm_cfg.clone();
                move |db| crate::local_delegate::persist_to_db(db, &cfg_db)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            state
                .app_state
                .llm
                .apply_local_delegate_config(mm_cfg.clone())
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }

        messages.push(match engine {
            crate::runtime_toggles::AgentEngine::GeminiAdk => {
                "Agent engine set to Gemini ADK (native multi-agent)."
            }
            crate::runtime_toggles::AgentEngine::Cursor => {
                "Agent engine set to Cursor SDK (local sidecar)."
            }
            crate::runtime_toggles::AgentEngine::ClassicCostRouting => {
                "Agent engine set to Classic · Cost routing."
            }
            crate::runtime_toggles::AgentEngine::Classic => {
                "Agent engine set to Classic · Single turn."
            }
        });
    }
    if let Some(requested) = body.max_tool_iterations {
        let profile = state.app_state.config.runtime_reliability_profile.as_str();
        let effective = crate::config::clamp_max_tool_iterations(profile, requested);
        toggles.set_max_tool_iterations(effective);
        let (floor, ceiling) = crate::config::max_tool_iteration_bounds(profile);
        max_turns_message = Some(if effective == requested {
            format!("Max turns set to {effective}. The next turn uses this limit.")
        } else if requested < floor {
            format!(
                "Max turns set to {effective}. This process does not go below {floor}. The next turn uses this limit."
            )
        } else {
            format!(
                "Max turns set to {effective}. This process does not go above {ceiling}. The next turn uses this limit."
            )
        });
    }
    if let Some(requested) = body.max_tokens {
        state
            .app_state
            .llm
            .apply_max_tokens(requested)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        max_tokens_message = Some(format!(
            "Max tokens set to {requested}. The next turn uses this limit."
        ));
    }

    let mm_cfg = state.app_state.llm.local_delegate_config();
    let engine = toggles.agent_engine();
    let local_ready = mm_cfg.local_routable();
    let cost_routing_effective = crate::local_delegate::cost_routing_effective(engine, &mm_cfg);
    let mut warnings: Vec<String> = Vec::new();
    if engine == crate::runtime_toggles::AgentEngine::ClassicCostRouting && !local_ready {
        if !mm_cfg.local_configured() {
            warnings.push(
                "Cost routing selected but local URL/model are not configured. Runs use cloud model only until configured and verified.".into(),
            );
        } else if !mm_cfg.local_tools_ok {
            warnings.push(
                "Cost routing selected but local tool calling is not verified. Runs use cloud model only until you run Test in Local delegate.".into(),
            );
        }
    }

    if messages.is_empty() && max_turns_message.is_none() && max_tokens_message.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No runtime toggle fields provided".to_string(),
        ));
    }
    let engine_name = engine.as_str();
    let debug_value = if toggles.tool_output_debug() {
        "true"
    } else {
        "false"
    };
    let pte_value = if toggles.post_tool_evaluator_enabled() {
        "true"
    } else {
        "false"
    };
    let pdqe_value = if toggles.response_quality_evaluator_enabled() {
        "true"
    } else {
        "false"
    };
    let max_turns_value = toggles.max_tool_iterations().to_string();
    let max_tokens_value = state.app_state.llm.max_tokens().to_string();
    if let Err(err) = env_config::persist_pairs(&[
        ("TOOL_OUTPUT_DEBUG", Some(debug_value)),
        ("POST_TOOL_EVALUATOR_ENABLED", Some(pte_value)),
        ("RESPONSE_QUALITY_EVALUATOR_ENABLED", Some(pdqe_value)),
        ("AGENT_ENGINE", Some(engine_name)),
        ("MAX_TOOL_ITERATIONS", Some(max_turns_value.as_str())),
        ("MAX_TOKENS", Some(max_tokens_value.as_str())),
    ]) {
        warnings.push(format!("Saved in the app. Could not update .env: {err}"));
    }
    let max_turn_bounds = crate::config::max_tool_iteration_bounds(
        &state.app_state.config.runtime_reliability_profile,
    );
    let mut message = messages.join(" ");
    for extra in [max_turns_message.as_deref(), max_tokens_message.as_deref()]
        .into_iter()
        .flatten()
    {
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(extra);
    }

    Ok(Json(json!({
        "ok": true,
        "tool_output_debug": toggles.tool_output_debug(),
        "post_tool_evaluator_enabled": toggles.post_tool_evaluator_enabled(),
        "response_quality_evaluator_enabled": toggles.response_quality_evaluator_enabled(),
        "agent_engine": engine.as_str(),
        "local_delegate_configured": mm_cfg.local_configured(),
        "local_delegate_tools_ok": mm_cfg.local_tools_ok,
        "local_delegate_ready": local_ready,
        "cost_routing_effective": cost_routing_effective,
        "max_tool_iterations": toggles.max_tool_iterations(),
        "max_tool_iterations_floor": max_turn_bounds.0,
        "max_tool_iterations_ceiling": max_turn_bounds.1,
        "max_tokens": state.app_state.llm.max_tokens(),
        "max_tokens_min": crate::config::MAX_TOKENS_MIN,
        "max_tokens_max": crate::config::MAX_TOKENS_MAX,
        "warnings": warnings,
        "source": "app_settings",
        "message": message,
    })))
}

async fn api_gemini_adk_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let settings = state
        .app_state
        .gemini_adk_settings
        .read()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "gemini_adk_settings lock poisoned".into(),
            )
        })?
        .clone();
    let api_key_configured =
        crate::gemini_adk::GeminiAdkSettings::api_key_configured(&state.app_state.config);
    let engine_ready = settings.engine_ready(&state.app_state.config);
    Ok(Json(json!({
        "ok": true,
        "default_model": settings.default_model,
        "max_iterations": settings.max_iterations,
        "api_key_configured": api_key_configured,
        "engine_ready": engine_ready,
        "agent_engine": state.app_state.runtime_toggles.agent_engine().as_str(),
        "schema_version": crate::gemini_adk::profile::SCHEMA_VERSION,
    })))
}

#[derive(Debug, Deserialize)]
struct GeminiAdkPatchRequest {
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    max_iterations: Option<usize>,
}

async fn api_gemini_adk_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<GeminiAdkPatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    if body.default_model.is_none() && body.max_iterations.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide default_model and/or max_iterations".into(),
        ));
    }
    let mut settings = state
        .app_state
        .gemini_adk_settings
        .read()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "gemini_adk_settings lock poisoned".into(),
            )
        })?
        .clone();
    if let Some(ref model) = body.default_model {
        let m = model.trim();
        if m.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "default_model must not be empty".into(),
            ));
        }
        call_blocking(state.app_state.db.clone(), {
            let m = m.to_string();
            move |db| crate::gemini_adk::config::persist_default_model(db, &m)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        settings.default_model = m.to_string();
    }
    if let Some(max) = body.max_iterations {
        if max == 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                "max_iterations must be >= 1".into(),
            ));
        }
        call_blocking(state.app_state.db.clone(), move |db| {
            crate::gemini_adk::config::persist_max_iterations(db, max)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        settings.max_iterations = max.min(200);
    }
    {
        let mut guard = state.app_state.gemini_adk_settings.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "gemini_adk_settings lock poisoned".into(),
            )
        })?;
        *guard = settings.clone();
    }
    Ok(Json(json!({
        "ok": true,
        "default_model": settings.default_model,
        "max_iterations": settings.max_iterations,
        "api_key_configured": crate::gemini_adk::GeminiAdkSettings::api_key_configured(&state.app_state.config),
        "engine_ready": settings.engine_ready(&state.app_state.config),
        "message": "Gemini ADK settings saved.",
    })))
}

#[derive(Debug, Deserialize)]
struct GeminiAdkTopologyQuery {
    #[serde(default)]
    persona_id: Option<i64>,
}

async fn api_gemini_adk_topology_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<GeminiAdkTopologyQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let global = state
        .app_state
        .gemini_adk_profile
        .read()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "gemini_adk_profile lock poisoned".into(),
            )
        })?
        .clone();
    let defaults = crate::gemini_adk::AdkTopologyProfile::default_profile();

    if let Some(pid) = query.persona_id {
        let persona = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(persona) = persona.filter(|p| p.chat_id == chat_id) else {
            return Err((StatusCode::NOT_FOUND, "persona not found".into()));
        };
        let uses_default = persona
            .gemini_adk_topology
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none();
        let profile = if uses_default {
            global.clone()
        } else {
            crate::gemini_adk::profile::parse_topology_json(
                persona.gemini_adk_topology.as_deref().unwrap_or(""),
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
        };
        return Ok(Json(json!({
            "ok": true,
            "scope": "persona",
            "persona_id": pid,
            "uses_default": uses_default,
            "profile": profile,
            "defaults": defaults,
            "global": global,
            "schema_version": crate::gemini_adk::profile::SCHEMA_VERSION,
        })));
    }

    Ok(Json(json!({
        "ok": true,
        "scope": "global",
        "uses_default": false,
        "profile": global,
        "defaults": defaults,
        "schema_version": crate::gemini_adk::profile::SCHEMA_VERSION,
    })))
}

#[derive(Debug, Deserialize)]
struct GeminiAdkTopologyPatchRequest {
    #[serde(default)]
    persona_id: Option<i64>,
    #[serde(default)]
    reset_defaults: bool,
    #[serde(default)]
    clear_persona_override: bool,
    #[serde(default)]
    profile: Option<crate::gemini_adk::AdkTopologyProfile>,
}

async fn api_gemini_adk_topology_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<GeminiAdkTopologyPatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;

    if let Some(pid) = body.persona_id {
        if body.clear_persona_override {
            call_blocking(state.app_state.db.clone(), move |db| {
                db.set_persona_gemini_adk_topology(chat_id, pid, None)
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let global = state
                .app_state
                .gemini_adk_profile
                .read()
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "gemini_adk_profile lock poisoned".into(),
                    )
                })?
                .clone();
            return Ok(Json(json!({
                "ok": true,
                "scope": "persona",
                "persona_id": pid,
                "uses_default": true,
                "profile": global,
                "message": "Persona Gemini ADK topology cleared; using global default.",
            })));
        }
        let new_profile = if body.reset_defaults {
            crate::gemini_adk::AdkTopologyProfile::default_profile()
        } else if let Some(p) = body.profile {
            p.migrate()
        } else {
            return Err((
                StatusCode::BAD_REQUEST,
                "Provide profile, reset_defaults=true, or clear_persona_override=true".into(),
            ));
        };
        if let Err(e) = new_profile.validate() {
            return Err((StatusCode::BAD_REQUEST, e));
        }
        let json = serde_json::to_string(&new_profile)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        call_blocking(state.app_state.db.clone(), {
            let json = json.clone();
            move |db| db.set_persona_gemini_adk_topology(chat_id, pid, Some(&json))
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(json!({
            "ok": true,
            "scope": "persona",
            "persona_id": pid,
            "uses_default": false,
            "profile": new_profile,
            "message": "Persona Gemini ADK topology saved.",
        })));
    }

    let new_profile = if body.reset_defaults {
        crate::gemini_adk::AdkTopologyProfile::default_profile()
    } else if let Some(p) = body.profile {
        p.migrate()
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide profile or reset_defaults=true".into(),
        ));
    };
    if let Err(e) = new_profile.validate() {
        return Err((StatusCode::BAD_REQUEST, e));
    }
    call_blocking(state.app_state.db.clone(), {
        let profile_db = new_profile.clone();
        move |db| crate::gemini_adk::profile::persist_to_db(db, &profile_db)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    {
        let mut guard = state.app_state.gemini_adk_profile.write().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "gemini_adk_profile lock poisoned".into(),
            )
        })?;
        *guard = new_profile.clone();
    }
    Ok(Json(json!({
        "ok": true,
        "scope": "global",
        "profile": new_profile,
        "message": if body.reset_defaults {
            "Global Gemini ADK topology reset to defaults."
        } else {
            "Global Gemini ADK topology saved."
        },
    })))
}

async fn api_llm_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<LlmGetQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let mut persona_id_out: Option<i64> = None;
    let mut model_scope = "global";

    let (
        settings_model,
        settings_provider,
        settings_base_url,
        thinking_source,
        show_thinking_source,
    ) = call_blocking(state.app_state.db.clone(), |db| {
        let settings = db.list_app_settings()?;
        let model_source = settings.iter().any(|s| {
            s.key
                .eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_MODEL)
                && !s.value.trim().is_empty()
        });
        let provider_source = settings.iter().any(|s| {
            s.key
                .eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_PROVIDER)
                && !s.value.trim().is_empty()
        });
        let base_url_source = settings.iter().any(|s| {
            s.key
                .eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_BASE_URL)
                && !s.value.trim().is_empty()
        });
        let thinking_source = settings.iter().any(|s| {
            s.key
                .eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_LLM_THINKING_ENABLED)
                && !s.value.trim().is_empty()
        });
        let show_thinking_source = settings.iter().any(|s| {
            s.key
                .eq_ignore_ascii_case(crate::llm_catalog::APP_SETTING_SHOW_THINKING)
                && !s.value.trim().is_empty()
        });
        Ok((
            model_source,
            provider_source,
            base_url_source,
            thinking_source,
            show_thinking_source,
        ))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let source_from_settings = |has: bool| -> &'static str {
        if has {
            "app_settings"
        } else {
            "default"
        }
    };

    let (strategy, provider_source, model_source, base_url_source) = if let Some(pid) =
        query.persona_id
    {
        let persona = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(persona) = persona.filter(|p| p.chat_id == chat_id) else {
            return Err((StatusCode::NOT_FOUND, "persona not found".into()));
        };
        let strategy = state
            .app_state
            .llm
            .resolve_strategy_selection(Some(&persona))
            .unwrap_or_else(|_| crate::llm::StrategyLlmSelection {
                provider_id: persona.llm_provider_override.clone().unwrap_or_default(),
                model: persona.model_override.clone().unwrap_or_default(),
                base_url: persona.llm_base_url_override.clone(),
                endpoint: String::new(),
                client: std::sync::Arc::from(crate::llm::create_provider(&state.app_state.config)),
                from_persona: true,
            });
        persona_id_out = Some(pid);
        let (provider_source, model_source, base_url_source) = if strategy.from_persona {
            model_scope = "persona";
            let provider_source = if persona
                .llm_provider_override
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                "persona"
            } else {
                source_from_settings(settings_provider)
            };
            let model_source = if persona
                .model_override
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                "persona"
            } else {
                source_from_settings(settings_model)
            };
            let base_url_source = if persona
                .llm_base_url_override
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                "persona"
            } else {
                source_from_settings(settings_base_url)
            };
            (provider_source, model_source, base_url_source)
        } else {
            (
                source_from_settings(settings_provider),
                source_from_settings(settings_model),
                source_from_settings(settings_base_url),
            )
        };
        (strategy, provider_source, model_source, base_url_source)
    } else {
        let strategy = crate::llm::StrategyLlmSelection {
            provider_id: String::new(),
            model: String::new(),
            base_url: None,
            endpoint: String::new(),
            client: std::sync::Arc::from(crate::llm::create_provider(&state.app_state.config)),
            from_persona: false,
        };
        (
            strategy,
            source_from_settings(settings_provider),
            source_from_settings(settings_model),
            source_from_settings(settings_base_url),
        )
    };

    let provider_id = strategy.provider_id.clone();
    let current_model = strategy.model.clone();
    let preset = crate::llm_catalog::find_provider(&provider_id);
    let is_local = crate::llm_catalog::is_local_provider(&provider_id);
    let default_base_url = if is_local {
        crate::llm_catalog::default_base_url_for_provider(&provider_id).map(|s| s.to_string())
    } else {
        None
    };
    let base_url = strategy.base_url.clone();

    let catalog_models = crate::llm_catalog::catalog_models_json(&provider_id, &current_model);
    let catalog: Vec<serde_json::Value> = catalog_models
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect();

    let in_catalog = catalog_models
        .iter()
        .any(|m| m.id == current_model && !m.from_active_config);

    let providers: Vec<serde_json::Value> = crate::llm_catalog::providers_catalog_json(
        &provider_id,
        &current_model,
        Some(&state.app_state.config.runtime_data_dir()),
    )
    .into_iter()
    .filter_map(|p| serde_json::to_value(p).ok())
    .collect();

    Ok(Json(json!({
        "ok": true,
        "provider": {
            "id": provider_id,
            "label": preset.map(|p| p.label).unwrap_or("Unknown provider"),
        },
        "provider_source": provider_source,
        "api_key_configured": crate::llm_catalog::is_api_key_configured_for_provider(&provider_id),
        "model": current_model,
        "model_in_catalog": in_catalog,
        "model_source": model_source,
        "model_scope": model_scope,
        "persona_id": persona_id_out,
        "is_local_provider": is_local,
        "base_url": base_url,
        "default_base_url": default_base_url,
        "base_url_source": if is_local {
            base_url_source
        } else {
            "n/a"
        },
        "catalog": catalog,
        "providers": providers,
        "catalog_source": "static_curated",
        "cost_reference_note": "Approximate USD per 1M tokens for curated ids — verify on your provider billing page. Model lists are loaded live from the provider (GET /api/llm/models); this payload is the curated fallback. Add provider keys under Settings → Connections.",
        "custom_model_allowed": true,
        "thinking_enabled": state.app_state.llm.thinking_enabled(),
        "thinking_source": if thinking_source {
            "app_settings"
        } else {
            "default"
        },
        "show_thinking": state.app_state.llm.show_thinking(),
        "show_thinking_source": if show_thinking_source {
            "app_settings"
        } else if std::env::var("SHOW_THINKING").is_ok() {
            "env"
        } else {
            "default"
        },
        "thinking_supported": provider_id == "google" || provider_id == "gemini",
    })))
}

async fn api_llm_models_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<LlmModelsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let provider_id = query
        .provider
        .as_deref()
        .map(crate::llm_catalog::resolve_catalog_provider_id)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| state.app_state.llm.current_provider());
    if crate::llm_catalog::find_provider(&provider_id).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Unknown provider {provider_id:?}"),
        ));
    }
    let current_provider = state.app_state.llm.current_provider();
    let current_model = if provider_id.eq_ignore_ascii_case(&current_provider) {
        state.app_state.llm.current_model()
    } else {
        String::new()
    };
    let is_local = crate::llm_catalog::is_local_provider(&provider_id);
    let base_url = if is_local {
        let raw = query
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| {
                if provider_id.eq_ignore_ascii_case(&current_provider) {
                    state.app_state.llm.current_base_url()
                } else {
                    None
                }
            });
        raw.map(|u| crate::llm_catalog::normalize_local_base_url(&u, &provider_id))
    } else {
        None
    };
    if is_local && base_url.as_deref().unwrap_or("").is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "base_url is required for Ollama and llama.cpp providers".into(),
        ));
    }

    let api_key = crate::llm_catalog::resolve_api_key_for_provider_with_config(
        &provider_id,
        Some(&state.app_state.config),
    );
    let static_models = crate::llm_catalog::catalog_models_json(&provider_id, &current_model);

    let live_result =
        crate::llm::fetch_live_provider_models(&provider_id, &api_key, base_url.as_deref()).await;

    let (source, models, truncated, live_count, message) = match live_result {
        Ok(ids) => {
            let merged =
                crate::llm_catalog::merge_live_model_ids(&provider_id, &current_model, &ids);
            (
                "live",
                merged.models,
                merged.truncated,
                Some(merged.live_count),
                None,
            )
        }
        Err(err) => ("static_fallback", static_models, false, None, Some(err)),
    };

    let models_json: Vec<serde_json::Value> = models
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect();

    Ok(Json(json!({
        "ok": true,
        "provider": provider_id,
        "source": source,
        "truncated": truncated,
        "live_count": live_count,
        "models": models_json,
        "base_url": base_url,
        "message": message,
    })))
}

async fn api_llm_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<LlmModelPatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let model = body.model.trim().to_string();
    let thinking_only = body.persona_id.is_none()
        && model.is_empty()
        && (body.thinking_enabled.is_some() || body.show_thinking.is_some());
    if model.is_empty() && !thinking_only {
        return Err((StatusCode::BAD_REQUEST, "model is required".into()));
    }
    if model.len() > 256 {
        return Err((StatusCode::BAD_REQUEST, "model id is too long".into()));
    }
    let _ = body.custom;
    let provider_id = body
        .provider
        .as_deref()
        .map(crate::llm_catalog::resolve_catalog_provider_id)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| state.app_state.llm.current_provider());
    let local_base_url = if crate::llm_catalog::is_local_provider(&provider_id) {
        Some(
            body.base_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "base_url is required for Ollama and llama.cpp providers".into(),
                ))?
                .to_string(),
        )
    } else {
        None
    };

    let thinking_enabled = body
        .thinking_enabled
        .unwrap_or_else(|| state.app_state.llm.thinking_enabled());
    let show_thinking = body
        .show_thinking
        .unwrap_or_else(|| state.app_state.llm.show_thinking());

    // Thinking toggles remain global even when model selection is persona-scoped.
    if body.thinking_enabled.is_some() || body.show_thinking.is_some() {
        state
            .app_state
            .llm
            .apply_thinking_settings(thinking_enabled, show_thinking)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        call_blocking(state.app_state.db.clone(), move |db| {
            db.set_app_setting(
                crate::llm_catalog::APP_SETTING_LLM_THINKING_ENABLED,
                if thinking_enabled { "true" } else { "false" },
            )?;
            db.set_app_setting(
                crate::llm_catalog::APP_SETTING_SHOW_THINKING,
                if show_thinking { "true" } else { "false" },
            )?;
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(pid) = body.persona_id {
        let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
        let (provider_saved, model_saved, base_url_db) = state
            .app_state
            .llm
            .validate_selection(provider_id.clone(), model.clone(), local_base_url)
            .map_err(|e| {
                if e.contains("No API key") || e.contains("base_url") || e.contains("Unknown") {
                    (StatusCode::BAD_REQUEST, e)
                } else {
                    (StatusCode::INTERNAL_SERVER_ERROR, e)
                }
            })?;

        let provider_db = provider_saved.clone();
        let model_db = model_saved.clone();
        let base_for_db = base_url_db.clone();
        let ok = call_blocking(state.app_state.db.clone(), move |db| {
            let persona = db.get_persona(pid)?;
            if persona.as_ref().is_none_or(|p| p.chat_id != chat_id) {
                return Ok(false);
            }
            db.set_persona_llm_selection(
                chat_id,
                pid,
                Some(&provider_db),
                Some(&model_db),
                base_for_db.as_deref(),
            )
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !ok {
            return Err((StatusCode::NOT_FOUND, "persona not found".into()));
        }

        return Ok(Json(json!({
            "ok": true,
            "provider": {
                "id": provider_saved,
            },
            "model": model_saved,
            "base_url": base_url_db,
            "thinking_enabled": thinking_enabled,
            "show_thinking": show_thinking,
            "provider_source": "persona",
            "model_source": "persona",
            "model_scope": "persona",
            "persona_id": pid,
            "base_url_source": if base_url_db.is_some() {
                "persona"
            } else {
                "n/a"
            },
            "message": "Provider and model saved for this persona. New agent runs for this persona use this selection immediately.",
        })));
    }

    if thinking_only {
        return Ok(Json(json!({
            "ok": true,
            "thinking_enabled": thinking_enabled,
            "show_thinking": show_thinking,
            "message": "Thinking settings saved.",
        })));
    }
    Err((
        StatusCode::BAD_REQUEST,
        "Set the chat model on the Persona page.".into(),
    ))
}

#[derive(Debug, Deserialize)]
struct MultimodelTestRequest {
    tier: String,
    base_url: String,
    model: String,
    #[serde(default)]
    persona_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MultimodelModelsQuery {
    base_url: String,
}

async fn api_multimodel_models_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<MultimodelModelsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let base_url_raw = query.base_url.trim();
    if base_url_raw.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "base_url query parameter is required".into(),
        ));
    }
    let base_url = crate::local_delegate::normalize_base_url_for_provider(base_url_raw, "");
    if base_url.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "base_url query parameter is required".into(),
        ));
    }
    match crate::llm::fetch_openai_compatible_models(&base_url).await {
        Ok(models) => Ok(Json(json!({
            "ok": true,
            "models": models,
            "base_url": base_url,
        }))),
        Err(e) => Err((StatusCode::BAD_GATEWAY, e)),
    }
}

async fn api_multimodel_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let cfg = state.app_state.llm.local_delegate_config();
    let strategy_provider =
        crate::llm_catalog::resolve_catalog_provider_id(&state.app_state.llm.current_provider());
    let strategy_model = state.app_state.llm.current_model();
    Ok(Json(json!({
        "ok": true,
        "routing_enabled": cfg.routing_enabled,
        "enabled": cfg.routing_enabled,
        "local_base_url": cfg.local_base_url,
        "local_model": cfg.local_model,
        "local_tools_ok": cfg.local_tools_ok,
        "tier1_base_url": cfg.tier1_base_url,
        "tier1_model": cfg.tier1_model,
        "tier2_base_url": cfg.tier2_base_url,
        "tier2_model": cfg.tier2_model,
        "tier1_tools_ok": cfg.tier1_tools_ok,
        "tier2_tools_ok": cfg.tier2_tools_ok,
        "strategy_provider": strategy_provider,
        "strategy_model": strategy_model,
        "description": "Local OpenAI-compatible endpoint for cost routing, PTE/PDQE, and local tool phases.",
    })))
}

async fn api_multimodel_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<crate::local_delegate::LocalDelegatePatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let old = state.app_state.llm.local_delegate_config();
    let mut cfg = old.clone();
    if let Some(routing) = body.routing_enabled.or(body.enabled) {
        cfg.routing_enabled = routing;
    }
    // New unified local fields
    if let Some(ref url) = body.local_base_url {
        cfg.local_base_url = url.trim().to_string();
    }
    if let Some(ref model) = body.local_model {
        cfg.local_model = model.trim().to_string();
    }
    // Legacy fields (accepted for backward compat)
    if let Some(ref url) = body.tier1_base_url {
        cfg.tier1_base_url = url.trim().to_string();
    }
    if let Some(ref model) = body.tier1_model {
        cfg.tier1_model = model.trim().to_string();
    }
    if let Some(ref url) = body.tier2_base_url {
        cfg.tier2_base_url = url.trim().to_string();
    }
    if let Some(ref model) = body.tier2_model {
        cfg.tier2_model = model.trim().to_string();
    }
    // Invalidate tools_ok if local config changed
    let local_changed = body.local_base_url.is_some() || body.local_model.is_some();
    if local_changed
        && (old.local_base_url != cfg.local_base_url || old.local_model != cfg.local_model)
    {
        cfg.local_tools_ok = false;
    }
    cfg = cfg.normalize();
    if cfg.routing_enabled && !cfg.local_configured() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Local model base URL and model are required to enable cost routing.".into(),
        ));
    }
    if cfg.routing_enabled && !cfg.local_tools_ok {
        return Err((
            StatusCode::BAD_REQUEST,
            "Run the tool-calling test for the local model before enabling cost routing.".into(),
        ));
    }
    call_blocking(state.app_state.db.clone(), {
        let cfg_db = cfg.clone();
        move |db| crate::local_delegate::persist_to_db(db, &cfg_db)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state
        .app_state
        .llm
        .apply_local_delegate_config(cfg.clone())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(json!({
        "ok": true,
        "routing_enabled": cfg.routing_enabled,
        "enabled": cfg.routing_enabled,
        "local_base_url": cfg.local_base_url,
        "local_model": cfg.local_model,
        "local_tools_ok": cfg.local_tools_ok,
        "message": if cfg.routing_enabled {
            "Local delegate routing enabled."
        } else {
            "Local delegate routing disabled."
        },
    })))
}

async fn api_multimodel_test_post(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<MultimodelTestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let tier = match body.tier.trim().to_ascii_lowercase().as_str() {
        "local" | "technical" | "tier1" | "1" | "knowledge" | "tier2" | "2" => {
            crate::local_delegate::ModelTier::LocalReadOnly
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown tier {:?}. Use 'local'.", other),
            ));
        }
    };
    let model = body.model.trim();
    let base_url_raw = body.base_url.trim();
    if model.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "model is required".into()));
    }
    if base_url_raw.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "base_url is required".into()));
    }
    let label = "local";
    let fallback_base = "";
    let base_url =
        crate::local_delegate::normalize_base_url_for_provider(base_url_raw, fallback_base);
    let base = state.app_state.config.clone();
    let mut test_cfg = base;
    test_cfg.llm_provider = "llama".into();
    test_cfg.api_key = String::new();
    test_cfg.model = model.to_string();
    test_cfg.llm_base_url = Some(base_url.clone());
    match crate::llm::test_model(&test_cfg, model).await {
        Ok(()) => {}
        Err(e) => return Err((StatusCode::BAD_GATEWAY, e)),
    }
    match crate::llm::test_local_delegate_tools(&test_cfg, model, tier).await {
        Ok(()) => {
            if let Some(pid) = body.persona_id {
                let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
                let base_for_db = base_url.clone();
                let model_for_db = model.to_string();
                let saved = call_blocking(state.app_state.db.clone(), move |db| {
                    db.set_persona_local_delegate(
                        chat_id,
                        pid,
                        Some(&base_for_db),
                        Some(&model_for_db),
                        Some(true),
                    )
                })
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                if !saved {
                    return Err((StatusCode::NOT_FOUND, "persona not found".into()));
                }
            } else {
                call_blocking(state.app_state.db.clone(), move |db| {
                    crate::local_delegate::persist_tier_tools_ok(db, tier, true)
                })
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            Ok(Json(json!({
                "ok": true,
                "tier": label,
                "model": model,
                "base_url": base_url,
                "tools_ok": true,
                "message": format!("{label} tier reachable and tool-calling verified at {base_url}."),
            })))
        }
        Err(e) => Err((
            StatusCode::BAD_GATEWAY,
            format!("Server reachable but tool-calling probe failed: {e}"),
        )),
    }
}

fn cursor_engine_json(
    cfg: &crate::cursor_engine_config::CursorEngineSettings,
    health: &crate::cursor_engine_config::SidecarHealth,
    agent_engine: &str,
    sidecar_managed: bool,
    web_port: u16,
    web_enabled: bool,
    persona_id: Option<i64>,
    model_scope: &str,
) -> serde_json::Value {
    json!({
        "ok": true,
        "sdk_runner_url": cfg.sdk_runner_url,
        "sdk_model": cfg.sdk_model,
        "sdk_model_params": cfg.sdk_model_params,
        "sdk_runner_ok": cfg.sdk_runner_ok,
        "sidecar_reachable": health.reachable,
        "api_key_configured": health.api_key_configured,
        "engine_ready": cfg.engine_ready(health),
        "sidecar_managed": sidecar_managed,
        "agent_engine": agent_engine,
        "persona_id": persona_id,
        "model_scope": model_scope,
        "cli_path": cfg.cli_path,
        "cli_model": cfg.cli_model,
        "cli_runner_url": cfg.cli_runner_url,
        "cli_on_path": crate::cursor_engine_config::cli_on_path(&cfg.cli_path),
        "timeout_secs": cfg.timeout_secs,
        "interactive_timeout_secs": cfg.interactive_timeout_secs,
        "tmux_enabled": cfg.tmux_enabled,
        "mcp_tools_enabled": cfg.mcp_tools_enabled,
        "mcp_expose_send_message": cfg.mcp_expose_send_message,
        "delegation_slim_prompt": cfg.delegation_slim_prompt,
        "delegation_resume_delta": cfg.delegation_resume_delta,
        "mcp_endpoint_url": crate::cursor_mcp_bridge::mcp_endpoint_url(web_port),
        "mcp_bridge_ready": web_enabled && cfg.mcp_tools_enabled,
        "install_steps": [
            "Set CURSOR_API_KEY in repo-root .env (Cursor Dashboard → Integrations)",
            "Restart the bot — it installs @cursor/sdk into a runtime Node prefix",
            "Optional: CURSOR_SDK_AUTO_INSTALL=false to manage Node deps yourself",
            "Rollback: CURSOR_SDK_RUNNER_SCRIPT=scripts/cursor-sdk-runner.py",
        ],
        "sidecar_error": health.error,
    })
}

#[derive(Debug, Deserialize)]
struct CursorEngineQuery {
    #[serde(default)]
    persona_id: Option<i64>,
}

async fn api_cursor_engine_get(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<CursorEngineQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let mut cfg = state
        .app_state
        .cursor_settings
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .clone();
    let mut model_scope = "global";
    let mut persona_id_out = None;
    if let Some(pid) = query.persona_id {
        let persona = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(persona) = persona.filter(|p| p.chat_id == chat_id) else {
            return Err((StatusCode::NOT_FOUND, "persona not found".into()));
        };
        let had_persona_model = persona
            .cursor_sdk_model
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
            || persona
                .cursor_sdk_model_params
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty());
        cfg = crate::cursor_engine_config::apply_persona_cursor_model_override(
            cfg,
            persona.cursor_sdk_model.as_deref(),
            persona.cursor_sdk_model_params.as_deref(),
        );
        persona_id_out = Some(pid);
        if had_persona_model {
            model_scope = "persona";
        }
    }
    let health = crate::cursor_engine_config::probe_sidecar_health(&cfg.sdk_runner_url).await;
    let global_engine = state.app_state.runtime_toggles.agent_engine();
    let agent_engine = if let Some(pid) = persona_id_out {
        call_blocking(state.app_state.db.clone(), move |db| {
            Ok(
                crate::runtime_toggles::resolve_run_agent_engine_from_persona(
                    db,
                    chat_id,
                    pid,
                    global_engine,
                )
                .as_str()
                .to_string(),
            )
        })
        .await
        .unwrap_or_else(|_| global_engine.as_str().to_string())
    } else {
        global_engine.as_str().to_string()
    };
    Ok(Json(cursor_engine_json(
        &cfg,
        &health,
        &agent_engine,
        state.app_state.cursor_sidecar.managed_locally,
        state.app_state.config.web_port,
        state.app_state.config.web_enabled,
        persona_id_out,
        model_scope,
    )))
}

async fn api_cursor_engine_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<crate::cursor_engine_config::CursorEnginePatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(None, &state.app_state.config)?;
    let mut cfg = state
        .app_state
        .cursor_settings
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .clone();

    if let Some(ref url) = body.sdk_runner_url {
        let trimmed = url.trim().to_string();
        if !trimmed.is_empty() {
            crate::cursor_engine_config::validate_runner_url(&trimmed)
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        }
        if trimmed != cfg.sdk_runner_url {
            cfg.sdk_runner_ok = false;
        }
        cfg.sdk_runner_url = trimmed;
    }
    let persona_model_patch = body.persona_id.is_some();
    let persona_sdk_model = body.sdk_model.clone();
    let persona_sdk_params = body.sdk_model_params.clone();
    if !persona_model_patch {
        if let Some(ref model) = body.sdk_model {
            cfg.sdk_model = model.trim().to_string();
        }
        if let Some(params) = body.sdk_model_params.clone() {
            cfg.sdk_model_params = params
                .into_iter()
                .filter(|p| !p.id.trim().is_empty() && !p.value.trim().is_empty())
                .collect();
        }
    }
    if let Some(ref path) = body.cli_path {
        cfg.cli_path = path.trim().to_string();
    }
    if let Some(ref model) = body.cli_model {
        cfg.cli_model = model.trim().to_string();
    }
    if let Some(ref url) = body.cli_runner_url {
        let trimmed = url.trim().to_string();
        if !trimmed.is_empty() {
            crate::cursor_engine_config::validate_runner_url(&trimmed)
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        }
        cfg.cli_runner_url = trimmed;
    }
    if let Some(secs) = body.timeout_secs {
        cfg.timeout_secs = secs.clamp(60, 86_400);
    } else if let Some(secs) = body.interactive_timeout_secs {
        cfg.timeout_secs = secs.clamp(60, 86_400);
    }
    cfg.interactive_timeout_secs = cfg.timeout_secs;
    if let Some(enabled) = body.tmux_enabled {
        cfg.tmux_enabled = enabled;
    }
    if let Some(enabled) = body.mcp_tools_enabled {
        cfg.mcp_tools_enabled = enabled;
    }
    if let Some(enabled) = body.mcp_expose_send_message {
        cfg.mcp_expose_send_message = enabled;
    }
    if let Some(enabled) = body.delegation_slim_prompt {
        cfg.delegation_slim_prompt = enabled;
    }
    cfg.delegation_resume_delta = false;

    if !persona_model_patch && cfg.sdk_model.trim().is_empty() {
        cfg.sdk_model = crate::config::default_cursor_sdk_model();
    }
    if cfg.cli_path.trim().is_empty() {
        cfg.cli_path = crate::config::default_cursor_agent_cli_path();
    }

    call_blocking(state.app_state.db.clone(), {
        let cfg_db = cfg.clone();
        move |db| crate::cursor_engine_config::persist_to_db(db, &cfg_db)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    {
        let mut guard = state
            .app_state
            .cursor_settings
            .write()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        *guard = cfg.clone();
    }

    let mut model_scope = "global";
    let mut persona_id_out = None;
    if let Some(pid) = body.persona_id {
        let existing = call_blocking(state.app_state.db.clone(), move |db| db.get_persona(pid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .filter(|p| p.chat_id == chat_id);
        // Explicit empty string / empty params clears persona override (falls back to global).
        // Omitted fields keep the existing persona values.
        let model_for_db = match persona_sdk_model.as_ref() {
            Some(s) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            None => existing
                .as_ref()
                .and_then(|p| p.cursor_sdk_model.clone())
                .filter(|s| !s.trim().is_empty()),
        };
        let params_json = match persona_sdk_params.as_ref() {
            Some(params) => {
                let filtered: Vec<_> = params
                    .iter()
                    .filter(|p| !p.id.trim().is_empty() && !p.value.trim().is_empty())
                    .cloned()
                    .collect();
                if filtered.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(&filtered).unwrap_or_else(|_| "[]".into()))
                }
            }
            None => existing
                .as_ref()
                .and_then(|p| p.cursor_sdk_model_params.clone())
                .filter(|s| !s.trim().is_empty()),
        };
        call_blocking(state.app_state.db.clone(), {
            let model = model_for_db.clone();
            let params_json = params_json.clone();
            move |db| {
                db.set_persona_cursor_sdk_model(
                    chat_id,
                    pid,
                    model.as_deref(),
                    params_json.as_deref(),
                )
            }
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        cfg = crate::cursor_engine_config::apply_persona_cursor_model_override(
            cfg,
            model_for_db.as_deref(),
            params_json.as_deref(),
        );
        if model_for_db.is_some() || params_json.is_some() {
            model_scope = "persona";
        }
        persona_id_out = Some(pid);
    }

    let health = crate::cursor_engine_config::probe_sidecar_health(&cfg.sdk_runner_url).await;
    let global_engine = state.app_state.runtime_toggles.agent_engine();
    let agent_engine = if let Some(pid) = persona_id_out {
        call_blocking(state.app_state.db.clone(), move |db| {
            Ok(
                crate::runtime_toggles::resolve_run_agent_engine_from_persona(
                    db,
                    chat_id,
                    pid,
                    global_engine,
                )
                .as_str()
                .to_string(),
            )
        })
        .await
        .unwrap_or_else(|_| global_engine.as_str().to_string())
    } else {
        global_engine.as_str().to_string()
    };
    let mut out = cursor_engine_json(
        &cfg,
        &health,
        &agent_engine,
        state.app_state.cursor_sidecar.managed_locally,
        state.app_state.config.web_port,
        state.app_state.config.web_enabled,
        persona_id_out,
        model_scope,
    );
    if let serde_json::Value::Object(ref mut map) = out {
        map.insert(
            "message".into(),
            serde_json::Value::String(
                if persona_id_out.is_some() {
                    "Cursor settings saved (model/params for this persona)."
                } else {
                    "Cursor settings saved."
                }
                .into(),
            ),
        );
    }
    Ok(Json(out))
}

async fn api_cursor_engine_health_post(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let mut cfg = state
        .app_state
        .cursor_settings
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .clone();
    let health = crate::cursor_engine_config::probe_sidecar_health(&cfg.sdk_runner_url).await;

    let ok = health.reachable && health.api_key_configured;
    if ok {
        cfg.sdk_runner_ok = true;
        call_blocking(state.app_state.db.clone(), {
            let cfg_db = cfg.clone();
            move |db| crate::cursor_engine_config::persist_to_db(db, &cfg_db)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Ok(mut guard) = state.app_state.cursor_settings.write() {
            *guard = cfg.clone();
        }
    }

    let message = if ok {
        "Sidecar reachable and CURSOR_API_KEY is configured on the host.".to_string()
    } else if !health.reachable {
        health
            .error
            .clone()
            .unwrap_or_else(|| "Sidecar is not reachable.".into())
    } else {
        "Sidecar is up but CURSOR_API_KEY is not set on the host.".into()
    };

    let agent_engine = state.app_state.runtime_toggles.agent_engine().as_str();
    let mut out = cursor_engine_json(
        &cfg,
        &health,
        agent_engine,
        state.app_state.cursor_sidecar.managed_locally,
        state.app_state.config.web_port,
        state.app_state.config.web_enabled,
        None,
        "global",
    );
    if let serde_json::Value::Object(ref mut map) = out {
        map.insert("message".into(), serde_json::Value::String(message));
        map.insert("health_ok".into(), serde_json::Value::Bool(ok));
    }
    Ok(Json(out))
}

async fn api_cursor_mcp_post(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    State(state): State<WebState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    crate::cursor_mcp_bridge::handle_cursor_mcp(addr, State(state.app_state.clone()), headers, body)
        .await
}

async fn api_cursor_mcp_get(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> axum::response::Response {
    crate::cursor_mcp_bridge::handle_cursor_mcp_get(addr, headers).await
}

async fn api_cursor_engine_models_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let cfg = state
        .app_state
        .cursor_settings
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .clone();
    match crate::cursor_engine_config::fetch_sidecar_model_catalog(&cfg.sdk_runner_url).await {
        Ok(models) => Ok(Json(json!({
            "ok": true,
            "models": models,
        }))),
        Err(e) => Err((StatusCode::BAD_GATEWAY, e)),
    }
}

async fn api_settings_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let settings = call_blocking(state.app_state.db.clone(), move |db| db.list_app_settings())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items: Vec<serde_json::Value> = settings
        .into_iter()
        .filter(|s| !crate::config::is_llm_related_runtime_setting_key(&s.key))
        .filter(|s| {
            !s.key
                .eq_ignore_ascii_case(crate::runtime_toggles::APP_SETTING_TOOL_OUTPUT_DEBUG)
        })
        .filter(|s| {
            !s.key.eq_ignore_ascii_case(
                crate::runtime_toggles::APP_SETTING_POST_TOOL_EVALUATOR_ENABLED,
            )
        })
        .filter(|s| {
            !s.key.eq_ignore_ascii_case(
                crate::runtime_toggles::APP_SETTING_RESPONSE_QUALITY_EVALUATOR_ENABLED,
            )
        })
        .filter(|s| {
            !matches!(
                s.key.trim().to_ascii_uppercase().as_str(),
                crate::channel_integration_config::APP_SETTING_BOT_USERNAME
                    | crate::channel_integration_config::APP_SETTING_ALLOWED_GROUPS
                    | crate::channel_integration_config::APP_SETTING_CONTROL_CHAT_IDS
                    | crate::channel_integration_config::APP_SETTING_DISCORD_ALLOWED_CHANNELS
                    | crate::channel_integration_config::APP_SETTING_WHATSAPP_PHONE_NUMBER_ID
                    | crate::channel_integration_config::APP_SETTING_WHATSAPP_VERIFY_TOKEN
                    | crate::channel_integration_config::APP_SETTING_WHATSAPP_WEBHOOK_PORT
                    | crate::channel_integration_config::APP_SETTING_CHANNEL_INTEGRATION_SEEDED
            )
        })
        .map(|s| {
            let secret = setting_is_secret(&s.key);
            json!({
                "key": s.key,
                "value": if secret { mask_setting_value(&s.value) } else { s.value.clone() },
                "raw_value": s.value,
                "is_secret": secret,
                "updated_at": s.updated_at,
                "source": "runtime_db",
            })
        })
        .collect();
    items.sort_by(|a, b| {
        let ak = a
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bk = b
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        ak.cmp(&bk)
    });

    let channel_ready = call_blocking(state.app_state.db.clone(), |db| {
        Ok::<bool, crate::error::FinallyAValueBotError>(is_channel_ready(db))
    })
    .await
    .unwrap_or(false);
    let cfg = &state.app_state.config;
    let cursor_cfg = state
        .app_state
        .cursor_settings
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .clone();
    let cursor_engine_ready = cursor_cfg.sdk_configured() && cursor_cfg.sdk_runner_ok;
    let mm_cfg = state.app_state.llm.local_delegate_config();
    let agent_engine = state.app_state.runtime_toggles.agent_engine();
    let local_delegate_ready = mm_cfg.local_routable();
    let installation = env_config::installation_flags(&state).await;
    Ok(Json(json!({
        "ok": true,
        "settings": items,
        "bootstrap": {
            "workspace_dir": cfg.workspace_dir,
            "web_enabled": cfg.web_enabled,
            "web_host": cfg.web_host,
            "web_port": cfg.web_port,
            "web_auth_token_set": cfg.web_auth_token.as_ref().is_some_and(|v| !v.trim().is_empty()),
        },
        "installation_status": {
            "llm_ready": is_llm_ready(cfg),
            "channel_ready": channel_ready,
            "cursor_engine_ready": cursor_engine_ready,
            "local_delegate_ready": local_delegate_ready,
            "agent_engine": agent_engine.as_str(),
            "cost_routing_effective": crate::local_delegate::cost_routing_effective(agent_engine, &mm_cfg),
            "web_enabled": cfg.web_enabled,
            "needs_initialization": installation.needs_initialization,
            "workspace_configured": installation.workspace_configured,
            "requires_restart_for_env_changes": env_config::note_disk_changes(&state).await,
            "runtime_env_merge_from_app_settings": true,
            "llm_model_from_app_settings": true,
            "tool_output_debug": state.app_state.runtime_toggles.tool_output_debug(),
            "post_tool_evaluator_enabled": state.app_state.runtime_toggles.post_tool_evaluator_enabled(),
            "response_quality_evaluator_enabled": state.app_state.runtime_toggles.response_quality_evaluator_enabled(),
            "terminal": web_terminal::capabilities_json(cfg),
        }
    })))
}

async fn api_settings_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(_): Json<SettingsPatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "Persisting generic key/value settings to SQLite is disabled. Configure the process via repo-root .env (or process environment). The app_settings table is legacy and not merged at startup.".to_string(),
    ))
}

/// Restart the running Meido process. The page disconnects until the new process binds.
async fn api_restart_post(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    crate::daemon::schedule_app_restart();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "message": "Restart scheduled. The page may disconnect briefly."
        })),
    ))
}

fn json_channel_bot_instance_redacted(inst: &ChannelBotInstance) -> serde_json::Value {
    let masked = mask_setting_value(&inst.token);
    json!({
        "id": inst.id,
        "platform": inst.platform,
        "label": inst.label,
        "token_set": !inst.token.trim().is_empty(),
        "token_redacted": masked,
        "bot_username": inst.bot_username,
        "allowed_groups": inst.allowed_groups.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
        "discord_allowed_channels": inst.discord_allowed_channels.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
        "whatsapp_phone_number_id": inst.whatsapp_phone_number_id,
        "whatsapp_verify_token_set": !inst.whatsapp_verify_token.trim().is_empty(),
        "whatsapp_verify_token_redacted": if inst.whatsapp_verify_token.trim().is_empty() {
            String::new()
        } else {
            mask_setting_value(&inst.whatsapp_verify_token)
        },
        "whatsapp_webhook_port": inst.whatsapp_webhook_port,
        "wecom_corp_id": inst.wecom_corp_id,
        "wecom_agent_id": inst.wecom_agent_id,
        "wecom_callback_token_set": !inst.wecom_callback_token.trim().is_empty(),
        "wecom_callback_token_redacted": if inst.wecom_callback_token.trim().is_empty() {
            String::new()
        } else {
            mask_setting_value(&inst.wecom_callback_token)
        },
        "wecom_encoding_aes_key_set": !inst.wecom_encoding_aes_key.trim().is_empty(),
        "wecom_encoding_aes_key_redacted": if inst.wecom_encoding_aes_key.trim().is_empty() {
            String::new()
        } else {
            mask_setting_value(&inst.wecom_encoding_aes_key)
        },
        "wecom_webhook_port": inst.wecom_webhook_port,
        "wecom_allowed_chats": inst.wecom_allowed_chats,
        "wecom_aibot_id": inst.wecom_aibot_id,
        "wecom_mode": inst.wecom_mode,
        "created_at": inst.created_at,
        "env_primary": false,
        "is_primary": matches!(
            inst.id,
            crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY
                | crate::db::BOT_INSTANCE_DISCORD_PRIMARY
                | crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY
                | crate::db::BOT_INSTANCE_WECOM_PRIMARY
        ),
    })
}

#[derive(Debug, Deserialize)]
struct ChannelBotInstanceCreateRequest {
    platform: String,
    label: String,
    token: String,
    #[serde(default)]
    bot_username: Option<String>,
    #[serde(default)]
    allowed_groups: Option<String>,
    #[serde(default)]
    discord_allowed_channels: Option<String>,
    #[serde(default)]
    whatsapp_phone_number_id: Option<String>,
    #[serde(default)]
    whatsapp_verify_token: Option<String>,
    #[serde(default)]
    whatsapp_webhook_port: Option<u16>,
    #[serde(default)]
    wecom_corp_id: Option<String>,
    #[serde(default)]
    wecom_agent_id: Option<i64>,
    #[serde(default)]
    wecom_callback_token: Option<String>,
    #[serde(default)]
    wecom_encoding_aes_key: Option<String>,
    #[serde(default)]
    wecom_webhook_port: Option<u16>,
    #[serde(default)]
    wecom_allowed_chats: Option<String>,
    #[serde(default)]
    wecom_aibot_id: Option<String>,
    #[serde(default)]
    wecom_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChannelBotInstanceUpdateRequest {
    label: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    bot_username: Option<String>,
    #[serde(default)]
    allowed_groups: Option<String>,
    #[serde(default)]
    discord_allowed_channels: Option<String>,
    #[serde(default)]
    whatsapp_phone_number_id: Option<String>,
    #[serde(default)]
    whatsapp_verify_token: Option<String>,
    #[serde(default)]
    whatsapp_webhook_port: Option<u16>,
    #[serde(default)]
    wecom_corp_id: Option<String>,
    #[serde(default)]
    wecom_agent_id: Option<i64>,
    #[serde(default)]
    wecom_callback_token: Option<String>,
    #[serde(default)]
    wecom_encoding_aes_key: Option<String>,
    #[serde(default)]
    wecom_webhook_port: Option<u16>,
    #[serde(default)]
    wecom_allowed_chats: Option<String>,
    #[serde(default)]
    wecom_aibot_id: Option<String>,
    #[serde(default)]
    wecom_mode: Option<String>,
}

fn parse_id_list_i64(raw: &str) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    let mut invalid = Vec::new();
    for part in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let token = token
            .strip_prefix("chat:")
            .or_else(|| token.strip_prefix("user:"))
            .unwrap_or(token)
            .trim_start_matches('+');
        match token.parse::<i64>() {
            Ok(id) => ids.push(id),
            Err(_) => invalid.push(token.to_string()),
        }
    }
    if !invalid.is_empty() {
        return Err(format!(
            "Invalid chat IDs (numeric Telegram IDs, comma-separated): {}",
            invalid.join(", ")
        ));
    }
    Ok(ids)
}

fn parse_id_list_u64(raw: &str) -> Result<Vec<u64>, String> {
    let mut ids = Vec::new();
    let mut invalid = Vec::new();
    for part in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        match token.parse::<u64>() {
            Ok(id) => ids.push(id),
            Err(_) => invalid.push(token.to_string()),
        }
    }
    if !invalid.is_empty() {
        return Err(format!(
            "Invalid channel IDs (numeric Discord IDs, comma-separated): {}",
            invalid.join(", ")
        ));
    }
    Ok(ids)
}

fn looks_like_masked_secret(value: &str) -> bool {
    let v = value.trim();
    v.contains("***") || v == "***"
}

async fn api_channel_bot_instances_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let list = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_all_channel_bot_instances()
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let items: Vec<serde_json::Value> = list
        .iter()
        .map(json_channel_bot_instance_redacted)
        .collect();
    Ok(Json(json!({ "ok": true, "instances": items })))
}

async fn api_channel_bot_instances_post(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ChannelBotInstanceCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let platform = body.platform;
    let platform_for_provision = platform.clone();
    let label = body.label;
    let token = body.token;
    let bot_username = body.bot_username.map(|v| v.trim().to_string());
    let allowed_groups = body
        .allowed_groups
        .as_deref()
        .map(parse_id_list_i64)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
        .unwrap_or_default();
    let discord_allowed_channels = body
        .discord_allowed_channels
        .as_deref()
        .map(parse_id_list_u64)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?
        .unwrap_or_default();
    let whatsapp_phone_number_id = body.whatsapp_phone_number_id.map(|v| v.trim().to_string());
    let whatsapp_verify_token = body.whatsapp_verify_token.map(|v| v.trim().to_string());
    let whatsapp_webhook_port = body.whatsapp_webhook_port;
    let wecom_corp_id = body.wecom_corp_id.map(|v| v.trim().to_string());
    let wecom_agent_id = body.wecom_agent_id;
    let wecom_callback_token = body.wecom_callback_token.map(|v| v.trim().to_string());
    let wecom_encoding_aes_key = body.wecom_encoding_aes_key.map(|v| v.trim().to_string());
    let wecom_webhook_port = body.wecom_webhook_port;
    let wecom_allowed_chats = body.wecom_allowed_chats.map(|v| v.trim().to_string());
    let wecom_aibot_id = body.wecom_aibot_id.map(|v| v.trim().to_string());
    let wecom_mode = body.wecom_mode.map(|v| v.trim().to_string());
    let db = state.app_state.db.clone();
    let id = call_blocking(db.clone(), move |db| {
        let id = db.create_channel_bot_instance(&platform, &label, &token)?;
        db.update_channel_bot_instance_options(
            id,
            bot_username.as_deref(),
            Some(&allowed_groups),
            Some(&discord_allowed_channels),
            whatsapp_phone_number_id.as_deref(),
            whatsapp_verify_token.as_deref(),
            whatsapp_webhook_port,
        )?;
        db.update_channel_bot_instance_wecom_options(
            id,
            wecom_corp_id.as_deref(),
            wecom_agent_id,
            wecom_callback_token.as_deref(),
            wecom_encoding_aes_key.as_deref(),
            wecom_webhook_port,
            wecom_allowed_chats.as_deref(),
            wecom_aibot_id.as_deref(),
            wecom_mode.as_deref(),
        )?;
        Ok(id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let linked = call_blocking(db, move |db| {
        db.provision_bindings_for_instance(&platform_for_provision, id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "id": id,
        "bindings_provisioned": linked,
        "message": "Bot instance created. Restart the process to run dispatchers for new instances. Existing contacts were auto-linked where possible."
    })))
}

async fn api_channel_bot_instances_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
    Json(body): Json<ChannelBotInstanceUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let label = body.label;
    let token = body.token;
    let bot_username = body.bot_username.map(|v| v.trim().to_string());
    let allowed_groups = body
        .allowed_groups
        .as_deref()
        .map(parse_id_list_i64)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let discord_allowed_channels = body
        .discord_allowed_channels
        .as_deref()
        .map(parse_id_list_u64)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let whatsapp_phone_number_id = body.whatsapp_phone_number_id.map(|v| v.trim().to_string());
    let whatsapp_verify_token = body.whatsapp_verify_token.and_then(|v| {
        if looks_like_masked_secret(&v) {
            None
        } else {
            Some(v.trim().to_string())
        }
    });
    let whatsapp_webhook_port = body.whatsapp_webhook_port;
    let wecom_corp_id = body.wecom_corp_id.map(|v| v.trim().to_string());
    let wecom_agent_id = body.wecom_agent_id;
    let wecom_callback_token = body.wecom_callback_token.and_then(|v| {
        if looks_like_masked_secret(&v) {
            None
        } else {
            Some(v.trim().to_string())
        }
    });
    let wecom_encoding_aes_key = body.wecom_encoding_aes_key.and_then(|v| {
        if looks_like_masked_secret(&v) {
            None
        } else {
            Some(v.trim().to_string())
        }
    });
    let wecom_webhook_port = body.wecom_webhook_port;
    let wecom_allowed_chats = body.wecom_allowed_chats.map(|v| v.trim().to_string());
    let wecom_aibot_id = body.wecom_aibot_id.map(|v| v.trim().to_string());
    let wecom_mode = body.wecom_mode.map(|v| v.trim().to_string());
    let updated = call_blocking(state.app_state.db.clone(), move |db| {
        let Some(current) = db.get_channel_bot_instance(id)? else {
            return Ok(false);
        };
        let next_token = token
            .as_deref()
            .filter(|t| !looks_like_masked_secret(t))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(current.token.as_str());
        db.update_channel_bot_instance(id, &label, next_token)?;
        db.update_channel_bot_instance_options(
            id,
            bot_username.as_deref(),
            allowed_groups.as_deref(),
            discord_allowed_channels.as_deref(),
            whatsapp_phone_number_id.as_deref(),
            whatsapp_verify_token.as_deref(),
            whatsapp_webhook_port,
        )?;
        db.update_channel_bot_instance_wecom_options(
            id,
            wecom_corp_id.as_deref(),
            wecom_agent_id,
            wecom_callback_token.as_deref(),
            wecom_encoding_aes_key.as_deref(),
            wecom_webhook_port,
            wecom_allowed_chats.as_deref(),
            wecom_aibot_id.as_deref(),
            wecom_mode.as_deref(),
        )?;
        Ok(true)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !updated {
        return Err((StatusCode::NOT_FOUND, "Unknown bot instance id".into()));
    }
    Ok(Json(
        json!({ "ok": true, "message": "Updated. Restart the process to apply token changes to dispatchers." }),
    ))
}

async fn api_channel_bot_instances_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let deleted = call_blocking(state.app_state.db.clone(), move |db| {
        db.delete_channel_bot_instance(id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Not found or cannot delete".into()));
    }
    Ok(Json(json!({ "ok": true, "removed": true })))
}

fn json_channel_integration_response(
    settings: &crate::channel_integration_config::ChannelIntegrationSettings,
    instances: &[ChannelBotInstance],
) -> serde_json::Value {
    let tg = instances
        .iter()
        .find(|i| i.id == crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY);
    let dc = instances
        .iter()
        .find(|i| i.id == crate::db::BOT_INSTANCE_DISCORD_PRIMARY);
    let wa = instances
        .iter()
        .find(|i| i.id == crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY);
    json!({
        "ok": true,
        "bot_username": settings.bot_username,
        "allowed_groups": settings.allowed_groups.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
        "control_chat_ids": settings.control_chat_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
        "discord_allowed_channels": settings.discord_allowed_channels.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
        "whatsapp_phone_number_id": settings.whatsapp_phone_number_id,
        "whatsapp_verify_token_set": !settings.whatsapp_verify_token.trim().is_empty(),
        "whatsapp_verify_token_redacted": if settings.whatsapp_verify_token.trim().is_empty() {
            String::new()
        } else {
            mask_setting_value(&settings.whatsapp_verify_token)
        },
        "whatsapp_webhook_port": settings.whatsapp_webhook_port,
        "telegram_token_set": tg.map(|i| !i.token.trim().is_empty()).unwrap_or(false),
        "telegram_token_redacted": tg.map(|i| mask_setting_value(&i.token)).unwrap_or_default(),
        "telegram_label": tg.map(|i| i.label.clone()).unwrap_or_else(|| "Primary Telegram".into()),
        "discord_token_set": dc.map(|i| !i.token.trim().is_empty()).unwrap_or(false),
        "discord_token_redacted": dc.map(|i| mask_setting_value(&i.token)).unwrap_or_default(),
        "discord_label": dc.map(|i| i.label.clone()).unwrap_or_else(|| "Primary Discord".into()),
        "whatsapp_access_token_set": wa.map(|i| !i.token.trim().is_empty()).unwrap_or(false),
        "whatsapp_access_token_redacted": wa.map(|i| mask_setting_value(&i.token)).unwrap_or_default(),
        "whatsapp_label": wa.map(|i| i.label.clone()).unwrap_or_else(|| "Primary WhatsApp".into()),
        "instances": instances.iter().map(json_channel_bot_instance_redacted).collect::<Vec<_>>(),
        "requires_restart": true,
    })
}

async fn api_channels_integration_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let config = state.app_state.config.clone();
    let (settings, instances) = call_blocking(state.app_state.db.clone(), move |db| {
        let settings = crate::channel_integration_config::load_from_db(db, &config)?;
        let instances = db.list_all_channel_bot_instances()?;
        Ok::<_, crate::error::FinallyAValueBotError>((settings, instances))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json_channel_integration_response(
        &settings, &instances,
    )))
}

async fn api_channels_integration_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<crate::channel_integration_config::ChannelIntegrationPatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let config = state.app_state.config.clone();
    let (settings, instances) = call_blocking(state.app_state.db.clone(), move |db| {
        let settings = body.apply_and_save(db, &config)?;
        let instances = db.list_all_channel_bot_instances()?;
        Ok::<_, crate::error::FinallyAValueBotError>((settings, instances))
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let mut resp = json_channel_integration_response(&settings, &instances);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert(
            "message".into(),
            json!("Saved. Restart the gateway to apply token and dispatcher changes."),
        );
    }
    Ok(Json(resp))
}

async fn api_channel_persona_policy_upsert(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ChannelPersonaPolicyUpsertRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let mode = match body.mode.as_str() {
        "all" => ChannelPersonaMode::All,
        "single" => ChannelPersonaMode::Single,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "mode must be 'all' or 'single'".to_string(),
            ));
        }
    };
    let persona_id = body.persona_id.filter(|id| *id > 0);
    let bot_instance_id = body.bot_instance_id;
    call_blocking(state.app_state.db.clone(), move |db| {
        db.set_channel_persona_policy(chat_id, bot_instance_id, mode, persona_id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(json!({
        "ok": true,
        "chat_id": chat_id,
        "bot_instance_id": bot_instance_id,
        "mode": body.mode,
        "persona_id": persona_id,
    })))
}

async fn api_channel_persona_policy_delete(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<ChannelPersonaPolicyDeleteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(body.chat_id, &state.app_state.config)?;
    ensure_web_binding_for_universal(&state, chat_id).await?;
    let removed = call_blocking(state.app_state.db.clone(), move |db| {
        db.clear_channel_persona_policy(chat_id, body.bot_instance_id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "removed": removed,
    })))
}

async fn api_terminal_sessions_post(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    web_terminal::require_terminal_api_auth(&headers, &state.app_state.config)?;
    let body = state
        .terminal_hub
        .create_session(&state.app_state.config)
        .await?;
    Ok(Json(body))
}

async fn api_terminal_ws(ws: WebSocketUpgrade, State(state): State<WebState>) -> impl IntoResponse {
    let hub = state.terminal_hub.clone();
    let config = Arc::new(state.app_state.config.clone());
    ws.on_upgrade(move |socket: WebSocket| async move {
        web_terminal::handle_websocket(socket, hub, config).await;
    })
}

/// Bind the web UI. When `port` is the default and it is in use, try the following ports.
pub async fn bind_web_listener(
    host: &str,
    port: u16,
) -> Result<(tokio::net::TcpListener, u16), std::io::Error> {
    let exact = std::env::var(crate::daemon::RESTART_EXACT_PORT_ENV)
        .ok()
        .as_deref()
        == Some("1");
    let walk = !exact && port == crate::config::default_web_port();
    let mut candidate = port;
    let bind_host = crate::config::web_bind_host(host);
    if bind_host != host.trim() {
        info!(
            "WEB_HOST {host} is a DNS name; listening on {bind_host} so http://{host}:{port} can reach this machine"
        );
    }
    loop {
        let addr = format!("{bind_host}:{candidate}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => return Ok((listener, candidate)),
            Err(err)
                if walk
                    && err.kind() == std::io::ErrorKind::AddrInUse
                    && candidate.saturating_sub(port) < crate::config::WEB_PORT_FALLBACK_SPAN =>
            {
                let Some(next) = candidate.checked_add(1) else {
                    return Err(err);
                };
                warn!("Web port {candidate} is in use; trying {next}");
                candidate = next;
            }
            Err(err) => return Err(err),
        }
    }
}

pub async fn start_web_server(state: Arc<AppState>, listener: tokio::net::TcpListener) {
    let limits = WebLimits::from_config(&state.config);
    let web_state = WebState {
        auth_token: state.config.web_auth_token.clone(),
        app_state: state.clone(),
        run_hub: RunHub::default(),
        request_hub: RequestHub::default(),
        terminal_hub: TerminalHub::default(),
        limits,
        web_binding_universal_done: Arc::new(Mutex::new(None)),
    };

    artifacts_api::start_backfill(web_state.clone());
    let router = build_router(web_state);

    let bound_port = listener
        .local_addr()
        .map(|socket| socket.port())
        .unwrap_or(state.config.web_port);
    let public_host = state.config.web_host.trim();
    let bind_host = crate::config::web_bind_host(public_host);
    if bind_host != public_host {
        info!("Web UI listening on {bind_host}:{bound_port}");
        info!("Web UI available at http://{public_host}:{bound_port}");
    } else {
        let addr = listener
            .local_addr()
            .map(|socket| socket.to_string())
            .unwrap_or_else(|_| format!("{public_host}:{bound_port}"));
        info!("Web UI available at http://{addr}");
    }
    if let Err(e) = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    {
        error!("Web server error: {e}");
    }
}

async fn asset_file(Path(file): Path<String>) -> impl IntoResponse {
    let clean = file.replace("..", "");
    match WEB_ASSETS.get_file(format!("assets/{clean}")) {
        Some(file) => {
            let content_type = if clean.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if clean.ends_with(".js") {
                "application/javascript; charset=utf-8"
            } else {
                "application/octet-stream"
            };
            ([("content-type", content_type)], file.contents().to_vec()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn upload_file(
    headers: HeaderMap,
    method: axum::http::Method,
    State(state): State<WebState>,
    Path(path): Path<String>,
    Query(query): Query<UploadQuery>,
) -> axum::response::Response {
    if let Err(e) = artifacts_api::media_auth(&headers, &state) {
        return e.into_response();
    }
    if path
        .split('/')
        .any(|s| s == ".." || s == "." || s.is_empty())
        || path.contains('\\')
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let root = state.app_state.config.workspace_root_absolute();
    let alias_url = format!("/api/uploads/{path}");
    let lookup = tokio::task::spawn_blocking({
        let root = root.clone();
        move || crate::artifacts::Catalog::open(&root)?.alias(&alias_url)
    })
    .await;
    if let Ok(Ok(Some(a))) = lookup {
        if a.status != "active" {
            return (
                StatusCode::GONE,
                "Artifact is in Trash or permanently deleted",
            )
                .into_response();
        }
        return match crate::artifact_http::serve_file(
            &a.blob_path,
            &a.name,
            &headers,
            &method,
            &query,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => e.into_response(),
        };
    }
    let shared = root.join("shared/upload");
    let legacy = FsPath::new(state.app_state.config.working_dir()).join("uploads");
    for base in [shared, legacy] {
        if let (Ok(base), Ok(full)) = (base.canonicalize(), base.join(&path).canonicalize()) {
            if full.starts_with(&base) && full.is_file() {
                return match crate::artifact_http::serve_file(
                    &full,
                    full.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
                    &headers,
                    &method,
                    &query,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => e.into_response(),
                };
            }
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn icon_file() -> impl IntoResponse {
    match WEB_ASSETS.get_file("icon.png") {
        Some(file) => ([("content-type", "image/png")], file.contents().to_vec()).into_response(),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn favicon_file() -> impl IntoResponse {
    if let Some(file) = WEB_ASSETS.get_file("favicon.ico") {
        return ([("content-type", "image/x-icon")], file.contents().to_vec()).into_response();
    }
    if let Some(file) = WEB_ASSETS.get_file("icon.png") {
        return ([("content-type", "image/png")], file.contents().to_vec()).into_response();
    }
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

fn embedded_public_file(name: &str, content_type: &'static str) -> axum::response::Response {
    match WEB_ASSETS.get_file(name) {
        Some(file) => ([("content-type", content_type)], file.contents().to_vec()).into_response(),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn meido_crest_file() -> axum::response::Response {
    embedded_public_file("meido-crest.svg", "image/svg+xml")
}

async fn meido_bow_file() -> axum::response::Response {
    embedded_public_file("meido-bow.svg", "image/svg+xml")
}

#[derive(Debug, Deserialize)]
struct OAuthAuthorizeQuery {
    chat_id: Option<i64>,
}

async fn api_oauth_authorize(
    State(state): State<WebState>,
    Path(platform): Path<String>,
    Query(query): Query<OAuthAuthorizeQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let platform = platform.to_lowercase();
    if !["tiktok", "instagram", "linkedin"].contains(&platform.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "Unknown platform".into()));
    }
    if state
        .app_state
        .config
        .social
        .as_ref()
        .is_none_or(|s| !s.is_platform_enabled(&platform))
    {
        return Err((StatusCode::BAD_REQUEST, "Platform not configured".into()));
    }

    let chat_id = resolve_chat_id_for_web(query.chat_id, &state.app_state.config)?;

    let state_token = uuid::Uuid::new_v4().simple().to_string();
    let expires_at = (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();

    let platform_clone = platform.clone();
    let state_token_clone = state_token.clone();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.create_oauth_pending_state(&state_token_clone, &platform_clone, chat_id, &expires_at)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let auth_url = social_oauth::authorize_url(&state.app_state.config, &platform, &state_token)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build authorize URL".into(),
            )
        })?;

    Ok(axum::response::Redirect::temporary(&auth_url))
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn api_oauth_callback(
    State(state): State<WebState>,
    Path(platform): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    let platform = platform.to_lowercase();

    if let (Some(err), desc) = (query.error, query.error_description.as_deref()) {
        let msg = desc.unwrap_or(&err);
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                <h1>Authorization failed</h1><p>{}</p></body></html>"#,
                msg.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            )),
        )
            .into_response();
    }

    let (code, state_token) = match (query.code, query.state) {
        (Some(c), Some(s)) => (c, s),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Html(
                    r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                    <h1>Missing code or state</h1></body></html>"#,
                ),
            )
                .into_response();
        }
    };

    let Some((stored_platform, chat_id)) = call_blocking(state.app_state.db.clone(), move |db| {
        db.consume_oauth_pending_state(&state_token)
    })
    .await
    .ok()
    .flatten() else {
        return (
            StatusCode::BAD_REQUEST,
            Html(
                r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                <h1>Invalid or expired state</h1><p>Please try the authorization flow again.</p></body></html>"#,
            ),
        )
            .into_response();
    };

    if stored_platform != platform {
        return (
            StatusCode::BAD_REQUEST,
            Html(
                r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                <h1>Platform mismatch</h1></body></html>"#,
            ),
        )
            .into_response();
    }

    let base = social_oauth::oauth_base_url(&state.app_state.config).unwrap_or_default();
    let redirect_uri = format!(
        "{}/api/oauth/callback/{}",
        base.trim_end_matches('/'),
        platform
    );

    let token_result =
        match social_oauth::exchange_code(&state.app_state.config, &platform, &code, &redirect_uri)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html(format!(
                        r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                    <h1>Token exchange failed</h1><p>{}</p></body></html>"#,
                        e.to_string()
                            .replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                    )),
                )
                    .into_response();
            }
        };

    let platform_for_db = platform.clone();
    if let Err(e) = call_blocking(state.app_state.db.clone(), move |db| {
        db.upsert_social_token(
            &platform_for_db,
            chat_id,
            &token_result.access_token,
            token_result.refresh_token.as_deref(),
            token_result.expires_at.as_deref(),
        )
    })
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!(
                r#"<!DOCTYPE html><html><head><title>OAuth Error</title></head><body>
                <h1>Failed to store token</h1><p>{}</p></body></html>"#,
                html_escape::encode_text(&e.to_string())
            )),
        )
            .into_response();
    }

    let platform_name = match platform.as_str() {
        "tiktok" => "TikTok",
        "instagram" => "Instagram",
        "linkedin" => "LinkedIn",
        _ => &platform,
    };

    (
        StatusCode::OK,
        Html(format!(
            r#"<!DOCTYPE html><html><head><title>Authorization successful</title></head><body>
            <h1>Authorization successful</h1>
            <p>{} has been connected. You can now ask the bot to fetch your feed.</p>
            <p><a href="/">Back to chat</a></p></body></html>"#,
            platform_name
        )),
    )
        .into_response()
}

fn build_router(web_state: WebState) -> Router {
    let json_limit = web_json_body_limit_bytes(&web_state.app_state.config);
    let multipart_limit = web_multipart_body_limit_bytes(&web_state.app_state.config);
    Router::new()
        .route("/", get(index))
        .route("/assets/*file", get(asset_file))
        .route("/api/uploads/*path", get(upload_file))
        .route("/icon.png", get(icon_file))
        .route("/favicon.ico", get(favicon_file))
        .route("/meido-crest.svg", get(meido_crest_file))
        .route("/meido-bow.svg", get(meido_bow_file))
        .route("/api/health", get(api_health))
        .route("/api/chat", get(api_chat))
        .route("/api/contacts/bind", post(api_contacts_bind))
        .route("/api/contacts/unlink", post(api_contacts_unlink))
        .route("/api/contacts/bindings", get(api_contacts_bindings))
        .route(
            "/api/settings",
            get(api_settings_get).patch(api_settings_patch),
        )
        .route(
            "/api/env-config",
            get(env_config::api_env_config_get).patch(env_config::api_env_config_patch),
        )
        .route("/api/setup/action", post(env_config::api_setup_action_post))
        .route("/api/llm", get(api_llm_get).patch(api_llm_patch))
        .route(
            "/api/secrets",
            get(api_secrets_list).delete(api_secrets_forget),
        )
        .route("/api/llm/models", get(api_llm_models_get))
        .route(
            "/api/multimodel",
            get(api_multimodel_get).patch(api_multimodel_patch),
        )
        .route("/api/multimodel/test", post(api_multimodel_test_post))
        .route("/api/multimodel/models", get(api_multimodel_models_get))
        .route(
            "/api/cursor-engine",
            get(api_cursor_engine_get).patch(api_cursor_engine_patch),
        )
        .route(
            "/api/cursor-engine/health",
            post(api_cursor_engine_health_post),
        )
        .route(
            "/api/cursor-engine/models",
            get(api_cursor_engine_models_get),
        )
        .route(
            "/internal/cursor-mcp",
            get(api_cursor_mcp_get).post(api_cursor_mcp_post),
        )
        .route(
            "/api/runtime",
            get(api_runtime_get).patch(api_runtime_patch),
        )
        .route(
            "/api/system-llm",
            get(api_topics_model_get).patch(api_topics_model_patch),
        )
        .route(
            "/api/topics/model",
            get(api_topics_model_get).patch(api_topics_model_patch),
        )
        .route(
            "/api/connections",
            get(api_connections_get).patch(api_connections_patch),
        )
        .route("/api/terminal/sessions", post(api_terminal_sessions_post))
        .route("/api/terminal/ws", get(api_terminal_ws))
        .route(
            "/api/gemini-adk",
            get(api_gemini_adk_get).patch(api_gemini_adk_patch),
        )
        .route(
            "/api/gemini-adk/topology",
            get(api_gemini_adk_topology_get).patch(api_gemini_adk_topology_patch),
        )
        .route("/api/restart", post(api_restart_post))
        .route(
            "/api/channels/integration",
            get(api_channels_integration_get).patch(api_channels_integration_patch),
        )
        .route(
            "/api/channel_bot_instances",
            get(api_channel_bot_instances_get).post(api_channel_bot_instances_post),
        )
        .route(
            "/api/channel_bot_instances/:id",
            patch(api_channel_bot_instances_patch).delete(api_channel_bot_instances_delete),
        )
        .route(
            "/api/channel_persona_policy",
            post(api_channel_persona_policy_upsert).delete(api_channel_persona_policy_delete),
        )
        .route(
            "/api/schedules",
            get(api_schedules_list).post(api_schedules_create),
        )
        .route("/api/schedules/:id", patch(api_schedules_update))
        .route("/api/todos", get(api_todos_list).post(api_todos_create))
        .route("/api/todos/:id/run", post(api_todos_run))
        .route("/api/todos/:id/events", get(api_todos_events))
        .route("/api/todos/:id", patch(api_todos_patch))
        .route("/api/background_jobs", get(api_background_jobs_list))
        .route("/api/background_jobs/:job_id", get(api_background_job_get))
        .route(
            "/api/background_jobs/cancel",
            post(api_background_job_cancel),
        )
        .route("/api/history", get(api_history))
        .route("/api/history/days", get(api_history_days))
        .route("/api/artifacts", get(artifacts_api::list))
        .route(
            "/api/artifacts/media-session",
            post(artifacts_api::media_session),
        )
        .route(
            "/api/artifacts/:id",
            get(artifacts_api::detail)
                .patch(artifacts_api::update)
                .delete(artifacts_api::remove),
        )
        .route("/api/artifacts/:id/content", get(artifacts_api::content))
        .route(
            "/api/artifacts/:id/deletion-review",
            get(artifacts_api::review),
        )
        .route(
            "/api/uploads",
            post(api_upload).layer(DefaultBodyLimit::max(multipart_limit)),
        )
        .route("/api/send", post(api_send))
        .route(
            "/api/send_stream",
            post(api_send_stream).layer(DefaultBodyLimit::max(json_limit)),
        )
        .route(
            "/api/subthread_stream",
            post(api_subthread_stream).layer(DefaultBodyLimit::max(json_limit)),
        )
        .route("/api/stream", get(api_stream))
        .route("/api/run_status", get(api_run_status))
        .route("/api/queue_diagnostics", get(api_queue_diagnostics))
        .route("/api/ops_poll", get(api_ops_poll))
        .route("/api/queue/cancel", post(api_queue_cancel))
        .route("/api/queue/remove", post(api_queue_remove))
        .route("/api/reset", post(api_reset))
        .route("/api/delete_session", post(api_delete_session))
        .route("/api/personas", get(api_personas))
        .route("/api/personas/read", post(api_personas_read))
        .route("/api/personas/switch", post(api_personas_switch))
        .route("/api/personas/create", post(api_personas_create))
        .route(
            "/api/personas/local-delegate",
            get(api_persona_local_get).patch(api_persona_local_patch),
        )
        .route("/api/personas/delete", post(api_personas_delete))
        .route("/api/personas/rename", post(api_personas_rename))
        .route(
            "/api/chat_sessions",
            get(api_chat_sessions_list).post(api_chat_sessions_create),
        )
        .route(
            "/api/chat_sessions/:session_id",
            get(api_chat_sessions_get)
                .patch(api_chat_sessions_patch)
                .delete(api_chat_sessions_delete),
        )
        .route("/api/skills", get(api_skills_get))
        .route("/api/hooks", get(api_hooks_get).post(api_hooks_post))
        .route("/api/hooks/:id", delete(api_hooks_delete))
        .route(
            "/api/personas/:persona_id/bulletin",
            get(api_persona_bulletin_get).patch(api_persona_bulletin_patch),
        )
        .route(
            "/api/personas/:persona_id/external-paths",
            get(api_persona_external_paths_get).put(api_persona_external_paths_put),
        )
        .route(
            "/api/personas/:persona_id/policy",
            get(api_persona_policy_get).patch(api_persona_policy_patch),
        )
        .route(
            "/api/personas/:persona_id/bookmarks",
            get(api_persona_bookmarks_get).post(api_persona_bookmarks_post),
        )
        .route(
            "/api/personas/:persona_id/bookmarks/:message_id",
            delete(api_persona_bookmarks_delete),
        )
        .route(
            "/api/personas/:persona_id/topics",
            get(api_persona_topics_list),
        )
        .route(
            "/api/personas/:persona_id/topics/refresh",
            post(api_persona_topics_refresh),
        )
        .route(
            "/api/personas/:persona_id/topics/split",
            post(api_persona_topics_split),
        )
        .route(
            "/api/personas/:persona_id/topics/:topic_id/dossier",
            get(api_persona_topic_dossier),
        )
        .route(
            "/api/personas/:persona_id/topics/:topic_id",
            patch(api_persona_topics_patch).delete(api_persona_topics_delete),
        )
        .route(
            "/api/personas/:persona_id/topics/:topic_id/merge",
            post(api_persona_topics_merge),
        )
        .route(
            "/api/personas/:persona_id/topics/:topic_id/section",
            post(api_persona_topics_section),
        )
        .route(
            "/api/personas/:persona_id/side_chats",
            get(api_persona_side_chats_list).post(api_persona_side_chats_ensure),
        )
        .route(
            "/api/personas/:persona_id/side_chats/:side_chat_id",
            get(api_persona_side_chat_get)
                .patch(api_persona_side_chat_patch)
                .delete(api_persona_side_chat_delete),
        )
        .route(
            "/api/personas/:persona_id/side_chats/:side_chat_id/promote_turn",
            post(api_persona_side_chat_promote_turn),
        )
        .route(
            "/api/personas/:persona_id/messages/:message_id",
            get(api_persona_message_get)
                .patch(api_persona_message_patch)
                .delete(api_persona_message_delete),
        )
        .route(
            "/api/personas/:persona_id/memory",
            get(api_persona_memory_get).put(api_persona_memory_put),
        )
        .route(
            "/api/personas/:persona_id/agent_history/latest",
            get(api_persona_agent_history_latest),
        )
        .route(
            "/api/personas/:persona_id/agent_history/latest/optimize",
            post(api_persona_agent_history_optimize),
        )
        .route(
            "/api/workspace/agents_md",
            get(api_workspace_agents_md_get).put(api_workspace_agents_md_put),
        )
        .route("/api/oauth/authorize/:platform", get(api_oauth_authorize))
        .route("/api/oauth/callback/:platform", get(api_oauth_callback))
        .merge(crate::web_collab::routes())
        .layer(DefaultBodyLimit::max(json_limit))
        .with_state(web_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::call_blocking;
    use crate::llm::LlmProvider;
    use crate::{claude::ResponseContentBlock, error::FinallyAValueBotError};
    use crate::{db::Database, memory::MemoryManager, skills::SkillManager, tools::ToolRegistry};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use teloxide::Bot;
    use tower::ServiceExt;

    #[test]
    fn test_web_assets_embedded() {
        assert!(
            WEB_ASSETS.get_file("index.html").is_some(),
            "embedded web asset missing: index.html"
        );
        assert!(
            WEB_ASSETS.get_file("icon.png").is_some(),
            "embedded web asset missing: icon.png"
        );
        assert!(
            WEB_ASSETS.get_file("meido-crest.svg").is_some(),
            "embedded web asset missing: meido-crest.svg"
        );
        assert!(
            WEB_ASSETS.get_file("meido-bow.svg").is_some(),
            "embedded web asset missing: meido-bow.svg"
        );
        let assets_dir = WEB_ASSETS.get_dir("assets");
        assert!(
            assets_dir.is_some(),
            "embedded web asset dir missing: assets"
        );
        assert!(
            assets_dir.unwrap().files().next().is_some(),
            "embedded web asset dir is empty: assets"
        );
    }

    #[test]
    fn test_ops_poll_include_personas_query() {
        assert!(ops_poll_include_personas(None));
        assert!(ops_poll_include_personas(Some("1")));
        assert!(ops_poll_include_personas(Some("true")));
        assert!(!ops_poll_include_personas(Some("0")));
        assert!(!ops_poll_include_personas(Some("false")));
        assert!(!ops_poll_include_personas(Some("OFF")));
    }

    #[test]
    fn test_background_job_result_preview_utf8_boundary() {
        // Em dash is 3 bytes; a raw `&t[..200]` panic sits inside it when byte 199 is mid-char.
        let prefix = "a".repeat(199);
        let text = format!("{prefix}—more");
        assert!(text.is_char_boundary(0));
        assert!(!text.is_char_boundary(200));
        let preview = background_job_result_preview(&text);
        assert!(preview.len() < 200);
        assert!(preview.ends_with('a'));
        assert!(!preview.contains('—'));
    }

    #[test]
    fn test_artifact_kind_from_filename() {
        assert_eq!(artifact_kind_from_filename("report.md"), "markdown");
        assert_eq!(artifact_kind_from_filename("image.png"), "image");
        assert_eq!(artifact_kind_from_filename("index.html"), "html");
        assert_eq!(artifact_kind_from_filename("notes.txt"), "text");
        assert_eq!(artifact_kind_from_filename("archive.zip"), "other");
    }

    #[test]
    fn test_extract_upload_urls_from_text() {
        let text = "See [/api/uploads/web/1/file.md](/api/uploads/web/1/file.md) and ![](/api/uploads/web/1/img.png)";
        let urls = extract_upload_urls_from_text(text);
        assert!(urls.iter().any(|u| u == "/api/uploads/web/1/file.md"));
        assert!(urls.iter().any(|u| u == "/api/uploads/web/1/img.png"));
    }

    #[tokio::test]
    async fn test_materialize_response_file_links_rewrites_file_url_target() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let persona_id = 3_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let spec = workspace_root
            .join("shared")
            .join("personas")
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join("ORIGIN/Projects/GN-dComm-Integration-Spec.md");
        std::fs::create_dir_all(spec.parent().unwrap()).unwrap();
        std::fs::write(&spec, b"# spec").unwrap();

        let file_url = format!("file://{}", spec.display());
        let input = format!("[GN Spec]({file_url})");
        let output = materialize_response_file_links(&web_state, chat_id, persona_id, &input)
            .await
            .unwrap();
        assert!(output.contains("/api/artifacts/"));
        assert!(!output.contains("file://"));
        let urls = extract_upload_urls_from_text(&output);
        assert_eq!(urls.len(), 1);
        assert!(upload_rel_url_exists(&web_state, &urls[0]));
    }

    #[tokio::test]
    async fn test_materialize_response_file_links_rewrites_local_markdown_target() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let shared_dir = workspace_root.join("shared");
        std::fs::create_dir_all(&shared_dir).unwrap();
        let local = shared_dir.join("report.pdf");
        std::fs::write(&local, b"pdf").unwrap();

        let input = "[Download](shared/report.pdf)";
        let output = materialize_response_file_links(&web_state, chat_id, 1, input)
            .await
            .unwrap();
        assert!(output.contains("/api/artifacts/"));
        assert!(!output.contains("(shared/report.pdf)"));

        let urls = extract_upload_urls_from_text(&output);
        assert_eq!(urls.len(), 1);
        assert!(upload_rel_url_exists(&web_state, &urls[0]));
    }

    #[tokio::test]
    async fn test_materialize_response_file_links_rejects_missing_upload_url() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let shared_dir = workspace_root.join("shared");
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("report.pdf"), b"pdf").unwrap();

        let input = "[Download](/api/uploads/web/997894126/report.pdf)";
        let output = materialize_response_file_links(&web_state, chat_id, 1, input)
            .await
            .unwrap();
        assert!(output.contains("file unavailable"));
        assert!(!output.contains("#artifact-unavailable"));
        assert!(!output.contains("/api/artifacts/"));
    }

    #[tokio::test]
    async fn test_materialize_rejects_fabricated_bot_copy_upload_url() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let persona_id = 24_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let persona_dir = workspace_root
            .join("shared")
            .join("personas")
            .join(chat_id.to_string())
            .join(persona_id.to_string());
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(
            persona_dir.join("PZ-20260608-PARK-HOTIFY-MEDIUM.png"),
            [137u8, 80, 78, 71, 13, 10, 26, 10],
        )
        .unwrap();

        let input = "![preview](/api/uploads/web/997894126/24/20260604-045735-bot-PZ-20260608-PARK-HOTIFY-MEDIUM.png)";
        let output = materialize_response_file_links(&web_state, chat_id, persona_id, input)
            .await
            .unwrap();
        assert_ne!(input, output);
        let urls = extract_upload_urls_from_text(&output);
        assert!(urls.is_empty());
        assert!(output.contains("file unavailable"));
        assert!(!output.contains("#artifact-unavailable"));
        assert!(!output.contains("/api/artifacts/"));
    }

    #[tokio::test]
    async fn test_materialize_after_bare_normalize_persona_scoped_image() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let persona_id = 24_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let persona_dir = workspace_root
            .join("shared")
            .join("personas")
            .join(chat_id.to_string())
            .join(persona_id.to_string());
        std::fs::create_dir_all(&persona_dir).unwrap();
        let img = persona_dir.join("PZ-foo.png");
        std::fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();

        let raw = "Preview:\n\nPZ-foo.png\n\nOK?";
        let normalized = crate::final_delivery_media::normalize_assistant_artifact_references(
            &raw,
            &workspace_root,
            chat_id,
            persona_id,
        );
        let output = materialize_response_file_links(&web_state, chat_id, persona_id, &normalized)
            .await
            .unwrap();
        assert!(output.contains("/api/artifacts/"));
        assert!(output.contains("![PZ-foo.png]"));
        let urls = extract_upload_urls_from_text(&output);
        assert_eq!(urls.len(), 1);
        assert!(upload_rel_url_exists(&web_state, &urls[0]));
    }

    #[tokio::test]
    async fn test_materialize_response_file_links_rewrites_parenthesized_local_path() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = 997894126_i64;
        let workspace_root = web_state.app_state.config.workspace_root_absolute();
        let shared_dir = workspace_root.join("shared");
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("report.pdf"), b"pdf").unwrap();

        let input = "(shared/report.pdf)";
        let output = materialize_response_file_links(&web_state, chat_id, 1, input)
            .await
            .unwrap();
        assert!(output.contains("/api/artifacts/"));
        assert!(!output.contains("(shared/report.pdf)"));

        let urls = extract_upload_urls_from_text(&output);
        assert_eq!(urls.len(), 1);
        assert!(upload_rel_url_exists(&web_state, &urls[0]));
    }

    struct DummyLlm;

    #[async_trait::async_trait]
    impl LlmProvider for DummyLlm {
        async fn send_message(
            &self,
            _system: &str,
            _messages: Vec<crate::claude::Message>,
            _tools: Option<Vec<crate::claude::ToolDefinition>>,
        ) -> Result<crate::claude::MessagesResponse, crate::error::FinallyAValueBotError> {
            Ok(crate::claude::MessagesResponse {
                content: vec![crate::claude::ResponseContentBlock::Text {
                    text: "hello from llm".into(),
                }],
                stop_reason: Some("end_turn".into()),
                usage: None,
            })
        }

        async fn send_message_stream(
            &self,
            _system: &str,
            _messages: Vec<crate::claude::Message>,
            _tools: Option<Vec<crate::claude::ToolDefinition>>,
            text_tx: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
        ) -> Result<crate::claude::MessagesResponse, crate::error::FinallyAValueBotError> {
            if let Some(tx) = text_tx {
                let _ = tx.send("hello ".into());
                let _ = tx.send("from llm".into());
            }
            self.send_message("", vec![], None).await
        }
    }

    #[allow(dead_code)]
    struct SlowLlm {
        sleep_ms: u64,
    }

    #[async_trait::async_trait]
    impl LlmProvider for SlowLlm {
        async fn send_message(
            &self,
            _system: &str,
            _messages: Vec<crate::claude::Message>,
            _tools: Option<Vec<crate::claude::ToolDefinition>>,
        ) -> Result<crate::claude::MessagesResponse, FinallyAValueBotError> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(crate::claude::MessagesResponse {
                content: vec![ResponseContentBlock::Text {
                    text: "slow".into(),
                }],
                stop_reason: Some("end_turn".into()),
                usage: None,
            })
        }
    }

    struct ToolFlowLlm {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolFlowLlm {
        async fn send_message(
            &self,
            _system: &str,
            _messages: Vec<crate::claude::Message>,
            _tools: Option<Vec<crate::claude::ToolDefinition>>,
        ) -> Result<crate::claude::MessagesResponse, FinallyAValueBotError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Ok(crate::claude::MessagesResponse {
                    content: vec![ResponseContentBlock::ToolUse {
                        id: "tool_1".into(),
                        name: "glob".into(),
                        input: json!({"pattern": "*.rs", "path": "."}),
                        thought_signature: None,
                    }],
                    stop_reason: Some("tool_use".into()),
                    usage: None,
                });
            }
            Ok(crate::claude::MessagesResponse {
                content: vec![ResponseContentBlock::Text {
                    text: "after tool".into(),
                }],
                stop_reason: Some("end_turn".into()),
                usage: None,
            })
        }
    }

    fn test_llm_from_provider(provider: Arc<dyn LlmProvider>) -> Arc<crate::llm::LlmHandle> {
        let cfg = crate::config::test_config();
        crate::llm::LlmHandle::from_pinned_provider(&cfg, provider)
    }

    fn test_state(llm: Arc<crate::llm::LlmHandle>) -> Arc<AppState> {
        let mut cfg = crate::config::test_config();
        cfg.web_port = 3900;
        cfg.universal_chat_id = Some(997894126);
        let dir = std::env::temp_dir().join(format!("meido_webtest_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        cfg.workspace_dir = dir.to_string_lossy().to_string();
        let runtime_dir = cfg.runtime_data_dir();
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let db = Arc::new(Database::new(&runtime_dir).unwrap());
        let bot = Bot::new("123456:TEST_TOKEN");
        let mut telegram_bots = std::collections::HashMap::new();
        telegram_bots.insert(crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY, bot.clone());
        let runtime_toggles = crate::runtime_toggles::RuntimeToggles::new(cfg.tool_output_debug);
        let env_redactor =
            std::sync::Arc::new(crate::safety_redaction::EnvSecretRedactor::discover(&cfg));
        let cursor_settings = Arc::new(std::sync::RwLock::new(
            crate::cursor_engine_config::CursorEngineSettings::from_env(&cfg),
        ));
        let gemini_adk_settings = Arc::new(std::sync::RwLock::new(
            crate::gemini_adk::GeminiAdkSettings::from_env(&cfg),
        ));
        let gemini_adk_profile = Arc::new(std::sync::RwLock::new(
            crate::gemini_adk::AdkTopologyProfile::default_profile(),
        ));
        let cursor_sidecar = crate::cursor_sdk_sidecar::SidecarHandle::inactive();
        let steel_browser = crate::steel_browser_sidecar::SteelBrowserHandle::inactive();
        let state = AppState {
            config: cfg.clone(),
            env_redactor: env_redactor.clone(),
            runtime_toggles: runtime_toggles.clone(),
            cursor_settings,
            gemini_adk_settings,
            gemini_adk_profile,
            cursor_sidecar,
            steel_browser,
            telegram_bots: Arc::new(telegram_bots),
            db: db.clone(),
            memory: MemoryManager::new(&runtime_dir, cfg.working_dir()),
            skills: SkillManager::from_skills_dirs(cfg.skill_discovery_dirs()),
            llm,
            tools: ToolRegistry::new(&cfg, bot, db, runtime_toggles, env_redactor),
            cursor_mcp: Arc::new(crate::cursor_mcp_bridge::CursorMcpRegistry::new()),
            discord_http: Arc::new(std::collections::HashMap::new()),
            wecom: None,
            chat_queue: crate::chat_queue::ChatRunQueue::default(),
            background_job_control: crate::background_jobs::BackgroundJobControl::default(),
            persona_external_paths: Arc::new(
                crate::persona_external_paths::PersonaExternalPaths::empty(None),
            ),
        };
        Arc::new(state)
    }

    fn test_web_state(
        llm: Arc<crate::llm::LlmHandle>,
        auth_token: Option<String>,
        limits: WebLimits,
    ) -> WebState {
        let state = test_state(llm);
        WebState {
            app_state: state,
            auth_token,
            run_hub: RunHub::default(),
            request_hub: RequestHub::default(),
            terminal_hub: TerminalHub::default(),
            limits,
            web_binding_universal_done: Arc::new(Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn test_api_hooks_get_persona_fields() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = web_state
            .app_state
            .config
            .universal_chat_id
            .unwrap_or(997894126_i64);
        let db = web_state.app_state.db.clone();

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

        let res_owner = api_hooks_get(
            HeaderMap::new(),
            State(web_state.clone()),
            Query(HooksQuery {
                persona_id: Some(owner_persona_id),
            }),
        )
        .await;
        let body_owner = res_owner.expect("owner ok").0;
        let hooks_owner = body_owner
            .get("hooks")
            .and_then(|v| v.as_array())
            .expect("hooks array");

        let find_hook = |id: i64| {
            hooks_owner
                .iter()
                .find(|h| h.get("id").and_then(|v| v.as_i64()) == Some(id))
                .expect("hook exists")
        };

        let global_hook = find_hook(global_hook_id);
        assert_eq!(
            global_hook
                .get("scoped_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            global_hook
                .get("allowed_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            global_hook
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        let scoped_hook = find_hook(scoped_hook_id);
        assert_eq!(
            scoped_hook
                .get("scoped_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            scoped_hook
                .get("allowed_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            scoped_hook
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        let disabled_hook = find_hook(disabled_hook_id);
        assert_eq!(
            disabled_hook
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        let res_other = api_hooks_get(
            HeaderMap::new(),
            State(web_state.clone()),
            Query(HooksQuery {
                persona_id: Some(other_persona_id),
            }),
        )
        .await;
        let body_other = res_other.expect("other ok").0;
        let hooks_other = body_other
            .get("hooks")
            .and_then(|v| v.as_array())
            .expect("hooks array");
        let find_hook_other = |id: i64| {
            hooks_other
                .iter()
                .find(|h| h.get("id").and_then(|v| v.as_i64()) == Some(id))
                .expect("hook exists")
        };

        let scoped_hook_other = find_hook_other(scoped_hook_id);
        assert_eq!(
            scoped_hook_other
                .get("scoped_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            scoped_hook_other
                .get("allowed_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            scoped_hook_other
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        let res_missing = api_hooks_get(
            HeaderMap::new(),
            State(web_state.clone()),
            Query(HooksQuery {
                persona_id: Some(99999),
            }),
        )
        .await;
        assert!(
            matches!(res_missing, Err((StatusCode::NOT_FOUND, _))),
            "expected 404 for missing persona"
        );

        db.set_persona_hook_skill_policy(chat_id, owner_persona_id, Some(&[]), None)
            .expect("block all hooks for owner");
        let res_owner_blocked = api_hooks_get(
            HeaderMap::new(),
            State(web_state.clone()),
            Query(HooksQuery {
                persona_id: Some(owner_persona_id),
            }),
        )
        .await;
        let body_owner_blocked = res_owner_blocked.expect("owner ok").0;
        let hooks_owner_blocked = body_owner_blocked
            .get("hooks")
            .and_then(|v| v.as_array())
            .expect("hooks array");
        let find_hook_blocked = |id: i64| {
            hooks_owner_blocked
                .iter()
                .find(|h| h.get("id").and_then(|v| v.as_i64()) == Some(id))
                .expect("hook exists")
        };

        let global_hook_blocked = find_hook_blocked(global_hook_id);
        assert_eq!(
            global_hook_blocked
                .get("scoped_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            global_hook_blocked
                .get("allowed_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            global_hook_blocked
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        let scoped_hook_blocked = find_hook_blocked(scoped_hook_id);
        assert_eq!(
            scoped_hook_blocked
                .get("scoped_for_persona")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            scoped_hook_blocked
                .get("allowed_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            scoped_hook_blocked
                .get("active_for_persona")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn test_multipart_upload_then_send_with_ref() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let chat_id = web_state
            .app_state
            .config
            .universal_chat_id
            .unwrap_or(997894126);
        let app = build_router(web_state);

        let boundary = "----testboundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\nPNGDATA\r\n--{boundary}--\r\n"
        );
        let upload_req = Request::builder()
            .method("POST")
            .uri(format!("/api/uploads?chat_id={chat_id}"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let upload_resp = app.clone().oneshot(upload_req).await.unwrap();
        assert_eq!(upload_resp.status(), StatusCode::OK);
        let upload_bytes = axum::body::to_bytes(upload_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let upload_json: serde_json::Value = serde_json::from_slice(&upload_bytes).unwrap();
        let tool_path = upload_json
            .get("tool_path")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let url = upload_json
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let send_body = json!({
            "chat_id": chat_id,
            "sender_name": "u",
            "message": "see image",
            "attachments": [{
                "filename": "photo.png",
                "media_type": "image/png",
                "tool_path": tool_path,
                "url": url,
            }]
        });
        let send_req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(send_body.to_string()))
            .unwrap();
        let send_resp = app.oneshot(send_req).await.unwrap();
        assert_eq!(send_resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_web_body_limit_helpers() {
        let mut cfg = crate::config::test_config();
        cfg.max_document_size_mb = 10;
        assert!(web_json_body_limit_bytes(&cfg) > 10 * 1024 * 1024);
        assert!(web_multipart_body_limit_bytes(&cfg) >= 10 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_send_stream_then_stream_done() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let app = build_router(web_state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"hi"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = v.get("run_id").and_then(|x| x.as_str()).unwrap();

        let req2 = Request::builder()
            .method("GET")
            .uri(format!("/api/stream?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("event: done"),
            "stream should end with done; body was: {text}"
        );
        assert!(
            text.contains("event: delta") || text.contains("event: replay_meta"),
            "stream should include deltas or replay metadata; body was: {text}"
        );
    }

    struct FailingLlm;

    #[async_trait::async_trait]
    impl LlmProvider for FailingLlm {
        async fn send_message(
            &self,
            _system: &str,
            _messages: Vec<crate::claude::Message>,
            _tools: Option<Vec<crate::claude::ToolDefinition>>,
        ) -> Result<crate::claude::MessagesResponse, FinallyAValueBotError> {
            Err(FinallyAValueBotError::LlmApi("sidecar boom".into()))
        }
    }

    #[tokio::test]
    async fn test_send_stream_agent_error_still_stores_bot_notice() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(FailingLlm)),
            None,
            WebLimits::default(),
        );
        let db = web_state.app_state.db.clone();
        let chat_id = web_state
            .app_state
            .config
            .universal_chat_id
            .unwrap_or(997894126_i64);
        let app = build_router(web_state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"sender_name":"u","message":"what should we do then?"}"#,
            ))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = v.get("run_id").and_then(|x| x.as_str()).unwrap();

        let req_stream = Request::builder()
            .method("GET")
            .uri(format!("/api/stream?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let resp_stream = app.oneshot(req_stream).await.unwrap();
        assert_eq!(resp_stream.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp_stream.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("event: done"),
            "failed turn must complete as done with a stored notice; body was: {text}"
        );
        assert!(
            text.contains("failed before a reply") || text.contains("sidecar boom"),
            "done payload should include the failure notice; body was: {text}"
        );

        let pid = db.get_current_persona_id(chat_id).unwrap_or(0);
        let messages = db.get_all_messages(chat_id, pid).unwrap();
        assert!(
            messages
                .iter()
                .any(|m| !m.is_from_bot && m.content.contains("what should we do then")),
            "user message must be stored"
        );
        assert!(
            messages.iter().any(|m| m.is_from_bot
                && (m.content.contains("failed before a reply")
                    || m.content.contains("sidecar boom"))),
            "bot failure notice must be stored; messages={messages:?}"
        );
    }

    #[tokio::test]
    async fn test_slash_command_via_send_stream_returns_done_with_response() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let app = build_router(web_state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"/reset"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = v.get("run_id").and_then(|x| x.as_str()).unwrap();

        let req_stream = Request::builder()
            .method("GET")
            .uri(format!("/api/stream?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let resp_stream = app.oneshot(req_stream).await.unwrap();
        assert_eq!(resp_stream.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp_stream.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);

        assert!(
            text.contains("event: done"),
            "stream should contain event: done"
        );
        assert!(
            text.contains("Conversation cleared"),
            "done event should contain slash command response"
        );
        assert!(
            !text.contains("event: delta"),
            "slash command should return only in done, no deltas"
        );
    }

    #[tokio::test]
    async fn test_auth_failure_requires_header() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            Some("secret-token".into()),
            WebLimits::default(),
        );
        let app = build_router(web_state);

        let req = Request::builder()
            .method("GET")
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_same_session_concurrency_limited() {
        // `/api/send` releases the per-session slot as soon as the work is queued, so inflight
        // does not stay held for the LLM. Rate limiting uses `max_requests_per_window` instead.
        let limits = WebLimits {
            max_inflight_per_session: 2,
            max_requests_per_window: 1,
            rate_window: Duration::from_secs(10),
            run_history_limit: 128,
            session_idle_ttl: Duration::from_secs(60),
        };
        let web_state = test_web_state(test_llm_from_provider(Arc::new(DummyLlm)), None, limits);
        let app = build_router(web_state);

        let req1 = Request::builder()
            .method("POST")
            .uri("/api/send")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"one"}"#))
            .unwrap();
        let req2 = Request::builder()
            .method("POST")
            .uri("/api/send")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"two"}"#))
            .unwrap();

        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_stream_includes_tool_events_and_replay() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(ToolFlowLlm {
                calls: AtomicUsize::new(0),
            })),
            None,
            WebLimits::default(),
        );
        let app = build_router(web_state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"do tool"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = v.get("run_id").and_then(|x| x.as_str()).unwrap();

        let req_stream = Request::builder()
            .method("GET")
            .uri(format!("/api/stream?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let resp_stream = app.clone().oneshot(req_stream).await.unwrap();
        assert_eq!(resp_stream.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp_stream.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("event: tool_start"));
        assert!(text.contains("event: tool_result"));
        assert!(text.contains("event: done"));

        let req_status = Request::builder()
            .method("GET")
            .uri(format!("/api/run_status?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let status_resp = app.clone().oneshot(req_status).await.unwrap();
        assert_eq!(status_resp.status(), StatusCode::OK);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        let last_event_id = status_json
            .get("last_event_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert!(last_event_id > 0);

        let req_replay = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/stream?run_id={run_id}&last_event_id={last_event_id}"
            ))
            .body(Body::empty())
            .unwrap();
        let replay_resp = app.oneshot(req_replay).await.unwrap();
        assert_eq!(replay_resp.status(), StatusCode::OK);
        let replay_bytes = axum::body::to_bytes(replay_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let replay_text = String::from_utf8_lossy(&replay_bytes);
        // Nothing newer than last_event_id; only replay metadata should be present.
        assert!(replay_text.contains("event: replay_meta"));
        assert!(!replay_text.contains("event: delta"));
        assert!(!replay_text.contains("event: done"));
    }

    #[tokio::test]
    async fn test_reconnect_from_last_event_id_gets_non_empty_replay() {
        let web_state = test_web_state(
            test_llm_from_provider(Arc::new(DummyLlm)),
            None,
            WebLimits::default(),
        );
        let app = build_router(web_state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/send_stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"sender_name":"u","message":"reconnect"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = v.get("run_id").and_then(|x| x.as_str()).unwrap();

        let req_stream = Request::builder()
            .method("GET")
            .uri(format!("/api/stream?run_id={run_id}"))
            .body(Body::empty())
            .unwrap();
        let resp_stream = app.clone().oneshot(req_stream).await.unwrap();
        assert_eq!(resp_stream.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp_stream.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);

        let mut ids = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("id: ") {
                if let Ok(id) = rest.trim().parse::<u64>() {
                    ids.push(id);
                }
            }
        }
        assert!(ids.len() >= 2);
        let reconnect_from = ids[0];

        let req_replay = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/stream?run_id={run_id}&last_event_id={reconnect_from}"
            ))
            .body(Body::empty())
            .unwrap();
        let replay_resp = app.oneshot(req_replay).await.unwrap();
        assert_eq!(replay_resp.status(), StatusCode::OK);
        let replay_bytes = axum::body::to_bytes(replay_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let replay_text = String::from_utf8_lossy(&replay_bytes);
        assert!(replay_text.contains("event: delta") || replay_text.contains("event: done"));
    }

    #[tokio::test]
    async fn test_rate_limit_window_recovers() {
        let limits = WebLimits {
            max_inflight_per_session: 2,
            max_requests_per_window: 1,
            rate_window: Duration::from_millis(200),
            run_history_limit: 128,
            session_idle_ttl: Duration::from_secs(60),
        };
        let web_state = test_web_state(test_llm_from_provider(Arc::new(DummyLlm)), None, limits);
        let app = build_router(web_state);

        let mk_req = |msg: &str| {
            Request::builder()
                .method("POST")
                .uri("/api/send")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"sender_name":"u","message":"{}"}}"#,
                    msg
                )))
                .unwrap()
        };

        let resp1 = app.clone().oneshot(mk_req("r1")).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        let resp2 = app.clone().oneshot(mk_req("r2")).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);

        tokio::time::sleep(Duration::from_millis(260)).await;
        let resp3 = app.oneshot(mk_req("r3")).await.unwrap();
        assert_eq!(resp3.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_db_paths_use_call_blocking_in_web_flow() {
        let state = test_state(test_llm_from_provider(Arc::new(DummyLlm)));
        let chat_id = 12345_i64;
        let cid = chat_id;
        let pid = call_blocking(state.db.clone(), move |db| db.get_current_persona_id(cid))
            .await
            .unwrap_or(0);
        let cid2 = chat_id;
        let message_count =
            call_blocking(state.db.clone(), move |db| db.get_all_messages(cid2, pid))
                .await
                .unwrap()
                .len();
        assert_eq!(message_count, 0);
    }
}
