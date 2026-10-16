use super::*;
use crate::artifact_http::{serve_file, ListQuery};
use crate::artifacts::{Artifact, Catalog};
use axum::http::Method;
use axum::response::Response;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex as StdMutex};

type ApiError = (StatusCode, String);
static INDEXING: LazyLock<StdMutex<HashSet<String>>> =
    LazyLock::new(|| StdMutex::new(HashSet::new()));
static INDEX_ERRORS: LazyLock<StdMutex<HashMap<String, String>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));
fn record_index_result(key: &str, result: &Result<(), ApiError>) {
    if let Ok(mut errors) = INDEX_ERRORS.lock() {
        match result {
            Ok(()) => {
                errors.remove(key);
            }
            Err(e) => {
                errors.insert(key.to_owned(), e.1.clone());
            }
        }
    }
}
static MEDIA_SESSIONS: LazyLock<StdMutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));
fn error(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::BAD_REQUEST, e.to_string())
}
async fn catalog<T: Send + 'static>(
    root: PathBuf,
    f: impl FnOnce(&mut Catalog) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(move || f(&mut Catalog::open(&root)?))
        .await
        .map_err(error)?
        .map_err(error)
}

async fn scope(
    state: &WebState,
    cid: Option<i64>,
    pid: Option<i64>,
) -> Result<(i64, i64), ApiError> {
    let cid = resolve_chat_id_for_web(cid, &state.app_state.config)?;
    let pid = resolve_history_persona_id(state, cid, pid).await?;
    Ok((cid, pid))
}
pub(super) async fn list(
    headers: HeaderMap,
    State(state): State<WebState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let started = Instant::now();
    let (cid, pid) = scope(&state, q.chat_id, q.persona_id).await?;
    let root = state.app_state.config.workspace_root_absolute();
    let key = format!("{cid}/{pid}");
    let done = catalog(root.clone(), {
        let key = key.clone();
        move |c| c.migration_complete(&key)
    })
    .await?;
    if !done {
        let start = INDEXING.lock().map_err(error)?.insert(key.clone());
        if start {
            let state = state.clone();
            let root = root.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let result = backfill(&state, root, cid, pid, &key).await;
                record_index_result(&key, &result);
                if let Err(e) = result {
                    warn!(error=%e.1,"artifact_backfill_failed");
                }
                if let Ok(mut set) = INDEXING.lock() {
                    set.remove(&key);
                }
            });
        }
    }
    let page = catalog(root, move |c| c.list(cid, pid, &q.into_filter())).await?;
    info!(
        items = page.artifacts.len(),
        duration_ms = started.elapsed().as_millis(),
        "artifact_page_loaded"
    );
    let indexing_error = INDEX_ERRORS.lock().map_err(error)?.get(&key).cloned();
    Ok(Json(
        json!({"artifacts":page.artifacts,"next_cursor":page.next_cursor,"indexing":!done,"indexing_error":indexing_error,"trash_retention_days":crate::artifacts::TRASH_RETENTION_DAYS}),
    ))
}

pub(super) fn start_backfill(state: WebState) {
    tokio::spawn(async move {
        let scopes = call_blocking(state.app_state.db.clone(), |db| db.artifact_scopes()).await;
        let Ok(scopes) = scopes else {
            warn!("artifact migration scope lookup failed");
            return;
        };
        for (cid, pid) in scopes {
            let root = state.app_state.config.workspace_root_absolute();
            let key = format!("{cid}/{pid}");
            let done = catalog(root.clone(), {
                let key = key.clone();
                move |c| c.migration_complete(&key)
            })
            .await
            .unwrap_or(false);
            if done {
                continue;
            }
            if !INDEXING
                .lock()
                .map(|mut s| s.insert(key.clone()))
                .unwrap_or(false)
            {
                continue;
            }
            let result = backfill(&state, root, cid, pid, &key).await;
            record_index_result(&key, &result);
            if let Err(e) = result {
                warn!(error=%e.1,"artifact migration failed; will retry on next library request");
            }
            if let Ok(mut set) = INDEXING.lock() {
                set.remove(&key);
            }
        }
    });
}

async fn backfill(
    state: &WebState,
    root: PathBuf,
    cid: i64,
    pid: i64,
    key: &str,
) -> Result<(), ApiError> {
    // This history read runs once per migration, never on the listing request path.
    let history = call_blocking(state.app_state.db.clone(), move |db| {
        db.get_all_messages(cid, pid)
    })
    .await
    .map_err(error)?;
    let key = key.to_string();
    catalog(root.clone(), move |c| {
        for dir in [
            root.join(format!("shared/upload/web/{cid}/{pid}")),
            root.join(format!("uploads/web/{cid}/{pid}")),
        ] {
            if dir.exists() {
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_file() {
                        continue;
                    }
                    let url = format!(
                        "/api/uploads/web/{cid}/{pid}/{}",
                        entry.file_name().to_string_lossy()
                    );
                    if c.alias(&url)?.is_some() {
                        continue;
                    }
                    let a = c.publish(cid, pid, &entry.path(), "legacy_upload")?;
                    c.add_alias(&url, &a.id)?;
                    // Historical file time avoids making all old files look newly generated.
                    let time = system_time_to_rfc3339(entry.metadata()?.modified()?);
                    if let Some(time) = time {
                        c.set_created(&a.id, &time)?;
                    }
                }
            }
        }
        let link = regex::Regex::new(r"\]\(([^)\n]+)\)|`(/[^`\n]+)`")?;
        for msg in history {
            for cap in link.captures_iter(&msg.content) {
                let Some(target) = cap.get(1).or_else(|| cap.get(2)).map(|m| m.as_str()) else {
                    continue;
                };
                if target.starts_with("/api/artifacts/") {
                    continue;
                }
                if let Some(a) = c.alias(target)? {
                    if a.chat_id != cid || a.persona_id != pid {
                        continue;
                    }
                    c.associate(&a.id, None, msg.session_id.as_deref(), Some(&msg.id))?;
                    continue;
                }
                let path = if let Some(rel) = target.strip_prefix("/api/uploads/") {
                    let p = root.join("shared/upload").join(rel);
                    if p.is_file() {
                        Some(p)
                    } else {
                        Some(root.join("uploads").join(rel))
                    }
                } else if PathBuf::from(target).is_absolute() {
                    Some(PathBuf::from(target))
                } else {
                    None
                };
                let Some(path) = path else { continue };
                if !path.is_file() {
                    let prefix = format!("/api/uploads/web/{cid}/");
                    if let Some(tail) = target.strip_prefix(&prefix) {
                        if !tail.contains('/') || tail.starts_with(&format!("{pid}/")) {
                            let a = c.missing_reference(cid, pid, target, &msg.timestamp)?;
                            c.associate(&a.id, None, msg.session_id.as_deref(), Some(&msg.id))?;
                        }
                    }
                    continue;
                }
                // Historical ambiguity is left unresolved, never repaired by basename guessing.
                match c.publish(
                    cid,
                    pid,
                    &path,
                    if target.starts_with("/api/uploads/") {
                        "legacy_upload"
                    } else {
                        "generated"
                    },
                ) {
                    Ok(a) => {
                        if target.starts_with("/api/uploads/") {
                            c.add_alias(target, &a.id)?;
                        }
                        c.associate(&a.id, None, msg.session_id.as_deref(), Some(&msg.id))?;
                        c.set_created(&a.id, &msg.timestamp)?;
                    }
                    Err(_) => continue,
                }
            }
        }
        let persona = root.join(format!("shared/personas/{cid}/{pid}"));
        let mut dirs = vec![(persona, 0usize)];
        while let Some((dir, depth)) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(ty) = entry.file_type() else { continue };
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if ty.is_dir()
                    && depth < 8
                    && !name.starts_with('.')
                    && !matches!(name.as_ref(), "node_modules" | "target" | "__pycache__")
                {
                    dirs.push((entry.path(), depth + 1));
                }
                if ty.is_file()
                    && name == "run.json"
                    && entry.metadata().is_ok_and(|m| m.len() <= 4 * 1024 * 1024)
                {
                    let Ok(bytes) = std::fs::read(entry.path()) else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    if value["artifacts"].as_array().is_some_and(|a| !a.is_empty()) {
                        // A Comfy prompt ID is producer provenance, not an invented gateway job.
                        if let Some(prompt) = value["prompt_id"].as_str() {
                            let _ = crate::artifacts::ingest_manifest(
                                &root,
                                cid,
                                pid,
                                &format!("comfy:{prompt}"),
                                None,
                                &entry.path(),
                            );
                        }
                    }
                }
            }
        }
        c.finish_migration(&key)
    })
    .await
}

pub(super) async fn media_session(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Response, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let token = uuid::Uuid::new_v4().to_string();
    let mut sessions = MEDIA_SESSIONS.lock().map_err(error)?;
    sessions.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(3600));
    sessions.insert(
        token.clone(),
        (
            Instant::now(),
            format!(
                "{:x}",
                Sha256::digest(state.auth_token.as_deref().unwrap_or("").as_bytes())
            ),
        ),
    );
    let secure = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https");
    let cookie = format!(
        "artifact_session={token}; HttpOnly; SameSite=Strict; Path=/api/; Max-Age=3600{}",
        if secure { "; Secure" } else { "" }
    );
    Ok(([("set-cookie", cookie)], Json(json!({"ok":true}))).into_response())
}
pub(super) fn media_auth(headers: &HeaderMap, state: &WebState) -> Result<(), ApiError> {
    if require_auth(headers, state.auth_token.as_deref()).is_ok() {
        return Ok(());
    }
    let token = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .map(str::trim)
                .find_map(|p| p.strip_prefix("artifact_session="))
        });
    if let Some(token) = token {
        if MEDIA_SESSIONS
            .lock()
            .map_err(error)?
            .get(token)
            .is_some_and(|(t, digest)| {
                t.elapsed() < Duration::from_secs(3600)
                    && *digest
                        == format!(
                            "{:x}",
                            Sha256::digest(state.auth_token.as_deref().unwrap_or("").as_bytes())
                        )
            })
        {
            return Ok(());
        }
    }
    Err((
        StatusCode::UNAUTHORIZED,
        "Open the library and sign in to view this file".into(),
    ))
}
async fn get_item(state: &WebState, id: String) -> Result<Artifact, ApiError> {
    catalog(state.app_state.config.workspace_root_absolute(), move |c| {
        c.get(&id)
    })
    .await
    .map_err(|_| (StatusCode::NOT_FOUND, "Artifact not found".into()))
}
pub(super) async fn detail(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Result<Json<Artifact>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    Ok(Json(get_item(&state, id).await?))
}
#[derive(Deserialize)]
pub(super) struct Change {
    status: Option<String>,
    pinned: Option<bool>,
    superseded_by: Option<String>,
}
async fn check_idle(state: &WebState, a: &Artifact) -> Result<(), ApiError> {
    // Conservative: no destructive cleanup while any producer in this persona is active.
    if state
        .app_state
        .chat_queue
        .diagnostics()
        .await
        .iter()
        .any(|lane| {
            lane.chat_id == a.chat_id
                && lane.persona_id == a.persona_id
                && (lane.active_for_ms > 0 || lane.pending > 0)
        })
    {
        return Err((
            StatusCode::CONFLICT,
            "Wait for this persona's queued/running work before cleanup".into(),
        ));
    }
    let cid = a.chat_id;
    let pid = a.persona_id;
    if call_blocking(state.app_state.db.clone(), move |db| {
        db.has_unfinished_background_jobs_for_chat_persona(cid, pid)
    })
    .await
    .map_err(error)?
    {
        return Err((
            StatusCode::CONFLICT,
            "Wait for this persona's active jobs before cleanup".into(),
        ));
    }
    Ok(())
}
pub(super) async fn update(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Json(change): Json<Change>,
) -> Result<Json<Artifact>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let a = get_item(&state, id.clone()).await?;
    if change.status.as_deref() == Some("trash") {
        check_idle(&state, &a).await?;
    }
    Ok(Json(
        catalog(state.app_state.config.workspace_root_absolute(), move |c| {
            c.update(
                &id,
                change.status.as_deref(),
                change.pinned,
                change.superseded_by.as_deref(),
            )
        })
        .await?,
    ))
}
pub(super) async fn review(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let a = get_item(&state, id.clone()).await?;
    check_idle(&state, &a).await?;
    Ok(Json(
        catalog(state.app_state.config.workspace_root_absolute(), move |c| {
            c.deletion_review(&id)
        })
        .await?,
    ))
}
#[derive(Deserialize)]
pub(super) struct DeleteRequest {
    confirmed: bool,
    #[serde(default)]
    delete_original: bool,
}
pub(super) async fn remove(
    headers: HeaderMap,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Json(body): Json<DeleteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    if !body.confirmed {
        return Err(error("Review and confirm permanent deletion"));
    }
    let a = get_item(&state, id.clone()).await?;
    check_idle(&state, &a).await?;
    catalog(state.app_state.config.workspace_root_absolute(), move |c| {
        c.delete(&id, body.delete_original)
    })
    .await?;
    Ok(Json(json!({"ok":true})))
}

pub(super) async fn content(
    headers: HeaderMap,
    method: Method,
    State(state): State<WebState>,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
) -> Result<Response, ApiError> {
    media_auth(&headers, &state)?;
    let a = get_item(&state, id).await?;
    if a.status != "active" {
        return Err((
            StatusCode::GONE,
            if a.status == "trash" {
                "This artifact is in Trash. Restore it in the library."
            } else {
                "This artifact was permanently deleted."
            }
            .into(),
        ));
    }
    serve_file(&a.blob_path, &a.name, &headers, &method, &query).await
}
