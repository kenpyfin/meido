//! WeCom (企业微信) channel: self-built app callback and shared ingest/delivery helpers.
//! Long-connection AI Bot lives in `wecom_aibot`.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::agent::{process_with_agent_with_events, AgentRequestContext, AppState, InboundImages};
use crate::channel::{deliver_agent_final_to_contact, DeliveryScope, CHANNEL_PROCESSING_ACK};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::config::Config;
use crate::db::{call_blocking, StoredMessage};
use crate::slash_commands::{parse as parse_slash_command, SlashCommand};

use super::wecom_aibot::WecomAiBotClient;
use super::wecom_crypt::WxBizMsgCrypt;

const WECOM_TEXT_MAX_BYTES: usize = 2048;
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(60);
pub(crate) const HANDLE_USER_PREFIX: &str = "user:";
pub(crate) const HANDLE_CHAT_PREFIX: &str = "chat:";

/// Callback self-built app or AI Bot long connection. `deliver_to_contact` uses `send_text`.
pub enum WecomGateway {
    Callback(Arc<WecomClient>),
    AiBot(Arc<WecomAiBotClient>),
}

impl WecomGateway {
    pub fn from_config(config: &Config) -> Option<Arc<Self>> {
        if let Some(aibot) = WecomAiBotClient::from_config(config) {
            return Some(Arc::new(Self::AiBot(aibot)));
        }
        WecomClient::from_config(config)
            .map(Self::Callback)
            .map(Arc::new)
    }

    pub fn is_aibot(&self) -> bool {
        matches!(self, Self::AiBot(_))
    }

    pub async fn send_text(&self, handle: &str, text: &str) -> Result<(), String> {
        match self {
            Self::Callback(client) => client.send_text(handle, text).await,
            Self::AiBot(client) => client.send_text(handle, text).await,
        }
    }

    /// Callback apps need a proactive interim message; AI Bot already stream-acks
    /// at `aibot_msg_callback` time via [`WecomAiBotClient::begin_stream_reply`].
    pub async fn send_processing_ack_if_callback(&self, handle: &str) {
        if let Self::Callback(client) = self {
            if let Err(e) = client.send_text(handle, CHANNEL_PROCESSING_ACK).await {
                warn!(
                    target: "wecom",
                    handle,
                    error = %e,
                    "WeCom callback processing ack failed"
                );
            }
        }
    }

    pub fn callback_client(&self) -> Option<Arc<WecomClient>> {
        match self {
            Self::Callback(client) => Some(client.clone()),
            Self::AiBot(_) => None,
        }
    }

    pub fn aibot_client(&self) -> Option<Arc<WecomAiBotClient>> {
        match self {
            Self::Callback(_) => None,
            Self::AiBot(client) => Some(client.clone()),
        }
    }
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

pub struct WecomClient {
    http: reqwest::Client,
    corp_id: String,
    corp_secret: String,
    agent_id: i64,
    callback_token: String,
    encoding_aes_key: String,
    app_name: String,
    token_cache: Mutex<Option<CachedToken>>,
}

impl WecomClient {
    pub fn from_config(config: &Config) -> Option<Arc<Self>> {
        let corp_id = config.wecom_corp_id.as_deref()?.trim();
        let corp_secret = config.wecom_corp_secret.as_deref()?.trim();
        let callback_token = config.wecom_callback_token.as_deref()?.trim();
        let encoding_aes_key = config.wecom_encoding_aes_key.as_deref()?.trim();
        if corp_id.is_empty()
            || corp_secret.is_empty()
            || callback_token.is_empty()
            || encoding_aes_key.is_empty()
            || config.wecom_agent_id <= 0
        {
            return None;
        }
        WxBizMsgCrypt::new(callback_token, encoding_aes_key, corp_id).ok()?;
        Some(Arc::new(Self {
            http: reqwest::Client::new(),
            corp_id: corp_id.to_string(),
            corp_secret: corp_secret.to_string(),
            agent_id: config.wecom_agent_id,
            callback_token: callback_token.to_string(),
            encoding_aes_key: encoding_aes_key.to_string(),
            app_name: config.bot_username.trim().to_string(),
            token_cache: Mutex::new(None),
        }))
    }

    fn crypt(&self) -> Result<WxBizMsgCrypt, String> {
        WxBizMsgCrypt::new(&self.callback_token, &self.encoding_aes_key, &self.corp_id)
    }

    async fn access_token(&self) -> Result<String, String> {
        {
            let cache = self.token_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.value.clone());
                }
            }
        }
        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={}&corpsecret={}",
            urlencoding::encode(&self.corp_id),
            urlencoding::encode(&self.corp_secret)
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("WeCom gettoken request failed: {e}"))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("WeCom gettoken response is not JSON: {e}"))?;
        let errcode = body.get("errcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if !status.is_success() || errcode != 0 {
            let errmsg = body
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!(
                "WeCom gettoken failed (HTTP {status}, errcode={errcode}): {errmsg}"
            ));
        }
        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "WeCom gettoken missing access_token".to_string())?
            .to_string();
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(7200);
        let ttl = Duration::from_secs(expires_in.saturating_sub(TOKEN_REFRESH_SKEW.as_secs()));
        let mut cache = self.token_cache.lock().await;
        *cache = Some(CachedToken {
            value: token.clone(),
            expires_at: Instant::now() + ttl,
        });
        Ok(token)
    }

    pub async fn send_text(&self, handle: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let token = self.access_token().await?;
        let chunks = split_text_bytes(text, WECOM_TEXT_MAX_BYTES);
        for chunk in chunks {
            self.send_text_chunk(&token, handle, &chunk).await?;
        }
        Ok(())
    }

    async fn send_text_chunk(
        &self,
        access_token: &str,
        handle: &str,
        text: &str,
    ) -> Result<(), String> {
        let (url, body) = if let Some(chatid) = handle.strip_prefix(HANDLE_CHAT_PREFIX) {
            (
                format!(
                    "https://qyapi.weixin.qq.com/cgi-bin/appchat/send?access_token={access_token}"
                ),
                serde_json::json!({
                    "chatid": chatid,
                    "msgtype": "text",
                    "text": { "content": text },
                    "safe": 0
                }),
            )
        } else {
            let userid = handle.strip_prefix(HANDLE_USER_PREFIX).unwrap_or(handle);
            (
                format!(
                    "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={access_token}"
                ),
                serde_json::json!({
                    "touser": userid,
                    "msgtype": "text",
                    "agentid": self.agent_id,
                    "text": { "content": text },
                    "safe": 0
                }),
            )
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("WeCom send request failed: {e}"))?;
        let status = resp.status();
        let resp_body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
        let errcode = resp_body
            .get("errcode")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if !status.is_success() || errcode != 0 {
            let errmsg = resp_body
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!(
                "WeCom send failed (HTTP {status}, errcode={errcode}): {errmsg}"
            ));
        }
        Ok(())
    }

    async fn download_media(&self, media_id: &str) -> anyhow::Result<(Vec<u8>, String)> {
        let token = self.access_token().await.map_err(|e| anyhow::anyhow!(e))?;
        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/media/get?access_token={}&media_id={}",
            urlencoding::encode(&token),
            urlencoding::encode(media_id)
        );
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("WeCom media download failed ({status}): {body}");
        }
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim()
            .to_string();
        let bytes = resp.bytes().await?.to_vec();
        Ok((bytes, mime))
    }
}

struct WecomCallbackState {
    app_state: Arc<AppState>,
    client: Arc<WecomClient>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    msg_signature: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    nonce: String,
    #[serde(default)]
    echostr: Option<String>,
}

async fn verify_callback(
    Query(params): Query<CallbackQuery>,
    State(state): State<Arc<WecomCallbackState>>,
) -> impl IntoResponse {
    let echostr = match params
        .echostr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "missing echostr".to_string()),
    };
    let crypt = match state.client.crypt() {
        Ok(c) => c,
        Err(e) => {
            error!("WeCom crypt init failed during verify");
            return (StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    };
    match crypt.verify_and_decrypt(
        &params.msg_signature,
        &params.timestamp,
        &params.nonce,
        echostr,
    ) {
        Ok(plain) => {
            info!("WeCom callback URL verified");
            (StatusCode::OK, plain)
        }
        Err(e) => {
            warn!(error = %e, "WeCom callback verification failed");
            (StatusCode::FORBIDDEN, "Verification failed".to_string())
        }
    }
}

async fn handle_callback(
    Query(params): Query<CallbackQuery>,
    State(state): State<Arc<WecomCallbackState>>,
    body: String,
) -> impl IntoResponse {
    tokio::spawn(async move {
        if let Err(e) = process_callback(&state, &params, &body).await {
            error!("WeCom callback processing error: {e}");
        }
    });
    (StatusCode::OK, "success".to_string())
}

async fn process_callback(
    state: &WecomCallbackState,
    params: &CallbackQuery,
    body: &str,
) -> anyhow::Result<()> {
    let encrypt = xml_child(body, "Encrypt").ok_or_else(|| anyhow::anyhow!("missing Encrypt"))?;
    let crypt = state.client.crypt().map_err(|e| anyhow::anyhow!(e))?;
    let xml = crypt
        .verify_and_decrypt(
            &params.msg_signature,
            &params.timestamp,
            &params.nonce,
            &encrypt,
        )
        .map_err(|e| anyhow::anyhow!(e))?;

    let msg_type = xml_child(&xml, "MsgType").unwrap_or_default();
    if msg_type == "event" {
        return Ok(());
    }

    let from_user = xml_child(&xml, "FromUserName").unwrap_or_default();
    if from_user.trim().is_empty() {
        return Ok(());
    }
    let chat_id_xml = xml_child(&xml, "ChatId").filter(|s| !s.is_empty());
    let raw_target = chat_id_xml.as_deref().unwrap_or(from_user.as_str());
    let allowed_chats = load_wecom_allowed_chats(&state.app_state).await;
    if !chat_allowed(&allowed_chats, raw_target) {
        warn!(
            target: "wecom",
            raw_id = %raw_target,
            "dropped inbound: not in Integrations allowed chats (use the WeCom chatid, not the group name)"
        );
        return Ok(());
    }

    let mut text = match msg_type.as_str() {
        "text" => xml_child(&xml, "Content").unwrap_or_default(),
        "image" => xml_child(&xml, "Content").unwrap_or_default(),
        "file" | "voice" | "video" => String::new(),
        _ => return Ok(()),
    };

    let mut images = InboundImages::new();
    if matches!(msg_type.as_str(), "image" | "file" | "voice" | "video") {
        if let Some(media_id) = xml_child(&xml, "MediaId").filter(|s| !s.is_empty()) {
            match state.client.download_media(&media_id).await {
                Ok((bytes, mime)) => {
                    let max_bytes = state
                        .app_state
                        .config
                        .max_document_size_mb
                        .saturating_mul(1024)
                        .saturating_mul(1024);
                    let filename = format!(
                        "wecom-media-{}.{}",
                        chrono::Utc::now().timestamp(),
                        mime_ext(&mime)
                    );
                    if (bytes.len() as u64) > max_bytes {
                        let note = format!(
                            "[document] filename={filename} bytes={} mime={mime} skipped=too_large",
                            bytes.len()
                        );
                        append_note(&mut text, &note);
                    } else {
                        match save_wecom_upload(
                            state.app_state.config.working_dir(),
                            0,
                            &filename,
                            &bytes,
                        )
                        .await
                        {
                            Ok(path) => {
                                images.push_bytes(&bytes, &mime);
                                let note = format!(
                                    "[document] filename={filename} bytes={} mime={mime} saved_path={path}",
                                    bytes.len()
                                );
                                append_note(&mut text, &note);
                            }
                            Err(e) => {
                                append_note(
                                    &mut text,
                                    &format!(
                                        "[document] filename={filename} bytes={} mime={mime} save_error={e}",
                                        bytes.len()
                                    ),
                                );
                            }
                        }
                    }
                }
                Err(e) => append_note(&mut text, &format!("[document] download failed: {e}")),
            }
        }
    }

    let msg_id = xml_child(&xml, "MsgId").unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    ingest_wecom_incoming(
        state.app_state.clone(),
        &state.client.app_name,
        &allowed_chats,
        &from_user,
        chat_id_xml.as_deref(),
        text,
        msg_id,
        images,
        true,
    )
    .await;
    Ok(())
}

pub(crate) async fn ingest_wecom_incoming(
    app_state: Arc<AppState>,
    app_name: &str,
    allowed_chats: &[String],
    from_user: &str,
    group_chat_id: Option<&str>,
    text: String,
    msg_id: String,
    images: InboundImages,
    mention_required_in_groups: bool,
) {
    let is_group = group_chat_id.is_some();
    let handle = wecom_handle(from_user, group_chat_id);
    let raw_target = group_chat_id.unwrap_or(from_user);
    if !chat_allowed(allowed_chats, raw_target) {
        warn!(
            target: "wecom",
            raw_id = %raw_target,
            "dropped inbound: not in Integrations allowed chats (use the WeCom chatid, not the group name)"
        );
        return;
    }
    if is_group && mention_required_in_groups && !group_is_mentioned(&text, app_name) {
        info!(
            target: "wecom",
            handle = %handle,
            "ignored group message: bot was not @mentioned"
        );
        return;
    }

    let inbox_chat_id = app_state.config.operator_inbox_chat_id();
    let universal_chat_id = app_state.config.universal_chat_id;
    let handle_for_bind = handle.clone();
    let chat_id = match call_blocking(app_state.db.clone(), move |db| {
        resolve_wecom_canonical_chat_id(db, &handle_for_bind, inbox_chat_id, universal_chat_id)
    })
    .await
    {
        Ok(cid) => cid,
        Err(e) => {
            error!("WeCom resolve canonical chat id failed: {e}");
            return;
        }
    };

    if let Some(cmd) = parse_slash_command(&text) {
        handle_slash_command(&app_state, chat_id, &handle, cmd, &text).await;
        return;
    }

    if text.trim().is_empty() && images.is_empty() {
        return;
    }

    let text_for_resolve = text.clone();
    let (persona_id, text) = match call_blocking(app_state.db.clone(), move |db| {
        crate::persona::resolve_incoming_run_persona_for_channel(
            db,
            chat_id,
            "wecom",
            crate::db::BOT_INSTANCE_WECOM_PRIMARY,
            &text_for_resolve,
        )
    })
    .await
    {
        Ok(pair) => pair,
        Err(e) => {
            warn!(
                target: "persona",
                error = %e,
                "resolve_incoming_run_persona failed; falling back to active persona"
            );
            let pid = call_blocking(app_state.db.clone(), move |db| {
                db.get_current_persona_id(chat_id)
            })
            .await
            .unwrap_or(0);
            (pid, text)
        }
    };
    if persona_id == 0 {
        warn!(
            target: "wecom",
            chat_id,
            handle = %handle,
            "dropped inbound: no persona resolved for this contact"
        );
        return;
    }

    let sender_name = from_user.to_string();
    let _ = call_blocking(app_state.db.clone(), move |db| {
        db.upsert_chat(chat_id, None, "web")
    })
    .await;
    let stored = StoredMessage {
        id: if msg_id.trim().is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            msg_id
        },
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
    let _ = call_blocking(app_state.db.clone(), move |db| db.store_message(&stored)).await;

    info!(
        "WeCom message from {} ({}): {}",
        sender_name,
        handle,
        text.chars().take(100).collect::<String>()
    );

    let handle_for_delivery = handle.clone();
    let chat_queue = app_state.chat_queue.clone();
    let chat_type = if is_group { "group" } else { "private" };
    let queue_label = text.chars().take(120).collect::<String>();
    let queue_run_id = uuid::Uuid::new_v4().to_string();
    let on_hard_abort = Some(crate::queue_abort::make_deliver_hard_abort_hook(
        app_state.db.clone(),
        app_state.telegram_bots.clone(),
        app_state.discord_http.clone(),
        app_state.wecom.clone(),
        app_state.config.bot_username.clone(),
        chat_id,
        persona_id,
        queue_run_id.clone(),
        crate::channel::DeliveryScope::platform_reply(
            "wecom",
            crate::db::BOT_INSTANCE_WECOM_PRIMARY,
            handle.clone(),
        ),
        Some(app_state.config.workspace_root_absolute()),
    ));
    let queue_meta = QueueEnqueueMeta {
        run_id: queue_run_id,
        persona_id,
        source: QueueSource::Wecom,
        label: queue_label,
        project_id: None,
        workflow_id: None,
        on_hard_abort,
    };
    let app_state_run = app_state.clone();
    let (queue_position, _) = chat_queue
        .enqueue_with_meta(chat_id, queue_meta, |cancel| async move {
            if let Some(client) = app_state_run.wecom.as_ref() {
                client
                    .send_processing_ack_if_callback(&handle_for_delivery)
                    .await;
            }
            match process_with_agent_with_events(
                &app_state_run,
                AgentRequestContext {
                    caller_channel: "wecom",
                    chat_id,
                    chat_type,
                    persona_id,
                    is_scheduled_task: false,
                    is_background_job: false,
                    run_key: None,
                    reply_bot_instance_id: Some(crate::db::BOT_INSTANCE_WECOM_PRIMARY),
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
                        warn!(
                            target: "wecom",
                            chat_id,
                            handle = %handle_for_delivery,
                            persona_id,
                            "WeCom agent run returned empty response"
                        );
                        return;
                    }
                    let response_text = agent_out.response.clone();
                    let delivery = deliver_agent_final_to_contact(
                        app_state_run.db.clone(),
                        app_state_run.telegram_bots.as_ref(),
                        app_state_run.discord_http.as_ref(),
                        app_state_run.wecom.as_deref(),
                        &app_state_run.config.bot_username,
                        chat_id,
                        persona_id,
                        &response_text,
                        Some(app_state_run.config.workspace_root_absolute()),
                        DeliveryScope::platform_reply(
                            "wecom",
                            crate::db::BOT_INSTANCE_WECOM_PRIMARY,
                            handle_for_delivery.clone(),
                        ),
                        None,
                    )
                    .await;
                    match delivery {
                        Ok(_) => {}
                        Err(e) => {
                            error!(
                                target: "wecom",
                                handle = %handle_for_delivery,
                                error = %e,
                                "WeCom agent delivery failed; sending directly to inbound handle"
                            );
                            if let Some(client) = app_state_run.wecom.as_ref() {
                                if let Err(send_err) =
                                    client.send_text(&handle_for_delivery, &response_text).await
                                {
                                    error!(
                                        target: "wecom",
                                        handle = %handle_for_delivery,
                                        error = %send_err,
                                        "WeCom direct reply failed"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => error!("WeCom agent run failed: {e}"),
            }
        })
        .await;
    info!(
        target: "queue",
        chat_id = chat_id,
        queue_position = queue_position,
        "Enqueued WeCom agent run"
    );
}

async fn handle_slash_command(
    app_state: &Arc<AppState>,
    chat_id: i64,
    handle: &str,
    cmd: SlashCommand,
    text: &str,
) {
    let reply = match cmd {
        SlashCommand::Reset => {
            let pid = call_blocking(app_state.db.clone(), move |db| {
                db.get_current_persona_id(chat_id)
            })
            .await
            .unwrap_or(0);
            if pid > 0 {
                let _ = call_blocking(app_state.db.clone(), move |db| {
                    db.delete_session(chat_id, pid)
                })
                .await;
            }
            "Conversation cleared. Principles and per-persona memory are unchanged.".to_string()
        }
        SlashCommand::Skills => app_state.skills.list_skills_formatted(),
        SlashCommand::Persona => {
            crate::persona::handle_persona_command(
                app_state.db.clone(),
                chat_id,
                text.trim(),
                Some(&app_state.config),
            )
            .await
        }
        SlashCommand::Schedule => {
            let pid = call_blocking(app_state.db.clone(), move |db| {
                db.get_current_persona_id(chat_id)
            })
            .await
            .unwrap_or(0);
            let tasks = if pid > 0 {
                call_blocking(app_state.db.clone(), move |db| {
                    db.get_scheduled_tasks_for_chat_and_persona(chat_id, pid)
                })
                .await
            } else {
                call_blocking(app_state.db.clone(), move |db| {
                    db.get_scheduled_tasks_for_chat_for_display(chat_id)
                })
                .await
            };
            match &tasks {
                Ok(t) => crate::tools::schedule::format_tasks_list(t),
                Err(e) => format!("Error listing tasks: {e}"),
            }
        }
        SlashCommand::Secrets => crate::secret_vault::secrets_command_reply(
            &app_state.config.runtime_data_dir(),
            &app_state.config.skills_data_dir_absolute(),
            chat_id,
            text,
        ),
        SlashCommand::Archive => {
            let pid = call_blocking(app_state.db.clone(), move |db| {
                db.get_current_persona_id(chat_id)
            })
            .await
            .unwrap_or(0);
            if pid == 0 {
                "No conversation to archive.".to_string()
            } else {
                let history = call_blocking(app_state.db.clone(), move |db| {
                    db.get_recent_messages(chat_id, pid, 500, false)
                })
                .await
                .unwrap_or_default();
                let messages: Vec<crate::claude::Message> = history
                    .into_iter()
                    .map(|m| crate::claude::Message {
                        role: if m.is_from_bot { "assistant" } else { "user" }.into(),
                        content: crate::claude::MessageContent::Text(m.content),
                    })
                    .collect();
                if messages.is_empty() {
                    "No conversation to archive.".to_string()
                } else {
                    crate::agent::archive_conversation(
                        &app_state.config.runtime_data_dir(),
                        chat_id,
                        &messages,
                    );
                    format!("Archived {} messages.", messages.len())
                }
            }
        }
    };
    if let Some(client) = app_state.wecom.as_ref() {
        if let Err(e) = client.send_text(handle, &reply).await {
            error!("WeCom slash-command reply failed: {e}");
        }
    }
}

/// All WeCom handles share the operator inbox for persona policy and web history.
/// Directional delivery (`DeliveryScope::platform_reply`) replies only on the inbound handle.
pub(crate) fn resolve_wecom_canonical_chat_id(
    db: &crate::db::Database,
    handle: &str,
    inbox_chat_id: i64,
    universal_chat_id: Option<i64>,
) -> Result<i64, crate::error::FinallyAValueBotError> {
    let bid = crate::db::BOT_INSTANCE_WECOM_PRIMARY;
    let target = universal_chat_id.unwrap_or(inbox_chat_id);
    db.upsert_chat(target, None, "web")?;
    db.link_channel(target, bid, "wecom", handle)?;
    Ok(target)
}

pub(crate) fn wecom_handle(from_user: &str, chat_id: Option<&str>) -> String {
    if let Some(cid) = chat_id.filter(|s| !s.is_empty()) {
        format!("{HANDLE_CHAT_PREFIX}{cid}")
    } else {
        format!("{HANDLE_USER_PREFIX}{from_user}")
    }
}

pub(crate) fn parse_wecom_allowed_chats(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Live allowlist from the WeCom bot instance row so Integrations saves apply without restart.
pub(crate) async fn load_wecom_allowed_chats(app_state: &AppState) -> Vec<String> {
    let fallback = app_state.config.wecom_allowed_chats.clone();
    match call_blocking(app_state.db.clone(), |db| {
        db.get_channel_bot_instance(crate::db::BOT_INSTANCE_WECOM_PRIMARY)
    })
    .await
    {
        Ok(Some(inst)) => parse_wecom_allowed_chats(&inst.wecom_allowed_chats),
        _ => fallback,
    }
}

fn wecom_allow_aliases(id: &str) -> Vec<String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut aliases = vec![trimmed.to_string()];
    if let Some(rest) = trimmed
        .strip_prefix(HANDLE_CHAT_PREFIX)
        .or_else(|| trimmed.strip_prefix(HANDLE_USER_PREFIX))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            aliases.push(rest.to_string());
        }
    } else {
        aliases.push(format!("{HANDLE_CHAT_PREFIX}{trimmed}"));
        aliases.push(format!("{HANDLE_USER_PREFIX}{trimmed}"));
    }
    aliases
}

pub(crate) fn chat_allowed(allowlist: &[String], raw_id: &str) -> bool {
    if allowlist.is_empty() {
        return true;
    }
    let incoming = wecom_allow_aliases(raw_id);
    allowlist.iter().any(|entry| {
        let allowed = wecom_allow_aliases(entry);
        allowed
            .iter()
            .any(|a| incoming.iter().any(|c| a.eq_ignore_ascii_case(c)))
    })
}

fn group_is_mentioned(content: &str, app_name: &str) -> bool {
    let trimmed = content.trim();
    let app = app_name.trim();
    if !app.is_empty() {
        let needle = format!("@{app}");
        if trimmed
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
        {
            return true;
        }
    }
    trimmed.contains('@')
}

fn xml_child(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let inner = xml[start..end].trim();
    let unwrapped = inner
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
        .unwrap_or(inner)
        .trim();
    if unwrapped.is_empty() {
        None
    } else {
        Some(unwrapped.to_string())
    }
}

pub(crate) fn append_note(text: &mut String, note: &str) {
    if text.trim().is_empty() {
        *text = note.to_string();
    } else {
        *text = format!("{}\n\n{note}", text.trim());
    }
}

pub(crate) fn mime_ext(mime: &str) -> &str {
    mime.split('/')
        .nth(1)
        .unwrap_or("bin")
        .split(';')
        .next()
        .unwrap_or("bin")
}

fn sanitize_upload_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "wecom-upload.bin".to_string()
    } else {
        sanitized
    }
}

pub(crate) async fn save_wecom_upload(
    working_dir: &str,
    chat_id: i64,
    filename: &str,
    bytes: &[u8],
) -> anyhow::Result<String> {
    let safe_name = sanitize_upload_filename(filename);
    let dir = Path::new(working_dir)
        .join("uploads")
        .join("wecom")
        .join(chat_id.to_string());
    tokio::fs::create_dir_all(&dir).await?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{ts}-{safe_name}"));
    tokio::fs::write(&path, bytes).await?;
    Ok(path.display().to_string())
}

pub(crate) fn split_text_bytes(text: &str, max_bytes: usize) -> Vec<String> {
    if text.len() <= max_bytes {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = start
                + text[start..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
        }
        if end < text.len() {
            if let Some(nl) = text[start..end].rfind('\n') {
                if nl > 0 {
                    end = start + nl;
                }
            }
        }
        chunks.push(text[start..end].to_string());
        start = end;
        if text.get(start..).is_some_and(|s| s.starts_with('\n')) {
            start += 1;
        }
    }
    chunks
}

pub async fn start_wecom_server(app_state: Arc<AppState>, client: Arc<WecomClient>, port: u16) {
    let state = Arc::new(WecomCallbackState { app_state, client });
    let app = Router::new()
        .route("/callback", get(verify_callback))
        .route("/callback", post(handle_callback))
        .with_state(state);
    let addr = format!("0.0.0.0:{port}");
    info!("WeCom callback server listening on {addr}/callback");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind WeCom callback server on {addr}: {e}");
            return;
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        error!("WeCom callback server error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_UNIVERSAL_CHAT_ID;
    use crate::db::{stable_canonical_chat_id_for_handle, Database, BOT_INSTANCE_WECOM_PRIMARY};

    fn test_db() -> (Database, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("meido_wecom_{}", uuid::Uuid::new_v4()));
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_wecom_binds_multiple_groups_to_operator_inbox() {
        let (db, dir) = test_db();
        let inbox = DEFAULT_UNIVERSAL_CHAT_ID;

        resolve_wecom_canonical_chat_id(&db, "chat:groupA", inbox, None).unwrap();
        resolve_wecom_canonical_chat_id(&db, "chat:groupB", inbox, None).unwrap();

        for handle in ["chat:groupA", "chat:groupB"] {
            let cid = db
                .lookup_canonical_chat_id(BOT_INSTANCE_WECOM_PRIMARY, "wecom", handle)
                .unwrap()
                .expect("binding should exist");
            assert_eq!(cid, inbox, "handle {handle} should map to operator inbox");
        }

        let bindings = db.list_bindings_for_contact(inbox).unwrap();
        let wecom_handles: Vec<_> = bindings
            .iter()
            .filter(|b| b.channel_type == "wecom")
            .map(|b| b.channel_handle.as_str())
            .collect();
        assert!(wecom_handles.contains(&"chat:groupA"));
        assert!(wecom_handles.contains(&"chat:groupB"));
        cleanup(&dir);
    }

    #[test]
    fn resolve_wecom_migrates_stale_hashed_binding_to_inbox() {
        let (db, dir) = test_db();
        let inbox = DEFAULT_UNIVERSAL_CHAT_ID;
        let handle = "chat:wrStaleGroup";
        let hashed = stable_canonical_chat_id_for_handle("wecom", handle);
        assert_ne!(hashed, inbox);

        db.link_channel(hashed, BOT_INSTANCE_WECOM_PRIMARY, "wecom", handle)
            .unwrap();
        assert_eq!(
            db.lookup_canonical_chat_id(BOT_INSTANCE_WECOM_PRIMARY, "wecom", handle)
                .unwrap(),
            Some(hashed)
        );

        let cid = resolve_wecom_canonical_chat_id(&db, handle, inbox, None).unwrap();
        assert_eq!(cid, inbox);
        assert_eq!(
            db.lookup_canonical_chat_id(BOT_INSTANCE_WECOM_PRIMARY, "wecom", handle)
                .unwrap(),
            Some(inbox)
        );
        cleanup(&dir);
    }

    #[test]
    fn resolve_wecom_prefers_universal_chat_id_when_set() {
        let (db, dir) = test_db();
        let inbox = DEFAULT_UNIVERSAL_CHAT_ID;
        let universal = inbox + 1;
        db.upsert_chat(universal, None, "web").unwrap();

        let cid =
            resolve_wecom_canonical_chat_id(&db, "chat:groupX", inbox, Some(universal)).unwrap();
        assert_eq!(cid, universal);
        assert_eq!(
            db.lookup_canonical_chat_id(BOT_INSTANCE_WECOM_PRIMARY, "wecom", "chat:groupX")
                .unwrap(),
            Some(universal)
        );
        cleanup(&dir);
    }

    #[test]
    fn wecom_handle_prefixes_user_and_chat() {
        assert_eq!(wecom_handle("zhangsan", None), "user:zhangsan");
        assert_eq!(wecom_handle("zhangsan", Some("wrABC")), "chat:wrABC");
    }

    #[test]
    fn chat_allowed_empty_allowlist_permits_all() {
        assert!(chat_allowed(&[], "wrABC"));
        assert!(chat_allowed(&[], "user:zhangsan"));
    }

    #[test]
    fn chat_allowed_matches_prefix_case_and_csv() {
        let allow = parse_wecom_allowed_chats("selling_oversea, wrABC");
        assert!(chat_allowed(&allow, "selling_oversea"));
        assert!(chat_allowed(&allow, "chat:selling_oversea"));
        assert!(chat_allowed(&allow, "Selling_Oversea"));
        assert!(chat_allowed(&allow, "wrABC"));
        assert!(!chat_allowed(&allow, "other-room"));
    }

    #[test]
    fn parse_wecom_allowed_chats_splits_commas_and_whitespace() {
        assert_eq!(
            parse_wecom_allowed_chats("a, b\nc;d"),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn xml_child_reads_cdata_and_plain() {
        let xml = "<xml><MsgType><![CDATA[text]]></MsgType><MsgId>99</MsgId></xml>";
        assert_eq!(xml_child(xml, "MsgType").as_deref(), Some("text"));
        assert_eq!(xml_child(xml, "MsgId").as_deref(), Some("99"));
    }

    #[test]
    fn split_text_bytes_respects_char_boundaries() {
        let chunks = split_text_bytes("你好世界", 4);
        assert!(chunks.iter().all(|c| c.is_char_boundary(c.len())));
        assert_eq!(chunks.concat(), "你好世界");
    }

    #[test]
    fn group_mention_detects_app_name() {
        assert!(group_is_mentioned("@Helper please", "Helper"));
        assert!(!group_is_mentioned("hello there", "Helper"));
        assert!(group_is_mentioned("@someone hi", ""));
    }

    #[test]
    fn stable_hash_is_used_for_non_universal_ids() {
        let a = stable_canonical_chat_id_for_handle("wecom", "user:zhangsan");
        let b = stable_canonical_chat_id_for_handle("wecom", "user:zhangsan");
        let c = stable_canonical_chat_id_for_handle("wecom", "user:lisi");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a > 4);
        assert_ne!(a, 997894126);
    }
}
