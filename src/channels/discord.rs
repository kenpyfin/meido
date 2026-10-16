use std::path::Path;
use std::sync::Arc;

use serenity::async_trait;
use serenity::model::channel::Message as DiscordMessage;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use tracing::{error, info};

use crate::agent::{
    archive_conversation, process_with_agent_with_events, AgentRequestContext, AppState,
    InboundImages,
};
use crate::channel::{DeliveryScope, CHANNEL_PROCESSING_ACK};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::claude::Message as ClaudeMessage;
use crate::db::call_blocking;
use crate::db::StoredMessage;
use crate::slash_commands::{parse as parse_slash_command, SlashCommand};

struct Handler {
    app_state: Arc<AppState>,
    discord_bot_instance_id: i64,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: DiscordMessage) {
        // Ignore messages from bots (including ourselves)
        if msg.author.bot {
            return;
        }

        let mut text = msg.content.clone();
        let channel_id = msg.channel_id.get() as i64;
        let channel_handle = channel_id.to_string();
        let sender_name = msg.author.name.clone();
        let mut images = InboundImages::new();
        let mut attachment_notes: Vec<String> = Vec::new();

        if !msg.attachments.is_empty() {
            let max_bytes = self
                .app_state
                .config
                .max_document_size_mb
                .saturating_mul(1024)
                .saturating_mul(1024);
            let upload_dir = Path::new(self.app_state.config.working_dir())
                .join("uploads")
                .join("discord")
                .join(channel_id.to_string());
            if let Err(e) = std::fs::create_dir_all(&upload_dir) {
                error!(
                    "Failed to create Discord upload dir {}: {e}",
                    upload_dir.display()
                );
            } else {
                for (idx, attachment) in msg.attachments.iter().enumerate() {
                    let size = attachment.size as u64;
                    let mime = attachment
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string());
                    if size > max_bytes {
                        attachment_notes.push(format!(
                            "[document] filename={} bytes={} mime={} skipped=too_large",
                            attachment.filename, size, mime
                        ));
                        continue;
                    }

                    match reqwest::get(attachment.url.as_str()).await {
                        Ok(resp) => match resp.bytes().await {
                            Ok(bytes) => {
                                let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                                let safe_name = sanitize_upload_filename(&attachment.filename);
                                let path =
                                    upload_dir.join(format!("{}-{}-{}", ts, idx + 1, safe_name));
                                if let Err(e) = tokio::fs::write(&path, &bytes).await {
                                    error!(
                                        "Failed to save Discord attachment {}: {e}",
                                        path.display()
                                    );
                                    attachment_notes.push(format!(
                                        "[document] filename={} bytes={} mime={} save_error={}",
                                        attachment.filename,
                                        bytes.len(),
                                        mime,
                                        e
                                    ));
                                    continue;
                                }

                                images.push_bytes(bytes.as_ref(), &mime);

                                attachment_notes.push(format!(
                                    "[document] filename={} bytes={} mime={} saved_path={}",
                                    attachment.filename,
                                    bytes.len(),
                                    mime,
                                    path.display()
                                ));
                            }
                            Err(e) => {
                                attachment_notes.push(format!(
                                    "[document] filename={} bytes={} mime={} download_error={}",
                                    attachment.filename, size, mime, e
                                ));
                            }
                        },
                        Err(e) => {
                            attachment_notes.push(format!(
                                "[document] filename={} bytes={} mime={} download_error={}",
                                attachment.filename, size, mime, e
                            ));
                        }
                    }
                }
            }
        }

        if !attachment_notes.is_empty() {
            let notes = attachment_notes.join("\n");
            if text.trim().is_empty() {
                text = notes;
            } else {
                text = format!("{}\n\n{}", text.trim(), notes);
            }
        }

        // Resolve to unified contact (canonical_chat_id).
        // When UNIVERSAL_CHAT_ID is configured, bind this Discord handle to that canonical contact.
        let universal_chat_id = self.app_state.config.universal_chat_id;
        let inst_id = self.discord_bot_instance_id;
        let canonical_chat_id = match call_blocking(self.app_state.db.clone(), move |db| {
            if let Some(cid) = universal_chat_id {
                db.upsert_chat(cid, None, "discord")?;
                db.link_channel(cid, inst_id, "discord", &channel_handle)?;
                Ok(cid)
            } else {
                db.resolve_canonical_chat_id(inst_id, "discord", &channel_handle, None)
            }
        })
        .await
        {
            Ok(cid) => cid,
            Err(e) => {
                error!("Discord resolve_canonical_chat_id: {e}");
                return;
            }
        };
        let allowed_channels = {
            let inst_id = self.discord_bot_instance_id;
            call_blocking(self.app_state.db.clone(), move |db| {
                db.get_channel_bot_instance(inst_id)
            })
            .await
            .ok()
            .flatten()
            .map(|inst| inst.discord_allowed_channels)
            .unwrap_or_else(|| self.app_state.config.discord_allowed_channels.clone())
        };

        // Check allowed channels (empty = all)
        if !allowed_channels.is_empty() && !allowed_channels.contains(&(channel_id as u64)) {
            return;
        }

        // Single entry point: parse slash command first. If command, run backend handler and return — never send to LLM.
        if let Some(cmd) = parse_slash_command(&text) {
            match cmd {
                SlashCommand::Reset => {
                    let pid = call_blocking(self.app_state.db.clone(), move |db| {
                        db.get_current_persona_id(canonical_chat_id)
                    })
                    .await
                    .unwrap_or(0);
                    if pid > 0 {
                        let _ = call_blocking(self.app_state.db.clone(), move |db| {
                            db.delete_session(canonical_chat_id, pid)
                        })
                        .await;
                    }
                    let _ = msg
                        .channel_id
                        .say(
                            &ctx.http,
                            "Conversation cleared. Principles and per-persona memory are unchanged.",
                        )
                        .await;
                }
                SlashCommand::Skills => {
                    let formatted = self.app_state.skills.list_skills_formatted();
                    let _ = msg.channel_id.say(&ctx.http, &formatted).await;
                }
                SlashCommand::Persona => {
                    let resp = crate::persona::handle_persona_command(
                        self.app_state.db.clone(),
                        canonical_chat_id,
                        text.trim(),
                        Some(&self.app_state.config),
                    )
                    .await;
                    let _ = msg.channel_id.say(&ctx.http, resp).await;
                }
                SlashCommand::Schedule => {
                    let pid = call_blocking(self.app_state.db.clone(), move |db| {
                        db.get_current_persona_id(canonical_chat_id)
                    })
                    .await
                    .unwrap_or(0);
                    let tasks = if pid > 0 {
                        call_blocking(self.app_state.db.clone(), move |db| {
                            db.get_scheduled_tasks_for_chat_and_persona(canonical_chat_id, pid)
                        })
                        .await
                    } else {
                        call_blocking(self.app_state.db.clone(), move |db| {
                            db.get_scheduled_tasks_for_chat_for_display(canonical_chat_id)
                        })
                        .await
                    };
                    let text = match &tasks {
                        Ok(t) => crate::tools::schedule::format_tasks_list(t),
                        Err(e) => format!("Error listing tasks: {e}"),
                    };
                    let _ = msg.channel_id.say(&ctx.http, &text).await;
                }
                SlashCommand::Secrets => {
                    let reply = crate::secret_vault::secrets_command_reply(
                        &self.app_state.config.runtime_data_dir(),
                        &self.app_state.config.skills_data_dir_absolute(),
                        canonical_chat_id,
                        &text,
                    );
                    let _ = msg.channel_id.say(&ctx.http, &reply).await;
                }
                SlashCommand::Archive => {
                    let pid = call_blocking(self.app_state.db.clone(), move |db| {
                        db.get_current_persona_id(canonical_chat_id)
                    })
                    .await
                    .unwrap_or(0);
                    if pid == 0 {
                        let _ = msg
                            .channel_id
                            .say(&ctx.http, "No conversation to archive.")
                            .await;
                    } else {
                        let pid_f = pid;
                        let history = call_blocking(self.app_state.db.clone(), move |db| {
                            db.get_recent_messages(canonical_chat_id, pid_f, 500, false)
                        })
                        .await
                        .unwrap_or_default();
                        let messages: Vec<ClaudeMessage> = history
                            .into_iter()
                            .map(|m| ClaudeMessage {
                                role: if m.is_from_bot { "assistant" } else { "user" }.into(),
                                content: crate::claude::MessageContent::Text(m.content),
                            })
                            .collect();
                        if messages.is_empty() {
                            let _ = msg
                                .channel_id
                                .say(&ctx.http, "No conversation to archive.")
                                .await;
                        } else {
                            archive_conversation(
                                &self.app_state.config.runtime_data_dir(),
                                canonical_chat_id,
                                &messages,
                            );
                            let _ = msg
                                .channel_id
                                .say(&ctx.http, format!("Archived {} messages.", messages.len()))
                                .await;
                        }
                    }
                }
            }
            return;
        }

        if text.trim().is_empty() && images.is_empty() {
            return;
        }

        // Resolve run persona: optional `[PersonaName]` prefix; does not change DB active.
        let text_for_resolve = text.clone();
        let inst_id = self.discord_bot_instance_id;
        let (persona_id, text) = match call_blocking(self.app_state.db.clone(), move |db| {
            crate::persona::resolve_incoming_run_persona_for_channel(
                &db,
                canonical_chat_id,
                "discord",
                inst_id,
                &text_for_resolve,
            )
        })
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(
                    target: "persona",
                    error = %e,
                    "resolve_incoming_run_persona failed; falling back to active persona"
                );
                let pid = call_blocking(self.app_state.db.clone(), move |db| {
                    db.get_current_persona_id(canonical_chat_id)
                })
                .await
                .unwrap_or(0);
                (pid, text)
            }
        };
        if persona_id == 0 {
            return;
        }

        // Store the chat and message
        let title = format!("discord-{}", msg.channel_id.get());
        let _ = call_blocking(self.app_state.db.clone(), move |db| {
            db.upsert_chat(canonical_chat_id, Some(&title), "discord")
        })
        .await;

        let stored = StoredMessage {
            id: msg.id.get().to_string(),
            chat_id: canonical_chat_id,
            persona_id,
            session_id: None,
            sender_name: sender_name.clone(),
            content: text.clone(),
            is_from_bot: false,
            timestamp: chrono::Utc::now().to_rfc3339(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        };
        let _ = call_blocking(self.app_state.db.clone(), move |db| {
            db.store_message(&stored)
        })
        .await;

        // Determine if we should respond
        let should_respond = if msg.guild_id.is_some() {
            // In a guild: only respond to @mentions
            let cache = &ctx.cache;
            let bot_id = cache.current_user().id;
            msg.mentions.iter().any(|u| u.id == bot_id)
        } else {
            // DM: respond to all messages
            true
        };

        if !should_respond {
            return;
        }

        info!(
            "Discord message from {} in channel {}: {}",
            sender_name,
            channel_id,
            text.chars().take(100).collect::<String>()
        );

        let app_state = self.app_state.clone();
        let discord_bot_instance_id = self.discord_bot_instance_id;
        let chat_queue = app_state.chat_queue.clone();
        let channel_id_for_send = msg.channel_id;
        let is_guild = msg.guild_id.is_some();
        let http = ctx.http.clone();
        let queue_run_id = uuid::Uuid::new_v4().to_string();
        let queue_label = text.chars().take(120).collect::<String>();
        let on_hard_abort = Some(crate::queue_abort::make_deliver_hard_abort_hook(
            app_state.db.clone(),
            app_state.telegram_bots.clone(),
            app_state.discord_http.clone(),
            app_state.wecom.clone(),
            app_state.config.bot_username.clone(),
            canonical_chat_id,
            persona_id,
            queue_run_id.clone(),
            crate::channel::DeliveryScope::PlatformInstance {
                channel_type: "discord",
                bot_instance_id: discord_bot_instance_id,
                channel_handle: None,
            },
            Some(app_state.config.workspace_root_absolute()),
        ));
        let queue_meta = QueueEnqueueMeta {
            run_id: queue_run_id,
            persona_id,
            source: QueueSource::Discord,
            label: queue_label,
            project_id: None,
            workflow_id: None,
            on_hard_abort,
        };
        let (queue_position, _) = chat_queue
            .enqueue_with_meta(canonical_chat_id, queue_meta, |cancel| async move {
                // Start typing indicator while this queued item is running.
                let typing = channel_id_for_send.start_typing(&http);

                // Immediate processing ack (deleted before the final reply).
                let processing_msg = channel_id_for_send
                    .say(&http, CHANNEL_PROCESSING_ACK)
                    .await
                    .ok();

                match process_with_agent_with_events(
                    &app_state,
                    AgentRequestContext {
                        caller_channel: "discord",
                        chat_id: canonical_chat_id,
                        chat_type: if is_guild { "group" } else { "private" },
                        persona_id,
                        is_scheduled_task: false,
                        is_background_job: false,
                        run_key: None,
                        reply_bot_instance_id: Some(discord_bot_instance_id),
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
                        drop(typing);
                        if let Some(msg) = processing_msg {
                            let _ = msg.delete(&http).await;
                        }
                        let to_send = if agent_out.response.trim().is_empty() {
                            "Done.".to_string()
                        } else {
                            agent_out.response.clone()
                        };
                        if !to_send.is_empty() {
                            let delivery_scope = DeliveryScope::platform_reply(
                                "discord",
                                discord_bot_instance_id,
                                channel_id_for_send.get().to_string(),
                            );
                            if let Err(e) = crate::channel::deliver_agent_final_to_contact(
                                app_state.db.clone(),
                                app_state.telegram_bots.as_ref(),
                                app_state.discord_http.as_ref(),
                                app_state.wecom.as_deref(),
                                &app_state.config.bot_username,
                                canonical_chat_id,
                                persona_id,
                                &to_send,
                                Some(app_state.config.workspace_root_absolute()),
                                delivery_scope,
                                None,
                            )
                            .await
                            {
                                tracing::warn!(target: "channel", error = %e, "deliver_agent_final_to_contact failed; sending to Discord only");
                                send_discord_response_to_http(&http, channel_id_for_send, &to_send)
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        drop(typing);
                        if let Some(msg) = processing_msg {
                            let _ = msg.delete(&http).await;
                        }
                        error!("Error processing Discord message: {e}");
                        let _ = channel_id_for_send.say(&http, format!("Error: {e}")).await;
                    }
                }
            })
            .await;
        info!(
            target: "queue",
            chat_id = canonical_chat_id,
            queue_position = queue_position,
            "Enqueued Discord agent run"
        );
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
    }
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
        "discord-upload.bin".to_string()
    } else {
        sanitized
    }
}

async fn send_discord_response_to_http(
    http: &std::sync::Arc<serenity::http::Http>,
    channel_id: ChannelId,
    text: &str,
) {
    const MAX_LEN: usize = 2000;

    if text.len() <= MAX_LEN {
        let _ = channel_id.say(http, text).await;
        return;
    }

    let mut remaining = text;
    while !remaining.is_empty() {
        let chunk_len = if remaining.len() <= MAX_LEN {
            remaining.len()
        } else {
            remaining[..MAX_LEN].rfind('\n').unwrap_or(MAX_LEN)
        };

        let chunk = &remaining[..chunk_len];
        let _ = channel_id.say(http, chunk).await;
        remaining = &remaining[chunk_len..];

        if remaining.starts_with('\n') {
            remaining = &remaining[1..];
        }
    }
}

/// Start one Discord bot instance. `discord_bot_instance_id` matches `channel_bot_instances.id`.
pub async fn start_discord_bot(
    app_state: Arc<AppState>,
    token: &str,
    discord_bot_instance_id: i64,
) {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = Handler {
        app_state,
        discord_bot_instance_id,
    };

    let mut client = match Client::builder(token, intents).event_handler(handler).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to create Discord client: {e}");
            return;
        }
    };

    info!("Starting Discord bot...");
    if let Err(e) = client.start().await {
        error!("Discord bot error: {e}");
    }
}
