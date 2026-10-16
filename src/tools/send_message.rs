use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use teloxide::prelude::*;
use teloxide::types::InputFile;

use super::{
    auth_context_from_input, authorize_chat_access, default_persona_id_for_chat, schema_object,
    Tool, ToolResult,
};
use crate::channel::{deliver_and_store_bot_message, enforce_channel_policy};
use crate::claude::ToolDefinition;
use crate::config::Config;
use crate::db::{call_blocking, Database, StoredMessage};

fn resolve_send_message_persona_id(
    input: &serde_json::Value,
    chat_id: i64,
    override_id: Option<i64>,
) -> Result<i64, String> {
    if let Some(pid) = override_id {
        if pid <= 0 {
            return Err("persona_id must be a positive integer".into());
        }
        if let Some(auth) = auth_context_from_input(input) {
            if !auth.is_control_chat() {
                let allowed = default_persona_id_for_chat(input, chat_id)
                    .filter(|id| *id > 0)
                    .unwrap_or_else(|| {
                        if auth.caller_chat_id == chat_id {
                            auth.caller_persona_id
                        } else {
                            0
                        }
                    });
                if allowed <= 0 || pid != allowed {
                    return Err(
                        "persona_id override must match the caller's run persona for this chat"
                            .into(),
                    );
                }
            }
        }
        return Ok(pid);
    }
    if let Some(pid) = default_persona_id_for_chat(input, chat_id) {
        return Ok(pid);
    }
    Err("Missing auth context to resolve persona for send_message".into())
}

enum ActiveAttachmentTarget {
    Telegram(i64),
    Discord(i64),
    Whatsapp(i64),
    Wecom(String),
    Web(i64),
}

pub struct SendMessageTool {
    bot: Bot,
    db: Arc<Database>,
    bot_username: String,
    config: Option<Config>,
    http_client: reqwest::Client,
}

impl SendMessageTool {
    pub fn new(bot: Bot, db: Arc<Database>, bot_username: String) -> Self {
        SendMessageTool {
            bot,
            db,
            bot_username,
            config: None,
            http_client: reqwest::Client::new(),
        }
    }

    pub fn new_with_config(
        bot: Bot,
        db: Arc<Database>,
        bot_username: String,
        config: Config,
    ) -> Self {
        SendMessageTool {
            bot,
            db,
            bot_username,
            config: Some(config),
            http_client: reqwest::Client::new(),
        }
    }

    async fn send_telegram_attachment(
        &self,
        chat_id: i64,
        file_path: PathBuf,
        caption: Option<String>,
    ) -> Result<String, String> {
        let is_image = matches!(
            file_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase())
                .as_deref(),
            Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp") | Some("bmp")
        );
        if is_image {
            let mut req = self
                .bot
                .send_photo(ChatId(chat_id), InputFile::file(file_path.clone()));
            if let Some(c) = &caption {
                req = req.caption(c.clone());
            }
            req.await
                .map_err(|e| format!("Failed to send Telegram image: {e}"))?;
        } else {
            let mut req = self
                .bot
                .send_document(ChatId(chat_id), InputFile::file(file_path.clone()));
            if let Some(c) = &caption {
                req = req.caption(c.clone());
            }
            req.await
                .map_err(|e| format!("Failed to send Telegram attachment: {e}"))?;
        }

        Ok(match caption {
            Some(c) => format!("[attachment:{}] {}", file_path.display(), c),
            None => format!("[attachment:{}]", file_path.display()),
        })
    }

    async fn send_discord_attachment(
        &self,
        chat_id: i64,
        file_path: PathBuf,
        caption: Option<String>,
    ) -> Result<String, String> {
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| "send_message config unavailable".to_string())?;
        let token = cfg
            .discord_bot_token
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "discord_bot_token not configured".to_string())?;

        let filename = file_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("attachment.bin")
            .to_string();
        let bytes = tokio::fs::read(&file_path)
            .await
            .map_err(|e| format!("Failed to read attachment file: {e}"))?;

        let payload = json!({ "content": caption.clone().unwrap_or_default() });
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part(
                "files[0]",
                reqwest::multipart::Part::bytes(bytes).file_name(filename),
            );

        let url = format!("https://discord.com/api/v10/channels/{chat_id}/messages");
        let resp = self
            .http_client
            .post(url)
            .header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Failed to send Discord attachment: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "Failed to send Discord attachment: HTTP {status} {}",
                body.chars().take(300).collect::<String>()
            ));
        }

        Ok(match caption {
            Some(c) => format!("[attachment:{}] {}", file_path.display(), c),
            None => format!("[attachment:{}]", file_path.display()),
        })
    }

    async fn send_whatsapp_attachment(
        &self,
        chat_id: i64,
        file_path: PathBuf,
        caption: Option<String>,
    ) -> Result<String, String> {
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| "send_message config unavailable".to_string())?;
        let access_token = cfg
            .whatsapp_access_token
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "whatsapp_access_token not configured".to_string())?;
        let phone_number_id = cfg
            .whatsapp_phone_number_id
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "whatsapp_phone_number_id not configured".to_string())?;

        let filename = file_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("attachment.bin")
            .to_string();
        let bytes = tokio::fs::read(&file_path)
            .await
            .map_err(|e| format!("Failed to read attachment file: {e}"))?;

        let upload_url = format!("https://graph.facebook.com/v23.0/{phone_number_id}/media");
        let form = reqwest::multipart::Form::new()
            .text("messaging_product", "whatsapp")
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name(filename.clone())
                    .mime_str("application/octet-stream")
                    .map_err(|e| format!("Invalid attachment mime: {e}"))?,
            );
        let upload_resp = self
            .http_client
            .post(upload_url)
            .bearer_auth(access_token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Failed to upload WhatsApp media: {e}"))?;
        if !upload_resp.status().is_success() {
            let status = upload_resp.status();
            let body = upload_resp.text().await.unwrap_or_default();
            return Err(format!(
                "Failed to upload WhatsApp media: HTTP {status} {}",
                body.chars().take(300).collect::<String>()
            ));
        }
        let upload_json: serde_json::Value = upload_resp
            .json()
            .await
            .map_err(|e| format!("Invalid WhatsApp media upload response: {e}"))?;
        let media_id = upload_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "WhatsApp media upload did not return id".to_string())?;

        let mut document = json!({ "id": media_id, "filename": filename });
        if let Some(c) = &caption {
            document["caption"] = json!(c);
        }
        let payload = json!({
            "messaging_product": "whatsapp",
            "to": chat_id.to_string(),
            "type": "document",
            "document": document,
        });
        let send_url = format!("https://graph.facebook.com/v23.0/{phone_number_id}/messages");
        let send_resp = self
            .http_client
            .post(send_url)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Failed to send WhatsApp attachment: {e}"))?;
        if !send_resp.status().is_success() {
            let status = send_resp.status();
            let body = send_resp.text().await.unwrap_or_default();
            return Err(format!(
                "Failed to send WhatsApp attachment: HTTP {status} {}",
                body.chars().take(300).collect::<String>()
            ));
        }

        Ok(match caption {
            Some(c) => format!("[attachment:{}] {}", file_path.display(), c),
            None => format!("[attachment:{}]", file_path.display()),
        })
    }

    async fn send_web_attachment(
        &self,
        chat_id: i64,
        persona_id: i64,
        file_path: PathBuf,
        caption: Option<String>,
    ) -> Result<String, String> {
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| "send_message config unavailable".to_string())?;
        let a = crate::artifacts::publish(
            cfg.workspace_root_absolute(),
            chat_id,
            persona_id,
            file_path,
            "generated",
        )
        .await
        .map_err(|e| e.to_string())?;
        let name = a.name.replace(['[', ']'], "_");
        Ok(format!(
            "{}\n\n[{}]({})",
            caption.unwrap_or_default(),
            name,
            a.url
        ))
    }

    fn normalize_caller_channel(channel: &str) -> Option<&'static str> {
        match channel.trim().to_ascii_lowercase().as_str() {
            "telegram" => Some("telegram"),
            "discord" => Some("discord"),
            "whatsapp" => Some("whatsapp"),
            "wecom" => Some("wecom"),
            "web" => Some("web"),
            _ => None,
        }
    }

    async fn resolve_active_attachment_target(
        &self,
        canonical_chat_id: i64,
        caller_channel: &str,
    ) -> Result<ActiveAttachmentTarget, String> {
        let normalized = Self::normalize_caller_channel(caller_channel).ok_or_else(|| {
            format!(
                "Attachment sending is only available from active channels: telegram, web, discord, whatsapp, wecom (got: {})",
                caller_channel
            )
        })?;

        if normalized == "web" {
            return Ok(ActiveAttachmentTarget::Web(canonical_chat_id));
        }

        let cid = canonical_chat_id;
        let bindings = call_blocking(self.db.clone(), move |db| db.list_bindings_for_contact(cid))
            .await
            .map_err(|e| format!("Failed to resolve channel bindings: {e}"))?;

        if let Some(binding) = bindings.iter().find(|b| b.channel_type == normalized) {
            if normalized == "wecom" {
                return Ok(ActiveAttachmentTarget::Wecom(
                    binding.channel_handle.clone(),
                ));
            }
            let handle = binding.channel_handle.parse::<i64>().map_err(|_| {
                format!(
                    "Invalid {} channel handle for contact {}: {}",
                    normalized, canonical_chat_id, binding.channel_handle
                )
            })?;
            return Ok(match normalized {
                "telegram" => ActiveAttachmentTarget::Telegram(handle),
                "discord" => ActiveAttachmentTarget::Discord(handle),
                "whatsapp" => ActiveAttachmentTarget::Whatsapp(handle),
                _ => unreachable!(),
            });
        }

        if normalized == "wecom" {
            return Err(format!(
                "No WeCom channel binding for contact {canonical_chat_id}"
            ));
        }

        // Back-compat: in single-channel setups, canonical chat id may itself be the handle.
        Ok(match normalized {
            "telegram" => ActiveAttachmentTarget::Telegram(canonical_chat_id),
            "discord" => ActiveAttachmentTarget::Discord(canonical_chat_id),
            "whatsapp" => ActiveAttachmentTarget::Whatsapp(canonical_chat_id),
            _ => unreachable!(),
        })
    }

    async fn send_wecom_attachment_note(
        &self,
        handle: &str,
        file_path: PathBuf,
        caption: Option<String>,
    ) -> Result<String, String> {
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| "send_message config unavailable".to_string())?;
        if cfg.wecom_uses_aibot() {
            return Err(
                "WeCom AI Bot long connection is owned by the running gateway; send_message cannot open a second websocket"
                    .into(),
            );
        }
        let client = crate::channels::wecom::WecomClient::from_config(cfg)
            .ok_or_else(|| "WeCom is not configured".to_string())?;
        let note = match caption {
            Some(c) => format!("[attachment:{}] {c}", file_path.display()),
            None => format!("[attachment:{}]", file_path.display()),
        };
        client
            .send_text(handle, &note)
            .await
            .map_err(|e| format!("Failed to send WeCom attachment note: {e}"))?;
        Ok(note)
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "send_message",
            "Send a message mid-conversation. Supports text for all channels, and attachments for Telegram/Discord/WhatsApp/web via attachment_path. For attachments, use an absolute local file path so the tool can find the file reliably.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "The target chat ID"
                    },
                    "text": {
                        "type": "string",
                        "description": "The message text to send"
                    },
                    "attachment_path": {
                        "type": "string",
                        "description": "Optional local file path to send as an attachment. Prefer an absolute path (for example: /home/user/project/workspace/shared/image.png)."
                    },
                    "caption": {
                        "type": "string",
                        "description": "Optional caption used when sending attachment"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Optional persona ID to override the default for this message"
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
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let attachment_path = input
            .get("attachment_path")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let caption = input
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let persona_id_override = input.get("persona_id").and_then(|v| v.as_i64());

        if text.is_empty() && attachment_path.is_none() {
            return ToolResult::error("Provide text and/or attachment_path".into());
        }

        if let Err(e) = authorize_chat_access(&input, chat_id) {
            return ToolResult::error(e);
        }

        if let Some(auth) = auth_context_from_input(&input) {
            if auth.is_scheduled_task && chat_id == auth.caller_chat_id {
                return ToolResult::error(
                    "Scheduled runs deliver the final reply automatically; do not use send_message for this chat. Put all user-facing output in your final assistant message."
                        .into(),
                );
            }
        }

        if let Err(e) = enforce_channel_policy(self.db.clone(), &input, chat_id).await {
            return ToolResult::error(e);
        }

        if let Some(path) = attachment_path {
            if let Err(e) = super::path_guard::check_path(&path) {
                return ToolResult::error(e).with_error_type("path_blocked");
            }
            let auth = match auth_context_from_input(&input) {
                Some(a) => a,
                None => {
                    return ToolResult::error(
                        "Attachment sending requires active channel context. Try again from the channel where you want to send the attachment.".into(),
                    )
                }
            };

            let file_path = PathBuf::from(&path);
            if !file_path.is_file() {
                return ToolResult::error(format!(
                    "attachment_path not found or not a file: {path}"
                ));
            }

            let used_caption = caption.or_else(|| {
                if text.is_empty() {
                    None
                } else {
                    Some(text.clone())
                }
            });

            let attachment_persona =
                match resolve_send_message_persona_id(&input, chat_id, persona_id_override) {
                    Ok(pid) => pid,
                    Err(_) if persona_id_override.is_none() => {
                        match call_blocking(self.db.clone(), move |db| {
                            db.get_or_create_default_persona(chat_id)
                        })
                        .await
                        {
                            Ok(pid) => pid,
                            Err(e) => return ToolResult::error(e.to_string()),
                        }
                    }
                    Err(e) => return ToolResult::error(e),
                };
            let target = match self
                .resolve_active_attachment_target(chat_id, &auth.caller_channel)
                .await
            {
                Ok(t) => t,
                Err(e) => return ToolResult::error(e),
            };

            let send_result = match target {
                ActiveAttachmentTarget::Telegram(handle) => {
                    self.send_telegram_attachment(handle, file_path.clone(), used_caption.clone())
                        .await
                }
                ActiveAttachmentTarget::Discord(handle) => {
                    self.send_discord_attachment(handle, file_path.clone(), used_caption.clone())
                        .await
                }
                ActiveAttachmentTarget::Whatsapp(handle) => {
                    self.send_whatsapp_attachment(handle, file_path.clone(), used_caption.clone())
                        .await
                }
                ActiveAttachmentTarget::Wecom(handle) => {
                    self.send_wecom_attachment_note(
                        &handle,
                        file_path.clone(),
                        used_caption.clone(),
                    )
                    .await
                }
                ActiveAttachmentTarget::Web(canonical) => {
                    self.send_web_attachment(
                        canonical,
                        attachment_persona,
                        file_path.clone(),
                        used_caption.clone(),
                    )
                    .await
                }
            };

            match send_result {
                Ok(content) => {
                    let db = self.db.clone();
                    let pid = attachment_persona;
                    let mut content = content;
                    let published = if let Some(cfg) = self.config.as_ref() {
                        match crate::artifacts::publish(
                            cfg.workspace_root_absolute(),
                            chat_id,
                            pid,
                            file_path.clone(),
                            "generated",
                        )
                        .await
                        {
                            Ok(a) => {
                                if !content.contains(&a.url) {
                                    content.push_str(&format!(
                                        "\n\n[{}]({})",
                                        a.name.replace(['[', ']'], "_"),
                                        a.url
                                    ));
                                }
                                Some(a)
                            }
                            Err(e) => {
                                tracing::warn!(error=%e,"attachment_catalog_registration_failed");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let msg = StoredMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        chat_id,
                        persona_id: pid,
                        session_id: None,
                        sender_name: self.bot_username.clone(),
                        content,
                        is_from_bot: true,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        origin: crate::db::message_origin_interactive(),
                        speaker_persona_id: None,
                    };
                    if let (Some(a), Some(cfg)) = (published, self.config.as_ref()) {
                        let root = cfg.workspace_root_absolute();
                        let mid = msg.id.clone();
                        let session = auth.session_id.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || {
                            crate::artifacts::Catalog::open(&root)?.associate(
                                &a.id,
                                None,
                                session.as_deref(),
                                Some(&mid),
                            )
                        })
                        .await
                        {
                            tracing::warn!(error=%e,"attachment_association_failed");
                        }
                    }
                    if let Err(e) = call_blocking(db, move |db| db.store_message(&msg)).await {
                        return ToolResult::error(e.to_string());
                    }
                    ToolResult::success("Attachment sent successfully.".into())
                }
                Err(e) => ToolResult::error(e),
            }
        } else {
            let cid = chat_id;
            let persona_id_result: Result<i64, String> =
                match resolve_send_message_persona_id(&input, chat_id, persona_id_override) {
                    Ok(pid) => Ok(pid),
                    Err(_) if persona_id_override.is_none() => {
                        call_blocking(self.db.clone(), move |db| {
                            db.get_or_create_default_persona(cid)
                        })
                        .await
                        .map_err(|e| e.to_string())
                    }
                    Err(e) => Err(e),
                };
            let persona_id = match persona_id_result {
                Ok(pid) => pid,
                Err(e) => return ToolResult::error(format!("Failed to resolve persona: {e}")),
            };

            let workspace_root = self.config.as_ref().map(|c| c.workspace_root_absolute());
            match deliver_and_store_bot_message(
                &self.bot,
                self.db.clone(),
                &self.bot_username,
                chat_id,
                persona_id,
                &text,
                workspace_root,
            )
            .await
            {
                Ok(_) => ToolResult::success("Message sent successfully.".into()),
                Err(e) => ToolResult::error(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use serde_json::json;

    fn test_db() -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_sendmsg_{}",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::new(dir.to_str().unwrap()).unwrap());
        (db, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_send_message_permission_denied_before_network() {
        let (db, dir) = test_db();
        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "text": "hello",
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
    async fn test_send_message_prefers_auth_caller_persona_over_db_active() {
        let (db, dir) = test_db();
        let chat_id = 777_i64;
        db.upsert_chat(chat_id, Some("web-main"), "web").unwrap();
        let alpha_id = db.create_persona(chat_id, "Alpha", None).unwrap();
        let beta_id = db.create_persona(chat_id, "Beta", None).unwrap();
        db.set_active_persona(chat_id, alpha_id).unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db.clone(), "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": chat_id,
                "text": "from beta run",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": chat_id,
                    "caller_persona_id": beta_id,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);

        let beta_msgs = db.get_all_messages(chat_id, beta_id).unwrap();
        assert_eq!(beta_msgs.len(), 1);
        assert!(beta_msgs[0].is_from_bot);
        let alpha_msgs = db.get_all_messages(chat_id, alpha_id).unwrap();
        assert!(
            alpha_msgs.is_empty(),
            "message should be stored under auth persona, not active"
        );
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_message_web_target_writes_to_db() {
        let (db, dir) = test_db();
        db.upsert_chat(999, Some("web-main"), "web").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db.clone(), "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "text": "hello web",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 999,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);

        let pid = db.get_or_create_default_persona(999).unwrap();
        let all = db.get_all_messages(999, pid).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "[default] hello web");
        assert!(all[0].is_from_bot);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_message_web_caller_cross_chat_denied() {
        let (db, dir) = test_db();
        db.upsert_chat(100, Some("web-main"), "web").unwrap();
        db.upsert_chat(200, Some("tg"), "private").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 200,
                "text": "hello",
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
    async fn test_send_message_requires_text_or_attachment() {
        let (db, dir) = test_db();
        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "text": "   "
            }))
            .await;
        assert!(result.is_error);
        assert!(result
            .content
            .contains("Provide text and/or attachment_path"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_message_blocks_scheduled_same_chat() {
        let (db, dir) = test_db();
        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "text": "scheduled update",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 999,
                    "caller_channel": "scheduler",
                    "is_scheduled_task": true,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result
            .content
            .contains("Scheduled runs deliver the final reply automatically"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_non_telegram_rejected_without_network() {
        let (db, dir) = test_db();
        db.upsert_chat(999, Some("web-main"), "web").unwrap();

        let attachment = dir.join("sample.txt");
        std::fs::write(&attachment, "hello").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "attachment_path": attachment.to_string_lossy(),
                "caption": "test",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 999,
                    "caller_channel": "web",
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("config unavailable"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_discord_without_config_fails_fast() {
        let (db, dir) = test_db();
        db.upsert_chat(123, Some("discord-123"), "discord").unwrap();

        let attachment = dir.join("sample.txt");
        std::fs::write(&attachment, "hello").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 123,
                "attachment_path": attachment.to_string_lossy(),
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 123,
                    "caller_channel": "discord",
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("config unavailable"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_whatsapp_without_config_fails_fast() {
        let (db, dir) = test_db();
        db.upsert_chat(861234567890i64, Some("wa"), "whatsapp")
            .unwrap();

        let attachment = dir.join("sample.txt");
        std::fs::write(&attachment, "hello").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 861234567890i64,
                "attachment_path": attachment.to_string_lossy(),
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 861234567890i64,
                    "caller_channel": "whatsapp",
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("config unavailable"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_requires_active_channel_context() {
        let (db, dir) = test_db();
        db.upsert_chat(999, Some("web-main"), "web").unwrap();

        let attachment = dir.join("sample.txt");
        std::fs::write(&attachment, "hello").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "attachment_path": attachment.to_string_lossy(),
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("requires active channel context"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_rejects_unknown_active_channel() {
        let (db, dir) = test_db();
        db.upsert_chat(999, Some("web-main"), "web").unwrap();

        let attachment = dir.join("sample.txt");
        std::fs::write(&attachment, "hello").unwrap();

        let tool = SendMessageTool::new(Bot::new("123456:TEST_TOKEN"), db, "bot".into());
        let result = tool
            .execute(json!({
                "chat_id": 999,
                "attachment_path": attachment.to_string_lossy(),
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 999,
                    "caller_channel": "scheduler",
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("active channels"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_send_attachment_web_uploads_and_stores_message() {
        let (db, dir) = test_db();
        db.upsert_chat(999, Some("web-main"), "web").unwrap();

        let workspace_dir = dir.join("workspace");
        std::fs::create_dir_all(workspace_dir.join("shared")).unwrap();
        std::fs::create_dir_all(workspace_dir.join("shared")).unwrap();
        let attachment = workspace_dir.join("shared/sample.pdf");
        std::fs::write(&attachment, b"pdf-bytes").unwrap();

        let mut cfg = test_config();
        cfg.workspace_dir = workspace_dir.to_string_lossy().to_string();
        let tool = SendMessageTool::new_with_config(
            Bot::new("123456:TEST_TOKEN"),
            db.clone(),
            "bot".into(),
            cfg,
        );

        let result = tool
            .execute(json!({
                "chat_id": 999,
                "attachment_path": attachment.to_string_lossy(),
                "caption": "Generated PDF",
                "__finally_a_value_bot_auth": {
                    "caller_chat_id": 999,
                    "caller_channel": "web",
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);

        let pid = db.get_or_create_default_persona(999).unwrap();
        let msgs = db.get_all_messages(999, pid).unwrap();
        assert_eq!(msgs.len(), 1);
        let content = &msgs[0].content;
        assert!(content.contains("Generated PDF"));
        assert!(content.contains("/api/artifacts/"));

        let url = content
            .split('(')
            .nth(1)
            .and_then(|s| s.strip_suffix(')'))
            .expect("expected markdown link URL");
        let id = url
            .strip_prefix("/api/artifacts/")
            .unwrap()
            .strip_suffix("/content")
            .unwrap();
        let artifact = crate::artifacts::Catalog::open(&workspace_dir)
            .unwrap()
            .get(id)
            .unwrap();
        assert_eq!(artifact.persona_id, pid);
        assert_eq!(std::fs::read(artifact.blob_path).unwrap(), b"pdf-bytes");

        cleanup(&dir);
    }
}
