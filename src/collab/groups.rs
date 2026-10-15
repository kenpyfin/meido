//! Group turns. A cheap moderator (or a fixed order) picks at most `max_speakers`
//! personas. Each speaks once, on its own queue lane, with a bounded window.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::oneshot;

use crate::channels::telegram::{process_with_agent_with_events, AgentRequestContext, AppState};
use crate::chat_queue::{QueueEnqueueMeta, QueueSource};
use crate::claude::{Message, MessageContent};
use crate::db::call_blocking;
use crate::db_collab::{GroupMember, GroupMessage};
use crate::topics::{load_topic_model_choice, response_text, send_topic_prompt, TopicModelChoice};

use super::{GROUP_SUMMARY_EVERY, GROUP_TOOL_CAP, GROUP_WINDOW};

fn round_cancels() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static CANCELS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn request_group_cancel(group_id: &str) {
    if let Ok(map) = round_cancels().lock() {
        if let Some(flag) = map.get(group_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

pub async fn run_group_round(
    state: Arc<AppState>,
    chat_id: i64,
    group_id: String,
    session_id: String,
    user_text: String,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    if let Ok(mut map) = round_cancels().lock() {
        map.insert(group_id.clone(), cancel.clone());
    }
    let db = state.db.clone();
    let gid = group_id.clone();
    let sid = session_id.clone();
    let text = user_text.clone();
    call_blocking(db, move |db| {
        db.append_group_message(&gid, &sid, None, &text, None)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?;

    let db = state.db.clone();
    let gid = group_id.clone();
    let (mode, max_speakers, provider, model, members) = call_blocking(db, move |db| {
        let group = db.get_group(chat_id, &gid)?.ok_or_else(|| {
            crate::error::FinallyAValueBotError::ToolExecution("group missing".into())
        })?;
        let members = db.list_group_members(&gid)?;
        Ok((
            group.turn_mode,
            group.max_speakers,
            group.moderator_provider,
            group.moderator_model,
            members,
        ))
    })
    .await
    .map_err(|e| e.to_string())?;

    if members.is_empty() {
        return Err("group has no members".into());
    }
    let speakers = if members.len() == 1 {
        vec![members[0].persona_id]
    } else if mode == "roundtable" {
        members
            .iter()
            .take(max_speakers as usize)
            .map(|m| m.persona_id)
            .collect()
    } else {
        pick_moderator(
            &state,
            &members,
            &user_text,
            &provider,
            &model,
            max_speakers as usize,
        )
        .await
    };

    let mut runs = 0usize;
    for persona_id in speakers {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let role = members
            .iter()
            .find(|m| m.persona_id == persona_id)
            .map(|m| m.role_note.clone())
            .unwrap_or_default();
        let window = speaker_window(&state, &session_id, &role).await?;
        let (tx, rx) = oneshot::channel();
        let app = state.clone();
        let sid = session_id.clone();
        let gid = group_id.clone();
        let cancel_run = cancel.clone();
        let run_id = uuid::Uuid::new_v4().to_string();
        let meta = QueueEnqueueMeta {
            run_id: run_id.clone(),
            persona_id,
            source: QueueSource::Web,
            label: format!("group:{gid}"),
            project_id: None,
            workflow_id: None,
            on_hard_abort: None,
        };
        state
            .chat_queue
            .enqueue_with_meta(chat_id, meta, move |lane_cancel| async move {
                let stop = cancel_run.clone();
                let lane = lane_cancel.clone();
                tokio::spawn(async move {
                    loop {
                        if stop.load(Ordering::SeqCst) || lane.load(Ordering::SeqCst) {
                            stop.store(true, Ordering::SeqCst);
                            lane.store(true, Ordering::SeqCst);
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                });
                let result = speak(&app, chat_id, persona_id, window, cancel_run).await;
                let _ = tx.send(result);
            })
            .await;
        let reply = match rx.await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => format!("(could not answer: {e})"),
            Err(_) => "(speaker cancelled)".into(),
        };
        let db = state.db.clone();
        call_blocking(db, move |db| {
            db.append_group_message(&gid, &sid, Some(persona_id), &reply, Some(&run_id))?;
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;
        runs += 1;
    }
    if let Ok(mut map) = round_cancels().lock() {
        map.remove(&group_id);
    }
    maybe_refresh_summary(&state, &session_id).await;
    tracing::info!(
        target: "collab",
        kind = "group",
        chat_id,
        runs,
        input_chars = user_text.chars().count(),
        duration_ms = started.elapsed().as_millis() as u64,
        "group round finished"
    );
    Ok(())
}

async fn pick_moderator(
    state: &AppState,
    members: &[GroupMember],
    user_text: &str,
    provider: &str,
    model: &str,
    max_speakers: usize,
) -> Vec<i64> {
    let mut roster = String::from("Members:\n");
    for member in members {
        roster.push_str(&format!(
            "- id {} name {} note {}\n",
            member.persona_id,
            member.name,
            member.role_note.chars().take(160).collect::<String>()
        ));
    }
    roster.push_str("\nUser message:\n");
    roster.push_str(&user_text.chars().take(1500).collect::<String>());
    let system = format!(
        "Choose who should answer. Reply with JSON only: {{\"speakers\":[persona_id,...]}}. \
         At most {max_speakers} ids, each from the member list. No prose."
    );
    let choice = resolve_choice(state, provider, model).await;
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(roster),
    }];
    let raw = match choice {
        Some((provider_id, model, base)) => send_topic_prompt(
            state,
            &provider_id,
            &model,
            base.as_deref(),
            &system,
            messages,
        )
        .await
        .map(|response| response_text(&response))
        .ok(),
        None => None,
    };
    let parsed = raw
        .as_deref()
        .and_then(|text| parse_speakers(text, members, max_speakers));
    parsed.unwrap_or_else(|| vec![members[0].persona_id])
}

async fn resolve_choice(
    state: &AppState,
    provider: &str,
    model: &str,
) -> Option<(String, String, Option<String>)> {
    if !provider.trim().is_empty() && !model.trim().is_empty() {
        return Some((provider.to_string(), model.to_string(), None));
    }
    match load_topic_model_choice(state).await {
        TopicModelChoice::Local { base_url, model } => {
            Some(("llama".into(), model, Some(base_url)))
        }
        TopicModelChoice::Provider { provider_id, model } => Some((provider_id, model, None)),
        TopicModelChoice::Strategy => None,
    }
}

fn parse_speakers(raw: &str, members: &[GroupMember], max_speakers: usize) -> Option<Vec<i64>> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(&raw[start..=end]).ok()?;
    let ids = value.get("speakers")?.as_array()?;
    let allowed: Vec<i64> = members.iter().map(|m| m.persona_id).collect();
    let mut out = Vec::new();
    for id in ids {
        let Some(id) = id.as_i64() else {
            continue;
        };
        if allowed.contains(&id) && !out.contains(&id) {
            out.push(id);
        }
        if out.len() >= max_speakers {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

async fn speaker_window(
    state: &AppState,
    session_id: &str,
    role_note: &str,
) -> Result<Vec<Message>, String> {
    let db = state.db.clone();
    let sid = session_id.to_string();
    let (summary, recent) = call_blocking(db, move |db| {
        let session = db.get_group_session(&sid)?;
        let summary = session.map(|s| s.summary).unwrap_or_default();
        let recent = db.recent_group_messages(&sid, GROUP_WINDOW)?;
        Ok((summary, recent))
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    if !role_note.trim().is_empty() || !summary.trim().is_empty() {
        messages.push(Message {
            role: "user".into(),
            content: MessageContent::Text(format!(
                "Group context.\nRole: {}\nSummary: {}",
                role_note.chars().take(500).collect::<String>(),
                summary.chars().take(2000).collect::<String>()
            )),
        });
    }
    for item in recent {
        messages.push(message_from_group(state, &item));
    }
    Ok(messages)
}

fn message_from_group(state: &AppState, item: &GroupMessage) -> Message {
    let text = state.env_redactor.redact(&item.content);
    let clipped: String = text.chars().take(2000).collect();
    if let Some(persona_id) = item.speaker_persona_id {
        Message {
            role: "assistant".into(),
            content: MessageContent::Text(format!("[persona {persona_id}] {clipped}")),
        }
    } else {
        Message {
            role: "user".into(),
            content: MessageContent::Text(clipped),
        }
    }
}

async fn speak(
    state: &AppState,
    chat_id: i64,
    persona_id: i64,
    history: Vec<Message>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let result = process_with_agent_with_events(
        state,
        AgentRequestContext {
            caller_channel: "web",
            chat_id,
            chat_type: "private",
            persona_id,
            is_scheduled_task: false,
            is_background_job: false,
            run_key: Some(format!("group:{}", uuid::Uuid::new_v4())),
            reply_bot_instance_id: None,
            session_id: None,
            history_override: Some(history),
            tool_iteration_cap: Some(GROUP_TOOL_CAP),
            skip_recall: true,
        },
        None,
        crate::telegram::InboundImages::new(),
        None,
        Some(cancel),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(state.env_redactor.redact(&result.response))
}

async fn maybe_refresh_summary(state: &AppState, session_id: &str) {
    let db = state.db.clone();
    let sid = session_id.to_string();
    let info = call_blocking(db, move |db| {
        let session = db.get_group_session(&sid)?;
        let Some(session) = session else {
            return Ok(None);
        };
        let count = db.group_messages_since_summary(&sid, session.summary_upto_id.as_deref())?;
        let recent = db.recent_group_messages(&sid, 30)?;
        Ok(Some((count, session.summary, recent)))
    })
    .await;
    let Ok(Some((count, previous, recent))) = info else {
        return;
    };
    if count < GROUP_SUMMARY_EVERY || recent.is_empty() {
        return;
    }
    let mut body = format!("Previous summary:\n{previous}\n\nNew lines:\n");
    for item in &recent {
        let who = item
            .speaker_persona_id
            .map(|id| format!("persona {id}"))
            .unwrap_or_else(|| "user".into());
        body.push_str(&who);
        body.push_str(": ");
        body.push_str(&item.content.chars().take(400).collect::<String>());
        body.push('\n');
    }
    let choice = resolve_choice(state, "", "").await;
    let Some((provider, model, base)) = choice else {
        return;
    };
    let Ok(response) = send_topic_prompt(
        state,
        &provider,
        &model,
        base.as_deref(),
        "Summarize this group session in under 150 words. Plain text.",
        vec![Message {
            role: "user".into(),
            content: MessageContent::Text(body),
        }],
    )
    .await
    else {
        return;
    };
    let summary = response_text(&response);
    let upto = recent.last().map(|m| m.id.clone()).unwrap_or_default();
    let db = state.db.clone();
    let sid = session_id.to_string();
    let _ = call_blocking(db, move |db| db.set_group_summary(&sid, &summary, &upto)).await;
}
