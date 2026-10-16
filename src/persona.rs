//! Persona system: per-chat identity that selects which session and message history to use.
//! Operations (list, switch, new, delete, model) are internal; use the HTTP API or this module.
//! Inbound messages resolve run persona via [`resolve_incoming_run_persona`] (optional `[Name]` prefix)
//! or `get_current_persona_id`; explicit switch commands still use `set_active_persona`.

use std::sync::Arc;

use crate::config::Config;
use crate::db::{call_blocking, ChannelPersonaMode, Database, BOT_INSTANCE_WEB};
use crate::error::FinallyAValueBotError;

/// Leading `[token]` values that are transport/system tags, not persona names.
const RESERVED_INBOUND_PERSONA_TOKENS: &[&str] = &["image", "document", "location", "voice"];

/// Resolve which persona owns this inbound turn and optional body text after stripping a leading
/// `[PersonaName]` tag. Does **not** call `set_active_persona` — DB active is unchanged.
///
/// - If the trimmed text starts with `[token]` and `token` matches a persona name for `chat_id`
///   (ASCII case-insensitive), returns that persona's id and the remainder (trimmed) as the stored
///   body.
/// - Reserved tokens (`image`, `document`, `location`, `voice`) never select a persona.
/// - Otherwise returns [`Database::get_current_persona_id`] and the original `text`.
pub fn resolve_incoming_run_persona(
    db: &Database,
    chat_id: i64,
    text: &str,
) -> Result<(i64, String), FinallyAValueBotError> {
    resolve_incoming_run_persona_for_channel(
        db,
        chat_id,
        "telegram",
        crate::db::BOT_INSTANCE_TELEGRAM_PRIMARY,
        text,
    )
}

/// Resolve run persona with optional per-bot-instance policy (Telegram/Discord/WhatsApp).
///
/// Web chat does not use this policy: pass `channel_type == "web"` or `bot_instance_id == 0`
/// to always use standard `[PersonaName]` resolution (Web UI selects persona separately).
///
/// Policy key: `(canonical_chat_id, bot_instance_id)`:
/// - `all`: use standard `[PersonaName]` token resolution.
/// - `single`: force the configured `persona_id` and strip only transport tags.
pub fn resolve_incoming_run_persona_for_channel(
    db: &Database,
    chat_id: i64,
    channel_type: &str,
    bot_instance_id: i64,
    text: &str,
) -> Result<(i64, String), FinallyAValueBotError> {
    if channel_type == "web" || bot_instance_id == BOT_INSTANCE_WEB {
        return resolve_incoming_run_persona_all_personas(db, chat_id, text);
    }

    let policy = db.get_channel_persona_policy(chat_id, bot_instance_id)?;
    if let Some(policy) = policy {
        if policy.mode == ChannelPersonaMode::Single {
            let forced = policy
                .persona_id
                .filter(|id| *id > 0)
                .filter(|id| db.persona_exists(chat_id, *id).unwrap_or(false))
                .unwrap_or(db.get_current_persona_id(chat_id)?);
            let trimmed = text.trim_start();
            if let Some((_, rest)) = parse_leading_token(trimmed) {
                return Ok((forced, rest.trim_start().to_string()));
            }
            return Ok((forced, text.to_string()));
        }
    }

    if channel_type == "whatsapp" {
        let forced = db.get_current_persona_id(chat_id)?;
        let trimmed = text.trim_start();
        if let Some((_, rest)) = parse_leading_token(trimmed) {
            return Ok((forced, rest.trim_start().to_string()));
        }
        return Ok((forced, text.to_string()));
    }

    resolve_incoming_run_persona_all_personas(db, chat_id, text)
}

fn resolve_incoming_run_persona_all_personas(
    db: &Database,
    chat_id: i64,
    text: &str,
) -> Result<(i64, String), FinallyAValueBotError> {
    let fallback_pid = db.get_current_persona_id(chat_id)?;
    let trimmed = text.trim_start();
    let Some((token, body)) = parse_leading_token(trimmed) else {
        return Ok((fallback_pid, text.to_string()));
    };
    let token_lower = token.to_lowercase();
    if RESERVED_INBOUND_PERSONA_TOKENS
        .iter()
        .any(|r| *r == token_lower.as_str())
    {
        return Ok((fallback_pid, text.to_string()));
    }
    let personas = db.list_personas(chat_id)?;
    let Some(persona) = personas.iter().find(|p| p.name.eq_ignore_ascii_case(token)) else {
        return Ok((fallback_pid, text.to_string()));
    };
    let stored = body.trim_start().to_string();
    Ok((persona.id, stored))
}

fn parse_leading_token(trimmed: &str) -> Option<(&str, &str)> {
    if !trimmed.starts_with('[') {
        return None;
    }
    let close_idx = trimmed.find(']')?;
    let token = &trimmed[1..close_idx];
    if token.is_empty() || token.len() > 64 || token.contains('\n') {
        return None;
    }
    Some((token, &trimmed[close_idx + 1..]))
}

/// Handle a persona command payload (e.g. from API or internal call).
/// `text` is the full message; the first token is typically "/persona" or "/personas", rest are subcommand and args.
/// When creating or updating a persona with a model, pass `config` so the model is tested first; if `config` is None, the test is skipped.
/// Returns a response string to show the user (or API client).
pub async fn handle_persona_command(
    db: Arc<Database>,
    chat_id: i64,
    text: &str,
    config: Option<&Config>,
) -> String {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let sub = parts.get(1).map(|s| *s).unwrap_or("");

    if sub.is_empty() || sub == "list" {
        // List personas
        let personas = match call_blocking(db.clone(), move |d| d.list_personas(chat_id)).await {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        let active_id =
            match call_blocking(db.clone(), move |d| d.get_active_persona_id(chat_id)).await {
                Ok(Some(id)) => id,
                _ => 0,
            };
        if personas.is_empty() {
            let _ = call_blocking(db.clone(), move |d| {
                d.get_or_create_default_persona(chat_id)
            })
            .await;
            return "Personas: default (active). Use /persona switch <name> to switch.".into();
        }
        let names: Vec<String> = personas
            .iter()
            .map(|p| {
                let suffix = if Some(p.id) == active_id.into() {
                    " (active)"
                } else {
                    ""
                };
                format!("{}{}", p.name, suffix)
            })
            .collect();
        format!(
            "Personas: {}. Use /persona switch <name> to switch.",
            names.join(", ")
        )
    } else if sub == "switch" {
        let name: String = parts.get(2).map(|s| (*s).to_string()).unwrap_or_default();
        if name.is_empty() {
            return "Usage: /persona switch <name>".into();
        }
        let name_for_fmt = name.clone();
        match call_blocking(db.clone(), move |d| d.get_persona_by_name(chat_id, &name)).await {
            Ok(Some(persona)) => {
                if let Ok(true) = call_blocking(db.clone(), move |d| {
                    d.set_active_persona(chat_id, persona.id)
                })
                .await
                {
                    format!("Switched to {}.", name_for_fmt)
                } else {
                    "Failed to switch.".into()
                }
            }
            Ok(None) => format!(
                "Persona '{}' not found. Use /persona new {} to create.",
                name_for_fmt, name_for_fmt
            ),
            Err(e) => format!("Error: {e}"),
        }
    } else if sub == "new" {
        let name = parts.get(2).map(|s| (*s).to_string()).unwrap_or_default();
        if name.is_empty() {
            return "Usage: /persona new <name> [model]".into();
        }
        let model: Option<String> = parts.get(3).map(|s| (*s).to_string());
        let model_note = model
            .as_ref()
            .map(|m| format!(" using model {}", m))
            .unwrap_or_default();
        let name_for_fmt = name.clone();
        // When a model is specified, test it before creating the persona (if config available)
        let model_ok_note = if let Some(ref model_str) = model {
            if let Some(cfg) = config {
                match crate::llm::test_model(cfg, model_str).await {
                    Ok(()) => "Model OK. ",
                    Err(e) => return format!("Model test failed: {e}. Persona not created."),
                }
            } else {
                ""
            }
        } else {
            ""
        };
        match call_blocking(db.clone(), move |d| {
            d.create_persona(chat_id, &name, model.as_deref())
        })
        .await
        {
            Ok(new_id) => {
                let _ =
                    call_blocking(db.clone(), move |d| d.set_active_persona(chat_id, new_id)).await;
                format!(
                    "{}Created persona {}{} and switched to it.",
                    model_ok_note, name_for_fmt, model_note
                )
            }
            Err(e) => format!("Error: {e}"),
        }
    } else if sub == "delete" {
        let name = parts.get(2).map(|s| (*s).to_string()).unwrap_or_default();
        if name.is_empty() {
            return "Usage: /persona delete <name>".into();
        }
        let name_for_fmt = name.clone();
        match call_blocking(db.clone(), move |d| d.get_persona_by_name(chat_id, &name)).await {
            Ok(Some(persona)) => {
                match call_blocking(db.clone(), move |d| d.delete_persona(chat_id, persona.id))
                    .await
                {
                    Ok(true) => format!("Deleted persona {}.", name_for_fmt),
                    Ok(false) => "Failed to delete.".into(),
                    Err(e) => format!("Error: {e}"),
                }
            }
            Ok(None) => format!("Persona '{}' not found.", name_for_fmt),
            Err(e) => format!("Error: {e}"),
        }
    } else if sub == "model" {
        let name = parts.get(2).map(|s| (*s).to_string()).unwrap_or_default();
        let model: Option<String> = parts.get(3).map(|s| (*s).to_string());
        if name.is_empty() {
            return "Usage: /persona model <name> <model>".into();
        }
        let model_str = match &model {
            Some(m) => m.as_str(),
            None => return "Usage: /persona model <name> <model>".into(),
        };
        // Test the model before updating (if config available)
        let model_ok_note = if let Some(cfg) = config {
            match crate::llm::test_model(cfg, model_str).await {
                Ok(()) => "Model OK. ",
                Err(e) => return format!("Model test failed: {e}. Persona model not updated."),
            }
        } else {
            ""
        };
        let name_for_fmt = name.clone();
        match call_blocking(db.clone(), move |d| d.get_persona_by_name(chat_id, &name)).await {
            Ok(Some(persona)) => {
                let persona_id = persona.id;
                let model_display = model.clone();
                if let Ok(true) = call_blocking(db.clone(), move |d| {
                    d.update_persona_model(chat_id, persona_id, model.as_deref())
                })
                .await
                {
                    format!(
                        "{}Set model for {} to {:?}.",
                        model_ok_note, name_for_fmt, model_display
                    )
                } else {
                    "Failed to update.".into()
                }
            }
            Ok(None) => format!("Persona '{}' not found.", name_for_fmt),
            Err(e) => format!("Error: {e}"),
        }
    } else {
        "Usage: /persona [list|switch|new|delete|model]".into()
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::resolve_incoming_run_persona;
    use crate::db::Database;

    fn db_with_personas(dir: &std::path::Path) -> Database {
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        let chat_id = 42_i64;
        db.upsert_chat(chat_id, None, "private").unwrap();
        let _ = db.create_persona(chat_id, "Alpha", None).unwrap();
        let _ = db.create_persona(chat_id, "Beta", None).unwrap();
        db.set_active_persona(
            chat_id,
            db.get_persona_by_name(chat_id, "Alpha")
                .unwrap()
                .unwrap()
                .id,
        )
        .unwrap();
        db
    }

    #[test]
    fn prefix_routes_to_named_persona_without_changing_active() {
        let dir = std::env::temp_dir().join(format!("persona_resolve_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let db = db_with_personas(&dir);
        let chat_id = 42_i64;
        let active_before = db.get_active_persona_id(chat_id).unwrap().unwrap();
        let beta_id = db.get_persona_by_name(chat_id, "Beta").unwrap().unwrap().id;
        let (pid, body) =
            resolve_incoming_run_persona(&db, chat_id, "[Beta] hello").expect("resolve");
        assert_eq!(pid, beta_id);
        assert_eq!(body, "hello");
        let active_after = db.get_active_persona_id(chat_id).unwrap().unwrap();
        assert_eq!(active_before, active_after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_prefix_uses_current_persona() {
        let dir = std::env::temp_dir().join(format!("persona_resolve2_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let db = db_with_personas(&dir);
        let chat_id = 42_i64;
        let alpha_id = db
            .get_persona_by_name(chat_id, "Alpha")
            .unwrap()
            .unwrap()
            .id;
        let (pid, body) = resolve_incoming_run_persona(&db, chat_id, "plain").expect("resolve");
        assert_eq!(pid, alpha_id);
        assert_eq!(body, "plain");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reserved_document_prefix_does_not_match_persona() {
        let dir = std::env::temp_dir().join(format!("persona_resolve3_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let db = db_with_personas(&dir);
        let chat_id = 42_i64;
        let alpha_id = db
            .get_persona_by_name(chat_id, "Alpha")
            .unwrap()
            .unwrap()
            .id;
        let raw = "[document] saved_path=/tmp/x.txt note";
        let (pid, body) = resolve_incoming_run_persona(&db, chat_id, raw).expect("resolve");
        assert_eq!(pid, alpha_id);
        assert_eq!(body, raw);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod wecom_channel_policy_tests {
    use super::resolve_incoming_run_persona_for_channel;
    use crate::config::DEFAULT_UNIVERSAL_CHAT_ID;
    use crate::db::{ChannelPersonaMode, Database, BOT_INSTANCE_WECOM_PRIMARY};

    #[test]
    fn wecom_inbox_honors_single_persona_policy() {
        let dir = std::env::temp_dir().join(format!("persona_wecom_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        let inbox = DEFAULT_UNIVERSAL_CHAT_ID;
        db.upsert_chat(inbox, None, "web").unwrap();
        let _ = db.create_persona(inbox, "default", None).unwrap();
        let selling_id = db.create_persona(inbox, "selling_oversea", None).unwrap();
        db.set_channel_persona_policy(
            inbox,
            BOT_INSTANCE_WECOM_PRIMARY,
            ChannelPersonaMode::Single,
            Some(selling_id),
        )
        .unwrap();

        let (pid, body) = resolve_incoming_run_persona_for_channel(
            &db,
            inbox,
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "@bot hello",
        )
        .unwrap();
        assert_eq!(pid, selling_id);
        assert_eq!(body, "@bot hello");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wecom_hashed_contact_does_not_inherit_inbox_single_policy() {
        let dir = std::env::temp_dir().join(format!("persona_wecom_hash_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        let inbox = DEFAULT_UNIVERSAL_CHAT_ID;
        let hashed = 8735507888382702007_i64;
        db.upsert_chat(inbox, None, "web").unwrap();
        db.upsert_chat(hashed, None, "wecom").unwrap();
        let selling_id = db.create_persona(inbox, "selling_oversea", None).unwrap();
        let default_hashed = db.get_or_create_default_persona(hashed).unwrap();
        db.set_channel_persona_policy(
            inbox,
            BOT_INSTANCE_WECOM_PRIMARY,
            ChannelPersonaMode::Single,
            Some(selling_id),
        )
        .unwrap();

        let (pid, _) = resolve_incoming_run_persona_for_channel(
            &db,
            hashed,
            "wecom",
            BOT_INSTANCE_WECOM_PRIMARY,
            "hello",
        )
        .unwrap();
        assert_eq!(pid, default_hashed);
        assert_ne!(pid, selling_id);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
