//! One recall path for historical dialogue and ORIGIN knowledge.
//!
//! SQLite messages, ORIGIN files, and `memory_state.json` stay authoritative.
//! MemPalace is a rebuildable index behind the light MCP server. Recent turns
//! are not searched here, and file location stays in `locate_file`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::config::{Config, MemPalaceConfig};
use crate::db::{Database, RecallIngestRow, StoredMessage};
use crate::mcp::{McpServer, McpServerConfig};
use crate::tools::search_vault::{query_snippets, SearchVaultMode};

pub const RECALL_RECONCILE_PREFIX: &str = "[[recall_reconcile]]";
pub const RECALL_RECONCILE_CRON: &str = "0 30 */6 * * *";
pub const RECALL_INDEX_PAUSE_KEY: &str = "recall_paused_origin_index_task_ids";
pub const RECALL_RECONCILE_PAUSE_KEY: &str = "recall_reconcile_paused_by_mode";

const INTERACTIONS_WING: &str = "interactions";
const KNOWLEDGE_WING: &str = "origin";
const ALLOWED_PALACE_TOOLS: &[&str] = &["palace_query", "palace_exec", "palace_coordinate"];
const MAX_QUERY_CHARS: usize = 200;
const MAX_DRAWER_CHARS: usize = 8_000;
const MAX_PAYLOAD_CHARS: usize = 16_000;
const MAX_EXCERPT_CHARS: usize = 400;
const MAX_INGEST_ATTEMPTS: i64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecallMode {
    Disabled,
    Shadow,
    Selected,
    Default,
}

impl RecallMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "shadow" => Self::Shadow,
            "selected" => Self::Selected,
            "default" => Self::Default,
            _ => Self::Disabled,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Shadow => "shadow",
            Self::Selected => "selected",
            Self::Default => "default",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecallScope {
    Auto,
    Interactions,
    Knowledge,
}

impl RecallScope {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "interactions" | "history" => Self::Interactions,
            "knowledge" | "vault" => Self::Knowledge,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Interactions => "interactions",
            Self::Knowledge => "knowledge",
        }
    }

    fn wants_interactions(self) -> bool {
        matches!(self, Self::Auto | Self::Interactions)
    }

    fn wants_knowledge(self) -> bool {
        matches!(self, Self::Auto | Self::Knowledge)
    }
}

#[derive(Clone, Debug)]
pub struct RecallHit {
    pub source_id: String,
    pub namespace: String,
    pub timestamp: String,
    pub score: f32,
    pub excerpt: String,
    pub provenance: String,
}

#[derive(Clone, Debug)]
pub struct RecallRequest {
    pub query: String,
    pub scope: RecallScope,
    pub chat_id: i64,
    pub persona_id: i64,
    pub limit: usize,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RecallResponse {
    pub hits: Vec<RecallHit>,
    pub degraded: bool,
    pub failure_class: String,
    pub freshness: String,
}

impl RecallResponse {
    pub fn to_json(&self) -> Value {
        json!({
            "hits": self.hits.iter().map(|hit| json!({
                "source_id": hit.source_id,
                "namespace": hit.namespace,
                "timestamp": hit.timestamp,
                "score": hit.score,
                "excerpt": hit.excerpt,
                "provenance": hit.provenance,
            })).collect::<Vec<_>>(),
            "degraded": self.degraded,
            "failure_class": self.failure_class,
            "freshness": self.freshness,
        })
    }
}

pub fn is_reconcile_prompt(prompt: &str) -> bool {
    prompt.trim_start().starts_with(RECALL_RECONCILE_PREFIX)
}

pub fn tool_allowed(name: &str) -> bool {
    ALLOWED_PALACE_TOOLS.contains(&name)
}

pub fn next_backoff_secs(failures: u32) -> u64 {
    1_u64.checked_shl(failures.min(6)).unwrap_or(60).min(60)
}

pub fn sanitize_fts_query(raw: &str) -> String {
    let mut tokens = Vec::new();
    for raw_token in raw.split_whitespace() {
        let token = raw_token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
        if token.is_empty() {
            continue;
        }
        let escaped = token.replace('"', "");
        if escaped.is_empty() {
            continue;
        }
        tokens.push(format!("\"{escaped}\""));
        if tokens.len() >= 12 {
            break;
        }
    }
    tokens.join(" ")
}

pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn interaction_room(chat_id: i64, persona_id: i64) -> String {
    format!("c{chat_id}-p{persona_id}")
}

pub fn compare_source_ids(legacy: &[RecallHit], palace: &[RecallHit]) -> (usize, usize, usize) {
    let palace_ids: std::collections::HashSet<&str> =
        palace.iter().map(|hit| hit.source_id.as_str()).collect();
    let overlap = legacy
        .iter()
        .filter(|hit| palace_ids.contains(hit.source_id.as_str()))
        .count();
    (legacy.len(), palace.len(), overlap)
}

fn classify_palace_error(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("backoff") {
        "backoff"
    } else if lower.contains("rejected") {
        "rejected"
    } else {
        "unavailable"
    }
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

struct DrawerHeader {
    kind: String,
    source_id: String,
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    timestamp: String,
}

fn parse_header(content: &str) -> Option<DrawerHeader> {
    let (head, _) = content.split_once("\n---\n")?;
    let mut kind = String::new();
    let mut source_id = String::new();
    let mut chat_id = None;
    let mut persona_id = None;
    let mut timestamp = String::new();
    for line in head.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "source_kind" => kind = value.to_string(),
            "source_id" => source_id = value.to_string(),
            "chat_id" => chat_id = value.parse().ok(),
            "persona_id" => persona_id = value.parse().ok(),
            "timestamp" => timestamp = value.to_string(),
            _ => {}
        }
    }
    if kind.is_empty() {
        return None;
    }
    Some(DrawerHeader {
        kind,
        source_id,
        chat_id,
        persona_id,
        timestamp,
    })
}

fn excerpt_of(content: &str) -> String {
    let body = content
        .split_once("\n---\n")
        .map(|(_, body)| body)
        .unwrap_or(content);
    clip_chars(body.trim(), MAX_EXCERPT_CHARS)
}

fn safe_relative_source(source: &str) -> bool {
    !source.is_empty()
        && !source.starts_with('/')
        && !source.starts_with('\\')
        && !source.contains("..")
        && !source.starts_with("message:")
}

/// Drop hits that do not belong to the caller. Interaction hits must match the
/// persona room or an embedded canonical header. Knowledge hits must name an
/// ORIGIN-relative path.
pub fn filter_palace_hit(
    item: &Value,
    scope: RecallScope,
    chat_id: i64,
    persona_id: i64,
) -> Option<RecallHit> {
    let content = item
        .get("content")
        .or_else(|| item.get("text"))
        .or_else(|| item.get("document"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let header = parse_header(content);
    let wing = item.get("wing").and_then(|value| value.as_str());
    let room = item.get("room").and_then(|value| value.as_str());
    let source_file = item
        .get("source_file")
        .or_else(|| item.get("source"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let expected_room = interaction_room(chat_id, persona_id);
    let distance = item
        .get("distance")
        .and_then(|value| value.as_f64())
        .or_else(|| {
            item.get("similarity")
                .and_then(|value| value.as_f64())
                .map(|similarity| (1.0 - similarity).clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);
    let score = (1.0 - distance).clamp(0.0, 1.0) as f32;
    let timestamp = item
        .get("filed_at")
        .or_else(|| item.get("created_at"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    let interaction = wing == Some(INTERACTIONS_WING)
        || source_file.is_some_and(|source| source.starts_with("message:"))
        || header
            .as_ref()
            .is_some_and(|header| header.kind == "message");
    if interaction {
        if !scope.wants_interactions() {
            return None;
        }
        if let Some(header) = &header {
            if header.chat_id != Some(chat_id) || header.persona_id != Some(persona_id) {
                return None;
            }
        }
        let room_ok = room == Some(expected_room.as_str());
        let header_ok = header.as_ref().is_some_and(|header| {
            header.kind == "message"
                && header.chat_id == Some(chat_id)
                && header.persona_id == Some(persona_id)
        });
        if room.is_some() && !room_ok {
            return None;
        }
        if !room_ok && !header_ok {
            return None;
        }
        let source_id = header
            .as_ref()
            .map(|header| header.source_id.clone())
            .or_else(|| {
                source_file
                    .and_then(|source| source.strip_prefix("message:"))
                    .map(str::to_string)
            })?;
        if source_id.is_empty() {
            return None;
        }
        let stamped = header
            .as_ref()
            .map(|header| header.timestamp.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or(timestamp);
        return Some(RecallHit {
            provenance: format!("message:{source_id}"),
            source_id,
            namespace: "interactions".into(),
            timestamp: stamped,
            score,
            excerpt: excerpt_of(content),
        });
    }

    if !scope.wants_knowledge() {
        return None;
    }
    if wing == Some(INTERACTIONS_WING) {
        return None;
    }
    let source = source_file
        .map(str::to_string)
        .or_else(|| header.as_ref().map(|header| header.source_id.clone()))
        .unwrap_or_default();
    if !safe_relative_source(&source) {
        return None;
    }
    Some(RecallHit {
        source_id: source.clone(),
        namespace: "knowledge".into(),
        timestamp,
        score,
        excerpt: excerpt_of(content),
        provenance: source,
    })
}

pub fn hits_from_palace_payload(
    payload: &str,
    scope: RecallScope,
    chat_id: i64,
    persona_id: i64,
) -> Result<Vec<RecallHit>, &'static str> {
    let parsed: Value = serde_json::from_str(payload).map_err(|_| "parse")?;
    if parsed.get("success").and_then(|value| value.as_bool()) == Some(false) {
        return Err("unavailable");
    }
    let results = parsed
        .get("results")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut hits = Vec::new();
    for item in results.into_iter().take(20) {
        if let Some(hit) = filter_palace_hit(&item, scope, chat_id, persona_id) {
            hits.push(hit);
        }
    }
    Ok(hits)
}

struct PalaceInner {
    server: Option<Arc<McpServer>>,
    unavailable_until: Option<Instant>,
    failures: u32,
}

pub struct PalaceClient {
    settings: MemPalaceConfig,
    inner: Mutex<PalaceInner>,
}

impl PalaceClient {
    pub fn new(settings: MemPalaceConfig) -> Self {
        Self {
            settings,
            inner: Mutex::new(PalaceInner {
                server: None,
                unavailable_until: None,
                failures: 0,
            }),
        }
    }

    pub fn configured(&self) -> bool {
        self.settings.configured()
    }

    fn fail(guard: &mut PalaceInner, class: &'static str) -> &'static str {
        guard.server = None;
        guard.failures = guard.failures.saturating_add(1);
        guard.unavailable_until =
            Some(Instant::now() + Duration::from_secs(next_backoff_secs(guard.failures)));
        class
    }

    pub async fn call(&self, tool: &str, mut args: Value) -> Result<String, &'static str> {
        if !tool_allowed(tool) {
            return Err("rejected");
        }
        if let Some(content) = args.get_mut("content").and_then(|value| value.as_str()) {
            if content.chars().count() > MAX_DRAWER_CHARS {
                args["content"] = json!(clip_chars(content, MAX_DRAWER_CHARS));
            }
        }
        if let Some(query) = args.get("query").and_then(|value| value.as_str()) {
            if query.chars().count() > MAX_QUERY_CHARS {
                args["query"] = json!(clip_chars(query, MAX_QUERY_CHARS));
            }
        }
        let payload = serde_json::to_string(&args).unwrap_or_default();
        if payload.chars().count() > MAX_PAYLOAD_CHARS {
            return Err("rejected");
        }

        let mut guard = self.inner.lock().await;
        if let Some(until) = guard.unavailable_until {
            if Instant::now() < until {
                return Err("backoff");
            }
        }
        if guard.server.is_none() {
            if !self.settings.configured() {
                return Err("unconfigured");
            }
            let startup = Duration::from_secs(self.settings.startup_timeout_secs.max(1));
            let settings = self.settings.clone();
            match tokio::time::timeout(startup, connect_palace(&settings)).await {
                Ok(Ok(server)) => {
                    guard.server = Some(server);
                    guard.failures = 0;
                    guard.unavailable_until = None;
                }
                Ok(Err(_)) => return Err(Self::fail(&mut guard, "unavailable")),
                Err(_) => return Err(Self::fail(&mut guard, "timeout")),
            }
        }
        let server = guard.server.clone().ok_or("unavailable")?;
        let (official_tool, official_args) = match translate_palace_tool(tool, &args) {
            Ok(translated) => translated,
            Err(class) => return Err(class),
        };
        let query_timeout = Duration::from_secs(self.settings.query_timeout_secs.max(1));
        match tokio::time::timeout(
            query_timeout,
            server.call_tool(&official_tool, official_args),
        )
        .await
        {
            Ok(Ok(text)) => {
                if palace_reported_failure(&text) {
                    return Err(Self::fail(&mut guard, "unavailable"));
                }
                guard.failures = 0;
                guard.unavailable_until = None;
                Ok(text)
            }
            Ok(Err(err)) => Err(Self::fail(&mut guard, classify_palace_error(&err))),
            Err(_) => Err(Self::fail(&mut guard, "timeout")),
        }
    }
}

/// Map the internal recall verbs onto the official MemPalace MCP tools.
pub fn translate_palace_tool(tool: &str, args: &Value) -> Result<(String, Value), &'static str> {
    match tool {
        "palace_query" => {
            if args.get("target").and_then(|value| value.as_str()) != Some("search") {
                return Err("rejected");
            }
            let mut out = json!({
                "query": args.get("query").cloned().unwrap_or(Value::String(String::new())),
                "limit": args.get("limit").cloned().unwrap_or(json!(5)),
            });
            for key in ["wing", "room", "since", "before"] {
                if let Some(value) = args.get(key) {
                    if !value.is_null() {
                        out[key] = value.clone();
                    }
                }
            }
            Ok(("mempalace_search".to_string(), out))
        }
        "palace_exec" => match args.get("action").and_then(|value| value.as_str()) {
            Some("add_drawer") => Ok((
                "mempalace_add_drawer".to_string(),
                json!({
                    "wing": args.get("wing").cloned().unwrap_or(Value::Null),
                    "room": args.get("room").cloned().unwrap_or(Value::Null),
                    "content": args.get("content").cloned().unwrap_or(Value::Null),
                    "source_file": args.get("source_file").cloned().unwrap_or(Value::Null),
                    "added_by": "meido",
                }),
            )),
            Some("delete_by_source") => Ok((
                "mempalace_delete_by_source".to_string(),
                json!({
                    "source_file": args.get("source_file").cloned().unwrap_or(Value::Null),
                    "dry_run": args.get("dry_run").cloned().unwrap_or(json!(false)),
                }),
            )),
            _ => Err("rejected"),
        },
        "palace_coordinate" => Err("rejected"),
        _ => Err("rejected"),
    }
}

fn palace_reported_failure(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| value.get("success").and_then(|flag| flag.as_bool()))
        == Some(false)
}

async fn connect_palace(settings: &MemPalaceConfig) -> Result<Arc<McpServer>, String> {
    let use_http = !settings.endpoint.trim().is_empty();
    let config = McpServerConfig {
        transport: if use_http {
            "streamable_http".to_string()
        } else {
            "stdio".to_string()
        },
        protocol_version: None,
        request_timeout_secs: Some(settings.query_timeout_secs.max(1)),
        max_retries: Some(0),
        health_interval_secs: Some(0),
        command: settings.command.clone(),
        args: settings.args.clone(),
        env: HashMap::new(),
        endpoint: settings.endpoint.clone(),
        headers: HashMap::new(),
    };
    let server = McpServer::connect("mempalace-light", &config, None).await?;
    Ok(Arc::new(server))
}

pub struct RecallService {
    db: Arc<Database>,
    settings: MemPalaceConfig,
    palace: Arc<PalaceClient>,
    vault_mode: std::sync::Mutex<Option<SearchVaultMode>>,
    origin_root: PathBuf,
}

impl RecallService {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        let origin_root = origin_root_from_config(config);
        let settings = config.mempalace.clone();
        Self {
            db,
            palace: Arc::new(PalaceClient::new(settings.clone())),
            settings,
            vault_mode: std::sync::Mutex::new(None),
            origin_root,
        }
    }

    pub fn set_vault_mode(&self, mode: SearchVaultMode) {
        if let Ok(mut guard) = self.vault_mode.lock() {
            *guard = Some(mode);
        }
    }

    pub fn mode(&self) -> RecallMode {
        RecallMode::parse(&self.settings.recall_mode)
    }

    pub fn settings(&self) -> &MemPalaceConfig {
        &self.settings
    }

    fn palace_serves(&self, persona_id: i64) -> bool {
        match self.mode() {
            RecallMode::Disabled | RecallMode::Shadow => false,
            RecallMode::Selected => self.settings.selected_persona_ids.contains(&persona_id),
            RecallMode::Default => true,
        }
    }

    fn freshness(&self) -> String {
        self.db
            .recall_checkpoint("last_success")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub async fn query(&self, request: RecallRequest) -> RecallResponse {
        let started = Instant::now();
        let limit = request.limit.clamp(1, 20);
        let mut response = if self.palace_serves(request.persona_id) {
            self.query_palace(&request, limit).await
        } else {
            self.query_legacy(&request, limit).await
        };
        if self.mode() == RecallMode::Shadow && self.palace.configured() {
            let palace = self.palace.clone();
            let legacy = response.hits.clone();
            let request = request.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                match palace_search(&palace, &request, limit).await {
                    Ok(palace_hits) => {
                        let (legacy_hits, palace_hits, overlap) =
                            compare_source_ids(&legacy, &palace_hits);
                        tracing::info!(
                            target: "recall",
                            latency_ms = started.elapsed().as_millis() as u64,
                            legacy_hits,
                            palace_hits,
                            overlap,
                            failure_class = "ok",
                            "recall shadow comparison"
                        );
                    }
                    Err(class) => {
                        tracing::info!(
                            target: "recall",
                            latency_ms = started.elapsed().as_millis() as u64,
                            failure_class = class,
                            "recall shadow comparison"
                        );
                    }
                }
            });
        }
        response.freshness = self.freshness();
        tracing::info!(
            target: "recall",
            latency_ms = started.elapsed().as_millis() as u64,
            hit_count = response.hits.len(),
            freshness = %response.freshness,
            failure_class = %response.failure_class,
            degraded = response.degraded,
            mode = self.mode().as_str(),
            scope = request.scope.as_str(),
            "recall completed"
        );
        response
    }

    async fn query_legacy(&self, request: &RecallRequest, limit: usize) -> RecallResponse {
        let mut hits = Vec::new();
        let mut failure_class = "ok".to_string();
        if request.scope.wants_interactions() {
            match search_interactions(
                &self.db,
                request.chat_id,
                request.persona_id,
                &request.query,
                limit,
                request.from_date.as_deref(),
                request.to_date.as_deref(),
            )
            .await
            {
                Ok(found) => hits.extend(found),
                Err(class) => failure_class = class,
            }
        }
        if request.scope.wants_knowledge() {
            match self.legacy_knowledge(&request.query, limit).await {
                Ok(found) => hits.extend(found),
                Err(class) => {
                    if failure_class == "ok" {
                        failure_class = class;
                    }
                }
            }
        }
        hits.truncate(limit);
        RecallResponse {
            hits,
            degraded: false,
            failure_class,
            freshness: String::new(),
        }
    }

    async fn query_palace(&self, request: &RecallRequest, limit: usize) -> RecallResponse {
        if !self.palace.configured() {
            let mut legacy = self.query_legacy(request, limit).await;
            legacy.degraded = true;
            legacy.failure_class = "unconfigured".into();
            return legacy;
        }
        match palace_search(&self.palace, request, limit).await {
            Ok(hits) => RecallResponse {
                hits,
                degraded: false,
                failure_class: "ok".into(),
                freshness: String::new(),
            },
            Err(class) => {
                let mut legacy = self.query_legacy(request, limit).await;
                legacy.degraded = true;
                legacy.failure_class = class.to_string();
                legacy
            }
        }
    }

    async fn legacy_knowledge(&self, query: &str, limit: usize) -> Result<Vec<RecallHit>, String> {
        let mode = self.vault_mode.lock().ok().and_then(|guard| guard.clone());
        let Some(mode) = mode else {
            return Ok(Vec::new());
        };
        let snippets = query_snippets(&mode, query, limit).await?;
        Ok(snippets
            .into_iter()
            .filter(|snippet| !snippet.source.is_empty() && snippet.source != "unknown")
            .map(|snippet| RecallHit {
                score: (1.0 - snippet.distance).clamp(0.0, 1.0) as f32,
                provenance: snippet.source.clone(),
                source_id: snippet.source.clone(),
                namespace: "knowledge".into(),
                timestamp: String::new(),
                excerpt: snippet.excerpt,
            })
            .collect())
    }

    pub async fn enqueue_recent(&self, chat_id: i64, persona_id: i64) {
        if self.mode() == RecallMode::Disabled {
            return;
        }
        let db = self.db.clone();
        let messages = tokio::task::spawn_blocking(move || {
            db.get_recent_messages(chat_id, persona_id, 6, false)
        })
        .await;
        let Ok(Ok(messages)) = messages else {
            tracing::info!(target: "recall", failure_class = "enqueue", "recall ingest enqueue failed");
            return;
        };
        for message in messages {
            if message.content.trim().is_empty() {
                continue;
            }
            let hash = content_hash(&message.content);
            if self
                .db
                .upsert_recall_ingest(
                    "message",
                    &message.id,
                    Some(message.chat_id),
                    Some(message.persona_id),
                    &hash,
                    0,
                )
                .is_err()
            {
                tracing::info!(target: "recall", failure_class = "enqueue", "recall ingest enqueue failed");
            }
        }
    }

    pub async fn drain_pending(&self, limit: usize) -> Result<String, String> {
        if self.mode() == RecallMode::Disabled || !self.palace.configured() {
            return Ok("skipped".into());
        }
        let owner = format!("drain-{}", std::process::id());
        let locked = self
            .db
            .try_recall_lock(&owner, 900)
            .map_err(|_| "lock".to_string())?;
        if !locked {
            return Ok("collapsed".into());
        }
        let result = self.drain_locked(limit).await;
        let _ = self.db.release_recall_lock(&owner);
        result
    }

    async fn drain_locked(&self, limit: usize) -> Result<String, String> {
        let rows = self
            .db
            .claim_recall_ingest_batch(limit)
            .map_err(|_| "claim".to_string())?;
        let mut done = 0_usize;
        let mut rejected = 0_usize;
        for row in rows {
            match self.ingest_row(&row).await {
                Ok(()) => {
                    let _ = self.db.finish_recall_ingest(
                        &row.source_kind,
                        &row.source_id,
                        "done",
                        None,
                    );
                    done += 1;
                }
                Err(class) => {
                    let status = if row.attempt_count + 1 >= MAX_INGEST_ATTEMPTS {
                        rejected += 1;
                        "rejected"
                    } else {
                        "pending"
                    };
                    let _ = self.db.finish_recall_ingest(
                        &row.source_kind,
                        &row.source_id,
                        status,
                        Some(class),
                    );
                }
            }
        }
        if done > 0 {
            let _ = self
                .db
                .set_recall_checkpoint("last_success", &chrono::Utc::now().to_rfc3339());
        }
        Ok(format!("done={done} rejected={rejected}"))
    }

    async fn ingest_row(&self, row: &RecallIngestRow) -> Result<(), &'static str> {
        match row.source_kind.as_str() {
            "message" => self.ingest_message(row).await,
            "origin" => self.ingest_origin(row).await,
            _ => Err("rejected"),
        }
    }

    async fn ingest_message(&self, row: &RecallIngestRow) -> Result<(), &'static str> {
        let (chat_id, persona_id) = match (row.chat_id, row.persona_id) {
            (Some(chat_id), Some(persona_id)) => (chat_id, persona_id),
            _ => return Err("rejected"),
        };
        let db = self.db.clone();
        let source_id = row.source_id.clone();
        let message = tokio::task::spawn_blocking(move || {
            db.get_message_for_recall(&source_id, chat_id, persona_id)
        })
        .await
        .map_err(|_| "unavailable")?
        .map_err(|_| "unavailable")?;
        let Some(message) = message else {
            return Err("missing");
        };
        let content = message_drawer(&message);
        let source_file = format!("message:{}", message.id);
        let _ = self
            .palace
            .call(
                "palace_exec",
                json!({
                    "action": "delete_by_source",
                    "source_file": source_file,
                    "dry_run": false,
                }),
            )
            .await;
        self.palace
            .call(
                "palace_exec",
                json!({
                    "action": "add_drawer",
                    "wing": INTERACTIONS_WING,
                    "room": interaction_room(chat_id, persona_id),
                    "content": content,
                    "source_file": source_file,
                }),
            )
            .await
            .map(|_| ())
    }

    async fn ingest_origin(&self, row: &RecallIngestRow) -> Result<(), &'static str> {
        let Some(path) = safe_origin_file(&self.origin_root, &row.source_id) else {
            return Err("rejected");
        };
        let source_id = row.source_id.clone();
        let text = tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
            .await
            .map_err(|_| "unavailable")?
            .map_err(|_| "missing")?;
        if text.len() > 1_000_000 {
            return Err("rejected");
        }
        let _ = self
            .palace
            .call(
                "palace_exec",
                json!({
                    "action": "delete_by_source",
                    "source_file": source_id,
                    "dry_run": false,
                }),
            )
            .await;
        self.palace
            .call(
                "palace_exec",
                json!({
                    "action": "add_drawer",
                    "wing": KNOWLEDGE_WING,
                    "room": room_for_origin(&source_id),
                    "content": origin_drawer(&source_id, &text),
                    "source_file": source_id,
                }),
            )
            .await
            .map(|_| ())
    }

    pub async fn run_reconcile(&self) -> Result<String, String> {
        if self.mode() == RecallMode::Disabled {
            return Ok("skipped".into());
        }
        let owner = format!("reconcile-{}", std::process::id());
        let locked = self
            .db
            .try_recall_lock(&owner, 1800)
            .map_err(|_| "lock".to_string())?;
        if !locked {
            return Ok("collapsed".into());
        }
        let result = self.reconcile_locked().await;
        let _ = self.db.release_recall_lock(&owner);
        result
    }

    async fn reconcile_locked(&self) -> Result<String, String> {
        let backfilled = self
            .backfill_messages(40)
            .map_err(|_| "backfill".to_string())?;
        let (discovered, deleted) = self
            .reconcile_origin()
            .await
            .map_err(|_| "origin".to_string())?;
        let drained = self.drain_locked(20).await?;
        Ok(format!(
            "backfilled={backfilled} discovered={discovered} deleted={deleted} {drained}"
        ))
    }

    fn backfill_messages(&self, limit: usize) -> Result<usize, String> {
        let cursor = self
            .db
            .recall_checkpoint("messages")
            .map_err(|_| "checkpoint".to_string())?
            .unwrap_or_default();
        let (after_ts, after_id) = split_cursor(&cursor);
        let db = self.db.clone();
        let after_ts = after_ts.to_string();
        let after_id = after_id.to_string();
        let messages = db
            .list_messages_after_cursor(&after_ts, &after_id, limit)
            .map_err(|_| "read".to_string())?;
        let count = messages.len();
        let mut last = None;
        for message in &messages {
            if message.content.trim().is_empty() {
                last = Some((message.timestamp.clone(), message.id.clone()));
                continue;
            }
            self.db
                .upsert_recall_ingest(
                    "message",
                    &message.id,
                    Some(message.chat_id),
                    Some(message.persona_id),
                    &content_hash(&message.content),
                    0,
                )
                .map_err(|_| "enqueue".to_string())?;
            last = Some((message.timestamp.clone(), message.id.clone()));
        }
        if let Some((timestamp, id)) = last {
            self.db
                .set_recall_checkpoint("messages", &format!("{timestamp}|{id}"))
                .map_err(|_| "checkpoint".to_string())?;
        }
        Ok(count)
    }

    async fn reconcile_origin(&self) -> Result<(usize, usize), String> {
        if !self.origin_root.is_dir() {
            return Ok((0, 0));
        }
        let generation = self
            .db
            .recall_checkpoint("origin_generation")
            .map_err(|_| "checkpoint".to_string())?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(1);
        let cursor = self
            .db
            .recall_checkpoint("origin_path")
            .map_err(|_| "checkpoint".to_string())?
            .unwrap_or_default();
        let files = list_origin_markdown(&self.origin_root);
        let mut window: Vec<&String> = files
            .iter()
            .filter(|path| path.as_str() > cursor.as_str())
            .take(40)
            .collect();
        if window.is_empty() && !cursor.is_empty() {
            let stale = self
                .db
                .stale_origin_source_ids(generation)
                .map_err(|_| "stale".to_string())?;
            let mut deleted = 0_usize;
            for source_id in stale {
                if self.palace.configured() {
                    let _ = self
                        .palace
                        .call(
                            "palace_exec",
                            json!({
                                "action": "delete_by_source",
                                "source_file": source_id,
                                "dry_run": false,
                            }),
                        )
                        .await;
                }
                if self.db.delete_recall_ingest("origin", &source_id).is_ok() {
                    deleted += 1;
                }
            }
            self.db
                .set_recall_checkpoint("origin_path", "")
                .map_err(|_| "checkpoint".to_string())?;
            self.db
                .set_recall_checkpoint("origin_generation", &(generation + 1).to_string())
                .map_err(|_| "checkpoint".to_string())?;
            return Ok((0, deleted));
        }
        if window.is_empty() {
            window = files.iter().take(40).collect();
        }
        let mut last = cursor;
        for rel in &window {
            let Some(path) = safe_origin_file(&self.origin_root, rel) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.len() > 1_000_000 {
                continue;
            }
            self.db
                .upsert_recall_ingest("origin", rel, None, None, &content_hash(&text), generation)
                .map_err(|_| "enqueue".to_string())?;
            last = (*rel).clone();
        }
        self.db
            .set_recall_checkpoint("origin_path", &last)
            .map_err(|_| "checkpoint".to_string())?;
        Ok((window.len(), 0))
    }
}

async fn palace_search(
    palace: &PalaceClient,
    request: &RecallRequest,
    limit: usize,
) -> Result<Vec<RecallHit>, &'static str> {
    let mut hits = Vec::new();
    if request.scope.wants_interactions() {
        let mut args = json!({
            "target": "search",
            "query": clip_chars(&request.query, MAX_QUERY_CHARS),
            "wing": INTERACTIONS_WING,
            "room": interaction_room(request.chat_id, request.persona_id),
            "limit": limit,
        });
        if let Some(since) = &request.from_date {
            args["since"] = json!(since);
        }
        if let Some(before) = &request.to_date {
            args["before"] = json!(before);
        }
        let payload = palace.call("palace_query", args).await?;
        hits.extend(hits_from_palace_payload(
            &payload,
            RecallScope::Interactions,
            request.chat_id,
            request.persona_id,
        )?);
    }
    if request.scope.wants_knowledge() {
        let args = json!({
            "target": "search",
            "query": clip_chars(&request.query, MAX_QUERY_CHARS),
            "wing": KNOWLEDGE_WING,
            "limit": limit,
        });
        let payload = palace.call("palace_query", args).await?;
        hits.extend(hits_from_palace_payload(
            &payload,
            RecallScope::Knowledge,
            request.chat_id,
            request.persona_id,
        )?);
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}

pub async fn search_interactions(
    db: &Arc<Database>,
    chat_id: i64,
    persona_id: i64,
    query: &str,
    limit: usize,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<Vec<RecallHit>, String> {
    let fts = sanitize_fts_query(query);
    if fts.is_empty() {
        return Ok(Vec::new());
    }
    let db = db.clone();
    let from_date = from_date.map(str::to_string);
    let to_date = to_date.map(str::to_string);
    let messages = tokio::task::spawn_blocking(move || {
        db.search_messages(
            chat_id,
            persona_id,
            &fts,
            limit,
            from_date.as_deref(),
            to_date.as_deref(),
        )
    })
    .await
    .map_err(|_| "fts".to_string())?
    .map_err(|_| "fts".to_string())?;
    Ok(messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| message_hit(&message, index))
        .collect())
}

fn message_hit(message: &StoredMessage, index: usize) -> RecallHit {
    RecallHit {
        source_id: message.id.clone(),
        namespace: "interactions".into(),
        timestamp: message.timestamp.clone(),
        score: (1.0 - (index as f32 * 0.01)).max(0.0),
        excerpt: clip_chars(&message.content, MAX_EXCERPT_CHARS),
        provenance: format!("message:{}", message.id),
    }
}

fn message_drawer(message: &StoredMessage) -> String {
    let role = if message.is_from_bot {
        "assistant"
    } else {
        "user"
    };
    format!(
        "source_kind: message\nsource_id: {}\nchat_id: {}\npersona_id: {}\ntimestamp: {}\nrole: {role}\n---\n{}",
        message.id,
        message.chat_id,
        message.persona_id,
        message.timestamp,
        clip_chars(&message.content, MAX_DRAWER_CHARS)
    )
}

fn origin_drawer(rel: &str, body: &str) -> String {
    format!(
        "source_kind: origin\nsource_id: {rel}\n---\n{}",
        clip_chars(body, MAX_DRAWER_CHARS)
    )
}

fn room_for_origin(rel: &str) -> String {
    let first = rel.split(['/', '\\']).next().unwrap_or("notes");
    let mut out = String::new();
    for ch in first.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        "notes".into()
    } else {
        out
    }
}

fn split_cursor(cursor: &str) -> (&str, &str) {
    cursor.split_once('|').unwrap_or((cursor, ""))
}

fn origin_root_from_config(config: &Config) -> PathBuf {
    config.origin_vault_root()
}

fn safe_origin_file(root: &Path, rel: &str) -> Option<PathBuf> {
    if !safe_relative_source(rel) {
        return None;
    }
    let path = root.join(rel);
    let root_canon = std::fs::canonicalize(root).ok()?;
    let file_canon = std::fs::canonicalize(&path).ok()?;
    if file_canon.starts_with(&root_canon) {
        Some(file_canon)
    } else {
        None
    }
}

fn list_origin_markdown(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0_u32)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= 5_000 || depth > 12 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                if let Some(rel) = path
                    .strip_prefix(root)
                    .ok()
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                {
                    if safe_relative_source(&rel) {
                        out.push(rel);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

pub fn command_available(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    let path = Path::new(command);
    if command.contains('/') || command.contains('\\') {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fts_query_quotes_tokens_and_drops_operators() {
        assert_eq!(sanitize_fts_query("secret"), "\"secret\"");
        assert_eq!(
            sanitize_fts_query("foo OR bar:baz*"),
            "\"foo\" \"OR\" \"bar\" \"baz\""
        );
        assert_eq!(sanitize_fts_query("   "), "");
    }

    #[test]
    fn unknown_recall_mode_stays_disabled() {
        assert_eq!(RecallMode::parse("nope"), RecallMode::Disabled);
        assert_eq!(RecallMode::parse(" default "), RecallMode::Default);
    }

    #[test]
    fn backoff_is_bounded() {
        assert_eq!(next_backoff_secs(0), 1);
        assert_eq!(next_backoff_secs(2), 4);
        assert_eq!(next_backoff_secs(30), 60);
        assert!(!tool_allowed("mempalace_mine"));
        assert!(tool_allowed("palace_query"));
        assert!(tool_allowed("palace_exec"));
        assert!(tool_allowed("palace_coordinate"));
    }

    #[test]
    fn official_mcp_search_uses_similarity_as_score() {
        let item = json!({
            "text": "source_kind: origin\nsource_id: projects/plan.md\n---\nship the index",
            "wing": "origin",
            "source_file": "projects/plan.md",
            "similarity": 0.8
        });
        let hit = filter_palace_hit(&item, RecallScope::Knowledge, 1, 1).expect("hit");
        assert!((hit.score - 0.8).abs() < 0.001);
    }

    #[test]
    fn translate_maps_recall_verbs_to_official_tools() {
        let (tool, args) = translate_palace_tool(
            "palace_query",
            &json!({"target": "search", "query": "plan", "wing": "origin", "limit": 3}),
        )
        .expect("search");
        assert_eq!(tool, "mempalace_search");
        assert_eq!(args["query"], "plan");
        assert!(args.get("target").is_none());

        let (tool, args) = translate_palace_tool(
            "palace_exec",
            &json!({
                "action": "delete_by_source",
                "source_file": "message:m1",
                "dry_run": false
            }),
        )
        .expect("delete");
        assert_eq!(tool, "mempalace_delete_by_source");
        assert_eq!(args["dry_run"], false);
        assert!(translate_palace_tool("palace_query", &json!({"target": "mine"})).is_err());
    }

    #[test]
    fn cross_persona_interaction_hits_are_dropped() {
        let item = json!({
            "content": "source_kind: message\nsource_id: m1\nchat_id: 9\npersona_id: 2\ntimestamp: 2026-01-01T00:00:00Z\nrole: user\n---\nsecret plan",
            "wing": "interactions",
            "room": "c9-p2",
            "source_file": "message:m1",
            "distance": 0.2
        });
        assert!(filter_palace_hit(&item, RecallScope::Auto, 9, 1).is_none());
        let hit = filter_palace_hit(&item, RecallScope::Auto, 9, 2).expect("in scope");
        assert_eq!(hit.provenance, "message:m1");
        assert_eq!(hit.namespace, "interactions");
        assert!(hit.excerpt.contains("secret plan"));
        assert!(!hit.excerpt.contains("source_kind"));
    }

    #[test]
    fn knowledge_hit_requires_a_relative_origin_path() {
        let good = json!({
            "content": "note",
            "wing": "origin",
            "source_file": "projects/plan.md",
            "distance": 0.4
        });
        let hit = filter_palace_hit(&good, RecallScope::Knowledge, 1, 1).expect("path");
        assert_eq!(hit.provenance, "projects/plan.md");
        let escaped = json!({
            "content": "note",
            "wing": "origin",
            "source_file": "../secrets.md"
        });
        assert!(filter_palace_hit(&escaped, RecallScope::Knowledge, 1, 1).is_none());
        let absolute = json!({
            "content": "note",
            "wing": "origin",
            "source_file": "/tmp/secrets.md"
        });
        assert!(filter_palace_hit(&absolute, RecallScope::Auto, 1, 1).is_none());
    }

    #[test]
    fn same_hash_ingest_stays_done_and_a_change_requeues() {
        let dir =
            std::env::temp_dir().join(format!("finally_recall_ingest_{}", uuid::Uuid::new_v4()));
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        db.upsert_recall_ingest("message", "m1", Some(1), Some(2), "hash-a", 0)
            .unwrap();
        db.finish_recall_ingest("message", "m1", "done", None)
            .unwrap();
        db.upsert_recall_ingest("message", "m1", Some(1), Some(2), "hash-a", 0)
            .unwrap();
        assert_eq!(
            db.recall_ingest_status("message", "m1").unwrap().as_deref(),
            Some("done")
        );
        db.upsert_recall_ingest("message", "m1", Some(1), Some(2), "hash-b", 0)
            .unwrap();
        assert_eq!(
            db.recall_ingest_status("message", "m1").unwrap().as_deref(),
            Some("pending")
        );
        let claimed = db.claim_recall_ingest_batch(10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].content_hash, "hash-b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlapping_recall_lock_collapses() {
        let dir =
            std::env::temp_dir().join(format!("finally_recall_lock_{}", uuid::Uuid::new_v4()));
        let db = Database::new(dir.to_str().unwrap()).unwrap();
        assert!(db.try_recall_lock("owner-a", 60).unwrap());
        assert!(!db.try_recall_lock("owner-b", 60).unwrap());
        db.release_recall_lock("owner-a").unwrap();
        assert!(db.try_recall_lock("owner-b", 60).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
