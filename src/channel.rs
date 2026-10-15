use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use teloxide::prelude::*;

use crate::channels::telegram::{
    send_response_result, strip_embedded_bulletin_focus, WorkspaceAutoImageContext,
};
use crate::channels::wecom::WecomGateway;
use crate::db::{call_blocking, message_origin_interactive, Database, StoredMessage};
use crate::final_delivery_dedupe::{
    plan_agent_final_delivery, AgentFinalDeliveryPlan, EMPTY_TURN_NOTICE,
};
use crate::final_delivery_media::{
    materialize_web_delivery_file_links, normalize_assistant_artifact_references,
};
use crate::tools::auth_context_from_input;

/// Re-export for call sites that already import from `crate::channel`.
pub use crate::channels::CHANNEL_PROCESSING_ACK;

/// How a stored outbound message should be classified in `messages.origin`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageStoreOrigin {
    #[default]
    Interactive,
    Scheduled,
}

impl MessageStoreOrigin {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Interactive => crate::db::MESSAGE_ORIGIN_INTERACTIVE,
            Self::Scheduled => crate::db::MESSAGE_ORIGIN_SCHEDULED,
        }
    }

    fn into_origin_string(self) -> String {
        self.as_db_str().to_string()
    }
}

pub async fn is_web_chat(db: Arc<Database>, chat_id: i64) -> bool {
    matches!(
        call_blocking(db, move |d| d.get_chat_type(chat_id)).await,
        Ok(Some(ref t)) if t == "web"
    )
}

pub async fn enforce_channel_policy(
    db: Arc<Database>,
    input: &serde_json::Value,
    target_chat_id: i64,
) -> Result<(), String> {
    let Some(auth) = auth_context_from_input(input) else {
        return Ok(());
    };

    if is_web_chat(db, auth.caller_chat_id).await && auth.caller_chat_id != target_chat_id {
        return Err("Permission denied: web UI sessions cannot operate on other chats".into());
    }

    Ok(())
}

fn strip_leading_persona_tokens(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        if !rest.starts_with('[') {
            break;
        }
        let Some(close_idx) = rest.find(']') else {
            break;
        };
        // Only treat short single-line bracket heads as transport persona tags.
        let token = &rest[1..close_idx];
        if token.is_empty() || token.len() > 64 || token.contains('\n') {
            break;
        }
        rest = rest[close_idx + 1..].trim_start();
    }
    rest
}

fn normalize_persona_prefixed_text(persona_name: &str, text: &str) -> String {
    let body = strip_leading_persona_tokens(text).trim();
    if body.is_empty() {
        format!("[{persona_name}]")
    } else {
        format!("[{persona_name}] {body}")
    }
}

/// Prepend `[PersonaName] ` to outbound bot text so users know which persona sent it.
pub async fn with_persona_indicator(db: Arc<Database>, persona_id: i64, text: &str) -> String {
    let name = match call_blocking(db, move |d| d.get_persona(persona_id)).await {
        Ok(Some(p)) => p.name,
        _ => "Unknown".to_string(),
    };
    normalize_persona_prefixed_text(&name, text)
}

pub async fn deliver_and_store_bot_message(
    bot: &Bot,
    db: Arc<Database>,
    bot_username: &str,
    chat_id: i64,
    persona_id: i64,
    text: &str,
    workspace_root: Option<PathBuf>,
) -> Result<(), String> {
    let text = strip_embedded_bulletin_focus(text);
    let text = crate::agent_turn_context::strip_stored_dialogue_markup(&text);
    let text = &with_persona_indicator(db.clone(), persona_id, &text).await;
    if is_web_chat(db.clone(), chat_id).await {
        let msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id,
            persona_id,
            session_id: None,
            sender_name: bot_username.to_string(),
            content: text.to_string(),
            is_from_bot: true,
            timestamp: chrono::Utc::now().to_rfc3339(),
            origin: message_origin_interactive(),
            speaker_persona_id: None,
        };
        call_blocking(db.clone(), move |d| d.store_message(&msg))
            .await
            .map_err(|e| format!("Failed to store web message: {e}"))
    } else {
        let auto_images = workspace_root
            .as_ref()
            .map(|root| WorkspaceAutoImageContext {
                root,
                chat_id,
                persona_id,
            });
        let send_result = send_response_result(bot, ChatId(chat_id), text, None, auto_images).await;
        let msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id,
            persona_id,
            session_id: None,
            sender_name: bot_username.to_string(),
            content: text.to_string(),
            is_from_bot: true,
            timestamp: chrono::Utc::now().to_rfc3339(),
            origin: message_origin_interactive(),
            speaker_persona_id: None,
        };
        match &send_result {
            Ok(_) => {}
            Err(e) => {
                let err_str = e.to_string();
                // Chat may have been deleted or bot removed; still store so conversation history is intact (e.g. web UI can show reply).
                if err_str.contains("chat not found")
                    || err_str.contains("Chat not found")
                    || err_str.contains("user is deactivated")
                {
                    tracing::warn!(
                        target: "channel",
                        chat_id = chat_id,
                        error = %err_str,
                        "Telegram delivery failed (chat unavailable); storing message anyway"
                    );
                    call_blocking(db.clone(), move |d| d.store_message(&msg))
                        .await
                        .map_err(|e| format!("Failed to store message: {e}"))?;
                    return Ok(());
                }

                return Err(format!("Failed to send message: {e}"));
            }
        }
        call_blocking(db.clone(), move |d| d.store_message(&msg))
            .await
            .map_err(|e| format!("Failed to store sent message: {e}"))
    }
}

/// Controls which external bindings receive an outbound message (web history always stored).
#[derive(Debug, Clone, Default)]
pub enum DeliveryScope {
    /// Deliver to every bound channel for this contact (legacy; avoid for interactive replies).
    ContactWide,
    /// Reply only on the platform + bot instance (+ optional handle) that received the inbound message.
    PlatformInstance {
        channel_type: &'static str,
        bot_instance_id: i64,
        /// When set, deliver only to this binding handle (required when one contact has multiple handles, e.g. WeCom groups).
        channel_handle: Option<String>,
    },
    /// Persist for web/history only; no Telegram/Discord/WhatsApp/WeCom send.
    #[default]
    StoreOnly,
}

impl DeliveryScope {
    pub fn platform_reply(
        channel_type: &'static str,
        bot_instance_id: i64,
        channel_handle: impl Into<String>,
    ) -> Self {
        Self::PlatformInstance {
            channel_type,
            bot_instance_id,
            channel_handle: Some(channel_handle.into()),
        }
    }
}

/// Store the bot message once under canonical_chat_id and deliver per [`DeliveryScope`].
pub async fn deliver_to_contact(
    db: Arc<Database>,
    telegram_bots: &HashMap<i64, Bot>,
    discord_http: &HashMap<i64, Arc<serenity::http::Http>>,
    wecom: Option<&WecomGateway>,
    bot_username: &str,
    canonical_chat_id: i64,
    persona_id: i64,
    text: &str,
    workspace_root: Option<PathBuf>,
    scope: DeliveryScope,
    session_id: Option<String>,
) -> Result<(), String> {
    deliver_to_contact_with_origin(
        db,
        telegram_bots,
        discord_http,
        wecom,
        bot_username,
        canonical_chat_id,
        persona_id,
        text,
        workspace_root,
        scope,
        session_id,
        MessageStoreOrigin::Interactive,
    )
    .await
}

pub async fn deliver_to_contact_with_origin(
    db: Arc<Database>,
    telegram_bots: &HashMap<i64, Bot>,
    discord_http: &HashMap<i64, Arc<serenity::http::Http>>,
    wecom: Option<&WecomGateway>,
    bot_username: &str,
    canonical_chat_id: i64,
    persona_id: i64,
    text: &str,
    workspace_root: Option<PathBuf>,
    scope: DeliveryScope,
    session_id: Option<String>,
    message_origin: MessageStoreOrigin,
) -> Result<(), String> {
    let text = strip_embedded_bulletin_focus(text);
    let text = crate::agent_turn_context::strip_stored_dialogue_markup(&text);
    let text = &with_persona_indicator(db.clone(), persona_id, &text).await;
    let scope = effective_delivery_scope(scope, session_id.as_deref());
    let msg = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: canonical_chat_id,
        persona_id,
        session_id,
        sender_name: bot_username.to_string(),
        content: text.to_string(),
        is_from_bot: true,
        timestamp: chrono::Utc::now().to_rfc3339(),
        origin: message_origin.into_origin_string(),
        speaker_persona_id: None,
    };
    let artifact_message = msg.clone();
    call_blocking(db.clone(), move |d| d.store_message(&msg))
        .await
        .map_err(|e| format!("Failed to store message: {e}"))?;
    if let Some(root) = workspace_root.clone() {
        let m = artifact_message;
        match tokio::task::spawn_blocking(move || {
            crate::artifacts::Catalog::open(&root)?.associate_text(
                m.chat_id,
                m.persona_id,
                &m.id,
                m.session_id.as_deref(),
                &m.content,
            )
        })
        .await
        {
            Ok(Ok(())) => {}
            failure => tracing::warn!(?failure, "artifact_message_association_failed"),
        }
    }

    let bindings = call_blocking(db.clone(), move |d| {
        d.list_bindings_for_contact(canonical_chat_id)
    })
    .await
    .map_err(|e| format!("Failed to list bindings: {e}"))?;
    let policies = call_blocking(db.clone(), move |d| {
        d.list_channel_persona_policies(canonical_chat_id)
    })
    .await
    .map_err(|e| format!("Failed to list channel persona policies: {e}"))?;
    let mut policy_by_instance: std::collections::HashMap<
        i64,
        (crate::db::ChannelPersonaMode, Option<i64>),
    > = std::collections::HashMap::new();
    for p in policies {
        policy_by_instance.insert(p.bot_instance_id, (p.mode, p.persona_id));
    }
    let active_persona_id = call_blocking(db.clone(), move |d| {
        d.get_current_persona_id(canonical_chat_id)
    })
    .await
    .ok();

    let mut delivered_targets: HashSet<(String, String)> = HashSet::new();
    if matches!(scope, DeliveryScope::StoreOnly) {
        return Ok(());
    }
    for b in &bindings {
        if !binding_matches_delivery_scope(
            &b.channel_type,
            b.bot_instance_id,
            &b.channel_handle,
            &scope,
        ) {
            continue;
        }
        if let Some((mode, policy_persona_id)) = policy_by_instance.get(&b.bot_instance_id) {
            if *mode == crate::db::ChannelPersonaMode::Single
                && policy_persona_id.is_some()
                && *policy_persona_id != Some(persona_id)
            {
                continue;
            }
        } else if b.channel_type == "whatsapp" && active_persona_id != Some(persona_id) {
            continue;
        }
        let target_key = (b.channel_type.clone(), b.channel_handle.clone());
        if !delivered_targets.insert(target_key) {
            continue;
        }
        match b.channel_type.as_str() {
            "telegram" => {
                let tg_bot = match &scope {
                    DeliveryScope::PlatformInstance {
                        bot_instance_id, ..
                    } => telegram_bots.get(bot_instance_id),
                    _ => telegram_bots
                        .get(&b.bot_instance_id)
                        .or_else(|| telegram_bots.get(&crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY)),
                };
                if let Some(bot) = tg_bot {
                    if let Ok(chat_id) = b.channel_handle.parse::<i64>() {
                        let auto_images =
                            workspace_root
                                .as_ref()
                                .map(|root| WorkspaceAutoImageContext {
                                    root,
                                    chat_id: canonical_chat_id,
                                    persona_id,
                                });
                        if let Err(e) =
                            send_response_result(bot, ChatId(chat_id), text, None, auto_images)
                                .await
                        {
                            let err_str = e.to_string();
                            if !err_str.contains("chat not found")
                                && !err_str.contains("Chat not found")
                                && !err_str.contains("user is deactivated")
                            {
                                tracing::warn!(target: "channel", chat_id = chat_id, error = %err_str, "Telegram delivery to bound channel failed");
                            }
                        }
                    }
                }
            }
            "discord" => {
                let http = match &scope {
                    DeliveryScope::PlatformInstance {
                        bot_instance_id, ..
                    } => discord_http.get(bot_instance_id),
                    _ => discord_http
                        .get(&b.bot_instance_id)
                        .or_else(|| discord_http.get(&crate::db::BOT_INSTANCE_DISCORD_PRIMARY)),
                };
                if let Some(http) = http {
                    if let Ok(channel_id_u64) = b.channel_handle.parse::<u64>() {
                        let channel_id = serenity::model::id::ChannelId::new(channel_id_u64);
                        const MAX_LEN: usize = 2000;
                        let content = text.to_string();
                        if content.len() <= MAX_LEN {
                            if let Err(e) = channel_id.say(http.as_ref(), &content).await {
                                tracing::warn!(target: "channel", channel_id = %channel_id_u64, error = %e, "Discord delivery to bound channel failed");
                            }
                        } else {
                            let chars: Vec<char> = content.chars().collect();
                            for chunk in chars.chunks(MAX_LEN) {
                                let s: String = chunk.iter().collect();
                                let _ = channel_id.say(http.as_ref(), &s).await;
                            }
                        }
                    }
                }
            }
            "web" => {
                // Already stored above; web clients load from history or SSE
            }
            "wecom" => {
                if let Some(client) = wecom {
                    if let Err(e) = client.send_text(&b.channel_handle, text).await {
                        tracing::warn!(
                            target: "channel",
                            handle = %b.channel_handle,
                            error = %e,
                            "WeCom delivery to bound channel failed"
                        );
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Web focused sessions must not fan out to external channels.
pub(crate) fn effective_delivery_scope(
    scope: DeliveryScope,
    session_id: Option<&str>,
) -> DeliveryScope {
    if session_id.is_some() && matches!(scope, DeliveryScope::ContactWide) {
        DeliveryScope::StoreOnly
    } else {
        scope
    }
}

/// Whether a binding should receive an outbound message for the given scope.
pub(crate) fn binding_matches_delivery_scope(
    binding_channel_type: &str,
    binding_bot_instance_id: i64,
    binding_handle: &str,
    scope: &DeliveryScope,
) -> bool {
    match scope {
        DeliveryScope::ContactWide => true,
        DeliveryScope::PlatformInstance {
            channel_type,
            bot_instance_id,
            channel_handle,
        } => {
            binding_channel_type == *channel_type
                && binding_bot_instance_id == *bot_instance_id
                && channel_handle
                    .as_ref()
                    .is_none_or(|want| want == binding_handle)
        }
        DeliveryScope::StoreOnly => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFinalDeliveryOutcome {
    /// Text the HTTP API should echo (`""` when the final was suppressed as redundant).
    pub response_for_client: String,
}

/// Agent loop completion only: plan delivery (empty-body skip) then [`deliver_to_contact`].
fn normalize_final_for_delivery(
    raw_final: &str,
    workspace_root: Option<&Path>,
    canonical_chat_id: i64,
    persona_id: i64,
) -> String {
    let cleaned = crate::agent_turn_context::strip_stored_dialogue_markup(
        &strip_embedded_bulletin_focus(raw_final),
    );
    match workspace_root {
        Some(root) => {
            normalize_assistant_artifact_references(&cleaned, root, canonical_chat_id, persona_id)
        }
        None => cleaned,
    }
}

pub async fn deliver_agent_final_to_contact(
    db: Arc<Database>,
    telegram_bots: &HashMap<i64, Bot>,
    discord_http: &HashMap<i64, Arc<serenity::http::Http>>,
    wecom: Option<&WecomGateway>,
    bot_username: &str,
    canonical_chat_id: i64,
    persona_id: i64,
    raw_final: &str,
    workspace_root: Option<PathBuf>,
    scope: DeliveryScope,
    session_id: Option<String>,
) -> Result<AgentFinalDeliveryOutcome, String> {
    deliver_agent_final_to_contact_with_origin(
        db,
        telegram_bots,
        discord_http,
        wecom,
        bot_username,
        canonical_chat_id,
        persona_id,
        raw_final,
        workspace_root,
        scope,
        session_id,
        MessageStoreOrigin::Interactive,
    )
    .await
}

pub async fn deliver_agent_final_to_contact_with_origin(
    db: Arc<Database>,
    telegram_bots: &HashMap<i64, Bot>,
    discord_http: &HashMap<i64, Arc<serenity::http::Http>>,
    wecom: Option<&WecomGateway>,
    bot_username: &str,
    canonical_chat_id: i64,
    persona_id: i64,
    raw_final: &str,
    workspace_root: Option<PathBuf>,
    scope: DeliveryScope,
    session_id: Option<String>,
    message_origin: MessageStoreOrigin,
) -> Result<AgentFinalDeliveryOutcome, String> {
    let mut cleaned = normalize_final_for_delivery(
        raw_final,
        workspace_root.as_deref(),
        canonical_chat_id,
        persona_id,
    );
    if let Some(root) = workspace_root.as_deref() {
        cleaned = materialize_web_delivery_file_links(
            root,
            None,
            canonical_chat_id,
            persona_id,
            &cleaned,
        )
        .await?;
    }
    let indicated = with_persona_indicator(db.clone(), persona_id, &cleaned).await;
    let plan = plan_agent_final_delivery(None, &indicated);

    match plan {
        AgentFinalDeliveryPlan::DeliverFull => {
            deliver_to_contact_with_origin(
                db.clone(),
                telegram_bots,
                discord_http,
                wecom,
                bot_username,
                canonical_chat_id,
                persona_id,
                &cleaned,
                workspace_root,
                scope,
                session_id,
                message_origin,
            )
            .await?;
            Ok(AgentFinalDeliveryOutcome {
                response_for_client: cleaned,
            })
        }
        AgentFinalDeliveryPlan::DeliverSuffixOnly(suffix) => {
            let mut suffix = normalize_final_for_delivery(
                &suffix,
                workspace_root.as_deref(),
                canonical_chat_id,
                persona_id,
            );
            if let Some(root) = workspace_root.as_deref() {
                suffix = materialize_web_delivery_file_links(
                    root,
                    None,
                    canonical_chat_id,
                    persona_id,
                    &suffix,
                )
                .await?;
            }
            deliver_to_contact_with_origin(
                db.clone(),
                telegram_bots,
                discord_http,
                wecom,
                bot_username,
                canonical_chat_id,
                persona_id,
                &suffix,
                workspace_root,
                scope,
                session_id,
                message_origin,
            )
            .await?;
            Ok(AgentFinalDeliveryOutcome {
                response_for_client: suffix,
            })
        }
        AgentFinalDeliveryPlan::Skip => {
            tracing::warn!(
                target: "channel",
                chat_id = canonical_chat_id,
                persona_id,
                "empty agent final; storing user-visible notice instead of skipping"
            );
            deliver_to_contact_with_origin(
                db.clone(),
                telegram_bots,
                discord_http,
                wecom,
                bot_username,
                canonical_chat_id,
                persona_id,
                EMPTY_TURN_NOTICE,
                workspace_root,
                scope,
                session_id,
                message_origin,
            )
            .await?;
            Ok(AgentFinalDeliveryOutcome {
                response_for_client: EMPTY_TURN_NOTICE.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        binding_matches_delivery_scope, effective_delivery_scope, normalize_persona_prefixed_text,
        DeliveryScope,
    };
    use crate::db::BOT_INSTANCE_WECOM_PRIMARY;

    #[test]
    fn platform_reply_delivers_only_to_matching_wecom_handle() {
        let scope =
            DeliveryScope::platform_reply("wecom", BOT_INSTANCE_WECOM_PRIMARY, "chat:groupA");
        assert!(binding_matches_delivery_scope(
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "chat:groupA",
            &scope,
        ));
        assert!(!binding_matches_delivery_scope(
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "chat:groupB",
            &scope,
        ));
        assert!(!binding_matches_delivery_scope(
            "telegram",
            BOT_INSTANCE_WECOM_PRIMARY,
            "chat:groupA",
            &scope,
        ));
    }

    #[test]
    fn contact_wide_delivers_to_all_bindings() {
        let scope = DeliveryScope::ContactWide;
        assert!(binding_matches_delivery_scope(
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "chat:any",
            &scope,
        ));
        assert!(binding_matches_delivery_scope("telegram", 1, "123", &scope,));
    }

    #[test]
    fn store_only_skips_external_bindings() {
        let scope = DeliveryScope::StoreOnly;
        assert!(!binding_matches_delivery_scope(
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "chat:groupA",
            &scope,
        ));
    }

    #[test]
    fn focused_session_contact_wide_becomes_store_only() {
        let scope = effective_delivery_scope(DeliveryScope::ContactWide, Some("session-1"));
        assert!(matches!(scope, DeliveryScope::StoreOnly));
    }

    #[test]
    fn main_chat_platform_reply_is_unchanged() {
        let scope = effective_delivery_scope(
            DeliveryScope::platform_reply("wecom", BOT_INSTANCE_WECOM_PRIMARY, "chat:g1"),
            None,
        );
        assert!(matches!(scope, DeliveryScope::PlatformInstance { .. }));
    }

    #[test]
    fn persona_prefix_is_added_once() {
        let out = normalize_persona_prefixed_text("InfluencerPZ", "Hello");
        assert_eq!(out, "[InfluencerPZ] Hello");
    }

    #[test]
    fn repeated_leading_persona_tags_are_collapsed() {
        let out = normalize_persona_prefixed_text(
            "InfluencerPZ",
            "[InfluencerPZ] [InfluencerPZ] [InfluencerPZ] Hi there",
        );
        assert_eq!(out, "[InfluencerPZ] Hi there");
    }

    #[test]
    fn other_persona_tag_is_replaced_with_current() {
        let out = normalize_persona_prefixed_text("Trader", "[InfluencerPZ] Market open");
        assert_eq!(out, "[Trader] Market open");
    }
}
