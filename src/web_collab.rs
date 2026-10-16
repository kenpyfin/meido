//! HTTP routes for schedules runs, groups, and the coordinator.
//! Merged into the main router so `web.rs` does not grow with each handler.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::{
    call_blocking, StoredMessage, MESSAGE_ORIGIN_ATTACHED_RUN, MESSAGE_ORIGIN_ATTACHED_TASK,
};
use crate::db_collab::{clip_chars, BoardPatch, GroupPatch};
use crate::web::{
    ensure_web_binding_for_universal, require_auth, resolve_chat_id_for_web, WebState,
};

use axum::http::{HeaderMap, StatusCode};

pub(crate) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/schedules/runs", get(list_runs))
        .route("/api/schedules/runs/:id", get(get_run))
        .route("/api/schedules/runs/:id/attach", post(attach_run))
        .route("/api/schedules/:id/attach", post(attach_task))
        .route("/api/groups", get(list_groups).post(create_group))
        .route("/api/groups/:id", get(get_group).patch(patch_group))
        .route("/api/groups/:id/members", post(set_members))
        .route("/api/groups/:id/sessions", post(new_session))
        .route("/api/groups/:id/history", get(group_history))
        .route("/api/groups/:id/send", post(group_send))
        .route("/api/groups/:id/cancel", post(group_cancel))
        .route(
            "/api/groups/:id/messages/:message_id/card",
            post(group_message_card),
        )
        .route("/api/coordinator/enable", post(enable_coordinator))
        .route("/api/coordinator/session", post(coordinator_session))
        .route("/api/coordinator/digest", get(coordinator_digest))
        .route("/api/coordinator/proposals", get(list_proposals))
        .route("/api/coordinator/proposals/:id", post(decide_proposal))
}

async fn chat(
    headers: &HeaderMap,
    state: &WebState,
    requested: Option<i64>,
) -> Result<i64, (StatusCode, String)> {
    require_auth(headers, state.auth_token.as_deref())?;
    let chat_id = resolve_chat_id_for_web(requested, &state.app_state.config)?;
    ensure_web_binding_for_universal(state, chat_id).await?;
    Ok(chat_id)
}

#[derive(Deserialize)]
struct ChatQuery {
    chat_id: Option<i64>,
    task_id: Option<i64>,
    before: Option<String>,
    before_id: Option<i64>,
    limit: Option<usize>,
}

async fn list_runs(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<ChatQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, query.chat_id).await?;
    let task_id = query.task_id;
    let before = query.before;
    let before_id = query.before_id;
    let limit = query.limit.unwrap_or(20);
    let rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_scheduled_runs(chat_id, task_id, before.as_deref(), before_id, limit)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let runs: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "task_id": row.task_id,
                "persona_id": row.persona_id,
                "started_at": row.started_at,
                "finished_at": row.finished_at,
                "status": row.status,
                "preview": row.preview,
                "read": row.read_at.is_some(),
            })
        })
        .collect();
    Ok(Json(json!({"ok": true, "runs": runs})))
}

async fn get_run(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let row = call_blocking(state.app_state.db.clone(), move |db| {
        db.mark_scheduled_run_read(chat_id, id)?;
        db.get_scheduled_run(chat_id, id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(row) = row else {
        return Err((StatusCode::NOT_FOUND, "run not found".into()));
    };
    let output = crate::db_collab::load_run_body(&row);
    Ok(Json(json!({
        "ok": true,
        "run": {
            "id": row.id,
            "task_id": row.task_id,
            "persona_id": row.persona_id,
            "started_at": row.started_at,
            "finished_at": row.finished_at,
            "status": row.status,
            "preview": row.preview,
            "output": output,
            "error": row.error,
        }
    })))
}

#[derive(Deserialize)]
struct AttachBody {
    persona_id: i64,
    #[serde(default)]
    session_id: Option<String>,
}

async fn attach_run(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
    Json(body): Json<AttachBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    store_attachment(
        &state,
        chat_id,
        body.persona_id,
        body.session_id,
        MESSAGE_ORIGIN_ATTACHED_RUN,
        &format!("[[scheduled_run:{id}]]"),
        id,
        true,
    )
    .await
}

async fn attach_task(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
    Json(body): Json<AttachBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    store_attachment(
        &state,
        chat_id,
        body.persona_id,
        body.session_id,
        MESSAGE_ORIGIN_ATTACHED_TASK,
        &format!("[[scheduled_task:{id}]]"),
        id,
        false,
    )
    .await
}

async fn store_attachment(
    state: &WebState,
    chat_id: i64,
    persona_id: i64,
    session_id: Option<String>,
    origin: &str,
    marker: &str,
    id: i64,
    is_run: bool,
) -> Result<Json<Value>, (StatusCode, String)> {
    let origin = origin.to_string();
    let marker = marker.to_string();
    let session = session_id.clone();
    let stored_id = call_blocking(state.app_state.db.clone(), move |db| {
        if !db.persona_exists(chat_id, persona_id)? {
            return Err(crate::error::FinallyAValueBotError::ToolExecution(
                "persona not in this chat".into(),
            ));
        }
        let excerpt = if is_run {
            db.get_scheduled_run(chat_id, id)?
                .map(|row| row.preview)
                .unwrap_or_default()
        } else {
            String::new()
        };
        let content = format!("{marker}\n{}", clip_chars(&excerpt, 500));
        let msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id,
            persona_id,
            session_id: session,
            sender_name: "operator".into(),
            content,
            is_from_bot: false,
            timestamp: Utc::now().to_rfc3339(),
            origin,
            speaker_persona_id: None,
        };
        let id = msg.id.clone();
        db.store_message(&msg)?;
        Ok(id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({"ok": true, "message_id": stored_id})))
}

#[derive(Deserialize)]
struct CreateGroupBody {
    name: String,
    #[serde(default = "default_mode")]
    turn_mode: String,
    #[serde(default = "default_speakers")]
    max_speakers: i64,
}

fn default_mode() -> String {
    "moderator".into()
}
fn default_speakers() -> i64 {
    2
}

fn group_json(
    group: &crate::db_collab::GroupRow,
    members: &[crate::db_collab::GroupMember],
) -> Value {
    json!({
        "id": group.id,
        "name": group.name,
        "turn_mode": group.turn_mode,
        "moderator_provider": group.moderator_provider,
        "moderator_model": group.moderator_model,
        "max_speakers": group.max_speakers,
        "archived_at": group.archived_at,
        "members": members.iter().map(|m| json!({
            "persona_id": m.persona_id,
            "name": m.name,
            "role_note": m.role_note,
            "position": m.position,
        })).collect::<Vec<_>>(),
    })
}

async fn list_groups(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<ChatQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, query.chat_id).await?;
    let groups = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_groups(chat_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    for group in groups {
        let gid = group.id.clone();
        let members = call_blocking(state.app_state.db.clone(), move |db| {
            db.list_group_members(&gid)
        })
        .await
        .unwrap_or_default();
        out.push(group_json(&group, &members));
    }
    Ok(Json(json!({"ok": true, "groups": out})))
}

async fn create_group(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<CreateGroupBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name is required".into()));
    }
    let group = call_blocking(state.app_state.db.clone(), move |db| {
        db.create_group(chat_id, &name, &body.turn_mode, body.max_speakers)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({"ok": true, "group": group_json(&group, &[])})))
}

async fn get_group(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let gid = id.clone();
    let group = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_group(chat_id, &gid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(group) = group else {
        return Err((StatusCode::NOT_FOUND, "group not found".into()));
    };
    let gid = id;
    let members = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_group_members(&gid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let session = call_blocking(state.app_state.db.clone(), {
        let gid = group.id.clone();
        move |db| db.current_group_session(&gid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "group": group_json(&group, &members),
        "session_id": session.map(|s| s.id),
    })))
}

#[derive(Deserialize)]
struct PatchGroupBody {
    name: Option<String>,
    turn_mode: Option<String>,
    moderator_provider: Option<String>,
    moderator_model: Option<String>,
    max_speakers: Option<i64>,
    archive: Option<bool>,
}

async fn patch_group(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Json(body): Json<PatchGroupBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let patch = GroupPatch {
        name: body.name,
        turn_mode: body.turn_mode,
        moderator_provider: body.moderator_provider,
        moderator_model: body.moderator_model,
        max_speakers: body.max_speakers,
        archive: body.archive,
    };
    let group = call_blocking(state.app_state.db.clone(), move |db| {
        db.update_group(chat_id, &id, &patch)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let Some(group) = group else {
        return Err((StatusCode::NOT_FOUND, "group not found".into()));
    };
    Ok(Json(json!({"ok": true, "group": group_json(&group, &[])})))
}

#[derive(Deserialize)]
struct MemberBody {
    persona_id: i64,
    #[serde(default)]
    role_note: String,
}

#[derive(Deserialize)]
struct MembersBody {
    members: Vec<MemberBody>,
}

async fn set_members(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Json(body): Json<MembersBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let members: Vec<(i64, String)> = body
        .members
        .into_iter()
        .map(|m| (m.persona_id, m.role_note))
        .collect();
    call_blocking(state.app_state.db.clone(), move |db| {
        db.set_group_members(chat_id, &id, &members)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

async fn new_session(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let session = call_blocking(state.app_state.db.clone(), move |db| {
        db.start_group_session(chat_id, &id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({"ok": true, "session_id": session.id})))
}

#[derive(Deserialize)]
struct HistoryQ {
    before: Option<String>,
    before_id: Option<String>,
    limit: Option<usize>,
}

async fn group_history(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Query(query): Query<HistoryQ>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let gid = id.clone();
    let session = call_blocking(state.app_state.db.clone(), move |db| {
        let group = db.get_group(chat_id, &gid)?;
        if group.is_none() {
            return Ok(None);
        }
        db.current_group_session(&gid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(session) = session else {
        return Err((StatusCode::NOT_FOUND, "group not found".into()));
    };
    let sid = session.id.clone();
    let before = query.before;
    let before_id = query.before_id;
    let limit = query.limit.unwrap_or(20);
    let messages = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_group_messages(&sid, before.as_deref(), before_id.as_deref(), limit)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rows: Vec<Value> = messages
        .into_iter()
        .map(|m| {
            json!({
                "id": m.id,
                "speaker_persona_id": m.speaker_persona_id,
                "content": m.content,
                "timestamp": m.timestamp,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "session_id": session.id,
        "summary": session.summary,
        "messages": rows,
    })))
}

#[derive(Deserialize)]
struct SendBody {
    message: String,
}

async fn group_send(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let text = body.message.trim().to_string();
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message is required".into()));
    }
    let gid = id.clone();
    let session = call_blocking(state.app_state.db.clone(), move |db| {
        let Some(_) = db.get_group(chat_id, &gid)? else {
            return Ok(None);
        };
        if let Some(session) = db.current_group_session(&gid)? {
            return Ok(Some(session));
        }
        db.start_group_session(chat_id, &gid).map(Some)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let Some(session) = session else {
        return Err((StatusCode::NOT_FOUND, "group not found".into()));
    };
    let app = state.app_state.clone();
    let session_id = session.id.clone();
    let group_id = id;
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(async move {
        if let Err(e) =
            crate::collab::groups::run_group_round(app, chat_id, group_id, session_id, text, cancel)
                .await
        {
            tracing::warn!(target: "collab", error = %e, "group round failed");
        }
    });
    Ok(Json(json!({"ok": true, "session_id": session.id})))
}

#[derive(Deserialize)]
struct CardBody {
    #[serde(default)]
    title: Option<String>,
}

async fn group_cancel(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let gid = id.clone();
    let found = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_group(chat_id, &gid)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if found.is_none() {
        return Err((StatusCode::NOT_FOUND, "group not found".into()));
    }
    crate::collab::groups::request_group_cancel(&id);
    Ok(Json(json!({"ok": true})))
}

async fn group_message_card(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path((id, message_id)): Path<(String, String)>,
    Json(body): Json<CardBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let title = body.title.unwrap_or_else(|| "From group".into());
    let item = call_blocking(state.app_state.db.clone(), move |db| {
        let Some(group) = db.get_group(chat_id, &id)? else {
            return Err(crate::error::FinallyAValueBotError::ToolExecution(
                "group not found".into(),
            ));
        };
        let persona_id = db
            .list_group_members(&group.id)?
            .first()
            .map(|m| m.persona_id)
            .unwrap_or(0);
        if persona_id <= 0 {
            return Err(crate::error::FinallyAValueBotError::ToolExecution(
                "group has no members".into(),
            ));
        }
        let todo = db.add_persona_todo(chat_id, persona_id, &title, Some("group"))?;
        db.set_todo_source(chat_id, todo.id, "group", &message_id)?;
        Ok(todo.id)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({"ok": true, "item_id": item})))
}

#[derive(Deserialize)]
struct EnableBody {
    #[serde(default)]
    brief_enabled: bool,
    #[serde(default = "default_hour")]
    brief_hour: u8,
}

fn default_hour() -> u8 {
    7
}

async fn enable_coordinator(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<EnableBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let tz = state.app_state.config.timezone.clone();
    let (persona_id, task_id) = crate::collab::coordinator::enable(
        &state.app_state,
        chat_id,
        body.brief_enabled,
        body.brief_hour,
        &tz,
    )
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": persona_id,
        "brief_task_id": task_id,
    })))
}

async fn coordinator_session(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let tz = state.app_state.config.timezone.clone();
    let title = crate::collab::coordinator::day_title(&tz);
    let db = state.app_state.db.clone();
    let found = call_blocking(db, move |db| {
        let persona_id = db.ensure_named_persona(chat_id, crate::collab::COORDINATOR_NAME)?;
        let session_id = db.ensure_coordinator_day_session(chat_id, persona_id, &title)?;
        Ok((persona_id, session_id))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "persona_id": found.0,
        "session_id": found.1,
    })))
}

async fn coordinator_digest(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let digest = crate::collab::coordinator::build_digest(&state.app_state, chat_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({"ok": true, "digest": digest})))
}

async fn list_proposals(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(query): Query<ChatQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, query.chat_id).await?;
    let rows = call_blocking(state.app_state.db.clone(), move |db| {
        db.list_proposals(chat_id, Some("pending"), None, 20)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let proposals: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "item_id": row.item_id,
                "change_json": row.change_json,
                "status": row.status,
                "created_at": row.created_at,
            })
        })
        .collect();
    Ok(Json(json!({"ok": true, "proposals": proposals})))
}

#[derive(Deserialize)]
struct DecideBody {
    status: String,
}

async fn decide_proposal(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<i64>,
    Json(body): Json<DecideBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chat_id = chat(&headers, &state, None).await?;
    let status = body.status.trim().to_string();
    let decided = call_blocking(state.app_state.db.clone(), move |db| {
        db.decide_proposal(chat_id, id, &status)
    })
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let Some(proposal) = decided else {
        return Err((StatusCode::NOT_FOUND, "proposal not found".into()));
    };
    if proposal.status == "accepted" {
        let change: Value = serde_json::from_str(&proposal.change_json).unwrap_or(json!({}));
        let item_id = change
            .get("item_id")
            .and_then(|v| v.as_i64())
            .or(proposal.item_id);
        if let Some(item_id) = item_id {
            let patch = BoardPatch {
                status: change
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                title: change
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                description: None,
                owner_persona_id: change.get("owner_persona_id").and_then(|v| v.as_i64()),
                priority: None,
                due_at: change
                    .get("due_at")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                blocked_reason: change
                    .get("blocked_reason")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                position: None,
                before_position: None,
                after_position: None,
                actor_kind: "operator",
                actor_persona_id: None,
            };
            let _ = call_blocking(state.app_state.db.clone(), move |db| {
                db.apply_board_patch(chat_id, item_id, &patch)
            })
            .await;
        } else if let Some(title) = change.get("title").and_then(|v| v.as_str()) {
            let title = title.to_string();
            let owner = change
                .get("owner_persona_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if owner > 0 {
                let _ = call_blocking(state.app_state.db.clone(), move |db| {
                    db.add_persona_todo(chat_id, owner, &title, Some("coordinator"))
                })
                .await;
            }
        }
    }
    Ok(Json(json!({"ok": true, "status": proposal.status})))
}
