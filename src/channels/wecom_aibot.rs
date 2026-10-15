//! WeCom 智能机器人 long-connection adapter (`wss://openws.work.weixin.qq.com`).
//!
//! Bot ID + Secret; no public callback URL. One live connection per bot.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::telegram::{AppState, InboundImages};

use super::wecom::{
    append_note, chat_allowed, ingest_wecom_incoming, load_wecom_allowed_chats, mime_ext,
    save_wecom_upload, split_text_bytes, wecom_handle, HANDLE_CHAT_PREFIX, HANDLE_USER_PREFIX,
};
use super::wecom_crypt::{decrypt_aes256_cbc_pkcs7_32, parse_media_aeskey};
use super::CHANNEL_PROCESSING_ACK;

const WS_URL: &str = "wss://openws.work.weixin.qq.com";
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Official stream session must finish within 10 minutes; leave margin.
const STREAM_REPLY_WINDOW: Duration = Duration::from_secs(9 * 60);
/// Reconnect if WeCom stops answering heartbeats (half-open sockets).
const PONG_STALE: Duration = Duration::from_secs(90);
const MARKDOWN_MAX_BYTES: usize = 20_000;
const RECONNECT_MIN: Duration = Duration::from_secs(2);
const RECONNECT_MAX: Duration = Duration::from_secs(60);

struct OutboundFrame {
    json: serde_json::Value,
}

struct PendingReply {
    req_id: String,
    stream_id: String,
    started: Instant,
}

pub struct WecomAiBotClient {
    bot_id: String,
    secret: String,
    app_name: String,
    outbound: mpsc::Sender<OutboundFrame>,
    outbound_rx: Mutex<Option<mpsc::Receiver<OutboundFrame>>>,
    pending: Mutex<HashMap<String, PendingReply>>,
}

impl WecomAiBotClient {
    pub fn from_config(config: &Config) -> Option<Arc<Self>> {
        if !config.wecom_uses_aibot() {
            return None;
        }
        let bot_id = config.wecom_aibot_id.as_deref()?.trim();
        let secret = config.wecom_corp_secret.as_deref()?.trim();
        if bot_id.is_empty() || secret.is_empty() {
            return None;
        }
        let (tx, rx) = mpsc::channel(64);
        Some(Arc::new(Self {
            bot_id: bot_id.to_string(),
            secret: secret.to_string(),
            app_name: config.bot_username.trim().to_string(),
            outbound: tx,
            outbound_rx: Mutex::new(Some(rx)),
            pending: Mutex::new(HashMap::new()),
        }))
    }

    /// Ack the inbound callback immediately with a stream placeholder so WeCom
    /// does not time out the long-connection reply window while the agent runs.
    pub async fn begin_stream_reply(&self, handle: &str, req_id: &str) -> Result<(), String> {
        if req_id.trim().is_empty() {
            return Ok(());
        }
        let stream_id = new_req_id();
        {
            let mut map = self.pending.lock().await;
            map.insert(
                handle.to_string(),
                PendingReply {
                    req_id: req_id.to_string(),
                    stream_id: stream_id.clone(),
                    started: Instant::now(),
                },
            );
        }
        self.outbound
            .send(OutboundFrame {
                json: respond_stream_frame(req_id, &stream_id, CHANNEL_PROCESSING_ACK, false),
            })
            .await
            .map_err(|_| "WeCom AI Bot websocket is not connected".to_string())?;
        Ok(())
    }

    pub async fn send_text(&self, handle: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let pending = {
            let mut map = self.pending.lock().await;
            map.remove(handle)
        };
        let use_stream = pending
            .as_ref()
            .is_some_and(|p| p.started.elapsed() < STREAM_REPLY_WINDOW);

        if use_stream {
            let p = pending.expect("checked above");
            // Stream content fully replaces prior updates; send one finish frame.
            // Oversized bodies: finish with truncated stream, push the rest via send_msg.
            let chunks = split_text_bytes(text, MARKDOWN_MAX_BYTES);
            let first = chunks.first().map(String::as_str).unwrap_or(text);
            self.outbound
                .send(OutboundFrame {
                    json: respond_stream_frame(&p.req_id, &p.stream_id, first, true),
                })
                .await
                .map_err(|_| "WeCom AI Bot websocket is not connected".to_string())?;
            for chunk in chunks.into_iter().skip(1) {
                self.outbound
                    .send(OutboundFrame {
                        json: send_markdown_frame(handle, &chunk),
                    })
                    .await
                    .map_err(|_| "WeCom AI Bot websocket is not connected".to_string())?;
            }
            return Ok(());
        }

        if let Some(p) = pending {
            warn!(
                target: "wecom",
                handle,
                "stream reply window expired; finishing stream then aibot_send_msg"
            );
            // Best-effort close of the callback stream before proactive send.
            let _ = self
                .outbound
                .send(OutboundFrame {
                    json: respond_stream_frame(
                        &p.req_id,
                        &p.stream_id,
                        "Reply delayed; sending as a new message.",
                        true,
                    ),
                })
                .await;
        }
        for chunk in split_text_bytes(text, MARKDOWN_MAX_BYTES) {
            self.outbound
                .send(OutboundFrame {
                    json: send_markdown_frame(handle, &chunk),
                })
                .await
                .map_err(|_| "WeCom AI Bot websocket is not connected".to_string())?;
        }
        Ok(())
    }
}

fn new_req_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn subscribe_frame(bot_id: &str, secret: &str) -> serde_json::Value {
    serde_json::json!({
        "cmd": "aibot_subscribe",
        "headers": { "req_id": new_req_id() },
        "body": { "bot_id": bot_id, "secret": secret }
    })
}

fn ping_frame() -> serde_json::Value {
    serde_json::json!({
        "cmd": "ping",
        "headers": { "req_id": new_req_id() }
    })
}

fn respond_stream_frame(
    req_id: &str,
    stream_id: &str,
    content: &str,
    finish: bool,
) -> serde_json::Value {
    serde_json::json!({
        "cmd": "aibot_respond_msg",
        "headers": { "req_id": req_id },
        "body": {
            "msgtype": "stream",
            "stream": {
                "id": stream_id,
                "finish": finish,
                "content": content
            }
        }
    })
}

fn send_markdown_frame(handle: &str, content: &str) -> serde_json::Value {
    let (chatid, chat_type) = if let Some(chatid) = handle.strip_prefix(HANDLE_CHAT_PREFIX) {
        (chatid.to_string(), 2_u32)
    } else {
        let userid = handle
            .strip_prefix(HANDLE_USER_PREFIX)
            .unwrap_or(handle)
            .to_string();
        (userid, 1_u32)
    };
    serde_json::json!({
        "cmd": "aibot_send_msg",
        "headers": { "req_id": new_req_id() },
        "body": {
            "chatid": chatid,
            "chat_type": chat_type,
            "msgtype": "markdown",
            "markdown": { "content": content }
        }
    })
}

pub async fn start_wecom_aibot(app_state: Arc<AppState>, client: Arc<WecomAiBotClient>) {
    let mut backoff = RECONNECT_MIN;
    loop {
        match run_connection(app_state.clone(), client.clone()).await {
            Ok(()) => {
                warn!("WeCom AI Bot websocket closed; reconnecting");
                backoff = RECONNECT_MIN;
            }
            Err(e) => {
                error!("WeCom AI Bot websocket error: {e}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

async fn run_connection(
    app_state: Arc<AppState>,
    client: Arc<WecomAiBotClient>,
) -> Result<(), String> {
    let (ws, _) = connect_async(WS_URL)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
        subscribe_frame(&client.bot_id, &client.secret).to_string(),
    ))
    .await
    .map_err(|e| format!("subscribe send failed: {e}"))?;

    let mut subscribed = false;
    let subscribe_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !subscribed {
        let remaining = subscribe_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("subscribe timed out".into());
        }
        let msg = tokio::time::timeout(remaining, stream.next())
            .await
            .map_err(|_| "subscribe timed out".to_string())?
            .ok_or_else(|| "websocket closed during subscribe".to_string())?
            .map_err(|e| format!("subscribe read failed: {e}"))?;
        let Some(value) = json_from_ws(msg)? else {
            continue;
        };
        let Some(errcode) = value.get("errcode").and_then(|v| v.as_i64()) else {
            continue;
        };
        if errcode == 0 {
            subscribed = true;
            info!("WeCom AI Bot long connection subscribed");
        } else {
            let errmsg = value
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!("subscribe failed: {errmsg}"));
        }
    }

    let mut rx = client
        .outbound_rx
        .lock()
        .await
        .take()
        .ok_or_else(|| "WeCom AI Bot outbound receiver already taken".to_string())?;
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_pong.elapsed() > PONG_STALE {
                    warn!(
                        target: "wecom",
                        stale_secs = last_pong.elapsed().as_secs(),
                        "WeCom AI Bot heartbeat stale; reconnecting"
                    );
                    *client.outbound_rx.lock().await = Some(rx);
                    return Ok(());
                }
                if sink.send(Message::text(ping_frame().to_string())).await.is_err() {
                    *client.outbound_rx.lock().await = Some(rx);
                    return Ok(());
                }
            }
            outbound = rx.recv() => {
                let Some(frame) = outbound else {
                    return Err("outbound channel closed".into());
                };
                if sink.send(Message::text(frame.json.to_string())).await.is_err() {
                    *client.outbound_rx.lock().await = Some(rx);
                    return Ok(());
                }
            }
            incoming = stream.next() => {
                match incoming {
                    None => {
                        *client.outbound_rx.lock().await = Some(rx);
                        return Ok(());
                    }
                    Some(Err(e)) => {
                        *client.outbound_rx.lock().await = Some(rx);
                        return Err(format!("read failed: {e}"));
                    }
                    Some(Ok(msg)) => {
                        if let Message::Ping(p) = &msg {
                            let _ = sink.send(Message::Pong(p.clone())).await;
                            last_pong = Instant::now();
                            continue;
                        }
                        let value = match json_from_ws(msg) {
                            Ok(Some(v)) => v,
                            Ok(None) => continue,
                            Err(e) => {
                                warn!("WeCom AI Bot skipped non-JSON frame: {e}");
                                continue;
                            }
                        };
                        let cmd = value.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                        if cmd == "pong" {
                            last_pong = Instant::now();
                            continue;
                        }
                        if !cmd.is_empty() {
                            info!(
                                target: "wecom",
                                cmd,
                                event = value
                                    .pointer("/body/event/eventtype")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                                "WeCom websocket frame"
                            );
                        }
                        if cmd == "aibot_msg_callback" {
                            let app_state = app_state.clone();
                            let client = client.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_msg_callback(&app_state, &client, &value).await {
                                    error!("WeCom AI Bot message handling failed: {e}");
                                }
                            });
                        } else if cmd == "aibot_event_callback" {
                            let event = value
                                .pointer("/body/event/eventtype")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if event == "disconnected_event" {
                                warn!("WeCom AI Bot connection replaced by another subscriber");
                                *client.outbound_rx.lock().await = Some(rx);
                                return Ok(());
                            }
                        } else if let Some(errcode) = value.get("errcode").and_then(|v| v.as_i64())
                        {
                            // Some WeCom frames ack with errcode without cmd.
                            last_pong = Instant::now();
                            if errcode != 0 {
                                let errmsg = value
                                    .get("errmsg")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                warn!("WeCom AI Bot frame errcode={errcode}: {errmsg}");
                            }
                        }
                    }
                }
            }
        }
    }
}

fn json_from_ws(msg: Message) -> Result<Option<serde_json::Value>, String> {
    let text = match msg {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => {
            String::from_utf8(b.to_vec()).map_err(|e| format!("binary utf8: {e}"))?
        }
        Message::Close(_) => return Err("websocket close frame".into()),
        _ => return Ok(None),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("json parse failed: {e}"))
}

async fn handle_msg_callback(
    app_state: &Arc<AppState>,
    client: &Arc<WecomAiBotClient>,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let req_id = json_pointer_text(value, "/headers/req_id").unwrap_or_default();
    let body = value.get("body").cloned().unwrap_or(serde_json::json!({}));
    let msgtype = json_text(body.get("msgtype")).unwrap_or_default();
    let from_user = json_pointer_text(&body, "/from/userid")
        .or_else(|| json_text(body.get("userid")))
        .unwrap_or_else(|| "unknown".to_string());
    let (is_group, chattype) = parse_chattype(&body);
    let chatid = json_text(body.get("chatid"));
    let group_chat_id = chatid.clone().filter(|_| is_group);
    info!(
        target: "wecom",
        chattype = %chattype,
        msgtype = %msgtype,
        has_chatid = chatid.is_some(),
        chatid = chatid.as_deref().unwrap_or(""),
        from_user = %from_user,
        "WeCom inbound message"
    );
    if from_user == "unknown" && group_chat_id.is_none() && msgtype.is_empty() {
        warn!(target: "wecom", "dropped inbound: missing from.userid, chatid, and msgtype");
        return Ok(());
    }
    let handle = wecom_handle(&from_user, group_chat_id.as_deref());
    let raw_target = group_chat_id
        .as_deref()
        .or(chatid.as_deref())
        .unwrap_or(from_user.as_str());
    let allowed_chats = load_wecom_allowed_chats(app_state).await;
    if !chat_allowed(&allowed_chats, raw_target) {
        warn!(
            target: "wecom",
            raw_id = %raw_target,
            allowlist = %allowed_chats.join(","),
            "dropped inbound: not in Integrations allowed chats (use the WeCom chatid, not the group name)"
        );
        return Ok(());
    }

    let mut text = match msgtype.as_str() {
        "text" => json_pointer_text(&body, "/text/content")
            .or_else(|| json_text(body.get("text")))
            .unwrap_or_default(),
        "voice" => json_pointer_text(&body, "/voice/content").unwrap_or_default(),
        "mixed" => mixed_text(&body),
        "image" | "file" | "video" => String::new(),
        "" => json_pointer_text(&body, "/text/content").unwrap_or_default(),
        other => {
            warn!(target: "wecom", msgtype = other, "WeCom inbound ignored unsupported msgtype");
            return Ok(());
        }
    };

    let mut images = InboundImages::new();
    if matches!(msgtype.as_str(), "image" | "file" | "video") {
        if let Some((note, img)) = download_aibot_media(app_state, &body, &msgtype, 0).await {
            append_note(&mut text, &note);
            if let Some((b64, mime)) = img {
                images.push_base64(b64, mime);
            }
        }
    } else if msgtype == "mixed" {
        if let Some(items) = body.pointer("/mixed/msg_item").and_then(|v| v.as_array()) {
            for (i, item) in items.iter().enumerate() {
                let kind = item.get("msgtype").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(kind, "image" | "file" | "video") {
                    if let Some((note, img)) = download_aibot_media(app_state, item, kind, i).await
                    {
                        append_note(&mut text, &note);
                        if let Some((b64, mime)) = img {
                            images.push_base64(b64, mime);
                        }
                    }
                }
            }
        }
    }

    let msg_id = body
        .get("msgid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() && images.is_empty() {
        return Ok(());
    }

    // Ack within WeCom's callback window before the (often long) agent run.
    client
        .begin_stream_reply(&handle, &req_id)
        .await
        .map_err(|e| anyhow::anyhow!("WeCom stream ack failed: {e}"))?;

    ingest_wecom_incoming(
        app_state.clone(),
        &client.app_name,
        &allowed_chats,
        &from_user,
        group_chat_id.as_deref(),
        text,
        msg_id,
        images,
        false,
    )
    .await;
    Ok(())
}

fn mixed_text(body: &serde_json::Value) -> String {
    body.pointer("/mixed/msg_item")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("msgtype").and_then(|v| v.as_str()) == Some("text") {
                        json_pointer_text(item, "/text/content")
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn json_text(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn json_pointer_text(v: &serde_json::Value, pointer: &str) -> Option<String> {
    json_text(v.pointer(pointer))
}

fn parse_chattype(body: &serde_json::Value) -> (bool, String) {
    match body.get("chattype") {
        Some(serde_json::Value::String(s)) => {
            let s = s.trim().to_ascii_lowercase();
            let group = s == "group" || s == "2";
            (group, if group { "group".into() } else { s })
        }
        Some(serde_json::Value::Number(n)) if n.as_u64() == Some(2) || n.as_i64() == Some(2) => {
            (true, "group".into())
        }
        _ => (false, "single".into()),
    }
}

async fn download_aibot_media(
    app_state: &AppState,
    obj: &serde_json::Value,
    kind: &str,
    index: usize,
) -> Option<(String, Option<(String, String)>)> {
    let (url, aeskey) = match kind {
        "image" => (
            obj.pointer("/image/url").and_then(|v| v.as_str())?,
            obj.pointer("/image/aeskey").and_then(|v| v.as_str()),
        ),
        "file" => (
            obj.pointer("/file/url").and_then(|v| v.as_str())?,
            obj.pointer("/file/aeskey").and_then(|v| v.as_str()),
        ),
        "video" => (
            obj.pointer("/video/url").and_then(|v| v.as_str())?,
            obj.pointer("/video/aeskey").and_then(|v| v.as_str()),
        ),
        _ => return None,
    };
    let http = reqwest::Client::new();
    let resp = match http.get(url).send().await {
        Ok(r) => r,
        Err(e) => return Some((format!("[document] download failed: {e}"), None)),
    };
    if !resp.status().is_success() {
        return Some((
            format!("[document] download failed ({})", resp.status()),
            None,
        ));
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
    let mut bytes = match resp.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => return Some((format!("[document] download failed: {e}"), None)),
    };
    if let Some(key_raw) = aeskey.filter(|s| !s.is_empty()) {
        match parse_media_aeskey(key_raw) {
            Ok(key) => match decrypt_aes256_cbc_pkcs7_32(&key, &bytes) {
                Ok(plain) => bytes = plain,
                Err(e) => return Some((format!("[document] decrypt failed: {e}"), None)),
            },
            Err(e) => return Some((format!("[document] aeskey invalid: {e}"), None)),
        }
    }
    let max_bytes = app_state
        .config
        .max_document_size_mb
        .saturating_mul(1024)
        .saturating_mul(1024);
    let filename = format!(
        "wecom-aibot-{index}-{}.{}",
        chrono::Utc::now().timestamp(),
        mime_ext(&mime)
    );
    if (bytes.len() as u64) > max_bytes {
        return Some((
            format!(
                "[document] filename={filename} bytes={} mime={mime} skipped=too_large",
                bytes.len()
            ),
            None,
        ));
    }
    let chat_id_for_path = 0_i64;
    match save_wecom_upload(
        app_state.config.working_dir(),
        chat_id_for_path,
        &filename,
        &bytes,
    )
    .await
    {
        Ok(path) => {
            let img = if mime.starts_with("image/") {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes.as_slice());
                Some((b64, mime.clone()))
            } else {
                None
            };
            Some((
                format!(
                    "[document] filename={filename} bytes={} mime={mime} saved_path={path}",
                    bytes.len()
                ),
                img,
            ))
        }
        Err(e) => Some((
            format!(
                "[document] filename={filename} bytes={} mime={mime} save_error={e}",
                bytes.len()
            ),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respond_stream_frame_sets_finish_and_id() {
        let frame = respond_stream_frame("req-1", "stream-9", CHANNEL_PROCESSING_ACK, false);
        assert_eq!(frame["cmd"], "aibot_respond_msg");
        assert_eq!(frame["headers"]["req_id"], "req-1");
        assert_eq!(frame["body"]["msgtype"], "stream");
        assert_eq!(frame["body"]["stream"]["id"], "stream-9");
        assert_eq!(frame["body"]["stream"]["finish"], false);
        assert_eq!(frame["body"]["stream"]["content"], CHANNEL_PROCESSING_ACK);

        let done = respond_stream_frame("req-1", "stream-9", "done", true);
        assert_eq!(done["body"]["stream"]["finish"], true);
    }

    #[test]
    fn send_markdown_frame_uses_group_chat_type() {
        let frame = send_markdown_frame("chat:wrABC", "hi");
        assert_eq!(frame["cmd"], "aibot_send_msg");
        assert_eq!(frame["body"]["chatid"], "wrABC");
        assert_eq!(frame["body"]["chat_type"], 2);
    }

    #[test]
    fn parse_chattype_accepts_string_and_number() {
        let group_str = serde_json::json!({"chattype": "group"});
        assert_eq!(parse_chattype(&group_str), (true, "group".into()));
        let group_num = serde_json::json!({"chattype": 2});
        assert_eq!(parse_chattype(&group_num), (true, "group".into()));
        let single = serde_json::json!({"chattype": "single"});
        assert_eq!(parse_chattype(&single), (false, "single".into()));
    }

    #[test]
    fn json_text_reads_string_or_number() {
        assert_eq!(
            json_text(Some(&serde_json::json!(" wrABC "))),
            Some("wrABC".into())
        );
        assert_eq!(json_text(Some(&serde_json::json!(2))), Some("2".into()));
        assert_eq!(json_text(Some(&serde_json::json!(""))), None);
    }
}
