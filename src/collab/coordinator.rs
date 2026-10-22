//! The Coordinator reads a SQL digest. It does not crawl persona transcripts.
//! The daily brief is off until the operator enables it.

use chrono::{Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;

use crate::agent::AppState;
use crate::db::call_blocking;
use crate::db_collab::clip_chars;

use super::{is_coordinator_name, BRIEF_PREFIX};

const DIGEST_CHARS: usize = 4_000;
const BRIEF_CHARS: usize = 6_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenBrief {
    pub text: String,
    pub at: String,
}

pub async fn build_digest(state: &AppState, chat_id: i64) -> Result<String, String> {
    let db = state.db.clone();
    let since = (Utc::now() - Duration::hours(24)).to_rfc3339();
    let since_proposals = (Utc::now() - Duration::days(14)).to_rfc3339();
    let prefix = BRIEF_PREFIX.to_string();
    let bundle = call_blocking(db, move |db| {
        let open = db.list_todos_for_chat(chat_id, Some("open"), 30)?;
        let events = db.list_board_events_since(chat_id, &since, 20)?;
        let runs = db.list_scheduled_runs(chat_id, None, None, None, 15)?;
        let topics = db.latest_topic_lines(chat_id, 12)?;
        let tasks = db.get_tasks_for_chat(chat_id)?;
        let rejected = db.list_proposals(chat_id, Some("rejected"), Some(&since_proposals), 10)?;
        let previous = db.previous_brief_preview(chat_id, &prefix)?;
        Ok((open, events, runs, topics, tasks, rejected, previous))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (open, events, runs, topics, tasks, rejected, previous) = bundle;
    let mut out = String::from("Digest:\n");
    if let Some(previous) = previous {
        out.push_str("Previous brief: ");
        out.push_str(&clip_chars(&previous, 400));
        out.push('\n');
    }
    out.push_str("Open board:\n");
    for item in open.iter().take(20) {
        let owner = item.owner_persona_id.unwrap_or(item.persona_id);
        out.push_str(&format!(
            "- #{} [{}] owner {} due {} {}\n",
            item.id,
            item.status,
            owner,
            item.due_at.as_deref().unwrap_or("-"),
            clip_chars(&item.title, 120)
        ));
    }
    out.push_str("Board changes (24h):\n");
    for event in events.iter().take(15) {
        out.push_str(&format!(
            "- #{} {} -> {} ({})\n",
            event.item_id,
            event.from_status.as_deref().unwrap_or("-"),
            event.to_status,
            event.actor_kind
        ));
    }
    out.push_str("Scheduled runs (24h):\n");
    for run in runs.iter().take(10) {
        out.push_str(&format!(
            "- run #{} task #{} {} {}\n",
            run.id,
            run.task_id,
            run.status,
            clip_chars(&run.preview, 120)
        ));
    }
    out.push_str("Upcoming schedules:\n");
    for task in tasks.iter().filter(|t| t.status == "active").take(12) {
        out.push_str(&format!(
            "- #{} persona {} next {} {}\n",
            task.id,
            task.persona_id,
            task.next_run,
            clip_chars(&task.prompt, 80)
        ));
    }
    out.push_str("Latest topics:\n");
    for (name, title, summary) in topics.iter().take(8) {
        out.push_str(&format!(
            "- {name}: {} — {}\n",
            clip_chars(title, 80),
            clip_chars(summary, 140)
        ));
    }
    if !rejected.is_empty() {
        out.push_str("Rejected proposals (14d), do not repeat:\n");
        for proposal in rejected.iter().take(8) {
            out.push_str("- ");
            out.push_str(&clip_chars(&proposal.change_json, 160));
            out.push('\n');
        }
    }
    Ok(clip_chars(&out, DIGEST_CHARS))
}

/// Latest stored daily brief, if a brief job has already run. The board overview does not read a persona chat.
pub async fn latest_written_brief(
    state: &AppState,
    chat_id: i64,
) -> Result<Option<WrittenBrief>, String> {
    let db = state.db.clone();
    let prefix = BRIEF_PREFIX.to_string();
    call_blocking(db, move |db| {
        let run = db.latest_brief_run(chat_id, &prefix)?.and_then(|row| {
            let at = row.finished_at.clone();
            let body = load_brief_body(&row);
            brief_from_text(&body, &at)
        });
        Ok(run)
    })
    .await
    .map_err(|e| e.to_string())
}

fn load_brief_body(row: &crate::db_collab::ScheduledRunRow) -> String {
    let body = crate::db_collab::load_run_body(row);
    if body.trim().is_empty() {
        row.error.clone().unwrap_or_default()
    } else {
        body
    }
}

fn brief_from_text(text: &str, at: &str) -> Option<WrittenBrief> {
    let text = strip_scheduled_header(text);
    let text = clip_chars(text.trim(), BRIEF_CHARS);
    if text.trim().is_empty() {
        None
    } else {
        Some(WrittenBrief {
            text,
            at: at.to_string(),
        })
    }
}

fn strip_scheduled_header(text: &str) -> String {
    let rest = text.trim_start();
    let Some(rest) = rest.strip_prefix("[Scheduled task #") else {
        return text.trim().to_string();
    };
    match rest.find('\n') {
        Some(idx) => rest[idx + 1..].trim().to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
fn pick_brief(chat: Option<WrittenBrief>, run: Option<WrittenBrief>) -> Option<WrittenBrief> {
    match (chat, run) {
        (Some(chat), Some(run)) if chat.at > run.at => Some(chat),
        (Some(chat), None) => Some(chat),
        (_, run) => run,
    }
}

pub fn prompt_is_brief(prompt: &str) -> bool {
    prompt.starts_with(BRIEF_PREFIX)
}

pub async fn persona_is_coordinator(state: &AppState, chat_id: i64, persona_id: i64) -> bool {
    let db = state.db.clone();
    call_blocking(db, move |db| {
        Ok(db
            .get_persona(persona_id)?
            .filter(|p| p.chat_id == chat_id)
            .map(|p| is_coordinator_name(&p.name))
            .unwrap_or(false))
    })
    .await
    .unwrap_or(false)
}

pub async fn enable(
    state: &AppState,
    chat_id: i64,
    brief_enabled: bool,
    brief_hour: u8,
    timezone: &str,
) -> Result<(i64, Option<i64>), String> {
    let hour = brief_hour.min(23);
    let db = state.db.clone();
    let tz = timezone.to_string();
    call_blocking(db, move |db| {
        let persona_id = match db.coordinator_persona_id(chat_id)? {
            Some(id) => id,
            None => {
                return Err(crate::error::FinallyAValueBotError::Config(
                    "the board overview does not create a Coordinator persona".into(),
                ));
            }
        };
        let task_id = if brief_enabled {
            let cron = format!("0 0 {hour} * * *");
            let next = next_cron(&cron, &tz)?;
            let prompt = format!(
                "{BRIEF_PREFIX} Write my day: what changed, what is due, what is blocked, and which schedules failed. Suggest board moves as proposals; do not change the board yourself."
            );
            let existing = db.list_task_ids_by_prompt_prefix(chat_id, BRIEF_PREFIX, "active")?;
            if let Some(id) = existing.first().copied() {
                Some(id)
            } else {
                Some(db.create_scheduled_task_for_persona(
                    chat_id,
                    persona_id,
                    &prompt,
                    "cron",
                    &cron,
                    &next,
                    "runs_only",
                )?)
            }
        } else {
            None
        };
        Ok((persona_id, task_id))
    })
    .await
    .map_err(|e| e.to_string())
}

fn next_cron(expr: &str, tz_name: &str) -> Result<String, crate::error::FinallyAValueBotError> {
    let tz: Tz = tz_name.parse().unwrap_or(chrono_tz::Tz::UTC);
    let schedule = Schedule::from_str(expr).map_err(|e| {
        crate::error::FinallyAValueBotError::Config(format!("invalid brief cron: {e}"))
    })?;
    let next = schedule
        .upcoming(tz)
        .next()
        .ok_or_else(|| crate::error::FinallyAValueBotError::Config("no next brief run".into()))?;
    Ok(next.with_timezone(&Utc).to_rfc3339())
}

pub fn day_title(timezone: &str) -> String {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::Tz::UTC);
    let day = Utc::now().with_timezone(&tz).format("%Y-%m-%d");
    format!("Coordinator {day}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(text: &str, at: &str) -> WrittenBrief {
        WrittenBrief {
            text: text.into(),
            at: at.into(),
        }
    }

    #[test]
    fn newer_chat_replaces_older_brief() {
        let picked = pick_brief(
            Some(brief("chat", "2026-10-20T18:00:00+00:00")),
            Some(brief("run", "2026-10-20T15:00:00+00:00")),
        );
        assert_eq!(picked.unwrap().text, "chat");
    }

    #[test]
    fn older_chat_keeps_the_brief() {
        let picked = pick_brief(
            Some(brief("chat", "2026-10-20T12:00:00+00:00")),
            Some(brief("run", "2026-10-20T15:00:00+00:00")),
        );
        assert_eq!(picked.unwrap().text, "run");
    }

    #[test]
    fn scheduled_header_is_not_the_overview() {
        let text = strip_scheduled_header("[Scheduled task #3 run #9]\nWhat changed today.");
        assert_eq!(text, "What changed today.");
    }
}
