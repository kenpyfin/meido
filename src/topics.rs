//! Incremental conversation topics.
//!
//! Messages stay in timestamp order. This module names contiguous spans and
//! never rewrites a closed topic or a topic the user has edited. Model output
//! that fails validation is dropped; the previous rows stay as they were.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tracing::warn;

use crate::agent::AppState;
use crate::claude::{Message, MessageContent, MessagesResponse, ResponseContentBlock};
use crate::db::{
    call_blocking, pending_section_setting_key, ConversationTopic, ConversationTopicDraft,
    Database, TopicScope, TopicWindowCursor, TopicWindowMessage,
};
use crate::error::FinallyAValueBotError;

pub const APP_SETTING_TOPICS_MODEL_SOURCE: &str = "TOPICS_MODEL_SOURCE";
pub const APP_SETTING_TOPICS_MODEL: &str = "TOPICS_MODEL";

const WINDOW_LIMIT: usize = 40;
const BATCHES_PER_RUN: usize = 3;
const PROMPT_CHARS: usize = 240;
const RECENT_SCOPE_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicTrigger {
    Scheduled,
    Lazy,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentStatus {
    Updated,
    Unchanged,
    FailedClosed,
    SkippedBusy,
    SkippedInFlight,
    SkippedDebounced,
    SkippedNothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicBoundary {
    pub after_message_id: String,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPlan {
    pub boundaries: Vec<TopicBoundary>,
    pub open_title: String,
    pub open_summary: String,
}

#[derive(Debug, Clone)]
struct SegmentMessage {
    id: String,
    timestamp: String,
    role: String,
    text: String,
}

struct Gate {
    in_flight: std::collections::HashSet<String>,
    last_attempt: std::collections::HashMap<String, Instant>,
}

fn gate() -> &'static Mutex<Gate> {
    static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
    GATE.get_or_init(|| {
        Mutex::new(Gate {
            in_flight: std::collections::HashSet::new(),
            last_attempt: std::collections::HashMap::new(),
        })
    })
}

fn scope_key(scope: &TopicScope) -> String {
    format!(
        "{}:{}:{}",
        scope.chat_id,
        scope.persona_id,
        scope.session_id.as_deref().unwrap_or("")
    )
}

pub fn scope_in_flight(scope: &TopicScope) -> bool {
    gate()
        .lock()
        .map(|gate| gate.in_flight.contains(&scope_key(scope)))
        .unwrap_or(false)
}

enum Enter {
    Started,
    InFlight,
    Debounced,
}

fn try_enter(scope: &TopicScope, trigger: TopicTrigger, debounce: Duration) -> Enter {
    let key = scope_key(scope);
    let Ok(mut gate) = gate().lock() else {
        return Enter::InFlight;
    };
    if gate.in_flight.contains(&key) {
        return Enter::InFlight;
    }
    if trigger != TopicTrigger::Manual {
        if let Some(at) = gate.last_attempt.get(&key) {
            if at.elapsed() < debounce {
                return Enter::Debounced;
            }
        }
    }
    gate.in_flight.insert(key);
    Enter::Started
}

fn finish(scope: &TopicScope) {
    let key = scope_key(scope);
    if let Ok(mut gate) = gate().lock() {
        gate.in_flight.remove(&key);
        gate.last_attempt.insert(key, Instant::now());
    }
}

pub async fn segment_due_tick(state: &std::sync::Arc<AppState>) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::Release);
        }
    }
    let _clear = Clear;

    let since = (Utc::now() - chrono::Duration::days(RECENT_SCOPE_DAYS)).to_rfc3339();
    let db = state.db.clone();
    let scopes = match call_blocking(db, move |db| db.list_recent_topic_scopes(&since, 20)).await {
        Ok(scopes) => scopes,
        Err(e) => {
            warn!("topic scope scan failed: {e}");
            return;
        }
    };
    for scope in scopes {
        if persona_has_active_work(state, scope.chat_id, scope.persona_id).await {
            continue;
        }
        match segment_scope(state, &scope, TopicTrigger::Scheduled).await {
            Ok(
                SegmentStatus::SkippedDebounced
                | SegmentStatus::SkippedNothing
                | SegmentStatus::SkippedInFlight
                | SegmentStatus::SkippedBusy,
            ) => continue,
            Ok(SegmentStatus::Updated | SegmentStatus::Unchanged) => break,
            Ok(SegmentStatus::FailedClosed) => break,
            Err(e) => {
                warn!(
                    chat_id = scope.chat_id,
                    persona_id = scope.persona_id,
                    "topic segmentation failed: {e}"
                );
                break;
            }
        }
    }
}

pub async fn segment_scope(
    state: &std::sync::Arc<AppState>,
    scope: &TopicScope,
    trigger: TopicTrigger,
) -> Result<SegmentStatus, FinallyAValueBotError> {
    if persona_has_active_work(state, scope.chat_id, scope.persona_id).await {
        return Ok(SegmentStatus::SkippedBusy);
    }
    let debounce = Duration::from_secs(state.config.topics_debounce_minutes.saturating_mul(60));
    match try_enter(scope, trigger, debounce) {
        Enter::InFlight => return Ok(SegmentStatus::SkippedInFlight),
        Enter::Debounced => return Ok(SegmentStatus::SkippedDebounced),
        Enter::Started => {}
    }
    let _flight = ScopeFlight {
        scope: scope.clone(),
    };
    segment_scope_locked(state, scope, trigger).await
}

struct ScopeFlight {
    scope: TopicScope,
}

impl Drop for ScopeFlight {
    fn drop(&mut self) {
        finish(&self.scope);
    }
}

async fn segment_scope_locked(
    state: &std::sync::Arc<AppState>,
    scope: &TopicScope,
    trigger: TopicTrigger,
) -> Result<SegmentStatus, FinallyAValueBotError> {
    let mut status = SegmentStatus::SkippedNothing;
    let mut drain = false;
    for _ in 0..BATCHES_PER_RUN {
        let once = segment_once(state, scope, trigger, drain).await?;
        match once {
            None => break,
            Some(OnceOutcome::Updated) => {
                status = SegmentStatus::Updated;
                drain = true;
            }
            Some(OnceOutcome::Unchanged) => {
                if status != SegmentStatus::Updated {
                    status = SegmentStatus::Unchanged;
                }
                break;
            }
            Some(OnceOutcome::FailedClosed) => {
                status = SegmentStatus::FailedClosed;
                break;
            }
        }
    }
    Ok(status)
}

enum OnceOutcome {
    Updated,
    Unchanged,
    FailedClosed,
}

async fn segment_once(
    state: &std::sync::Arc<AppState>,
    scope: &TopicScope,
    trigger: TopicTrigger,
    drain: bool,
) -> Result<Option<OnceOutcome>, FinallyAValueBotError> {
    let loaded = load_window(state, scope).await?;
    let Some(window) = loaded else {
        return Ok(None);
    };
    if !should_segment(&window, state, trigger, drain) {
        return Ok(None);
    }
    let plan = if state.config.topics_enabled {
        match request_plan(state, scope, &window.messages).await {
            Ok(plan) => plan,
            Err(reason) => {
                warn!(
                    chat_id = scope.chat_id,
                    persona_id = scope.persona_id,
                    messages = window.messages.len(),
                    "topic segmentation kept previous topics: {reason}"
                );
                return Ok(Some(OnceOutcome::FailedClosed));
            }
        }
    } else {
        heuristic_plan(&window.messages, state.config.topics_gap_hours)
    };
    let ids: Vec<String> = window.messages.iter().map(|m| m.id.clone()).collect();
    if let Err(reason) = validate_segment_plan(&plan, &ids) {
        warn!(
            chat_id = scope.chat_id,
            persona_id = scope.persona_id,
            messages = window.messages.len(),
            "topic segmentation kept previous topics: {reason}"
        );
        return Ok(Some(OnceOutcome::FailedClosed));
    }
    let drafts = match drafts_from_plan(&window.messages, &plan) {
        Ok(drafts) => drafts,
        Err(reason) => {
            warn!(
                chat_id = scope.chat_id,
                persona_id = scope.persona_id,
                "topic segmentation kept previous topics: {reason}"
            );
            return Ok(Some(OnceOutcome::FailedClosed));
        }
    };
    let mut drafts = drafts;
    let pending_key = pending_section_setting_key(scope);
    let mut applied_pending = false;
    if window.replace_id.is_none() {
        let db = state.db.clone();
        let key = pending_key.clone();
        let pending = call_blocking(db, move |db| db.get_app_setting(&key)).await?;
        if let Some(title) = pending {
            if let Some(open) = drafts.iter_mut().rev().find(|draft| draft.status == "open") {
                open.title = title;
                applied_pending = true;
            }
        }
    }
    if drafts_match_open(&window, &drafts) {
        return Ok(Some(OnceOutcome::Unchanged));
    }
    let db = state.db.clone();
    let scope = scope.clone();
    let replace = window.replace_id.clone();
    call_blocking(db, move |db| {
        db.commit_auto_segmentation(&scope, replace.as_deref(), &drafts)?;
        if applied_pending {
            db.remove_app_setting(&pending_key)?;
        }
        Ok(())
    })
    .await?;
    Ok(Some(OnceOutcome::Updated))
}

struct LoadedWindow {
    replace_id: Option<String>,
    open_topic: Option<ConversationTopic>,
    messages: Vec<SegmentMessage>,
    unsegmented_after: i64,
    latest_ts: Option<String>,
}

fn should_segment(
    window: &LoadedWindow,
    state: &AppState,
    trigger: TopicTrigger,
    drain: bool,
) -> bool {
    if window.messages.is_empty() {
        return false;
    }
    if drain || trigger == TopicTrigger::Manual {
        return true;
    }
    if trigger == TopicTrigger::Lazy {
        return window.unsegmented_after > 0 || window.open_topic.is_none();
    }
    let min = state.config.topics_min_unsegmented as i64;
    if window.unsegmented_after >= min {
        return true;
    }
    if window.unsegmented_after < 1 {
        return false;
    }
    let Some(latest) = window.latest_ts.as_deref() else {
        return false;
    };
    let Ok(ts) = DateTime::parse_from_rfc3339(latest) else {
        return false;
    };
    let idle = chrono::Duration::minutes(state.config.topics_idle_minutes as i64);
    Utc::now().signed_duration_since(ts) >= idle
}

async fn load_window(
    state: &AppState,
    scope: &TopicScope,
) -> Result<Option<LoadedWindow>, FinallyAValueBotError> {
    let db = state.db.clone();
    let scope = scope.clone();
    call_blocking(db, move |db| load_window_sync(db, &scope)).await
}

fn load_window_sync(
    db: &Database,
    scope: &TopicScope,
) -> Result<Option<LoadedWindow>, FinallyAValueBotError> {
    let topics = db.list_conversation_topics(scope)?;
    let (replace_id, cursor, open_topic) = mutable_tail(&topics);
    let unsegmented_after = match topics.last() {
        Some(last) => db.count_messages_in_topic_scope(
            scope,
            Some((last.end_ts.as_str(), last.end_message_id.as_str())),
        )?,
        None => db.count_messages_in_topic_scope(scope, None)?,
    };
    let latest_ts = db.latest_message_timestamp_in_topic_scope(scope)?;
    let rows = db.list_topic_window_messages(scope, &cursor, WINDOW_LIMIT)?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(LoadedWindow {
        replace_id,
        open_topic,
        messages: rows.into_iter().map(segment_message).collect(),
        unsegmented_after,
        latest_ts,
    }))
}

fn mutable_tail(
    topics: &[ConversationTopic],
) -> (Option<String>, TopicWindowCursor, Option<ConversationTopic>) {
    let Some(last) = topics.last() else {
        return (None, TopicWindowCursor::Beginning, None);
    };
    if last.source == "auto" && last.status == "open" {
        return (
            Some(last.id.clone()),
            TopicWindowCursor::Inclusive {
                timestamp: last.start_ts.clone(),
                id: last.start_message_id.clone(),
            },
            Some(last.clone()),
        );
    }
    (
        None,
        TopicWindowCursor::Exclusive {
            timestamp: last.end_ts.clone(),
            id: last.end_message_id.clone(),
        },
        None,
    )
}

fn segment_message(row: TopicWindowMessage) -> SegmentMessage {
    SegmentMessage {
        id: row.id,
        timestamp: row.timestamp,
        role: if row.is_from_bot {
            "assistant".to_string()
        } else {
            "user".to_string()
        },
        text: prompt_text(&row.content),
    }
}

fn prompt_text(raw: &str) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(PROMPT_CHARS).collect()
}

fn drafts_match_open(window: &LoadedWindow, drafts: &[ConversationTopicDraft]) -> bool {
    let Some(open) = &window.open_topic else {
        return false;
    };
    let Some(only) = drafts.first() else {
        return false;
    };
    drafts.len() == 1
        && only.status == "open"
        && only.start_message_id == open.start_message_id
        && only.end_message_id == open.end_message_id
        && only.title == open.title
        && only.summary == open.summary
        && only.message_count == open.message_count
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicModelChoice {
    /// OpenAI-compatible server from Settings → Connections.
    Local { base_url: String, model: String },
    /// A catalog provider and model, called directly. Not the cost-routing strategy tier.
    Provider { provider_id: String, model: String },
    /// No explicit choice yet: the persona's chat model.
    Strategy,
}

pub struct TopicModelInputs<'a> {
    pub saved_source: Option<&'a str>,
    pub saved_model: Option<&'a str>,
    pub prefer_tier2: bool,
    pub tier2_base_url: &'a str,
    pub tier2_model: &'a str,
    pub tier2_tools_ok: bool,
    pub local_base_url: &'a str,
    pub local_model: &'a str,
}

/// Saved Settings override `TOPICS_PREFER_TIER2`. With no saved choice, a verified
/// Tier 2 endpoint is used when that flag is on.
pub fn resolve_topic_model_choice(input: TopicModelInputs<'_>) -> TopicModelChoice {
    let saved_source = input
        .saved_source
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let local_base = first_nonempty(input.tier2_base_url, input.local_base_url);
    let default_local_model = first_nonempty(input.tier2_model, input.local_model);
    match saved_source.as_deref() {
        Some("strategy") => TopicModelChoice::Strategy,
        Some("local") => local_choice(
            local_base.as_deref().unwrap_or(""),
            input
                .saved_model
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or(default_local_model.as_deref())
                .unwrap_or(""),
        ),
        Some(provider) => catalog_choice(
            provider,
            input.saved_model,
            local_base.as_deref().unwrap_or(""),
            default_local_model.as_deref().unwrap_or(""),
        ),
        None => {
            if input.prefer_tier2 && input.tier2_tools_ok {
                local_choice(input.tier2_base_url.trim(), input.tier2_model.trim())
            } else {
                TopicModelChoice::Strategy
            }
        }
    }
}

fn catalog_choice(
    provider: &str,
    saved_model: Option<&str>,
    local_base_url: &str,
    default_local_model: &str,
) -> TopicModelChoice {
    let provider_id = crate::llm_catalog::resolve_catalog_provider_id(provider);
    if crate::llm_catalog::find_provider(&provider_id).is_none() {
        return TopicModelChoice::Strategy;
    }
    let saved_model = saved_model.map(str::trim).filter(|s| !s.is_empty());
    if crate::llm_catalog::is_local_provider(&provider_id) {
        return local_choice(local_base_url, saved_model.unwrap_or(default_local_model));
    }
    let model =
        saved_model.unwrap_or_else(|| crate::llm_catalog::default_model_for_provider(&provider_id));
    if model.is_empty() {
        return TopicModelChoice::Strategy;
    }
    TopicModelChoice::Provider {
        provider_id,
        model: model.to_string(),
    }
}

fn local_choice(base_url: &str, model: &str) -> TopicModelChoice {
    if base_url.is_empty() || model.is_empty() {
        TopicModelChoice::Strategy
    } else {
        TopicModelChoice::Local {
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }
}

fn first_nonempty(primary: &str, fallback: &str) -> Option<String> {
    let primary = primary.trim();
    if !primary.is_empty() {
        return Some(primary.to_string());
    }
    let fallback = fallback.trim();
    if fallback.is_empty() {
        None
    } else {
        Some(fallback.to_string())
    }
}

pub(crate) async fn load_topic_model_choice(state: &AppState) -> TopicModelChoice {
    let db = state.db.clone();
    let saved = call_blocking(db, |db| {
        Ok((
            db.get_app_setting(APP_SETTING_TOPICS_MODEL_SOURCE)?,
            db.get_app_setting(APP_SETTING_TOPICS_MODEL)?,
        ))
    })
    .await;
    let (saved_source, saved_model) = match saved {
        Ok(pair) => pair,
        Err(_) => {
            warn!("topic model setting could not be read");
            (None, None)
        }
    };
    let mm = state.llm.local_delegate_config();
    resolve_topic_model_choice(TopicModelInputs {
        saved_source: saved_source.as_deref(),
        saved_model: saved_model.as_deref(),
        prefer_tier2: state.config.topics_prefer_tier2,
        tier2_base_url: &mm.tier2_base_url,
        tier2_model: &mm.tier2_model,
        tier2_tools_ok: mm.tier2_tools_ok,
        local_base_url: &mm.local_base_url,
        local_model: &mm.local_model,
    })
}

async fn request_plan(
    state: &AppState,
    scope: &TopicScope,
    messages: &[SegmentMessage],
) -> Result<SegmentPlan, String> {
    let system = "You segment one chat window into topics. Reply with JSON only, no markdown. \
Schema: {\"boundaries\":[{\"after_message_id\":\"string\",\"title\":\"string\",\"summary\":\"string\"}],\"open_title\":\"string\",\"open_summary\":\"string\"}. \
Each boundary ends a topic. after_message_id must be one of the given ids, in increasing order, and must not be the last id. \
Titles are at most 60 characters. Messages after the last boundary are the open topic named by open_title. \
If the window is a single topic, boundaries is empty. Do not invent ids.";
    let mut body = String::from("Messages:\n");
    for message in messages {
        body.push('[');
        body.push_str(&message.id);
        body.push_str("] ");
        body.push_str(&message.role);
        body.push_str(": ");
        body.push_str(&message.text);
        body.push('\n');
    }
    let chat_messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(body),
    }];
    let (provider_id, model, local_base_url) = match load_topic_model_choice(state).await {
        TopicModelChoice::Local { base_url, model } => ("llama".to_string(), model, Some(base_url)),
        TopicModelChoice::Provider { provider_id, model } => (provider_id, model, None),
        TopicModelChoice::Strategy => {
            let db = state.db.clone();
            let persona_id = scope.persona_id;
            let persona = call_blocking(db, move |db| db.get_persona(persona_id))
                .await
                .map_err(|e| e.to_string())?;
            let strategy = state.llm.resolve_strategy_selection(persona.as_ref())?;
            (strategy.provider_id, strategy.model, strategy.base_url)
        }
    };
    let response = send_topic_prompt(
        state,
        &provider_id,
        &model,
        local_base_url.as_deref(),
        system,
        chat_messages,
    )
    .await?;
    parse_segment_response(&response_text(&response))
}

pub(crate) async fn send_topic_prompt(
    state: &AppState,
    provider_id: &str,
    model: &str,
    local_base_url: Option<&str>,
    system: &str,
    messages: Vec<Message>,
) -> Result<MessagesResponse, String> {
    let mut cfg = state.config.clone();
    cfg.apply_llm_provider_switch(provider_id, model, local_base_url);
    if !crate::llm_catalog::is_local_provider(&cfg.llm_provider) && cfg.api_key.trim().is_empty() {
        return Err(format!(
            "No API key for {}. Set it under Settings → Connections.",
            cfg.llm_provider
        ));
    }
    let provider = crate::llm::create_provider(&cfg);
    provider
        .send_message(system, messages, None)
        .await
        .map_err(|_| "topic model request failed".to_string())
}

pub(crate) fn response_text(response: &MessagesResponse) -> String {
    response
        .content
        .iter()
        .filter_map(|block| match block {
            ResponseContentBlock::Text { text } => Some(text.as_str()),
            ResponseContentBlock::ToolUse { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_segment_response(raw: &str) -> Result<SegmentPlan, String> {
    let json = extract_json_object(raw).ok_or_else(|| "invalid topic json".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "invalid topic json".to_string())?;
    let boundaries = value
        .get("boundaries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "invalid topic json".to_string())?;
    let mut parsed = Vec::with_capacity(boundaries.len());
    for boundary in boundaries {
        let after_message_id = boundary
            .get("after_message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if after_message_id.is_empty() {
            return Err("invalid topic json".to_string());
        }
        parsed.push(TopicBoundary {
            after_message_id,
            title: normalize_title(boundary.get("title").and_then(|v| v.as_str()).unwrap_or("")),
            summary: normalize_summary(
                boundary
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            ),
        });
    }
    Ok(SegmentPlan {
        boundaries: parsed,
        open_title: normalize_title(
            value
                .get("open_title")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        ),
        open_summary: normalize_summary(
            value
                .get("open_summary")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        ),
    })
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&raw[start..=end])
}

pub fn validate_segment_plan(plan: &SegmentPlan, window_ids: &[String]) -> Result<(), String> {
    if window_ids.is_empty() {
        return Err("empty topic window".to_string());
    }
    let mut last = None;
    for boundary in &plan.boundaries {
        let idx = window_ids
            .iter()
            .position(|id| id == &boundary.after_message_id)
            .ok_or_else(|| "topic boundary is outside the window".to_string())?;
        if idx + 1 >= window_ids.len() {
            return Err("topic boundary leaves no open topic".to_string());
        }
        if let Some(prev) = last {
            if idx <= prev {
                return Err("topic boundaries are out of order".to_string());
            }
        }
        last = Some(idx);
    }
    Ok(())
}

fn drafts_from_plan(
    messages: &[SegmentMessage],
    plan: &SegmentPlan,
) -> Result<Vec<ConversationTopicDraft>, String> {
    if messages.is_empty() {
        return Err("empty topic window".to_string());
    }
    let mut cursor = 0usize;
    let mut drafts = Vec::new();
    for boundary in &plan.boundaries {
        let idx = messages
            .iter()
            .position(|m| m.id == boundary.after_message_id)
            .ok_or_else(|| "topic boundary is outside the window".to_string())?;
        if idx < cursor || idx + 1 >= messages.len() {
            return Err("topic boundary is outside the window".to_string());
        }
        drafts.push(draft_for(
            &messages[cursor..=idx],
            &boundary.title,
            &boundary.summary,
            "closed",
        )?);
        cursor = idx + 1;
    }
    if cursor >= messages.len() {
        return Err("topic boundary leaves no open topic".to_string());
    }
    drafts.push(draft_for(
        &messages[cursor..],
        &plan.open_title,
        &plan.open_summary,
        "open",
    )?);
    Ok(drafts)
}

fn draft_for(
    messages: &[SegmentMessage],
    title: &str,
    summary: &str,
    status: &str,
) -> Result<ConversationTopicDraft, String> {
    let Some(first) = messages.first() else {
        return Err("empty topic segment".to_string());
    };
    let Some(last) = messages.last() else {
        return Err("empty topic segment".to_string());
    };
    Ok(ConversationTopicDraft {
        title: normalize_title(title),
        summary: normalize_summary(summary),
        start_message_id: first.id.clone(),
        end_message_id: last.id.clone(),
        start_ts: first.timestamp.clone(),
        end_ts: last.timestamp.clone(),
        message_count: messages.len() as i64,
        status: status.to_string(),
    })
}

fn heuristic_plan(messages: &[SegmentMessage], gap_hours: u64) -> SegmentPlan {
    let mut boundaries = Vec::new();
    let mut segment_start = 0usize;
    if gap_hours > 0 {
        for index in 0..messages.len().saturating_sub(1) {
            if gap_between(
                &messages[index].timestamp,
                &messages[index + 1].timestamp,
                gap_hours as i64,
            ) {
                boundaries.push(TopicBoundary {
                    after_message_id: messages[index].id.clone(),
                    title: date_title(&messages[segment_start].timestamp),
                    summary: String::new(),
                });
                segment_start = index + 1;
            }
        }
    }
    let open_title = messages
        .get(segment_start)
        .map(|m| date_title(&m.timestamp))
        .unwrap_or_else(|| "Untitled".to_string());
    SegmentPlan {
        boundaries,
        open_title,
        open_summary: String::new(),
    }
}

fn gap_between(earlier: &str, later: &str, hours: i64) -> bool {
    let Ok(start) = DateTime::parse_from_rfc3339(earlier) else {
        return false;
    };
    let Ok(end) = DateTime::parse_from_rfc3339(later) else {
        return false;
    };
    end.signed_duration_since(start) >= chrono::Duration::hours(hours)
}

fn date_title(timestamp: &str) -> String {
    let title: String = timestamp.chars().take(10).collect();
    normalize_title(&title)
}

fn normalize_title(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = collapsed.chars().take(60).collect();
    if title.is_empty() {
        "Untitled".to_string()
    } else {
        title
    }
}

fn normalize_summary(raw: &str) -> String {
    raw.trim().chars().take(280).collect()
}

async fn persona_has_active_work(state: &AppState, cid: i64, pid: i64) -> bool {
    if state.chat_queue.diagnostics().await.iter().any(|lane| {
        lane.chat_id == cid
            && lane.persona_id == pid
            && (lane.active_for_ms > 0 || lane.pending > 0)
    }) {
        return true;
    }
    call_blocking(state.db.clone(), move |db| {
        db.has_unfinished_background_jobs_for_chat_persona(cid, pid)
    })
    .await
    .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, ts: &str) -> SegmentMessage {
        SegmentMessage {
            id: id.to_string(),
            timestamp: ts.to_string(),
            role: "user".to_string(),
            text: format!("text {id}"),
        }
    }

    #[test]
    fn parses_fenced_json_and_rejects_unknown_ids() {
        let raw = "```json\n{\"boundaries\":[{\"after_message_id\":\"m1\",\"title\":\"Deploy logs\",\"summary\":\"shipped\"}],\"open_title\":\"Follow up\",\"open_summary\":\"\"}\n```";
        let plan = parse_segment_response(raw).unwrap();
        assert_eq!(plan.boundaries.len(), 1);
        assert_eq!(plan.open_title, "Follow up");
        let ids = vec!["m1".to_string(), "m2".to_string()];
        assert!(validate_segment_plan(&plan, &ids).is_ok());
        let missing = vec!["m2".to_string(), "m3".to_string()];
        assert!(validate_segment_plan(&plan, &missing).is_err());
    }

    #[test]
    fn rejects_boundary_on_the_last_message_and_out_of_order() {
        let ids = vec!["a".into(), "b".into(), "c".into()];
        let last = SegmentPlan {
            boundaries: vec![TopicBoundary {
                after_message_id: "c".into(),
                title: "End".into(),
                summary: String::new(),
            }],
            open_title: "Open".into(),
            open_summary: String::new(),
        };
        assert!(validate_segment_plan(&last, &ids).is_err());
        let unordered = SegmentPlan {
            boundaries: vec![
                TopicBoundary {
                    after_message_id: "b".into(),
                    title: "Later".into(),
                    summary: String::new(),
                },
                TopicBoundary {
                    after_message_id: "a".into(),
                    title: "Earlier".into(),
                    summary: String::new(),
                },
            ],
            open_title: "Open".into(),
            open_summary: String::new(),
        };
        assert!(validate_segment_plan(&unordered, &ids).is_err());
    }

    #[test]
    fn drafts_cover_the_window_without_overlap() {
        let messages = vec![
            msg("a", "2026-01-01T00:00:00Z"),
            msg("b", "2026-01-01T00:01:00Z"),
            msg("c", "2026-01-01T00:02:00Z"),
            msg("d", "2026-01-01T00:03:00Z"),
        ];
        let plan = SegmentPlan {
            boundaries: vec![TopicBoundary {
                after_message_id: "b".into(),
                title: "Setup".into(),
                summary: "start".into(),
            }],
            open_title: "Review".into(),
            open_summary: String::new(),
        };
        let drafts = drafts_from_plan(&messages, &plan).unwrap();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].start_message_id, "a");
        assert_eq!(drafts[0].end_message_id, "b");
        assert_eq!(drafts[0].status, "closed");
        assert_eq!(drafts[0].message_count, 2);
        assert_eq!(drafts[1].start_message_id, "c");
        assert_eq!(drafts[1].end_message_id, "d");
        assert_eq!(drafts[1].status, "open");
        assert_eq!(drafts[1].title, "Review");
    }

    #[test]
    fn heuristic_splits_on_a_long_gap_and_titles_by_date() {
        let messages = vec![
            msg("a", "2026-01-01T00:00:00Z"),
            msg("b", "2026-01-01T01:00:00Z"),
            msg("c", "2026-01-02T12:00:00Z"),
        ];
        let plan = heuristic_plan(&messages, 6);
        assert_eq!(plan.boundaries.len(), 1);
        assert_eq!(plan.boundaries[0].after_message_id, "b");
        assert_eq!(plan.boundaries[0].title, "2026-01-01");
        assert_eq!(plan.open_title, "2026-01-02");
        let drafts = drafts_from_plan(&messages, &plan).unwrap();
        assert_eq!(drafts[0].message_count, 2);
        assert_eq!(drafts[1].start_message_id, "c");
        assert_eq!(drafts[1].status, "open");
    }

    #[test]
    fn invalid_json_fails_closed() {
        assert!(parse_segment_response("not json").is_err());
        assert!(parse_segment_response("{\"boundaries\":[]}").is_ok());
    }

    #[test]
    fn topic_model_defaults_to_verified_tier2() {
        let choice = resolve_topic_model_choice(TopicModelInputs {
            saved_source: None,
            saved_model: None,
            prefer_tier2: true,
            tier2_base_url: "http://127.0.0.1:8080/v1",
            tier2_model: "qwen2.5-coder:14b",
            tier2_tools_ok: true,
            local_base_url: "http://127.0.0.1:8080/v1",
            local_model: "qwen3-32b",
        });
        assert_eq!(
            choice,
            TopicModelChoice::Local {
                base_url: "http://127.0.0.1:8080/v1".into(),
                model: "qwen2.5-coder:14b".into(),
            }
        );
    }

    #[test]
    fn saved_topic_model_overrides_the_env_default() {
        let strategy = resolve_topic_model_choice(TopicModelInputs {
            saved_source: Some("strategy"),
            saved_model: None,
            prefer_tier2: true,
            tier2_base_url: "http://127.0.0.1:8080/v1",
            tier2_model: "qwen2.5-coder:14b",
            tier2_tools_ok: true,
            local_base_url: "",
            local_model: "",
        });
        assert_eq!(strategy, TopicModelChoice::Strategy);

        let picked = resolve_topic_model_choice(TopicModelInputs {
            saved_source: Some("local"),
            saved_model: Some("qwen3-32b"),
            prefer_tier2: false,
            tier2_base_url: "http://127.0.0.1:8080/v1",
            tier2_model: "qwen2.5-coder:14b",
            tier2_tools_ok: false,
            local_base_url: "",
            local_model: "",
        });
        assert_eq!(
            picked,
            TopicModelChoice::Local {
                base_url: "http://127.0.0.1:8080/v1".into(),
                model: "qwen3-32b".into(),
            }
        );

        let named = resolve_topic_model_choice(TopicModelInputs {
            saved_source: Some("anthropic"),
            saved_model: Some("claude-sonnet-4-5"),
            prefer_tier2: true,
            tier2_base_url: "http://127.0.0.1:8080/v1",
            tier2_model: "qwen2.5-coder:14b",
            tier2_tools_ok: true,
            local_base_url: "",
            local_model: "",
        });
        assert_eq!(
            named,
            TopicModelChoice::Provider {
                provider_id: "anthropic".into(),
                model: "claude-sonnet-4-5".into(),
            }
        );
    }
}
