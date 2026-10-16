//! Telegram channel: inbound handling and delivery.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use regex::Regex;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, ChatAction, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode,
    ThreadId,
};
use tracing::{error, info, warn};

use crate::agent::*;
use crate::channel::{DeliveryScope, CHANNEL_PROCESSING_ACK};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::db::{call_blocking, StoredMessage};
use crate::slash_commands::{parse as parse_slash_command, SlashCommand};

pub(crate) async fn spawn_dispatchers(
    state: Arc<AppState>,
    bots: HashMap<i64, Bot>,
) -> anyhow::Result<()> {
    const TELEGRAM_RETRY_DELAY_SECS: u64 = 10;
    info!(
        "Starting {} Telegram dispatcher(s): {:?}",
        bots.len(),
        bots.keys().collect::<Vec<_>>()
    );
    for (inst_id, bot) in bots.iter() {
        let bot = bot.clone();
        let inst_id = *inst_id;
        let state_clone = state.clone();
        info!("Telegram dispatcher starting for instance {inst_id}");
        tokio::spawn(async move {
            loop {
                let bot_clone = bot.clone();
                let state_inner = state_clone.clone();
                let join_result = tokio::spawn(async move {
                    let handler = dptree::entry()
                        .branch(Update::filter_message().endpoint(crate::telegram::handle_message))
                        .branch(
                            Update::filter_callback_query()
                                .endpoint(crate::telegram::handle_callback_query),
                        );
                    Dispatcher::builder(bot_clone, handler)
                        .default_handler(|_| async {})
                        .dependencies(dptree::deps![state_inner, inst_id])
                        .enable_ctrlc_handler()
                        .build()
                        .dispatch()
                        .await;
                })
                .await;

                match join_result {
                    Ok(()) => break,
                    Err(e) => {
                        error!(
                            "Telegram dispatcher crashed (instance {}): {}. Retrying in {}s (other channels stay active).",
                            inst_id, e, TELEGRAM_RETRY_DELAY_SECS
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(
                            TELEGRAM_RETRY_DELAY_SECS,
                        ))
                        .await;
                    }
                }
            }
        });
    }
    Ok(())
}

async fn resolve_canonical_chat_id_for_telegram(
    state: Arc<AppState>,
    telegram_chat_id: i64,
    telegram_bot_instance_id: i64,
) -> Result<i64, String> {
    let telegram_handle = telegram_chat_id.to_string();
    let universal_chat_id = state.config.universal_chat_id;
    let bid = telegram_bot_instance_id;
    call_blocking(state.db.clone(), move |db| {
        if let Some(cid) = universal_chat_id {
            db.upsert_chat(cid, None, "telegram")?;
            db.link_channel(cid, bid, "telegram", &telegram_handle)?;
            Ok(cid)
        } else {
            db.resolve_canonical_chat_id(bid, "telegram", &telegram_handle, None)
        }
    })
    .await
    .map_err(|e| format!("resolve_canonical_chat_id: {e}"))
}

async fn build_persona_menu_payload(
    state: Arc<AppState>,
    canonical_chat_id: i64,
    locked_persona_id: Option<i64>,
) -> Result<(String, InlineKeyboardMarkup), String> {
    let _ = call_blocking(state.db.clone(), move |db| {
        db.get_or_create_default_persona(canonical_chat_id)
    })
    .await
    .map_err(|e| format!("ensure default persona: {e}"))?;

    let personas = call_blocking(state.db.clone(), move |db| {
        db.list_personas(canonical_chat_id)
    })
    .await
    .map_err(|e| format!("list personas: {e}"))?;
    let filtered_personas = if let Some(locked_id) = locked_persona_id {
        personas
            .into_iter()
            .filter(|p| p.id == locked_id)
            .collect::<Vec<_>>()
    } else {
        personas
    };
    let active_id = call_blocking(state.db.clone(), move |db| {
        db.get_active_persona_id(canonical_chat_id)
    })
    .await
    .map_err(|e| format!("get active persona: {e}"))?
    .unwrap_or(0);

    if filtered_personas.is_empty() {
        let text = if locked_persona_id.is_some() {
            "Single-persona mode is enabled for Telegram, but the configured persona was not found."
                .to_string()
        } else {
            "No personas found. Use /persona new <name> to create one.".to_string()
        };
        return Ok((
            text,
            InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new()),
        ));
    }

    let names: Vec<String> = filtered_personas
        .iter()
        .map(|p| {
            if p.id == active_id {
                format!("{} (active)", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect();
    let text = if locked_persona_id.is_some() {
        format!(
            "Telegram is in single-persona mode.\nAllowed persona: {}.",
            names.join(", ")
        )
    } else {
        format!(
            "Personas: {}.\nTap a persona below to switch, or use /persona new <name> to create.",
            names.join(", ")
        )
    };

    let rows: Vec<Vec<InlineKeyboardButton>> = if locked_persona_id.is_some() {
        Vec::new()
    } else {
        filtered_personas
            .iter()
            .map(|p| {
                let label = if p.id == active_id {
                    format!("✅ {}", p.name)
                } else {
                    p.name.clone()
                };
                vec![InlineKeyboardButton::callback(
                    label,
                    format!("{PERSONA_SWITCH_CALLBACK_PREFIX}{}", p.id),
                )]
            })
            .collect()
    };

    Ok((text, InlineKeyboardMarkup::new(rows)))
}

async fn send_persona_menu(
    bot: &Bot,
    state: Arc<AppState>,
    chat_id: ChatId,
    canonical_chat_id: i64,
    thread_id: Option<ThreadId>,
    telegram_bot_instance_id: i64,
) {
    let policy = call_blocking(state.db.clone(), move |db| {
        db.get_channel_persona_policy(canonical_chat_id, telegram_bot_instance_id)
    })
    .await
    .ok()
    .flatten();
    let locked_persona_id = policy.and_then(|p| {
        if p.mode == crate::db::ChannelPersonaMode::Single {
            p.persona_id
        } else {
            None
        }
    });
    match build_persona_menu_payload(state, canonical_chat_id, locked_persona_id).await {
        Ok((text, keyboard)) => {
            let mut req = bot.send_message(chat_id, text).reply_markup(keyboard);
            if let Some(tid) = thread_id {
                req = req.message_thread_id(tid);
            }
            if let Err(e) = req.await {
                error!("Failed to send persona menu: {}", e);
                let _ = send_response_plain(
                    bot,
                    chat_id,
                    "Failed to show persona menu.",
                    thread_id,
                    None,
                )
                .await;
            }
        }
        Err(e) => {
            error!("Failed to build persona menu: {}", e);
            let _ =
                send_response_plain(bot, chat_id, &format!("Error: {e}"), thread_id, None).await;
        }
    }
}

pub(crate) async fn handle_callback_query(
    bot: Bot,
    q: CallbackQuery,
    state: Arc<AppState>,
    telegram_bot_instance_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };

    let callback_id = q.id.clone();
    let Some(message) = q.regular_message().cloned() else {
        let _ = bot
            .answer_callback_query(callback_id)
            .text("This menu is no longer available.")
            .await;
        return Ok(());
    };

    let chat_id = message.chat.id;
    let thread_id = message.thread_id;
    if !data.starts_with(PERSONA_SWITCH_CALLBACK_PREFIX) {
        return Ok(());
    }

    let selected_id = data[PERSONA_SWITCH_CALLBACK_PREFIX.len()..].parse::<i64>();
    let persona_id = match selected_id {
        Ok(id) if id > 0 => id,
        _ => {
            let _ = bot
                .answer_callback_query(q.id)
                .text("Invalid persona selection.")
                .await;
            return Ok(());
        }
    };

    let canonical_chat_id = match resolve_canonical_chat_id_for_telegram(
        state.clone(),
        chat_id.0,
        telegram_bot_instance_id,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            error!("Persona callback resolve chat failed: {}", e);
            let _ = bot
                .answer_callback_query(q.id)
                .text("Could not resolve chat for persona switch.")
                .await;
            return Ok(());
        }
    };

    let locked_single = call_blocking(state.db.clone(), move |db| {
        Ok(matches!(
            db.get_channel_persona_policy(canonical_chat_id, telegram_bot_instance_id)?,
            Some(p) if p.mode == crate::db::ChannelPersonaMode::Single
        ))
    })
    .await
    .unwrap_or(false);
    if locked_single {
        let _ = bot
            .answer_callback_query(q.id)
            .text("Persona switching is locked in Web UI for Telegram.")
            .await;
        return Ok(());
    }

    let exists = call_blocking(state.db.clone(), move |db| {
        db.persona_exists(canonical_chat_id, persona_id)
    })
    .await
    .unwrap_or(false);
    if !exists {
        let _ = bot
            .answer_callback_query(q.id)
            .text("Persona not found.")
            .await;
        send_persona_menu(
            &bot,
            state.clone(),
            chat_id,
            canonical_chat_id,
            thread_id,
            telegram_bot_instance_id,
        )
        .await;
        return Ok(());
    }

    let switched = call_blocking(state.db.clone(), move |db| {
        db.set_active_persona(canonical_chat_id, persona_id)
    })
    .await
    .unwrap_or(false);
    if !switched {
        let _ = bot
            .answer_callback_query(q.id)
            .text("Failed to switch persona.")
            .await;
        return Ok(());
    }

    let _ = bot
        .answer_callback_query(q.id)
        .text("Persona switched.")
        .await;

    match build_persona_menu_payload(state, canonical_chat_id, None).await {
        Ok((text, keyboard)) => {
            if let Err(e) = bot
                .edit_message_text(chat_id, message.id, text)
                .reply_markup(keyboard)
                .await
            {
                warn!("Failed to update persona menu message: {}", e);
                let _ =
                    send_response_plain(&bot, chat_id, "Persona switched.", thread_id, None).await;
            }
        }
        Err(e) => {
            error!("Failed to refresh persona menu after switch: {}", e);
            let _ = send_response_plain(&bot, chat_id, "Persona switched.", thread_id, None).await;
        }
    }

    Ok(())
}

pub(crate) async fn handle_message(
    bot: Bot,
    msg: teloxide::types::Message,
    state: Arc<AppState>,
    telegram_bot_instance_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id.0;

    // Resolve to unified contact (canonical_chat_id).
    let canonical_chat_id =
        resolve_canonical_chat_id_for_telegram(state.clone(), chat_id, telegram_bot_instance_id)
            .await?;

    // Extract content: text, photo, or voice
    let mut text = msg.text().unwrap_or("").to_string();
    // Use caption when there's no body text so slash commands in photo/document captions are handled
    if text.trim().is_empty() {
        if let Some(cap) = msg.caption() {
            text = cap.to_string();
        }
    }
    let mut images = InboundImages::new();
    let mut document_saved_path: Option<String> = None;

    // Single entry point: parse slash command first. If command, run backend handler and return — never send to LLM.
    let cmd = parse_slash_command(&text);
    if text.len() <= 80 {
        let codepoints: Vec<String> = text
            .chars()
            .take(12)
            .map(|c| format!("U+{:04X}", c as u32))
            .collect();
        info!(
            "slash_parse len={} codepoints={:?} result={:?}",
            text.len(),
            codepoints,
            cmd
        );
    }
    if let Some(cmd) = cmd {
        match cmd {
            SlashCommand::Reset => {
                let pid = call_blocking(state.db.clone(), move |db| {
                    db.get_current_persona_id(canonical_chat_id)
                })
                .await
                .unwrap_or(0);
                if pid > 0 {
                    let _ = call_blocking(state.db.clone(), move |db| {
                        db.delete_session(canonical_chat_id, pid)
                    })
                    .await;
                }
                let mut req = bot.send_message(
                    msg.chat.id,
                    "Conversation cleared. Principles and per-persona memory are unchanged.",
                );
                if let Some(tid) = msg.thread_id {
                    req = req.message_thread_id(tid);
                }
                let _ = req.await;
            }
            SlashCommand::Skills => {
                let formatted = state.skills.list_skills_formatted();
                send_response(&bot, msg.chat.id, &formatted, msg.thread_id, None).await;
            }
            SlashCommand::Persona => {
                let parts: Vec<&str> = text.split_whitespace().collect();
                let sub = parts.get(1).map(|s| *s).unwrap_or("");
                if sub.is_empty() || sub.eq_ignore_ascii_case("list") {
                    send_persona_menu(
                        &bot,
                        state.clone(),
                        msg.chat.id,
                        canonical_chat_id,
                        msg.thread_id,
                        telegram_bot_instance_id,
                    )
                    .await;
                } else {
                    let resp = crate::persona::handle_persona_command(
                        state.db.clone(),
                        canonical_chat_id,
                        text.trim(),
                        Some(&state.config),
                    )
                    .await;
                    send_response(&bot, msg.chat.id, &resp, msg.thread_id, None).await;
                }
            }
            SlashCommand::Schedule => {
                let pid = call_blocking(state.db.clone(), move |db| {
                    db.get_current_persona_id(canonical_chat_id)
                })
                .await
                .unwrap_or(0);
                let tasks = if pid > 0 {
                    call_blocking(state.db.clone(), move |db| {
                        db.get_tasks_for_chat_and_persona(canonical_chat_id, pid)
                    })
                    .await
                } else {
                    call_blocking(state.db.clone(), move |db| {
                        db.get_tasks_for_chat(canonical_chat_id)
                    })
                    .await
                };
                let text = match &tasks {
                    Ok(t) => crate::tools::schedule::format_tasks_list_persona(t),
                    Err(e) => format!("Error listing tasks: {e}"),
                };
                info!(
                    "schedule_cmd: {} tasks, sending response (len={})",
                    tasks.as_ref().map(|v| v.len()).unwrap_or(0),
                    text.len()
                );
                if let Err(e) =
                    send_response_plain(&bot, msg.chat.id, &text, msg.thread_id, None).await
                {
                    error!("schedule_cmd: failed to send response: {e}");
                }
            }
            SlashCommand::Secrets => {
                let reply = crate::secret_vault::secrets_command_reply(
                    &state.config.runtime_data_dir(),
                    &state.config.skills_data_dir_absolute(),
                    canonical_chat_id,
                    &text,
                );
                send_response(&bot, msg.chat.id, &reply, msg.thread_id, None).await;
            }
            SlashCommand::Archive => {
                let pid = call_blocking(state.db.clone(), move |db| {
                    db.get_current_persona_id(canonical_chat_id)
                })
                .await
                .unwrap_or(0);
                let send_archive_msg = |text: &str| {
                    let mut req = bot.send_message(msg.chat.id, text);
                    if let Some(tid) = msg.thread_id {
                        req = req.message_thread_id(tid);
                    }
                    req
                };
                if pid == 0 {
                    let _ = send_archive_msg("No conversation to archive.").await;
                } else {
                    let pid_f = pid;
                    let history = call_blocking(state.db.clone(), move |db| {
                        db.get_recent_messages(canonical_chat_id, pid_f, 500, false)
                    })
                    .await
                    .unwrap_or_default();
                    let messages =
                        history_to_claude_messages(&history, &state.config.bot_username, false);
                    if messages.is_empty() {
                        let _ = send_archive_msg("No conversation to archive.").await;
                    } else {
                        archive_conversation(
                            &state.config.runtime_data_dir(),
                            canonical_chat_id,
                            &messages,
                        );
                        let _ = send_archive_msg(&format!("Archived {} messages.", messages.len()))
                            .await;
                    }
                }
            }
        }
        return Ok(());
    }

    if let Some(photos) = msg.photo() {
        // Pick the largest photo (last in the array)
        if let Some(photo) = photos.last() {
            match download_telegram_file(&bot, &photo.file.id.0).await {
                Ok(bytes) => {
                    let media_type = guess_image_media_type(&bytes);
                    images.push_bytes(&bytes, &media_type);
                }
                Err(e) => {
                    error!("Failed to download photo: {e}");
                }
            }
        }
        // Use caption as text if present
        if text.is_empty() {
            text = msg.caption().unwrap_or("").to_string();
        }
    }

    // Handle document messages (text/code/file attachments)
    if let Some(document) = msg.document() {
        let max_bytes = state
            .config
            .max_document_size_mb
            .saturating_mul(1024)
            .saturating_mul(1024);
        let doc_bytes = u64::from(document.file.size);
        if doc_bytes > max_bytes {
            let _ = bot
                .send_message(
                    msg.chat.id,
                    format!(
                        "Document is too large ({} bytes). Max allowed is {} MB.",
                        doc_bytes, state.config.max_document_size_mb
                    ),
                )
                .await;
            return Ok(());
        }

        match download_telegram_file(&bot, &document.file.id.0).await {
            Ok(bytes) => {
                let original_name = document
                    .file_name
                    .as_deref()
                    .unwrap_or("telegram-document.bin");
                let safe_name = original_name
                    .chars()
                    .map(|c| match c {
                        'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
                        _ => '_',
                    })
                    .collect::<String>();

                let dir = Path::new(state.config.working_dir())
                    .join("uploads")
                    .join("telegram")
                    .join(chat_id.to_string());
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    error!("Failed to create upload dir {}: {e}", dir.display());
                } else {
                    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                    let path = dir.join(format!("{}-{}", ts, safe_name));
                    match tokio::fs::write(&path, &bytes).await {
                        Ok(()) => {
                            document_saved_path = Some(path.display().to_string());
                        }
                        Err(e) => {
                            error!("Failed to save telegram document {}: {e}", path.display());
                        }
                    }
                }

                let file_note = format!(
                    "[document] filename={} bytes={} mime={}{}",
                    original_name,
                    bytes.len(),
                    document
                        .mime_type
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    document_saved_path
                        .as_ref()
                        .map(|p| format!(" saved_path={}", p))
                        .unwrap_or_default(),
                );

                if text.trim().is_empty() {
                    text = file_note;
                } else {
                    text = format!("{}\n\n{}", text.trim(), file_note);
                }
            }
            Err(e) => {
                error!("Failed to download document: {e}");
                if text.trim().is_empty() {
                    text = format!("[document] download failed: {e}");
                }
            }
        }

        if text.trim().is_empty() {
            text = msg.caption().unwrap_or("").to_string();
        }
    }

    // Handle voice messages
    if let Some(voice) = msg.voice() {
        if let Some(ref openai_key) = state.config.openai_api_key {
            match download_telegram_file(&bot, &voice.file.id.0).await {
                Ok(bytes) => {
                    let sender_name = msg
                        .from
                        .as_ref()
                        .map(|u| u.username.clone().unwrap_or_else(|| u.first_name.clone()))
                        .unwrap_or_else(|| "Unknown".into());
                    match crate::transcribe::transcribe_audio(openai_key, &bytes).await {
                        Ok(transcription) => {
                            text = format!(
                                "[voice message from {}]: {}",
                                sanitize_xml(&sender_name),
                                sanitize_xml(&transcription)
                            );
                        }
                        Err(e) => {
                            error!("Whisper transcription failed: {e}");
                            text = format!(
                                "[voice message from {}]: [transcription failed: {e}]",
                                sanitize_xml(&sender_name)
                            );
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to download voice message: {e}");
                }
            }
        } else {
            let _ = bot
                .send_message(
                    msg.chat.id,
                    "Voice messages not supported (no Whisper API key configured)",
                )
                .await;
            return Ok(());
        }
    }

    // Handle location/venue (shared location pin)
    if text.trim().is_empty() {
        if let Some(venue) = msg.venue() {
            text = format!(
                "[location] Title: {}, Address: {}, lat: {}, lon: {}",
                venue.title, venue.address, venue.location.latitude, venue.location.longitude
            );
        } else if let Some(loc) = msg.location() {
            text = format!("[location] lat: {}, lon: {}", loc.latitude, loc.longitude);
        }
    }

    // If no text/image/document content, nothing to process
    if text.trim().is_empty() && images.is_empty() && document_saved_path.is_none() {
        return Ok(());
    }
    let sender_name = msg
        .from
        .as_ref()
        .map(|u| u.username.clone().unwrap_or_else(|| u.first_name.clone()))
        .unwrap_or_else(|| "Unknown".into());

    let (runtime_chat_type, db_chat_type) = match msg.chat.kind {
        teloxide::types::ChatKind::Private(_) => ("private", "telegram_private"),
        teloxide::types::ChatKind::Public(teloxide::types::ChatPublic {
            kind: teloxide::types::PublicChatKind::Group,
            ..
        }) => ("group", "telegram_group"),
        teloxide::types::ChatKind::Public(teloxide::types::ChatPublic {
            kind: teloxide::types::PublicChatKind::Supergroup(_),
            ..
        }) => ("group", "telegram_supergroup"),
        teloxide::types::ChatKind::Public(teloxide::types::ChatPublic {
            kind: teloxide::types::PublicChatKind::Channel(_),
            ..
        }) => ("group", "telegram_channel"),
    };

    let chat_title = msg.chat.title().map(|t| t.to_string());
    let instance_config = {
        let bid = telegram_bot_instance_id;
        call_blocking(state.db.clone(), move |db| db.get_channel_bot_instance(bid))
            .await
            .ok()
            .flatten()
    };
    let allowed_groups = instance_config
        .as_ref()
        .map(|inst| inst.allowed_groups.clone())
        .unwrap_or_else(|| state.config.allowed_groups.clone());
    let bot_username = instance_config
        .as_ref()
        .and_then(|inst| {
            let username = inst.bot_username.trim();
            (!username.is_empty()).then(|| username.to_string())
        })
        .unwrap_or_else(|| state.config.bot_username.clone());

    // Resolve run persona: optional `[PersonaName]` prefix; does not change DB active.
    let text_for_resolve = text.clone();
    let bid = telegram_bot_instance_id;
    let (persona_id, text) = match call_blocking(state.db.clone(), move |db| {
        crate::persona::resolve_incoming_run_persona_for_channel(
            &db,
            canonical_chat_id,
            "telegram",
            bid,
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
            let pid = call_blocking(state.db.clone(), move |db| {
                db.get_current_persona_id(canonical_chat_id)
            })
            .await
            .unwrap_or(0);
            (pid, text)
        }
    };
    if persona_id == 0 {
        return Ok(());
    }

    // Check group allowlist (by Telegram chat id)
    if (db_chat_type == "telegram_group" || db_chat_type == "telegram_supergroup")
        && !allowed_groups.is_empty()
        && !allowed_groups.contains(&chat_id)
    {
        tracing::info!(
            target: "telegram",
            chat_id,
            "dropped inbound: chat not in Integrations allowed groups"
        );
        // Store message but don't process
        let chat_title_owned = chat_title.clone();
        let chat_type_owned = db_chat_type.to_string();
        let _ = call_blocking(state.db.clone(), move |db| {
            db.upsert_chat(
                canonical_chat_id,
                chat_title_owned.as_deref(),
                &chat_type_owned,
            )
        })
        .await;
        let stored_content = if !images.is_empty() {
            format!(
                "[image]{}",
                if text.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {text}")
                }
            )
        } else if let Some(path) = &document_saved_path {
            if text.trim().is_empty() {
                format!("[document] saved_path={path}")
            } else {
                format!("[document] saved_path={path} {text}")
            }
        } else {
            text
        };
        let stored = StoredMessage {
            id: msg.id.0.to_string(),
            chat_id: canonical_chat_id,
            persona_id,
            session_id: None,
            sender_name,
            content: stored_content,
            is_from_bot: false,
            timestamp: chrono::Utc::now().to_rfc3339(),
            origin: crate::db::message_origin_interactive(),
            speaker_persona_id: None,
        };
        let _ = call_blocking(state.db.clone(), move |db| db.store_message(&stored)).await;
        return Ok(());
    }

    // Store the chat and message
    let chat_title_owned = chat_title.clone();
    let chat_type_owned = db_chat_type.to_string();
    let _ = call_blocking(state.db.clone(), move |db| {
        db.upsert_chat(
            canonical_chat_id,
            chat_title_owned.as_deref(),
            &chat_type_owned,
        )
    })
    .await;

    let stored_content = if !images.is_empty() {
        format!(
            "[image]{}",
            if text.trim().is_empty() {
                String::new()
            } else {
                format!(" {text}")
            }
        )
    } else if let Some(path) = &document_saved_path {
        if text.trim().is_empty() {
            format!("[document] saved_path={path}")
        } else {
            format!("[document] saved_path={path} {text}")
        }
    } else {
        text.clone()
    };
    let stored = StoredMessage {
        id: msg.id.0.to_string(),
        chat_id: canonical_chat_id,
        persona_id,
        session_id: None,
        sender_name: sender_name.clone(),
        content: stored_content,
        is_from_bot: false,
        timestamp: chrono::Utc::now().to_rfc3339(),
        origin: crate::db::message_origin_interactive(),
        speaker_persona_id: None,
    };
    let _ = call_blocking(state.db.clone(), move |db| db.store_message(&stored)).await;

    // Groups: only respond when @bot appears in the (prefix-stripped) body
    let should_respond = match runtime_chat_type {
        "private" => true,
        _ => {
            let bot_mention = format!("@{}", bot_username);
            text.contains(&bot_mention)
        }
    };

    if !should_respond {
        return Ok(());
    }

    info!(
        "Processing message from {} in chat {}: {}",
        sender_name,
        chat_id,
        text.chars().take(100).collect::<String>()
    );

    // Queue agent work per (canonical chat, persona) so different personas can run in parallel.
    let state_spawn = state.clone();
    let bot_spawn = bot.clone();
    let telegram_bot_instance_id_spawn = telegram_bot_instance_id;
    let chat_id_spawn = msg.chat.id;
    let thread_id_spawn = msg.thread_id;
    let runtime_chat_type_owned = runtime_chat_type.to_string();
    let canonical_chat_id_spawn = canonical_chat_id;
    let queue_label = text.chars().take(120).collect::<String>();
    let queue_run_id = uuid::Uuid::new_v4().to_string();
    let on_hard_abort = Some(crate::queue_abort::make_deliver_hard_abort_hook(
        state.db.clone(),
        state.telegram_bots.clone(),
        state.discord_http.clone(),
        state.wecom.clone(),
        state.config.bot_username.clone(),
        canonical_chat_id_spawn,
        persona_id,
        queue_run_id.clone(),
        crate::channel::DeliveryScope::PlatformInstance {
            channel_type: "telegram",
            bot_instance_id: telegram_bot_instance_id,
            channel_handle: None,
        },
        Some(state.config.workspace_root_absolute()),
    ));
    let queue_meta = QueueEnqueueMeta {
        run_id: queue_run_id,
        persona_id,
        source: QueueSource::Telegram,
        label: queue_label,
        project_id: None,
        workflow_id: None,
        on_hard_abort,
    };
    let (queue_position, _) = state
        .chat_queue
        .enqueue_with_meta(canonical_chat_id_spawn, queue_meta, |cancel| async move {
        // Typing indicator for the duration of the run
        let typing_bot = bot_spawn.clone();
        let typing_chat_id = chat_id_spawn;
        let typing_handle = tokio::spawn(async move {
            loop {
                let _ = typing_bot
                    .send_chat_action(typing_chat_id, ChatAction::Typing)
                    .await;
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }
        });

        // Event channel: receives tool-start/result events from the agentic loop
        // so we can show live progress to the user.
        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        // Shared slot so the event handler can tell the outer task which
        // status message to delete after the agent finishes.
        let status_msg_id: std::sync::Arc<tokio::sync::Mutex<Option<teloxide::types::MessageId>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let status_msg_id_ev = status_msg_id.clone();

        // Immediate processing ack (edited into tool status, deleted on finish).
        {
            let mut req = bot_spawn.send_message(chat_id_spawn, CHANNEL_PROCESSING_ACK);
            if let Some(tid) = thread_id_spawn {
                req = req.message_thread_id(tid);
            }
            if let Ok(Ok(sent)) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                req,
            )
            .await
            {
                *status_msg_id.lock().await = Some(sent.id);
            }
        }

        let event_bot = bot_spawn.clone();
        let event_chat_id = chat_id_spawn;
        let event_thread_id = thread_id_spawn;
        const STATUS_API_TIMEOUT_SECS: u64 = 5;
        let mut event_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let AgentEvent::ToolStart { name, input, .. } = event {
                    let text = format_tool_status(&name, &input);
                    let current_id = *status_msg_id_ev.lock().await;
                    // Wrap each Telegram API call with a timeout so a slow/hung
                    // API response never blocks the event handler indefinitely.
                    if let Some(mid) = current_id {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(STATUS_API_TIMEOUT_SECS),
                            event_bot.edit_message_text(event_chat_id, mid, &text),
                        )
                        .await;
                    } else {
                        let mut req = event_bot.send_message(event_chat_id, &text);
                        if let Some(tid) = event_thread_id {
                            req = req.message_thread_id(tid);
                        }
                        if let Ok(Ok(sent)) = tokio::time::timeout(
                            std::time::Duration::from_secs(STATUS_API_TIMEOUT_SECS),
                            req,
                        )
                        .await
                        {
                            *status_msg_id_ev.lock().await = Some(sent.id);
                        }
                    }
                }
            }
        });

        let result = process_with_agent_with_events(
            &state_spawn,
            AgentRequestContext {
                caller_channel: "telegram",
                chat_id: canonical_chat_id_spawn,
                chat_type: &runtime_chat_type_owned,
                persona_id,
                is_scheduled_task: false,
                is_background_job: false,
                run_key: None,
                reply_bot_instance_id: Some(telegram_bot_instance_id_spawn),
                session_id: None,
                history_override: None,
                tool_iteration_cap: None,
                skip_recall: false,
            },
            None,
            images,
            Some(&event_tx),
            Some(cancel),
        )
        .await;

        // Close the event channel and wait for the handler to drain.
        // Cap the wait so a slow Telegram API call never blocks the final reply.
        drop(event_tx);
        let drained = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            &mut event_handle,
        )
        .await;
        if drained.is_err() {
            info!("Event handler drain timed out after 10s; aborting");
        }
        event_handle.abort();

        // Delete the tool-progress status message if one was sent
        if let Some(mid) = *status_msg_id.lock().await {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                bot_spawn.delete_message(chat_id_spawn, mid),
            )
            .await;
        }

        typing_handle.abort();

        match result {
            Ok(agent_out) => {
                let to_send = if agent_out.response.trim().is_empty() {
                    "Done.".to_string()
                } else {
                    agent_out.response.clone()
                };
                info!(
                    "Delivering response to contact {}: {} chars",
                    canonical_chat_id_spawn,
                    to_send.len()
                );
                let ws_root = state_spawn.config.workspace_root_absolute();
                let delivery_scope = DeliveryScope::platform_reply(
                    "telegram",
                    telegram_bot_instance_id_spawn,
                    chat_id_spawn.0.to_string(),
                );
                if let Err(e) = crate::channel::deliver_agent_final_to_contact(
                    state_spawn.db.clone(),
                    state_spawn.telegram_bots.as_ref(),
                    state_spawn.discord_http.as_ref(),
                    state_spawn.wecom.as_deref(),
                    &state_spawn.config.bot_username,
                    canonical_chat_id_spawn,
                    persona_id,
                    &to_send,
                    Some(ws_root.clone()),
                    delivery_scope,
                    None,
                )
                .await
                {
                    tracing::warn!(target: "channel", error = %e, "deliver_agent_final_to_contact failed; sending to Telegram only");
                    let auto_ctx = WorkspaceAutoImageContext {
                        root: ws_root.as_path(),
                        chat_id: canonical_chat_id_spawn,
                        persona_id,
                    };
                    send_response(
                        &bot_spawn,
                        chat_id_spawn,
                        &to_send,
                        thread_id_spawn,
                        Some(auto_ctx),
                    )
                    .await;
                }
            }
            Err(e) => {
                error!("Error processing message: {}", e);
                let mut req = bot_spawn.send_message(chat_id_spawn, format!("Error: {e}"));
                if let Some(tid) = thread_id_spawn {
                    req = req.message_thread_id(tid);
                }
                let _ = req.await;
            }
        }
    })
        .await;
    info!(
        target: "queue",
        chat_id = canonical_chat_id,
        queue_position = queue_position,
        "Enqueued Telegram agent run"
    );

    Ok(())
}

async fn download_telegram_file(
    bot: &Bot,
    file_id: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let file = bot
        .get_file(teloxide::types::FileId(file_id.to_string()))
        .await?;
    let mut buf = Vec::new();
    teloxide::net::Download::download_file(bot, &file.path, &mut buf).await?;
    Ok(buf)
}

fn guess_image_media_type(data: &[u8]) -> String {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png".into()
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg".into()
    } else if data.starts_with(b"GIF") {
        "image/gif".into()
    } else if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        "image/webp".into()
    } else {
        "image/jpeg".into() // default
    }
}

/// Convert common LLM markdown (code blocks, inline code, bold, italic) to Telegram HTML
/// so messages render cleanly. Escapes &, <, > for Telegram parse_mode=HTML.
pub fn markdown_to_telegram_html(text: &str) -> String {
    // 1) Escape HTML so we can safely add tags
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    // 2) Identify non-formatting zones (fenced code, inline code)
    // We'll replace them with placeholders, apply other formatting, then put them back.
    let mut s = escaped;
    let mut placeholders = Vec::new();

    // Fenced code blocks: ```optional_lang\n...\n```
    let mut result = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find("```") {
        result.push_str(&rest[..open]);
        let after_open = open + 3;
        rest = &rest[after_open..];

        // Skip optional language line
        let mut content_start = rest;
        if rest.starts_with('\n') {
            content_start = &rest[1..];
        } else if let Some(nl) = rest.find('\n') {
            let first_line = &rest[..nl];
            if first_line.len() < 25 && !first_line.contains(' ') {
                content_start = &rest[nl + 1..];
            }
        }

        if let Some(close) = content_start.find("```") {
            let content = &content_start[..close];
            let placeholder = format!("FENCEDCODE{}PLACEHOLDER", placeholders.len());
            result.push_str(&placeholder);
            placeholders.push(format!("<pre>{}</pre>", content));
            rest = &content_start[close + 3..];
        } else {
            result.push_str("```");
            break;
        }
    }
    result.push_str(rest);
    s = result;

    // Inline code: `...`
    let mut result = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find('`') {
        result.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('`') {
            let content = &rest[..close];
            let placeholder = format!("INLINECODE{}PLACEHOLDER", placeholders.len());
            result.push_str(&placeholder);
            placeholders.push(format!("<code>{}</code>", content));
            rest = &rest[close + 1..];
        } else {
            result.push('`');
            break;
        }
    }
    result.push_str(rest);
    s = result;

    // 3) Apply formatting to the "safe" text (bold, italic)
    // We use a simple stack-based parser to ensure tags are nested correctly.
    // e.g. **bold *italic*** -> <b>bold <i>italic</i></b>

    let mut result = String::with_capacity(s.len());
    let mut stack: Vec<&'static str> = Vec::new();
    let mut byte_idx = 0;

    while byte_idx < s.len() {
        let remaining = &s[byte_idx..];

        // Bold: ** or __
        if remaining.starts_with("**") || remaining.starts_with("__") {
            let tag = "b";
            if stack.contains(&tag) {
                // Close tags until we reach our tag
                while let Some(top) = stack.pop() {
                    result.push_str(&format!("</{}>", top));
                    if top == tag {
                        break;
                    }
                }
            } else {
                result.push_str("<b>");
                stack.push(tag);
            }
            byte_idx += 2;
        }
        // Italic: * or _
        else if remaining.starts_with('*') || remaining.starts_with('_') {
            let tag = "i";
            if stack.contains(&tag) {
                // Close tags until we reach our tag
                while let Some(top) = stack.pop() {
                    result.push_str(&format!("</{}>", top));
                    if top == tag {
                        break;
                    }
                }
            } else {
                result.push_str("<i>");
                stack.push(tag);
            }
            byte_idx += 1;
        } else {
            let c = remaining.chars().next().unwrap();
            result.push(c);
            byte_idx += c.len_utf8();
        }
    }

    // Close any remaining tags in reverse order
    while let Some(tag) = stack.pop() {
        result.push_str(&format!("</{}>", tag));
    }
    s = result;

    // 4) Restore code blocks
    for (i, replacement) in placeholders.iter().enumerate() {
        let placeholder = format!("FENCEDCODE{}PLACEHOLDER", i);
        s = s.replace(&placeholder, replacement);
        let placeholder = format!("INLINECODE{}PLACEHOLDER", i);
        s = s.replace(&placeholder, replacement);
    }

    s
}

#[cfg(test)]
fn split_response_text(text: &str) -> Vec<String> {
    const MAX_LEN: usize = 4096;
    if text.len() <= MAX_LEN {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let chunk_len = if remaining.len() <= MAX_LEN {
            remaining.len()
        } else {
            remaining[..MAX_LEN].rfind('\n').unwrap_or(MAX_LEN)
        };
        chunks.push(remaining[..chunk_len].to_string());
        remaining = &remaining[chunk_len..];
        if remaining.starts_with('\n') {
            remaining = &remaining[1..];
        }
    }
    chunks
}

fn markdown_image_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\(([^)]+)\)").unwrap())
}

fn backtick_abs_image_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`(/[^`]+?\.(?:png|jpg|jpeg|gif|webp|bmp))`").unwrap())
}

fn resolve_workspace_image_ref(
    raw: &str,
    workspace_root: &Path,
    chat_id: Option<i64>,
    persona_id: Option<i64>,
) -> Option<PathBuf> {
    crate::final_delivery_media::resolve_workspace_artifact_path(
        workspace_root,
        chat_id,
        persona_id,
        raw,
        crate::final_delivery_media::ArtifactResolveKind::ImageOnly,
    )
}

/// Context for workspace-root auto-image delivery (markdown images and backtick paths).
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceAutoImageContext<'a> {
    pub root: &'a Path,
    pub chat_id: i64,
    pub persona_id: i64,
}

/// Find workspace image files referenced in assistant text (markdown images and absolute paths
/// in backticks), return them in document order (deduped) plus text with those markers removed.
pub(crate) fn prepare_telegram_workspace_auto_images(
    text: &str,
    ctx: &WorkspaceAutoImageContext<'_>,
) -> (Vec<PathBuf>, String) {
    let workspace_root = ctx.root;
    let chat_id = Some(ctx.chat_id);
    let persona_id = Some(ctx.persona_id);
    let md_re = markdown_image_regex();
    let bt_re = backtick_abs_image_regex();

    let mut ordered: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut consider = |raw: &str| {
        if let Some(p) = resolve_workspace_image_ref(raw, workspace_root, chat_id, persona_id) {
            if seen.insert(p.clone()) {
                ordered.push(p);
            }
        }
    };

    for caps in md_re.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            consider(m.as_str());
        }
    }
    for caps in bt_re.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            consider(m.as_str());
        }
    }

    if ordered.is_empty() {
        return (ordered, text.to_string());
    }

    let sent: HashSet<PathBuf> = ordered.iter().cloned().collect();
    let mut body = text.to_string();

    for caps in md_re.captures_iter(text) {
        let Some(full) = caps.get(0) else {
            continue;
        };
        let Some(inner) = caps.get(1) else {
            continue;
        };
        if let Some(p) =
            resolve_workspace_image_ref(inner.as_str(), workspace_root, chat_id, persona_id)
        {
            if sent.contains(&p) {
                body = body.replace(full.as_str(), "");
            }
        }
    }
    for caps in bt_re.captures_iter(text) {
        let Some(full) = caps.get(0) else {
            continue;
        };
        let Some(inner) = caps.get(1) else {
            continue;
        };
        if let Some(p) =
            resolve_workspace_image_ref(inner.as_str(), workspace_root, chat_id, persona_id)
        {
            if sent.contains(&p) {
                body = body.replace(full.as_str(), "");
            }
        }
    }

    while body.contains("\n\n\n") {
        body = body.replace("\n\n\n", "\n\n");
    }
    let body = body.trim().to_string();
    (ordered, body)
}

async fn send_workspace_images_telegram(
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
    paths: &[PathBuf],
) {
    for p in paths {
        let mut req = bot.send_photo(chat_id, InputFile::file(p.clone()));
        if let Some(tid) = thread_id {
            req = req.message_thread_id(tid);
        }
        if let Err(e) = req.await {
            warn!(
                target: "channel",
                path = %p.display(),
                error = %e,
                "Telegram auto workspace image send failed"
            );
        }
    }
}

async fn send_plain_chunks(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    thread_id: Option<ThreadId>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    const MAX_LEN: usize = 4096;
    let mut remaining = text;
    while !remaining.is_empty() {
        let chunk_len = if remaining.len() <= MAX_LEN {
            remaining.len()
        } else {
            remaining[..MAX_LEN].rfind('\n').unwrap_or(MAX_LEN)
        };
        let chunk = &remaining[..chunk_len];
        let mut req = bot.send_message(chat_id, chunk);
        if let Some(tid) = thread_id {
            req = req.message_thread_id(tid);
        }
        req.await?;
        remaining = &remaining[chunk_len..];
        if remaining.starts_with('\n') {
            remaining = &remaining[1..];
        }
    }
    Ok(())
}

/// Send text to a chat, optionally in a forum topic. Returns Result for error handling.
/// When plain_text is true, skips markdown-to-HTML conversion (use for cron, prompts, etc.).
///
/// When `workspace_auto_images` is set, local image paths referenced in the text (markdown images
/// or `` `/abs/path/to/file.png` `` under the workspace root) are sent as Telegram photos before
/// the text message.
pub async fn send_response_result(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    thread_id: Option<ThreadId>,
    workspace_auto_images: Option<WorkspaceAutoImageContext<'_>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_response_result_impl(bot, chat_id, text, thread_id, false, workspace_auto_images).await
}

/// Send plain text (no HTML parsing). Use for content with cron expressions, asterisks, etc.
pub async fn send_response_plain(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    thread_id: Option<ThreadId>,
    workspace_auto_images: Option<WorkspaceAutoImageContext<'_>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_response_result_impl(bot, chat_id, text, thread_id, true, workspace_auto_images).await
}

async fn send_response_result_impl(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    thread_id: Option<ThreadId>,
    plain_text: bool,
    workspace_auto_images: Option<WorkspaceAutoImageContext<'_>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    const MAX_LEN: usize = 4096;

    let (paths, body_text) = match workspace_auto_images {
        Some(ctx) => prepare_telegram_workspace_auto_images(text, &ctx),
        None => (Vec::new(), text.to_string()),
    };

    if !paths.is_empty() {
        send_workspace_images_telegram(bot, chat_id, thread_id, &paths).await;
    }

    let trimmed = body_text.trim();
    let body_text = if trimmed.is_empty() {
        if !paths.is_empty() {
            return Ok(());
        }
        "Done.".to_string()
    } else {
        body_text
    };

    let formatted_len = if plain_text {
        body_text.len()
    } else {
        markdown_to_telegram_html(&body_text).len()
    };

    if formatted_len > MAX_LEN {
        return send_plain_chunks(bot, chat_id, &body_text, thread_id).await;
    }

    if plain_text {
        let mut req = bot.send_message(chat_id, &body_text);
        if let Some(tid) = thread_id {
            req = req.message_thread_id(tid);
        }
        if let Err(e) = req.await {
            return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
        }
        return Ok(());
    }

    let formatted = markdown_to_telegram_html(&body_text);
    let mut req = bot
        .send_message(chat_id, &formatted)
        .parse_mode(ParseMode::Html);
    if let Some(tid) = thread_id {
        req = req.message_thread_id(tid);
    }
    match req.await {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("can't parse entities") {
                warn!(
                    target: "channel",
                    error = %err_str,
                    "Telegram HTML parse failed, sending as plain text"
                );
                let mut req = bot.send_message(chat_id, &body_text);
                if let Some(tid) = thread_id {
                    req = req.message_thread_id(tid);
                }
                if let Err(e) = req.await {
                    return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                }
                Ok(())
            } else {
                Err(e.into())
            }
        }
    }
}

/// Send text to a chat, optionally in a forum topic. In forum groups, pass thread_id
/// so the reply appears in the same topic as the user's message.
/// Falls back to plain text if the HTML-formatted version is rejected by Telegram.
pub async fn send_response(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    thread_id: Option<ThreadId>,
    workspace_auto_images: Option<WorkspaceAutoImageContext<'_>>,
) {
    if let Err(e) = send_response_result(bot, chat_id, text, thread_id, workspace_auto_images).await
    {
        error!(
            "Telegram send failed after HTML/plain handling inside send_response_result: {}",
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_telegram_html() {
        // Plain text unchanged except HTML escape
        assert_eq!(markdown_to_telegram_html("hello"), "hello");
        assert_eq!(
            markdown_to_telegram_html("a < b & c > d"),
            "a &lt; b &amp; c &gt; d"
        );
        // Inline code
        assert_eq!(
            markdown_to_telegram_html("use `foo` here"),
            "use <code>foo</code> here"
        );
        assert_eq!(
            markdown_to_telegram_html("**bold** and *italic*"),
            "<b>bold</b> and <i>italic</i>"
        );
        // Robustness: ensure Markdown inside <code> or <pre> is NOT converted
        assert_eq!(
            markdown_to_telegram_html("No `*italic*` here"),
            "No <code>*italic*</code> here"
        );
        assert_eq!(
            markdown_to_telegram_html("No ```\n**bold**\n``` here"),
            "No <pre>**bold**\n</pre> here"
        );
        // Emoji regression (multi-byte characters before formatting)
        assert_eq!(markdown_to_telegram_html("🔥 **bold**"), "🔥 <b>bold</b>");
        // Fenced code block
        let input = "text\n```rust\nfn main() {}\n```\nmore";
        let out = markdown_to_telegram_html(input);
        assert!(out.contains("<pre>"));
        assert!(out.contains("fn main() {}"));
        assert!(out.contains("</pre>"));
        assert!(!out.contains("```"));
        // Inline code inside formatting
        assert_eq!(
            markdown_to_telegram_html("**bold `code`**"),
            "<b>bold <code>code</code></b>"
        );

        // Nested bold and italic (trailing `*` may leave an empty italic span)
        assert_eq!(
            markdown_to_telegram_html("**bold *italic***"),
            "<b>bold <i>italic</i></b><i></i>"
        );
        assert_eq!(
            markdown_to_telegram_html("***bold italic***"),
            "<b><i>bold italic</i></b><i></i>"
        );
        assert_eq!(
            markdown_to_telegram_html("*italic **bold***"),
            "<i>italic <b>bold</b></i>"
        );

        // Overlapping delimiters: parser may emit an empty italic pair after bold closes
        assert_eq!(
            markdown_to_telegram_html("**bold _italic**_"),
            "<b>bold <i>italic</i></b><i></i>"
        );
    }

    #[test]
    fn test_split_response_text_short() {
        let chunks = split_response_text("hello world");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    #[test]
    fn test_split_response_text_long() {
        // Create a string longer than 4096 chars with newlines
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("Line {i}: some content here that takes space\n"));
        }
        assert!(text.len() > 4096);

        let chunks = split_response_text(&text);
        assert!(chunks.len() > 1);
        // All chunks should be <= 4096
        for chunk in &chunks {
            assert!(chunk.len() <= 4096);
        }
        // Recombined should approximate original (newlines at split points are consumed)
        let total_len: usize = chunks.iter().map(|c| c.len()).sum();
        assert!(total_len > 0);
    }

    #[test]
    fn test_split_response_text_no_newlines() {
        // Long string without newlines - should split at MAX_LEN
        let text = "a".repeat(5000);
        let chunks = split_response_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4096);
        assert_eq!(chunks[1].len(), 904);
    }

    #[test]
    fn test_guess_image_media_type_jpeg() {
        let data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(guess_image_media_type(&data), "image/jpeg");
    }

    #[test]
    fn test_guess_image_media_type_png() {
        let data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A];
        assert_eq!(guess_image_media_type(&data), "image/png");
    }

    #[test]
    fn test_guess_image_media_type_gif() {
        let data = b"GIF89a".to_vec();
        assert_eq!(guess_image_media_type(&data), "image/gif");
    }

    #[test]
    fn test_guess_image_media_type_webp() {
        let mut data = b"RIFF".to_vec();
        data.extend_from_slice(&[0, 0, 0, 0]); // file size
        data.extend_from_slice(b"WEBP");
        assert_eq!(guess_image_media_type(&data), "image/webp");
    }

    #[test]
    fn test_guess_image_media_type_unknown_defaults_jpeg() {
        let data = vec![0x00, 0x01, 0x02];
        assert_eq!(guess_image_media_type(&data), "image/jpeg");
    }

    #[test]
    fn test_split_response_text_empty() {
        let chunks = split_response_text("");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn test_split_response_text_exact_4096() {
        let text = "a".repeat(4096);
        let chunks = split_response_text(&text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 4096);
    }

    #[test]
    fn test_split_response_text_4097() {
        let text = "a".repeat(4097);
        let chunks = split_response_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4096);
        assert_eq!(chunks[1].len(), 1);
    }

    #[test]
    fn test_split_response_text_newline_at_boundary() {
        // Total 4201 > 4096. Newline at position 4000, split should happen there.
        let mut text = "a".repeat(4000);
        text.push('\n');
        text.push_str(&"b".repeat(200));
        assert_eq!(text.len(), 4201);
        let chunks = split_response_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4000);
        assert_eq!(chunks[1].len(), 200);
    }

    #[test]
    fn test_guess_image_media_type_webp_too_short() {
        // RIFF header without WEBP at position 8-12 should default to jpeg
        let data = b"RIFF".to_vec();
        assert_eq!(guess_image_media_type(&data), "image/jpeg");
    }

    #[test]
    fn test_guess_image_media_type_empty() {
        assert_eq!(guess_image_media_type(&[]), "image/jpeg");
    }

    #[test]
    fn test_prepare_telegram_workspace_auto_images_markdown_absolute() {
        let root = std::env::temp_dir().join(format!("tg_auto_img_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shared")).unwrap();
        let img = root.join("shared").join("mark.png");
        std::fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let root = root.canonicalize().unwrap();
        let img = img.canonicalize().unwrap();

        let text = format!("Hello\n\n![]({})\n\nBye", img.display());
        let ctx = WorkspaceAutoImageContext {
            root: &root,
            chat_id: 1,
            persona_id: 2,
        };
        let (paths, body) = prepare_telegram_workspace_auto_images(&text, &ctx);
        assert_eq!(paths, vec![img.clone()]);
        assert!(body.contains("Hello"));
        assert!(body.contains("Bye"));
        assert!(!body.contains("!("));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_prepare_telegram_workspace_auto_images_skips_http() {
        let root = std::env::temp_dir().join(format!("tg_auto_img_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shared")).unwrap();
        let root = root.canonicalize().unwrap();

        let text = "x ![](https://example.com/a.png) y";
        let ctx = WorkspaceAutoImageContext {
            root: &root,
            chat_id: 1,
            persona_id: 2,
        };
        let (paths, body) = prepare_telegram_workspace_auto_images(text, &ctx);
        assert!(paths.is_empty());
        assert_eq!(body, text);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_prepare_telegram_workspace_auto_images_after_bare_normalize() {
        let root = std::env::temp_dir().join(format!("tg_auto_img_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shared").join("personas").join("9").join("3")).unwrap();
        let img = root
            .join("shared")
            .join("personas")
            .join("9")
            .join("3")
            .join("PZ-foo.png");
        std::fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let root = root.canonicalize().unwrap();

        let raw = "Hello\n\nPZ-foo.png\n\nBye";
        let normalized =
            crate::final_delivery_media::normalize_assistant_artifact_references(&raw, &root, 9, 3);
        let ctx = WorkspaceAutoImageContext {
            root: &root,
            chat_id: 9,
            persona_id: 3,
        };
        let (paths, body) = prepare_telegram_workspace_auto_images(&normalized, &ctx);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with(std::path::Path::new("PZ-foo.png")));
        assert!(body.contains("Hello"));
        assert!(body.contains("Bye"));
        assert!(!body.contains("PZ-foo.png"));
        let _ = std::fs::remove_dir_all(root);
    }
}
