//! User-visible recovery when a chat-queue task is hard-aborted (timeout / panic).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serenity::http::Http as SerenityHttp;
use teloxide::Bot;
use tracing::{info, warn};

use crate::channel::{deliver_to_contact, DeliveryScope};
use crate::channels::wecom::WecomGateway;
use crate::chat_queue::QueueHardAbortHook;
use crate::db::{call_blocking, Database};

/// Default copy when the persona lane hits the queue hard timeout.
pub fn hard_timeout_user_message(timeout_secs: u64) -> String {
    format!(
        "This run was stopped after {timeout_secs}s without finishing (queue hard timeout). \
Please send your request again — if it keeps happening, try a smaller step or check Background / Cursor bridge health."
    )
}

pub fn hard_abort_user_message(reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        "This run was stopped unexpectedly before a reply was ready. Please send your request again."
            .to_string()
    } else if reason.contains("hard timeout") || reason.contains("queue hard timeout") {
        reason.to_string()
    } else {
        format!(
            "This run was stopped before a reply was ready ({reason}). Please send your request again."
        )
    }
}

fn stop_reason_from_abort(reason: &str) -> &'static str {
    if reason.contains("hard timeout") || reason.contains("queue hard timeout") {
        "queue_hard_timeout"
    } else if reason.contains("panic") {
        "queue_task_panic"
    } else {
        "queue_hard_abort"
    }
}

/// Persist a bot message + `run_finished` so the UI is not left spinning with no history row.
pub async fn persist_hard_abort_notice(
    db: Arc<Database>,
    bot_username: &str,
    chat_id: i64,
    persona_id: i64,
    run_key: &str,
    user_message: &str,
    stop_reason: &str,
) {
    let scope = DeliveryScope::StoreOnly;
    if let Err(e) = deliver_to_contact(
        db.clone(),
        &HashMap::new(),
        &HashMap::new(),
        None,
        bot_username,
        chat_id,
        persona_id,
        user_message,
        None,
        scope,
        None,
    )
    .await
    {
        warn!(
            chat_id,
            persona_id,
            run_key,
            error = %e,
            "hard abort: failed to store user-visible notice"
        );
    }

    let run_key_owned = run_key.to_string();
    let stop_reason_owned = stop_reason.to_string();
    let payload = format!(r#"{{"stop_reason":"{stop_reason_owned}"}}"#);
    if let Err(e) = call_blocking(db, move |db| {
        db.append_run_timeline_event(
            &run_key_owned,
            chat_id,
            persona_id,
            "run_finished",
            Some(&payload),
        )
    })
    .await
    {
        warn!(
            chat_id,
            persona_id,
            run_key,
            error = %e,
            "hard abort: failed to append run_finished"
        );
    } else {
        info!(
            chat_id,
            persona_id, run_key, stop_reason, "hard abort: recorded run_finished + notice"
        );
    }
}

/// Build a queue hook that always stores a notice (web history) and marks the run finished.
pub fn make_store_only_hard_abort_hook(
    db: Arc<Database>,
    bot_username: String,
    chat_id: i64,
    persona_id: i64,
    run_key: String,
) -> QueueHardAbortHook {
    Arc::new(move |reason: String| {
        let db = db.clone();
        let bot_username = bot_username.clone();
        let run_key = run_key.clone();
        let user_message = hard_abort_user_message(&reason);
        let stop_reason = stop_reason_from_abort(&reason).to_string();
        Box::pin(async move {
            persist_hard_abort_notice(
                db,
                &bot_username,
                chat_id,
                persona_id,
                &run_key,
                &user_message,
                &stop_reason,
            )
            .await;
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    })
}

/// Deliver via channel bindings when available; always ensure `run_finished` is recorded.
#[allow(clippy::too_many_arguments)]
pub fn make_deliver_hard_abort_hook(
    db: Arc<Database>,
    telegram_bots: Arc<HashMap<i64, Bot>>,
    discord_http: Arc<HashMap<i64, Arc<SerenityHttp>>>,
    wecom: Option<Arc<WecomGateway>>,
    bot_username: String,
    chat_id: i64,
    persona_id: i64,
    run_key: String,
    scope: DeliveryScope,
    workspace_root: Option<PathBuf>,
) -> QueueHardAbortHook {
    Arc::new(move |reason: String| {
        let db = db.clone();
        let telegram_bots = telegram_bots.clone();
        let discord_http = discord_http.clone();
        let wecom = wecom.clone();
        let bot_username = bot_username.clone();
        let run_key = run_key.clone();
        let scope = scope.clone();
        let workspace_root = workspace_root.clone();
        let user_message = hard_abort_user_message(&reason);
        let stop_reason = stop_reason_from_abort(&reason).to_string();
        Box::pin(async move {
            if let Err(e) = deliver_to_contact(
                db.clone(),
                telegram_bots.as_ref(),
                discord_http.as_ref(),
                wecom.as_deref(),
                &bot_username,
                chat_id,
                persona_id,
                &user_message,
                workspace_root,
                scope,
                None,
            )
            .await
            {
                warn!(
                    chat_id,
                    persona_id,
                    run_key = %run_key,
                    error = %e,
                    "hard abort: deliver_to_contact failed; falling back to store-only"
                );
                persist_hard_abort_notice(
                    db.clone(),
                    &bot_username,
                    chat_id,
                    persona_id,
                    &run_key,
                    &user_message,
                    &stop_reason,
                )
                .await;
                return;
            }

            let run_key_owned = run_key.clone();
            let payload = format!(r#"{{"stop_reason":"{stop_reason}"}}"#);
            let _ = call_blocking(db, move |db| {
                db.append_run_timeline_event(
                    &run_key_owned,
                    chat_id,
                    persona_id,
                    "run_finished",
                    Some(&payload),
                )
            })
            .await;
            info!(
                chat_id,
                persona_id,
                run_key = %run_key,
                stop_reason = %stop_reason,
                "hard abort: delivered notice + run_finished"
            );
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_timeout_message_mentions_seconds() {
        let msg = hard_timeout_user_message(3600);
        assert!(msg.contains("3600"));
        assert!(msg.contains("stopped"));
    }

    #[test]
    fn hard_abort_passes_through_timeout_reason() {
        let reason = hard_timeout_user_message(60);
        let msg = hard_abort_user_message(&reason);
        assert_eq!(msg, reason);
    }
}
