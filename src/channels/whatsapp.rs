use std::path::Path;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tracing::{error, info};

use crate::agent::{process_with_agent_with_events, AgentRequestContext, AppState, InboundImages};
use crate::channel::{deliver_agent_final_to_contact, DeliveryScope, CHANNEL_PROCESSING_ACK};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::db::call_blocking;
use crate::db::StoredMessage;
use crate::slash_commands::{parse as parse_slash_command, SlashCommand};

// --- Webhook query params for verification ---

#[derive(Debug, Deserialize)]
struct WebhookQuery {
    #[serde(rename = "hub.mode", default)]
    hub_mode: Option<String>,
    #[serde(rename = "hub.verify_token", default)]
    hub_verify_token: Option<String>,
    #[serde(rename = "hub.challenge", default)]
    hub_challenge: Option<String>,
}

// --- WhatsApp Cloud API webhook payload types ---

#[derive(Debug, Deserialize)]
struct WebhookPayload {
    #[serde(default)]
    entry: Vec<WebhookEntry>,
}

#[derive(Debug, Deserialize)]
struct WebhookEntry {
    #[serde(default)]
    changes: Vec<WebhookChange>,
}

#[derive(Debug, Deserialize)]
struct WebhookChange {
    value: Option<WebhookValue>,
}

#[derive(Debug, Deserialize)]
struct WebhookValue {
    #[serde(default)]
    messages: Vec<WhatsAppMessage>,
    #[serde(default)]
    contacts: Vec<WhatsAppContact>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMessage {
    from: String,
    id: String,
    #[serde(rename = "type")]
    msg_type: String,
    text: Option<WhatsAppText>,
    image: Option<WhatsAppMediaRef>,
    document: Option<WhatsAppDocumentRef>,
    #[allow(dead_code)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppText {
    body: String,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMediaRef {
    id: String,
    mime_type: Option<String>,
    caption: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppDocumentRef {
    id: String,
    mime_type: Option<String>,
    filename: Option<String>,
    caption: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppContact {
    profile: Option<WhatsAppProfile>,
    wa_id: String,
}

#[derive(Debug, Deserialize)]
struct WhatsAppProfile {
    name: Option<String>,
}

// --- Shared state for WhatsApp handlers ---

struct WhatsAppState {
    app_state: Arc<AppState>,
    access_token: String,
    phone_number_id: String,
    verify_token: String,
    http_client: reqwest::Client,
}

// --- Webhook verification (GET /webhook) ---

async fn verify_webhook(
    Query(params): Query<WebhookQuery>,
    State(state): State<Arc<WhatsAppState>>,
) -> impl IntoResponse {
    if params.hub_mode.as_deref() == Some("subscribe")
        && params.hub_verify_token.as_deref() == Some(&state.verify_token)
    {
        if let Some(challenge) = params.hub_challenge {
            info!("WhatsApp webhook verified");
            return (StatusCode::OK, challenge);
        }
    }
    (StatusCode::FORBIDDEN, "Verification failed".to_string())
}

// --- Incoming messages (POST /webhook) ---

async fn handle_webhook(
    State(state): State<Arc<WhatsAppState>>,
    Json(payload): Json<WebhookPayload>,
) -> impl IntoResponse {
    // Respond immediately (WhatsApp requires fast 200 response)
    tokio::spawn(async move {
        if let Err(e) = process_webhook(&state, payload).await {
            error!("WhatsApp webhook processing error: {e}");
        }
    });
    StatusCode::OK
}

async fn process_webhook(state: &WhatsAppState, payload: WebhookPayload) -> anyhow::Result<()> {
    for entry in payload.entry {
        for change in entry.changes {
            let value = match change.value {
                Some(v) => v,
                None => continue,
            };

            for message in &value.messages {
                let mut text = match message.msg_type.as_str() {
                    "text" => match &message.text {
                        Some(t) => t.body.clone(),
                        None => continue,
                    },
                    "image" => message
                        .image
                        .as_ref()
                        .and_then(|m| m.caption.clone())
                        .unwrap_or_default(),
                    "document" => message
                        .document
                        .as_ref()
                        .and_then(|d| d.caption.clone())
                        .unwrap_or_default(),
                    _ => continue,
                };

                // Parse sender phone handle.
                let parsed_phone_chat_id: i64 = message.from.parse().unwrap_or(0);
                if parsed_phone_chat_id == 0 {
                    error!("Invalid WhatsApp phone number: {}", message.from);
                    continue;
                }
                // Resolve to unified contact (canonical_chat_id).
                // When UNIVERSAL_CHAT_ID is configured, bind this WhatsApp handle to that canonical contact.
                let wa_handle = message.from.clone();
                let universal_chat_id = state.app_state.config.universal_chat_id;
                let chat_id = match call_blocking(state.app_state.db.clone(), move |db| {
                    if let Some(cid) = universal_chat_id {
                        db.upsert_chat(cid, None, "whatsapp")?;
                        db.link_channel(
                            cid,
                            crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY,
                            "whatsapp",
                            &wa_handle,
                        )?;
                        Ok(cid)
                    } else {
                        Ok(parsed_phone_chat_id)
                    }
                })
                .await
                {
                    Ok(cid) => cid,
                    Err(e) => {
                        error!("WhatsApp resolve canonical chat id failed: {e}");
                        continue;
                    }
                };

                // Single entry point: parse slash command first. If command, run backend handler and return — never send to LLM.
                if let Some(cmd) = parse_slash_command(&text) {
                    match cmd {
                        SlashCommand::Reset => {
                            let pid = call_blocking(state.app_state.db.clone(), move |db| {
                                db.get_current_persona_id(chat_id)
                            })
                            .await
                            .unwrap_or(0);
                            if pid > 0 {
                                let _ = call_blocking(state.app_state.db.clone(), move |db| {
                                    db.delete_session(chat_id, pid)
                                })
                                .await;
                            }
                            send_whatsapp_message(
                                &state.http_client,
                                &state.access_token,
                                &state.phone_number_id,
                                &message.from,
                                "Conversation cleared. Principles and per-persona memory are unchanged.",
                            )
                            .await;
                        }
                        SlashCommand::Skills => {
                            let formatted = state.app_state.skills.list_skills_formatted();
                            send_whatsapp_message(
                                &state.http_client,
                                &state.access_token,
                                &state.phone_number_id,
                                &message.from,
                                &formatted,
                            )
                            .await;
                        }
                        SlashCommand::Persona => {
                            let resp = crate::persona::handle_persona_command(
                                state.app_state.db.clone(),
                                chat_id,
                                text.trim(),
                                Some(&state.app_state.config),
                            )
                            .await;
                            send_whatsapp_message(
                                &state.http_client,
                                &state.access_token,
                                &state.phone_number_id,
                                &message.from,
                                &resp,
                            )
                            .await;
                        }
                        SlashCommand::Schedule => {
                            let pid = call_blocking(state.app_state.db.clone(), move |db| {
                                db.get_current_persona_id(chat_id)
                            })
                            .await
                            .unwrap_or(0);
                            let tasks = if pid > 0 {
                                call_blocking(state.app_state.db.clone(), move |db| {
                                    db.get_scheduled_tasks_for_chat_and_persona(chat_id, pid)
                                })
                                .await
                            } else {
                                call_blocking(state.app_state.db.clone(), move |db| {
                                    db.get_scheduled_tasks_for_chat_for_display(chat_id)
                                })
                                .await
                            };
                            let text = match &tasks {
                                Ok(t) => crate::tools::schedule::format_tasks_list(t),
                                Err(e) => format!("Error listing tasks: {e}"),
                            };
                            send_whatsapp_message(
                                &state.http_client,
                                &state.access_token,
                                &state.phone_number_id,
                                &message.from,
                                &text,
                            )
                            .await;
                        }
                        SlashCommand::Secrets => {
                            let reply = crate::secret_vault::secrets_command_reply(
                                &state.app_state.config.runtime_data_dir(),
                                &state.app_state.config.skills_data_dir_absolute(),
                                chat_id,
                                &text,
                            );
                            send_whatsapp_message(
                                &state.http_client,
                                &state.access_token,
                                &state.phone_number_id,
                                &message.from,
                                &reply,
                            )
                            .await;
                        }
                        SlashCommand::Archive => {
                            let pid = call_blocking(state.app_state.db.clone(), move |db| {
                                db.get_current_persona_id(chat_id)
                            })
                            .await
                            .unwrap_or(0);
                            if pid == 0 {
                                send_whatsapp_message(
                                    &state.http_client,
                                    &state.access_token,
                                    &state.phone_number_id,
                                    &message.from,
                                    "No conversation to archive.",
                                )
                                .await;
                            } else {
                                let pid_f = pid;
                                let history =
                                    call_blocking(state.app_state.db.clone(), move |db| {
                                        db.get_recent_messages(chat_id, pid_f, 500, false)
                                    })
                                    .await
                                    .unwrap_or_default();
                                let messages: Vec<crate::claude::Message> = history
                                    .into_iter()
                                    .map(|m| crate::claude::Message {
                                        role: if m.is_from_bot { "assistant" } else { "user" }
                                            .into(),
                                        content: crate::claude::MessageContent::Text(m.content),
                                    })
                                    .collect();
                                if messages.is_empty() {
                                    send_whatsapp_message(
                                        &state.http_client,
                                        &state.access_token,
                                        &state.phone_number_id,
                                        &message.from,
                                        "No conversation to archive.",
                                    )
                                    .await;
                                } else {
                                    crate::agent::archive_conversation(
                                        &state.app_state.config.runtime_data_dir(),
                                        chat_id,
                                        &messages,
                                    );
                                    send_whatsapp_message(
                                        &state.http_client,
                                        &state.access_token,
                                        &state.phone_number_id,
                                        &message.from,
                                        &format!("Archived {} messages.", messages.len()),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    continue;
                }

                // Find sender name from contacts
                let sender_name = value
                    .contacts
                    .iter()
                    .find(|c| c.wa_id == message.from)
                    .and_then(|c| c.profile.as_ref())
                    .and_then(|p| p.name.clone())
                    .unwrap_or_else(|| message.from.clone());

                let mut images = InboundImages::new();
                if message.msg_type == "image" || message.msg_type == "document" {
                    let preferred_mime = message
                        .image
                        .as_ref()
                        .and_then(|m| m.mime_type.clone())
                        .or_else(|| message.document.as_ref().and_then(|d| d.mime_type.clone()));
                    let preferred_filename =
                        message.document.as_ref().and_then(|d| d.filename.clone());
                    let media_id = if let Some(img) = &message.image {
                        Some(img.id.as_str())
                    } else {
                        message.document.as_ref().map(|d| d.id.as_str())
                    };

                    if let Some(media_id) = media_id {
                        match download_whatsapp_media(
                            &state.http_client,
                            &state.access_token,
                            media_id,
                        )
                        .await
                        {
                            Ok((bytes, fetched_mime, fetched_name)) => {
                                let effective_mime =
                                    preferred_mime.clone().unwrap_or(fetched_mime.clone());
                                let effective_name =
                                    preferred_filename.as_deref().or(fetched_name.as_deref());
                                let max_bytes = state
                                    .app_state
                                    .config
                                    .max_document_size_mb
                                    .saturating_mul(1024)
                                    .saturating_mul(1024);
                                if (bytes.len() as u64) > max_bytes {
                                    let note = format!(
                                        "[document] filename={} bytes={} mime={} skipped=too_large",
                                        effective_name.unwrap_or("whatsapp-media.bin"),
                                        bytes.len(),
                                        effective_mime
                                    );
                                    if text.trim().is_empty() {
                                        text = note;
                                    } else {
                                        text = format!("{}\n\n{}", text.trim(), note);
                                    }
                                } else {
                                    match save_whatsapp_upload(
                                        state.app_state.config.working_dir(),
                                        chat_id,
                                        fetched_name
                                            .as_deref()
                                            .or(preferred_filename.as_deref())
                                            .unwrap_or("whatsapp-media.bin"),
                                        &bytes,
                                    )
                                    .await
                                    {
                                        Ok(path) => {
                                            images.push_bytes(&bytes, &effective_mime);
                                            let note = format!(
                                                "[document] filename={} bytes={} mime={} saved_path={}",
                                                effective_name.unwrap_or("whatsapp-media.bin"),
                                                bytes.len(),
                                                effective_mime,
                                                path
                                            );
                                            if text.trim().is_empty() {
                                                text = note;
                                            } else {
                                                text = format!("{}\n\n{}", text.trim(), note);
                                            }
                                        }
                                        Err(e) => {
                                            let note = format!(
                                                "[document] filename={} bytes={} mime={} save_error={}",
                                                effective_name.unwrap_or("whatsapp-media.bin"),
                                                bytes.len(),
                                                effective_mime,
                                                e
                                            );
                                            if text.trim().is_empty() {
                                                text = note;
                                            } else {
                                                text = format!("{}\n\n{}", text.trim(), note);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let note = format!("[document] download failed: {e}");
                                if text.trim().is_empty() {
                                    text = note;
                                } else {
                                    text = format!("{}\n\n{}", text.trim(), note);
                                }
                            }
                        }
                    }
                }

                if text.trim().is_empty() && images.is_empty() {
                    continue;
                }

                // Resolve run persona: optional `[PersonaName]` prefix; does not change DB active.
                let text_for_resolve = text.clone();
                let (persona_id, text) = match call_blocking(
                    state.app_state.db.clone(),
                    move |db| {
                        crate::persona::resolve_incoming_run_persona_for_channel(
                            &db,
                            chat_id,
                            "whatsapp",
                            crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY,
                            &text_for_resolve,
                        )
                    },
                )
                .await
                {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(
                            target: "persona",
                            error = %e,
                            "resolve_incoming_run_persona failed; falling back to active persona"
                        );
                        let pid = call_blocking(state.app_state.db.clone(), move |db| {
                            db.get_current_persona_id(chat_id)
                        })
                        .await
                        .unwrap_or(0);
                        (pid, text)
                    }
                };
                if persona_id == 0 {
                    continue;
                }

                // Store message in DB
                let sender_name_for_chat = sender_name.clone();
                let _ = call_blocking(state.app_state.db.clone(), move |db| {
                    db.upsert_chat(chat_id, Some(&sender_name_for_chat), "whatsapp")
                })
                .await;
                let stored = StoredMessage {
                    id: message.id.clone(),
                    chat_id,
                    persona_id,
                    session_id: None,
                    sender_name: sender_name.clone(),
                    content: text.clone(),
                    is_from_bot: false,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    origin: crate::db::message_origin_interactive(),
                    speaker_persona_id: None,
                };
                let _ = call_blocking(state.app_state.db.clone(), move |db| {
                    db.store_message(&stored)
                })
                .await;

                info!(
                    "WhatsApp message from {} ({}): {}",
                    sender_name,
                    message.from,
                    text.chars().take(100).collect::<String>()
                );

                // Queue by canonical chat so WhatsApp runs are backgrounded and ordered.
                let app_state = state.app_state.clone();
                let chat_queue = app_state.chat_queue.clone();
                let http_client = state.http_client.clone();
                let access_token = state.access_token.clone();
                let phone_number_id = state.phone_number_id.clone();
                let to_phone = message.from.clone();
                let queue_run_id = uuid::Uuid::new_v4().to_string();
                let queue_label = text.chars().take(120).collect::<String>();
                let on_hard_abort = Some(crate::queue_abort::make_deliver_hard_abort_hook(
                    app_state.db.clone(),
                    app_state.telegram_bots.clone(),
                    app_state.discord_http.clone(),
                    app_state.wecom.clone(),
                    app_state.config.bot_username.clone(),
                    chat_id,
                    persona_id,
                    queue_run_id.clone(),
                    crate::channel::DeliveryScope::StoreOnly,
                    Some(app_state.config.workspace_root_absolute()),
                ));
                let queue_meta = QueueEnqueueMeta {
                    run_id: queue_run_id,
                    persona_id,
                    source: QueueSource::Whatsapp,
                    label: queue_label,
                    project_id: None,
                    workflow_id: None,
                    on_hard_abort,
                };
                let (queue_position, _) = chat_queue
                    .enqueue_with_meta(chat_id, queue_meta, |cancel| async move {
                        send_whatsapp_message(
                            &http_client,
                            &access_token,
                            &phone_number_id,
                            &to_phone,
                            CHANNEL_PROCESSING_ACK,
                        )
                        .await;
                        match process_with_agent_with_events(
                            &app_state,
                            AgentRequestContext {
                                caller_channel: "whatsapp",
                                chat_id,
                                chat_type: "private",
                                persona_id,
                                is_scheduled_task: false,
                                is_background_job: false,
                                run_key: None,
                                reply_bot_instance_id: Some(
                                    crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY,
                                ),
                                session_id: None,
                                history_override: None,
                                tool_iteration_cap: None,
                                skip_recall: false,
                            },
                            None,
                            images,
                            None,
                            Some(cancel),
                        )
                        .await
                        {
                            Ok(agent_out) => {
                                if agent_out.response.trim().is_empty() {
                                    return;
                                }

                                let delivery = deliver_agent_final_to_contact(
                                    app_state.db.clone(),
                                    app_state.telegram_bots.as_ref(),
                                    app_state.discord_http.as_ref(),
                                    app_state.wecom.as_deref(),
                                    &app_state.config.bot_username,
                                    chat_id,
                                    persona_id,
                                    &agent_out.response,
                                    Some(app_state.config.workspace_root_absolute()),
                                    DeliveryScope::platform_reply(
                                        "whatsapp",
                                        crate::db::BOT_INSTANCE_WHATSAPP_PRIMARY,
                                        to_phone.clone(),
                                    ),
                                    None,
                                )
                                .await;

                                let text_for_whatsapp = match delivery {
                                    Ok(outcome) if !outcome.response_for_client.is_empty() => {
                                        outcome.response_for_client
                                    }
                                    Ok(_) => return,
                                    Err(e) => {
                                        tracing::warn!(
                                            target: "channel",
                                            error = %e,
                                            "deliver_agent_final_to_contact failed for WhatsApp"
                                        );
                                        return;
                                    }
                                };

                                send_whatsapp_message(
                                    &http_client,
                                    &access_token,
                                    &phone_number_id,
                                    &to_phone,
                                    &text_for_whatsapp,
                                )
                                .await;
                            }
                            Err(e) => {
                                error!("Error processing WhatsApp message: {e}");
                                send_whatsapp_message(
                                    &http_client,
                                    &access_token,
                                    &phone_number_id,
                                    &to_phone,
                                    &format!("Error: {e}"),
                                )
                                .await;
                            }
                        }
                    })
                    .await;
                info!(
                    target: "queue",
                    chat_id = chat_id,
                    queue_position = queue_position,
                    "Enqueued WhatsApp agent run"
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct WhatsAppMediaInfo {
    url: Option<String>,
    mime_type: Option<String>,
}

async fn download_whatsapp_media(
    client: &reqwest::Client,
    access_token: &str,
    media_id: &str,
) -> anyhow::Result<(Vec<u8>, String, Option<String>)> {
    let info_url = format!("https://graph.facebook.com/v21.0/{media_id}");
    let info_resp = client
        .get(&info_url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;
    if !info_resp.status().is_success() {
        let status = info_resp.status();
        let body = info_resp.text().await.unwrap_or_default();
        anyhow::bail!("media metadata request failed ({status}): {body}");
    }
    let info: WhatsAppMediaInfo = info_resp.json().await?;
    let download_url = info
        .url
        .ok_or_else(|| anyhow::anyhow!("missing media url"))?;

    let media_resp = client
        .get(&download_url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;
    if !media_resp.status().is_success() {
        let status = media_resp.status();
        let body = media_resp.text().await.unwrap_or_default();
        anyhow::bail!("media download failed ({status}): {body}");
    }
    let bytes = media_resp.bytes().await?.to_vec();
    let mime = info
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let ext = mime
        .split('/')
        .nth(1)
        .unwrap_or("bin")
        .split(';')
        .next()
        .unwrap_or("bin");
    let fallback_name = format!("whatsapp-media-{}.{}", chrono::Utc::now().timestamp(), ext);
    Ok((bytes, mime, Some(fallback_name)))
}

async fn save_whatsapp_upload(
    working_dir: &str,
    chat_id: i64,
    filename: &str,
    bytes: &[u8],
) -> anyhow::Result<String> {
    let safe_name = sanitize_upload_filename(filename);
    let dir = Path::new(working_dir)
        .join("uploads")
        .join("whatsapp")
        .join(chat_id.to_string());
    tokio::fs::create_dir_all(&dir).await?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{}-{}", ts, safe_name));
    tokio::fs::write(&path, bytes).await?;
    Ok(path.display().to_string())
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
        "whatsapp-upload.bin".to_string()
    } else {
        sanitized
    }
}

// --- Send message via WhatsApp Cloud API ---

async fn send_whatsapp_message(
    client: &reqwest::Client,
    access_token: &str,
    phone_number_id: &str,
    to: &str,
    text: &str,
) {
    let url = format!("https://graph.facebook.com/v21.0/{phone_number_id}/messages");

    // Split long messages (WhatsApp limit ~4096 chars)
    const MAX_LEN: usize = 4096;
    let chunks = split_text(text, MAX_LEN);

    for chunk in chunks {
        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": to,
            "text": { "body": chunk }
        });

        match client
            .post(&url)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    error!("WhatsApp API error {status}: {body}");
                }
            }
            Err(e) => {
                error!("Failed to send WhatsApp message: {e}");
            }
        }
    }
}

fn split_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let chunk_len = if remaining.len() <= max_len {
            remaining.len()
        } else {
            remaining[..max_len].rfind('\n').unwrap_or(max_len)
        };
        chunks.push(remaining[..chunk_len].to_string());
        remaining = &remaining[chunk_len..];
        if remaining.starts_with('\n') {
            remaining = &remaining[1..];
        }
    }
    chunks
}

// --- Start the WhatsApp webhook server ---

pub async fn start_whatsapp_server(
    app_state: Arc<AppState>,
    access_token: String,
    phone_number_id: String,
    verify_token: String,
    port: u16,
) {
    let wa_state = Arc::new(WhatsAppState {
        app_state,
        access_token,
        phone_number_id,
        verify_token,
        http_client: reqwest::Client::new(),
    });

    let app = Router::new()
        .route("/webhook", get(verify_webhook))
        .route("/webhook", post(handle_webhook))
        .with_state(wa_state);

    let addr = format!("0.0.0.0:{port}");
    info!("WhatsApp webhook server listening on {addr}");

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind WhatsApp webhook server on {addr}: {e}");
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("WhatsApp webhook server error: {e}");
    }
}
