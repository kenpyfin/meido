use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MEMORY_SCHEMA_VERSION: u32 = 1;
const MEMORY_STATE_FILE: &str = "memory_state.json";
const MEMORY_EVENTS_FILE: &str = "memory_events.jsonl";
const MEMORY_FIELD_LEGEND_MAX_CHARS: usize = 2400;

/// How persona memory is embedded in the system prompt (`MEMORY_PROMPT_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPromptMode {
    /// Pretty JSON only (default). Best for `patch_memory_state`; use `read_memory_state` for the on-disk file, field legend, or uncapped lists.
    Json,
    /// Markdown projection only (no JSON in the prompt).
    Markdown,
    /// Legend + markdown + JSON (highest token cost; legacy layout).
    Both,
}

/// Controls `MemoryManager::build_memory_context` output shape and caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPromptBuildOptions {
    pub mode: MemoryPromptMode,
    /// When true, emits `<memory_field_legend>` (compact schema prose). For `Both` mode this defaults on from [`Self::from_env`] unless overridden by env.
    pub include_legend: bool,
    /// Max `tier1.workflow_principles` entries in the **prompt** projection. `0` means no cap (full list). Disk state is never truncated.
    pub workflow_principles_prompt_max: usize,
}

impl MemoryPromptBuildOptions {
    /// Reads `MEMORY_PROMPT_MODE`, `MEMORY_PROMPT_INCLUDE_LEGEND`, and `WORKFLOW_PRINCIPLES_PROMPT_MAX`.
    pub fn from_env() -> Self {
        let mode = match std::env::var("MEMORY_PROMPT_MODE").ok().as_deref() {
            Some(s) if s.trim().eq_ignore_ascii_case("markdown") => MemoryPromptMode::Markdown,
            Some(s) if s.trim().eq_ignore_ascii_case("both") => MemoryPromptMode::Both,
            _ => MemoryPromptMode::Json,
        };
        let include_legend = match std::env::var("MEMORY_PROMPT_INCLUDE_LEGEND")
            .ok()
            .as_deref()
        {
            Some(s)
                if s == "1" || s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("yes") =>
            {
                true
            }
            _ => matches!(mode, MemoryPromptMode::Both),
        };
        let workflow_principles_prompt_max = std::env::var("WORKFLOW_PRINCIPLES_PROMPT_MAX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(25);
        Self {
            mode,
            include_legend,
            workflow_principles_prompt_max,
        }
    }

    /// Legend + markdown + JSON with no workflow cap (matches pre-optimization tests).
    pub fn all_sections_no_workflow_cap() -> Self {
        Self {
            mode: MemoryPromptMode::Both,
            include_legend: true,
            workflow_principles_prompt_max: 0,
        }
    }
}

fn cap_workflow_principles_for_prompt(
    state: &PersonaMemoryState,
    max_entries: usize,
) -> (PersonaMemoryState, Option<String>) {
    if max_entries == 0 || state.tier1.workflow_principles.len() <= max_entries {
        return (state.clone(), None);
    }
    let total = state.tier1.workflow_principles.len();
    let mut s = state.clone();
    s.tier1.workflow_principles.truncate(max_entries);
    let omitted = total - max_entries;
    let note = format!(
        "tier1.workflow_principles: showing {} of {} entries ({} omitted); use read_memory_state for the full on-disk list.",
        max_entries, total, omitted
    );
    (s, Some(note))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStateMeta {
    #[serde(default = "default_memory_schema_version")]
    pub version: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub updated_at: String,
}

fn default_memory_schema_version() -> u32 {
    MEMORY_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdentityMemory {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub self_model: String,
    #[serde(default)]
    pub voice_style: String,
    #[serde(default)]
    pub non_negotiables: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tier1Memory {
    #[serde(default)]
    pub stable_facts: Vec<String>,
    #[serde(default)]
    pub workflow_principles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LegacyActiveProjectMemory {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub updated_at: String,
}

/// Vault SOP reference stored in Tier 2 memory (`tier2.sops`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SopPointer {
    /// Short handle for matching tasks (e.g. `pz-post-pipeline`).
    #[serde(default)]
    pub id: String,
    /// Vault-relative path (must start with `ORIGIN/`).
    pub vault_path: String,
    /// Optional one-line hint; full procedure is always loaded from `vault_path`.
    #[serde(default)]
    pub summary: String,
}

impl SopPointer {
    pub fn derive_id_from_vault_path(vault_path: &str) -> String {
        Path::new(vault_path.trim())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sop")
            .to_ascii_lowercase()
            .replace(' ', "-")
    }

    pub fn normalized_vault_path(vault_path: &str) -> String {
        let t = vault_path.trim().trim_start_matches("./");
        if t.starts_with("ORIGIN/") {
            t.to_string()
        } else if t.starts_with("origin/") {
            format!("ORIGIN/{}", &t[7..])
        } else {
            t.to_string()
        }
    }

    /// Parse a legacy Tier 2 line or `write_tiered_memory` SOP row into a pointer.
    pub fn from_legacy_line(line: &str) -> Option<Self> {
        let mut t = line.trim();
        if let Some(rest) = t.strip_prefix("- SOP|") {
            let parts: Vec<&str> = rest.splitn(3, '|').collect();
            if parts.len() >= 2 {
                let vault_path = Self::normalized_vault_path(parts[1]);
                if !vault_path.starts_with("ORIGIN/") {
                    return None;
                }
                let id = parts[0].trim();
                let summary = parts.get(2).map(|s| s.trim()).unwrap_or("").to_string();
                return Some(Self {
                    id: if id.is_empty() {
                        Self::derive_id_from_vault_path(&vault_path)
                    } else {
                        id.to_string()
                    },
                    vault_path,
                    summary,
                });
            }
        }
        t = t.strip_prefix("SOP:").unwrap_or(t).trim();
        let lower = t.to_ascii_lowercase();
        let origin_idx = lower.find("origin/")?;
        let vault_path = Self::normalized_vault_path(&t[origin_idx..]);
        if !vault_path.starts_with("ORIGIN/") {
            return None;
        }
        let before = t[..origin_idx].trim().trim_end_matches(['—', '-', ':']);
        let summary = before
            .strip_prefix("SOP")
            .unwrap_or(before)
            .trim()
            .trim_start_matches(':')
            .trim()
            .to_string();
        Some(Self {
            id: Self::derive_id_from_vault_path(&vault_path),
            vault_path,
            summary,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tier2Memory {
    #[serde(default)]
    pub user_terminology: Vec<String>,
    /// Standard Operating Procedures — pointers to vault markdown only.
    #[serde(default)]
    pub sops: Vec<SopPointer>,
    #[serde(default)]
    pub preferences: Vec<String>,
    /// Legacy `known_steps` strings; migrated on `normalize`, not persisted.
    #[serde(default, rename = "known_steps", skip_serializing)]
    pub legacy_known_steps: Vec<String>,
    #[serde(default, rename = "active_projects", skip_serializing)]
    pub legacy_active_projects: Vec<LegacyActiveProjectMemory>,
    #[serde(skip)]
    pub legacy_migration: Option<Tier2LegacyMigrationStats>,
}

#[derive(Debug, Clone, Default)]
pub struct Tier2LegacyMigrationStats {
    pub moved_terminology: usize,
    pub moved_sops: usize,
    pub moved_preferences: usize,
    pub dropped_active_projects: usize,
    pub legacy_known_steps_to_principles: usize,
}

pub fn dedupe_sops(mut sops: Vec<SopPointer>) -> Vec<SopPointer> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for mut sop in sops.drain(..) {
        sop.vault_path = SopPointer::normalized_vault_path(&sop.vault_path);
        if sop.vault_path.is_empty() || !sop.vault_path.starts_with("ORIGIN/") {
            continue;
        }
        if sop.id.trim().is_empty() {
            sop.id = SopPointer::derive_id_from_vault_path(&sop.vault_path);
        }
        sop.summary = sop.summary.trim().to_string();
        let key = sop.vault_path.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(sop);
        }
    }
    out.into_iter().take(20).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tier3Memory {
    #[serde(default)]
    pub recent_focus: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowMemoryEntry {
    #[serde(default)]
    pub intent_signature: String,
    #[serde(default)]
    pub approach_summary: String,
    #[serde(default)]
    pub step_trace: Vec<String>,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub support_count: u64,
    #[serde(default)]
    pub last_seen_at: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowMemory {
    #[serde(default)]
    pub intents: Vec<WorkflowMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryLinks {
    #[serde(default)]
    pub mem_palace_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonaMemoryState {
    #[serde(default)]
    pub meta: MemoryStateMeta,
    #[serde(default)]
    pub identity: IdentityMemory,
    #[serde(default)]
    pub tier1: Tier1Memory,
    #[serde(default)]
    pub tier2: Tier2Memory,
    #[serde(default)]
    pub tier3: Tier3Memory,
    #[serde(default)]
    pub workflow_memory: WorkflowMemory,
    #[serde(default)]
    pub links: MemoryLinks,
}

impl PersonaMemoryState {
    fn migrate_tier2_legacy_known_steps(&mut self) {
        if self.tier2.legacy_known_steps.is_empty() {
            return;
        }
        let mut principles = self.tier1.workflow_principles.clone();
        let mut migrated = 0usize;
        for line in self.tier2.legacy_known_steps.drain(..) {
            if let Some(sop) = SopPointer::from_legacy_line(&line) {
                self.tier2.sops.push(sop);
                migrated += 1;
            } else {
                let t = line.trim();
                if !t.is_empty() {
                    principles.push(t.to_string());
                }
            }
        }
        if migrated > 0 || !principles.is_empty() {
            self.tier1.workflow_principles = dedupe_trimmed_lines(&principles);
        }
    }

    pub fn normalize(&mut self) {
        self.meta.version = MEMORY_SCHEMA_VERSION;
        self.identity.non_negotiables = dedupe_trimmed_lines(&self.identity.non_negotiables);
        self.tier1.stable_facts = dedupe_trimmed_lines(&self.tier1.stable_facts);
        self.tier1.workflow_principles = dedupe_trimmed_lines(&self.tier1.workflow_principles);
        self.tier3.recent_focus = dedupe_trimmed_lines(&self.tier3.recent_focus)
            .into_iter()
            .take(15)
            .collect();
        self.links.mem_palace_refs = dedupe_trimmed_lines(&self.links.mem_palace_refs);

        let now = Utc::now().to_rfc3339();
        if self.meta.updated_at.trim().is_empty() {
            self.meta.updated_at = now.clone();
        }

        self.tier2.user_terminology = dedupe_trimmed_lines(&self.tier2.user_terminology)
            .into_iter()
            .take(40)
            .collect();
        self.migrate_tier2_legacy_known_steps();
        self.tier2.sops = dedupe_sops(std::mem::take(&mut self.tier2.sops));
        self.tier2.preferences = dedupe_trimmed_lines(&self.tier2.preferences)
            .into_iter()
            .take(40)
            .collect();
        self.tier2.legacy_migration = None;

        if !self.tier2.legacy_active_projects.is_empty() {
            let mut terminology = self.tier2.user_terminology.clone();
            let mut sops = self.tier2.sops.clone();
            let mut preferences = self.tier2.preferences.clone();
            let mut principles = self.tier1.workflow_principles.clone();
            let mut moved_terminology = 0usize;
            let mut moved_sops = 0usize;
            let mut moved_preferences = 0usize;
            let mut legacy_known_steps_to_principles = 0usize;
            let mut dropped = 0usize;
            for p in self.tier2.legacy_active_projects.drain(..) {
                let summary = p.summary.trim();
                if summary.is_empty() {
                    dropped += 1;
                    continue;
                }
                let lower = summary.to_ascii_lowercase();
                if lower.contains("published")
                    || lower.contains("scrapped")
                    || lower.contains("completed")
                    || lower.contains("cancelled")
                    || lower.contains("canceled")
                {
                    dropped += 1;
                    continue;
                }
                if lower.starts_with("term:")
                    || lower.starts_with("terminology:")
                    || lower.starts_with("[term]")
                {
                    terminology.push(summary.to_string());
                    moved_terminology += 1;
                    continue;
                }
                if let Some(sop) = SopPointer::from_legacy_line(summary) {
                    sops.push(sop);
                    moved_sops += 1;
                    continue;
                }
                if lower.starts_with("step:")
                    || lower.starts_with("known step:")
                    || lower.starts_with("sop:")
                    || lower.starts_with("[step]")
                {
                    principles.push(summary.to_string());
                    legacy_known_steps_to_principles += 1;
                    continue;
                }
                if lower.starts_with("pref:")
                    || lower.starts_with("preference:")
                    || lower.starts_with("[preference]")
                {
                    preferences.push(summary.to_string());
                    moved_preferences += 1;
                    continue;
                }
                principles.push(summary.to_string());
                legacy_known_steps_to_principles += 1;
            }
            self.tier2.user_terminology = dedupe_trimmed_lines(&terminology)
                .into_iter()
                .take(40)
                .collect();
            self.tier2.sops = dedupe_sops(sops);
            self.tier2.preferences = dedupe_trimmed_lines(&preferences)
                .into_iter()
                .take(40)
                .collect();
            self.tier1.workflow_principles = dedupe_trimmed_lines(&principles);
            self.tier2.legacy_migration = Some(Tier2LegacyMigrationStats {
                moved_terminology,
                moved_sops,
                moved_preferences,
                dropped_active_projects: dropped,
                legacy_known_steps_to_principles,
            });
        }

        let mut by_intent: HashMap<String, WorkflowMemoryEntry> = HashMap::new();
        for mut item in self.workflow_memory.intents.drain(..) {
            item.intent_signature = item.intent_signature.trim().to_ascii_lowercase();
            if item.intent_signature.is_empty() {
                continue;
            }
            item.approach_summary = item.approach_summary.trim().to_string();
            item.step_trace = dedupe_trimmed_lines(&item.step_trace);
            item.evidence_refs = dedupe_trimmed_lines(&item.evidence_refs);
            item.confidence = item.confidence.clamp(0.0, 1.0);
            if item.outcome.trim().is_empty() {
                item.outcome = "unknown".to_string();
            }
            if item.last_seen_at.trim().is_empty() {
                item.last_seen_at = now.clone();
            }
            match by_intent.get(&item.intent_signature) {
                Some(existing) if existing.support_count >= item.support_count => {}
                _ => {
                    by_intent.insert(item.intent_signature.clone(), item);
                }
            }
        }
        self.workflow_memory.intents = by_intent.into_values().collect();
        self.workflow_memory
            .intents
            .sort_by_key(|a| std::cmp::Reverse(a.support_count));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub ts: String,
    pub event_type: String,
    pub actor: String,
    pub chat_id: i64,
    pub persona_id: i64,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct MemoryManager {
    /// Directory containing groups/ (for per-chat memory and daily logs).
    data_dir: PathBuf,
    /// Global AGENTS.md is read/written from workspace root shared/AGENTS.md (single source of truth).
    working_dir: PathBuf,
    /// Optional override for principles path (relative to working_dir). Default: AGENTS.md at workspace root.
    principles_path_override: Option<String>,
}

impl MemoryManager {
    pub fn new(data_dir: &str, working_dir: &str) -> Self {
        MemoryManager::with_principles_path(data_dir, working_dir, None)
    }

    /// Create MemoryManager with optional principles path override (e.g. "shared/ORIGIN/AGENTS.md").
    pub fn with_principles_path(
        data_dir: &str,
        working_dir: &str,
        principles_path_override: Option<String>,
    ) -> Self {
        MemoryManager {
            data_dir: PathBuf::from(data_dir).join("groups"),
            working_dir: PathBuf::from(working_dir),
            principles_path_override: principles_path_override.filter(|p| !p.trim().is_empty()),
        }
    }

    /// Path for global principles/memory: workspace root shared/AGENTS.md (single source of truth).
    fn global_memory_path(&self) -> PathBuf {
        self.working_dir.join("shared").join("AGENTS.md")
    }

    fn chat_memory_path(&self, chat_id: i64) -> PathBuf {
        self.data_dir.join(chat_id.to_string()).join("AGENTS.md")
    }

    /// Path for shared principles for all chats/personas: workspace_dir/AGENTS.md (at workspace root),
    /// or vault.principles_path if configured (e.g. shared/ORIGIN/AGENTS.md).
    pub fn groups_root_memory_path(&self) -> PathBuf {
        if let Some(ref p) = self.principles_path_override {
            self.working_dir.join(p.trim().trim_start_matches('/'))
        } else {
            self.working_dir.join("AGENTS.md")
        }
    }

    /// Path string for AGENTS.md (principles, for display in system prompt).
    pub fn groups_root_memory_path_display(&self) -> String {
        self.groups_root_memory_path().to_string_lossy().to_string()
    }

    /// Path for per-persona tiered memory: groups/{chat_id}/{persona_id}/MEMORY.md.
    pub fn persona_memory_path(&self, chat_id: i64, persona_id: i64) -> PathBuf {
        self.data_dir
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join("MEMORY.md")
    }

    /// Path for canonical per-persona memory state.
    pub fn persona_memory_state_path(&self, chat_id: i64, persona_id: i64) -> PathBuf {
        self.data_dir
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join(MEMORY_STATE_FILE)
    }

    /// Path for append-only canonical memory events.
    pub fn persona_memory_events_path(&self, chat_id: i64, persona_id: i64) -> PathBuf {
        self.data_dir
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join(MEMORY_EVENTS_FILE)
    }

    /// Path for per-persona daily log: `groups/{chat_id}/{persona_id}/memory/YYYY-MM-DD.md`
    fn daily_log_path(&self, chat_id: i64, persona_id: i64, date: &str) -> PathBuf {
        self.data_dir
            .join(chat_id.to_string())
            .join(persona_id.to_string())
            .join("memory")
            .join(format!("{date}.md"))
    }

    pub fn read_global_memory(&self) -> Option<String> {
        let path = self.global_memory_path();
        std::fs::read_to_string(path).ok()
    }

    /// Read shared AGENTS.md at workspace root. Used as principles for all personas.
    pub fn read_groups_root_memory(&self) -> Option<String> {
        let path = self.groups_root_memory_path();
        std::fs::read_to_string(path).ok()
    }

    /// Write principles to the same path as [`Self::read_groups_root_memory`] (workspace AGENTS.md or `vault.principles_path`).
    pub fn write_groups_root_memory(&self, content: &str) -> std::io::Result<()> {
        let path = self.groups_root_memory_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    pub fn read_chat_memory(&self, chat_id: i64) -> Option<String> {
        let path = self.chat_memory_path(chat_id);
        std::fs::read_to_string(path).ok()
    }

    /// Read per-persona tiered memory from canonical JSON state.
    /// Falls back to legacy MEMORY.md and migrates when possible.
    pub fn read_persona_memory(&self, chat_id: i64, persona_id: i64) -> Option<String> {
        self.read_or_migrate_persona_memory_state(chat_id, persona_id)
            .map(|s| render_memory_markdown(&s))
            .or_else(|| std::fs::read_to_string(self.persona_memory_path(chat_id, persona_id)).ok())
    }

    /// Read a single daily log file if it exists. `date` must be "YYYY-MM-DD".
    pub fn read_daily_log(&self, chat_id: i64, persona_id: i64, date: &str) -> Option<String> {
        let path = self.daily_log_path(chat_id, persona_id, date);
        std::fs::read_to_string(path).ok()
    }

    /// Read today's and yesterday's daily logs and return combined content for injection.
    /// Returns empty string if neither file exists.
    pub fn read_daily_logs_today_yesterday(&self, chat_id: i64, persona_id: i64) -> String {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let yesterday = (Utc::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let mut out = String::new();
        if let Some(content) = self.read_daily_log(chat_id, persona_id, &yesterday) {
            if !content.trim().is_empty() {
                out.push_str(&format!("## {yesterday}\n{content}\n\n"));
            }
        }
        if let Some(content) = self.read_daily_log(chat_id, persona_id, &today) {
            if !content.trim().is_empty() {
                out.push_str(&format!("## {today}\n{content}\n\n"));
            }
        }
        out.trim().to_string()
    }

    /// Append content to the daily log for the given date. Creates file and parent dir if needed.
    /// `date` must be "YYYY-MM-DD".
    pub fn append_daily_log(
        &self,
        chat_id: i64,
        persona_id: i64,
        date: &str,
        content: &str,
    ) -> std::io::Result<()> {
        let path = self.daily_log_path(chat_id, persona_id, date);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        if !content.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        f.write_all(content.as_bytes())?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_global_memory(&self, content: &str) -> std::io::Result<()> {
        let path = self.global_memory_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    #[allow(dead_code)]
    pub fn write_chat_memory(&self, chat_id: i64, content: &str) -> std::io::Result<()> {
        let path = self.chat_memory_path(chat_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    /// Build memory context for the system prompt from canonical per-persona memory_state.json.
    /// Principles (workspace_dir/AGENTS.md) are loaded separately and injected as the "Principles" section.
    /// Daily logs are intentionally excluded to reduce context pollution.
    ///
    /// Shape is controlled by [`MemoryPromptBuildOptions::from_env`] (`MEMORY_PROMPT_MODE`,
    /// `MEMORY_PROMPT_INCLUDE_LEGEND`, `WORKFLOW_PRINCIPLES_PROMPT_MAX`). Default is JSON-only
    /// with a cap on `tier1.workflow_principles` in the prompt; `patch_memory_state` should target
    /// the same JSON paths—use `read_memory_state` first when you need the uncapped file or legend.
    pub fn build_memory_context(&self, chat_id: i64, persona_id: i64) -> String {
        self.build_memory_context_with_options(
            chat_id,
            persona_id,
            MemoryPromptBuildOptions::from_env(),
        )
    }

    /// Same as [`Self::build_memory_context`] with explicit options (tests, custom runners).
    pub fn build_memory_context_with_options(
        &self,
        chat_id: i64,
        persona_id: i64,
        options: MemoryPromptBuildOptions,
    ) -> String {
        let mut context = String::new();

        if let Some(state) = self.read_or_migrate_persona_memory_state(chat_id, persona_id) {
            let (prompt_state, trunc_note) =
                cap_workflow_principles_for_prompt(&state, options.workflow_principles_prompt_max);
            let Ok(state_json) = serde_json::to_string_pretty(&prompt_state) else {
                return context;
            };

            let push_legend = |ctx: &mut String| {
                ctx.push_str("<memory_field_legend>\n");
                ctx.push_str(&render_memory_field_legend_compact());
                ctx.push_str("\n</memory_field_legend>\n");
            };
            let push_trunc_note = |ctx: &mut String| {
                if let Some(ref n) = trunc_note {
                    ctx.push_str("<memory_prompt_note>\n");
                    ctx.push_str(n);
                    ctx.push_str("\n</memory_prompt_note>\n");
                }
            };

            match options.mode {
                MemoryPromptMode::Json => {
                    if options.include_legend {
                        push_legend(&mut context);
                    }
                    context.push_str("<memory_state_json>\n");
                    context.push_str(&state_json);
                    context.push_str("\n</memory_state_json>\n");
                    push_trunc_note(&mut context);
                }
                MemoryPromptMode::Markdown => {
                    if options.include_legend {
                        push_legend(&mut context);
                    }
                    context.push_str("<memory_this_persona>\n");
                    context.push_str(&render_memory_markdown(&prompt_state));
                    context.push_str("\n</memory_this_persona>\n");
                    push_trunc_note(&mut context);
                }
                MemoryPromptMode::Both => {
                    if options.include_legend {
                        push_legend(&mut context);
                    }
                    context.push_str("<memory_this_persona>\n");
                    context.push_str(&render_memory_markdown(&prompt_state));
                    context.push_str("\n</memory_this_persona>\n");
                    context.push_str("<memory_state_json>\n");
                    context.push_str(&state_json);
                    context.push_str("\n</memory_state_json>\n");
                    push_trunc_note(&mut context);
                }
            }
        } else if let Some(persona_mem) =
            std::fs::read_to_string(self.persona_memory_path(chat_id, persona_id)).ok()
        {
            if !persona_mem.trim().is_empty() {
                context.push_str("<memory_this_persona>\n");
                context.push_str(&persona_mem);
                context.push_str("\n</memory_this_persona>\n");
            }
        }

        context
    }

    pub fn read_persona_memory_state(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Option<PersonaMemoryState> {
        let persist_migration_if_needed =
            |this: &MemoryManager, mut state: PersonaMemoryState| -> PersonaMemoryState {
                if let Some(stats) = state.tier2.legacy_migration.clone() {
                    let _ = this.append_persona_memory_event(
                    chat_id,
                    persona_id,
                    "memory_state_migrated_tier2_knowledge",
                    "system",
                    json!({
                        "moved_terminology": stats.moved_terminology,
                        "moved_sops": stats.moved_sops,
                        "legacy_known_steps_to_principles": stats.legacy_known_steps_to_principles,
                        "moved_preferences": stats.moved_preferences,
                        "dropped_active_projects": stats.dropped_active_projects,
                    }),
                );
                    let _ = this.write_persona_memory_state(chat_id, persona_id, state.clone());
                    state.tier2.legacy_migration = None;
                }
                state
            };
        let path = self.persona_memory_state_path(chat_id, persona_id);
        let content = std::fs::read_to_string(&path).ok()?;
        match self.parse_and_validate_state(&content) {
            Ok(state) => Some(persist_migration_if_needed(self, state)),
            Err(primary_err) => {
                let backup_path = path.with_extension("json.bak");
                let recovered = std::fs::read_to_string(&backup_path)
                    .ok()
                    .and_then(|backup_content| self.parse_and_validate_state(&backup_content).ok());
                if let Some(state) = recovered {
                    let _ = std::fs::copy(&backup_path, &path);
                    let _ = self.append_persona_memory_event(
                        chat_id,
                        persona_id,
                        "memory_parse_error_recovered",
                        "system",
                        json!({
                            "path": path.to_string_lossy().to_string(),
                            "backup_path": backup_path.to_string_lossy().to_string(),
                            "error": primary_err,
                        }),
                    );
                    Some(persist_migration_if_needed(self, state))
                } else {
                    let _ = self.append_persona_memory_event(
                        chat_id,
                        persona_id,
                        "memory_parse_error",
                        "system",
                        json!({
                            "path": path.to_string_lossy().to_string(),
                            "backup_path": backup_path.to_string_lossy().to_string(),
                            "error": primary_err,
                        }),
                    );
                    None
                }
            }
        }
    }

    pub fn validate_memory_state(&self, state: &PersonaMemoryState) -> Result<(), String> {
        if state.meta.version == 0 {
            return Err("meta.version must be >= 1".to_string());
        }
        if state.tier3.recent_focus.len() > 15 {
            return Err("tier3.recent_focus must not exceed 15 entries".to_string());
        }
        for entry in &state.workflow_memory.intents {
            if entry.intent_signature.trim().is_empty() {
                return Err("workflow_memory.intents intent_signature cannot be empty".to_string());
            }
            if !(0.0..=1.0).contains(&entry.confidence) {
                return Err(format!(
                    "workflow_memory confidence out of range for intent '{}'",
                    entry.intent_signature
                ));
            }
        }
        for (i, sop) in state.tier2.sops.iter().enumerate() {
            let path = sop.vault_path.trim();
            if path.is_empty() {
                return Err(format!("tier2.sops[{i}].vault_path cannot be empty"));
            }
            if !path.starts_with("ORIGIN/") {
                return Err(format!(
                    "tier2.sops[{i}].vault_path must start with ORIGIN/ (got '{path}')"
                ));
            }
        }
        Ok(())
    }

    pub fn write_persona_memory_state(
        &self,
        chat_id: i64,
        persona_id: i64,
        mut state: PersonaMemoryState,
    ) -> std::io::Result<()> {
        state.normalize();
        if let Err(err) = self.validate_memory_state(&state) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, err));
        }
        state.meta.revision = state.meta.revision.saturating_add(1);
        state.meta.updated_at = Utc::now().to_rfc3339();

        let path = self.persona_memory_state_path(chat_id, persona_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension("json.tmp");
        let backup_path = path.with_extension("json.bak");
        let bytes = serde_json::to_vec_pretty(&state).map_err(std::io::Error::other)?;
        std::fs::write(&tmp_path, bytes)?;
        if path.exists() {
            let _ = std::fs::copy(&path, &backup_path);
        }
        std::fs::rename(&tmp_path, &path)?;
        let _ = self.write_origin_snapshot(chat_id, persona_id, &state);
        Ok(())
    }

    pub fn append_persona_memory_event(
        &self,
        chat_id: i64,
        persona_id: i64,
        event_type: &str,
        actor: &str,
        payload: serde_json::Value,
    ) -> std::io::Result<()> {
        let path = self.persona_memory_events_path(chat_id, persona_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let event = MemoryEvent {
            ts: Utc::now().to_rfc3339(),
            event_type: event_type.to_string(),
            actor: actor.to_string(),
            chat_id,
            persona_id,
            payload,
        };
        let line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn read_or_migrate_persona_memory_state(
        &self,
        chat_id: i64,
        persona_id: i64,
    ) -> Option<PersonaMemoryState> {
        if let Some(state) = self.read_persona_memory_state(chat_id, persona_id) {
            return Some(state);
        }
        let legacy_path = self.persona_memory_path(chat_id, persona_id);
        let legacy = std::fs::read_to_string(&legacy_path).ok()?;
        let mut state = legacy_markdown_to_state(&legacy);
        state.normalize();
        let _ = self.write_persona_memory_state(chat_id, persona_id, state.clone());
        let _ = self.append_persona_memory_event(
            chat_id,
            persona_id,
            "memory_migrated_from_markdown",
            "migration",
            json!({
                "legacy_path": legacy_path.to_string_lossy().to_string(),
                "schema_version": MEMORY_SCHEMA_VERSION
            }),
        );
        Some(state)
    }

    pub fn ensure_persona_memory_state_exists(
        &self,
        chat_id: i64,
        persona_id: i64,
        display_name: Option<&str>,
    ) -> std::io::Result<()> {
        if self.persona_memory_state_path(chat_id, persona_id).exists() {
            return Ok(());
        }
        let mut state = PersonaMemoryState::default();
        state.meta.version = MEMORY_SCHEMA_VERSION;
        state.meta.updated_at = Utc::now().to_rfc3339();
        if let Some(name) = display_name {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                state.identity.display_name = trimmed.to_string();
            }
        }
        self.write_persona_memory_state(chat_id, persona_id, state.clone())?;
        self.append_persona_memory_event(
            chat_id,
            persona_id,
            "memory_state_initialized",
            "system",
            json!({ "schema_version": MEMORY_SCHEMA_VERSION }),
        )?;
        Ok(())
    }

    fn parse_and_validate_state(&self, content: &str) -> Result<PersonaMemoryState, String> {
        let mut state: PersonaMemoryState =
            serde_json::from_str(content).map_err(|e| format!("invalid_json: {e}"))?;
        state.normalize();
        self.validate_memory_state(&state)
            .map_err(|e| format!("invalid_state: {e}"))?;
        Ok(state)
    }

    fn write_origin_snapshot(
        &self,
        chat_id: i64,
        persona_id: i64,
        state: &PersonaMemoryState,
    ) -> std::io::Result<()> {
        let snapshot_path = self
            .working_dir
            .join("shared")
            .join("ORIGIN")
            .join("MemorySnapshots")
            .join(format!("chat-{}-persona-{}.md", chat_id, persona_id));
        if let Some(parent) = snapshot_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content = String::new();
        content.push_str(&format!(
            "# Memory Snapshot chat={} persona={}\n\n",
            chat_id, persona_id
        ));
        content.push_str(&format!(
            "Updated: {}\nSchema: {}\nRevision: {}\n\n",
            state.meta.updated_at, state.meta.version, state.meta.revision
        ));
        content.push_str(&render_memory_markdown(state));
        std::fs::write(snapshot_path, content)
    }

    #[allow(dead_code)]
    pub fn groups_dir(&self) -> &Path {
        &self.data_dir
    }
}

fn dedupe_trimmed_lines(lines: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn extract_tier_sections(full: &str) -> [String; 3] {
    const TIER_HEADERS: [&str; 3] = [
        "## Tier 1 — Long term",
        "## Tier 2 — Mid term",
        "## Tier 3 — Short term",
    ];
    let mut sections = [String::new(), String::new(), String::new()];
    let mut current_tier: Option<usize> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    let mut flush_current = |tier_idx: usize, lines: &mut Vec<&str>| {
        let block = lines.join("\n").trim().to_string();
        lines.clear();
        if block.is_empty() {
            return;
        }
        if sections[tier_idx].is_empty() {
            sections[tier_idx] = block;
        } else {
            sections[tier_idx].push_str("\n\n");
            sections[tier_idx].push_str(&block);
        }
    };

    for line in full.lines() {
        if line.starts_with("## ") {
            if let Some(prev_idx) = current_tier {
                flush_current(prev_idx, &mut current_lines);
            }
            current_tier = TIER_HEADERS.iter().position(|h| line.trim() == *h);
            continue;
        }
        if current_tier.is_some() {
            current_lines.push(line);
        }
    }
    if let Some(prev_idx) = current_tier {
        flush_current(prev_idx, &mut current_lines);
    }
    sections
}

fn legacy_markdown_to_state(markdown: &str) -> PersonaMemoryState {
    let tiers = extract_tier_sections(markdown);
    let tier1_lines: Vec<String> = tiers[0]
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let tier2_known_steps: Vec<String> = tiers[1]
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    // Legacy Tier 2 lines that are not SOP pointers are preserved as preferences
    // so migration does not silently drop mid-term notes.
    let mut tier2_sops = Vec::new();
    let mut tier2_preferences = Vec::new();
    for line in &tier2_known_steps {
        if let Some(sop) = SopPointer::from_legacy_line(line) {
            tier2_sops.push(sop);
        } else {
            let cleaned = line.trim_start_matches('-').trim();
            if !cleaned.is_empty() {
                tier2_preferences.push(cleaned.to_string());
            }
        }
    }
    let tier3_lines: Vec<String> = tiers[2]
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let mut state = PersonaMemoryState {
        meta: MemoryStateMeta {
            version: MEMORY_SCHEMA_VERSION,
            revision: 0,
            updated_at: Utc::now().to_rfc3339(),
        },
        identity: IdentityMemory::default(),
        tier1: Tier1Memory {
            stable_facts: tier1_lines,
            workflow_principles: Vec::new(),
        },
        tier2: Tier2Memory {
            user_terminology: Vec::new(),
            sops: tier2_sops,
            preferences: tier2_preferences,
            legacy_known_steps: Vec::new(),
            legacy_active_projects: Vec::new(),
            legacy_migration: None,
        },
        tier3: Tier3Memory {
            recent_focus: tier3_lines,
        },
        workflow_memory: WorkflowMemory::default(),
        links: MemoryLinks::default(),
    };
    state.normalize();
    state
}

const WORKFLOW_MEMORY_INTENTS_PROMPT_MAX: usize = 10;

fn append_identity_sections(sections: &mut Vec<String>, state: &PersonaMemoryState) {
    let mut identity = Vec::new();
    if !state.identity.display_name.trim().is_empty() {
        identity.push(format!("**Name:** {}", state.identity.display_name.trim()));
    }
    if !state.identity.self_model.trim().is_empty() {
        identity.push(state.identity.self_model.trim().to_string());
    }
    if !state.identity.voice_style.trim().is_empty() {
        identity.push(format!("**Voice:** {}", state.identity.voice_style.trim()));
    }
    for item in &state.identity.non_negotiables {
        let t = item.trim();
        if !t.is_empty() {
            identity.push(format!("- {t}"));
        }
    }
    if identity.is_empty() {
        return;
    }
    let mut block = String::from("### Identity\n\n");
    for line in identity {
        block.push_str(&line);
        block.push('\n');
    }
    sections.push(block);
}

fn append_tier1_sections(sections: &mut Vec<String>, state: &PersonaMemoryState) {
    let mut facts = Vec::new();
    for item in &state.tier1.stable_facts {
        let t = item.trim();
        if !t.is_empty() {
            facts.push(format!("- {t}"));
        }
    }
    let principles: Vec<String> = state
        .tier1
        .workflow_principles
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("- {p}"))
        .collect();
    if facts.is_empty() && principles.is_empty() {
        return;
    }
    let mut block = String::from("### Long-term context (Tier 1)\n\n");
    for line in &facts {
        block.push_str(line);
        block.push('\n');
    }
    if !principles.is_empty() {
        if !facts.is_empty() {
            block.push('\n');
        }
        block.push_str("Workflow principles:\n");
        for line in &principles {
            block.push_str(line);
            block.push('\n');
        }
    }
    sections.push(block);
}

/// Fill empty identity/tier1 fields for prompt rendering only (does not write disk).
pub fn enrich_persona_memory_for_prompt(
    mut state: PersonaMemoryState,
    persona_display_name: Option<&str>,
    legacy_memory_markdown: Option<&str>,
) -> PersonaMemoryState {
    if state.identity.display_name.trim().is_empty() {
        if let Some(name) = persona_display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            state.identity.display_name = name.to_string();
        }
    }
    let tier1_empty = state.tier1.stable_facts.iter().all(|s| s.trim().is_empty())
        && state
            .tier1
            .workflow_principles
            .iter()
            .all(|s| s.trim().is_empty());
    if tier1_empty {
        if let Some(md) = legacy_memory_markdown
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let legacy = legacy_markdown_to_state(md);
            if !legacy.tier1.stable_facts.is_empty() {
                state.tier1.stable_facts = legacy.tier1.stable_facts;
            }
        }
    }
    state.normalize();
    state
}

/// Identity + Tier 1 prose for the main agent **system prompt** (stable persona context).
pub fn render_identity_and_tier1_for_system(
    state: &PersonaMemoryState,
    workflow_principles_max: usize,
) -> String {
    let (prompt_state, trunc_note) =
        cap_workflow_principles_for_prompt(state, workflow_principles_max);
    let mut sections = Vec::new();
    append_identity_sections(&mut sections, &prompt_state);
    append_tier1_sections(&mut sections, &prompt_state);
    if sections.is_empty() {
        return String::new();
    }
    let mut out = sections.join("\n");
    if let Some(note) = trunc_note {
        out.push_str("\n_Note: ");
        out.push_str(&note);
        out.push_str("_\n");
    }
    out
}

/// Tier 2/3 and links for the `[persona_context]` message block.
pub fn render_persona_context_memory_with_options(
    state: &PersonaMemoryState,
    suppress_recent_focus: bool,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    if !state.tier2.sops.is_empty() {
        let mut block = String::from("### SOPs (Tier 2)\n\n");
        for sop in &state.tier2.sops {
            if sop.vault_path.trim().is_empty() {
                continue;
            }
            let id = if sop.id.trim().is_empty() {
                SopPointer::derive_id_from_vault_path(&sop.vault_path)
            } else {
                sop.id.trim().to_string()
            };
            if sop.summary.trim().is_empty() {
                block.push_str(&format!("- **{id}** → `{}`\n", sop.vault_path.trim()));
            } else {
                block.push_str(&format!(
                    "- **{id}** → `{}` — {}\n",
                    sop.vault_path.trim(),
                    sop.summary.trim()
                ));
            }
        }
        sections.push(block);
    }

    let mut tier2_lines: Vec<String> = Vec::new();
    for t in &state.tier2.user_terminology {
        let s = t.trim();
        if !s.is_empty() {
            tier2_lines.push(format!("- Terminology: {s}"));
        }
    }
    for p in &state.tier2.preferences {
        let p = p.trim();
        if !p.is_empty() {
            tier2_lines.push(format!("- Preference: {p}"));
        }
    }
    if !tier2_lines.is_empty() {
        let mut block = String::from("### Tier 2 knowledge\n\n");
        for line in tier2_lines {
            block.push_str(&line);
            block.push('\n');
        }
        sections.push(block);
    }

    let mut tier3: Vec<String> = state
        .tier3
        .recent_focus
        .iter()
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(format!("- {t}"))
            }
        })
        .collect();
    if !suppress_recent_focus && !tier3.is_empty() {
        let mut block = String::from("### Recent focus\n\n");
        for line in tier3.drain(..) {
            block.push_str(&line);
            block.push('\n');
        }
        sections.push(block);
    }

    let intents: Vec<_> = state
        .workflow_memory
        .intents
        .iter()
        .take(WORKFLOW_MEMORY_INTENTS_PROMPT_MAX)
        .filter(|e| !e.intent_signature.trim().is_empty())
        .collect();
    if !intents.is_empty() {
        let mut block = String::from("### Learned approaches\n\n");
        for e in intents {
            let sig = e.intent_signature.trim();
            let approach = e.approach_summary.trim();
            let outcome = e.outcome.trim();
            if approach.is_empty() {
                block.push_str(&format!("- **{sig}** ({outcome})\n"));
            } else {
                block.push_str(&format!("- **{sig}:** {approach} ({outcome})\n"));
            }
        }
        sections.push(block);
    }

    let refs: Vec<String> = state
        .links
        .mem_palace_refs
        .iter()
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(format!("- {t}"))
            }
        })
        .collect();
    if !refs.is_empty() {
        let mut block = String::from("### Mem-Palace references\n\n");
        for line in refs {
            block.push_str(&line);
            block.push('\n');
        }
        sections.push(block);
    }

    if sections.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Memory (background reference)\n\n");
    out.push_str(&sections.join("\n"));
    out
}

/// Tier 2/3 and links for the `[persona_context]` message block.
pub fn render_persona_context_memory(state: &PersonaMemoryState) -> String {
    render_persona_context_memory_with_options(state, false)
}

/// Full compiled memory (all tiers) — used by PTE and tests.
pub fn render_memory_for_llm(state: &PersonaMemoryState, workflow_principles_max: usize) -> String {
    let system = render_identity_and_tier1_for_system(state, workflow_principles_max);
    let ctx = render_persona_context_memory(state);
    match (system.trim().is_empty(), ctx.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => ctx,
        (false, true) => format!("## Memory\n\n{system}"),
        (false, false) => format!("## Memory\n\n{system}\n\n{ctx}"),
    }
}

pub fn render_memory_markdown(state: &PersonaMemoryState) -> String {
    let mut out = String::new();
    out.push_str("# Memory\n\n");

    out.push_str("## Tier 1 — Long term\n\n");
    if !state.identity.display_name.trim().is_empty() {
        out.push_str(&format!(
            "- Identity|display_name={}\n",
            state.identity.display_name.trim()
        ));
    }
    if !state.identity.self_model.trim().is_empty() {
        out.push_str(&format!(
            "- Identity|self_model={}\n",
            state.identity.self_model.trim()
        ));
    }
    if !state.identity.voice_style.trim().is_empty() {
        out.push_str(&format!(
            "- Identity|voice_style={}\n",
            state.identity.voice_style.trim()
        ));
    }
    for item in &state.identity.non_negotiables {
        out.push_str(&format!("- IdentityConstraint|{}\n", item));
    }
    for item in &state.tier1.stable_facts {
        out.push_str(&format!("{item}\n"));
    }
    for item in &state.tier1.workflow_principles {
        out.push_str(&format!("- WorkflowPrinciple|{}\n", item));
    }
    out.push('\n');

    out.push_str("## Tier 2 — Mid term\n\n");
    for term in &state.tier2.user_terminology {
        out.push_str(&format!("- Terminology|{}\n", term.trim()));
    }
    if !state.tier2.sops.is_empty() {
        out.push_str("### SOPs\n\n");
        for sop in &state.tier2.sops {
            let id = if sop.id.trim().is_empty() {
                SopPointer::derive_id_from_vault_path(&sop.vault_path)
            } else {
                sop.id.trim().to_string()
            };
            out.push_str(&format!(
                "- SOP|{id}|{}|{}\n",
                sop.vault_path.trim(),
                sop.summary.trim()
            ));
        }
        out.push('\n');
    }
    for pref in &state.tier2.preferences {
        out.push_str(&format!("- Preference|{}\n", pref.trim()));
    }
    out.push('\n');

    out.push_str("## Tier 3 — Short term\n\n");
    for line in &state.tier3.recent_focus {
        out.push_str(&format!("{line}\n"));
    }
    out
}

fn render_memory_field_legend_compact() -> String {
    let mut out = [
        "meta.version: schema version; normalized to current version.",
        "meta.revision: monotonic state write counter.",
        "meta.updated_at: RFC3339 timestamp of last state write.",
        "identity.display_name: canonical persona name.",
        "identity.self_model: concise self-description for behavior framing.",
        "identity.voice_style: preferred tone/style guidance.",
        "identity.non_negotiables[]: hard constraints the persona should not violate.",
        "tier1.stable_facts[]: long-lived durable facts.",
        "tier1.workflow_principles[]: reusable workflow rules from repeated success.",
        "tier2.user_terminology[]: user-specific terms, aliases, and phrasing.",
        "tier2.sops[]: vault SOP pointers ({ id, vault_path, summary }); load vault_path from ORIGIN when executing.",
        "tier2.preferences[]: durable operating preferences and defaults.",
        "tier3.recent_focus[]: short-term focus items; passive context only; capped to 15.",
        "workflow_memory.intents[]: learned intent-pattern retention entries.",
        "workflow_memory.intents[].intent_signature: canonical lowercased intent key.",
        "workflow_memory.intents[].approach_summary: concise strategy summary.",
        "workflow_memory.intents[].step_trace[]: high-level ordered step/tool flow.",
        "workflow_memory.intents[].outcome: attempt result label (success|failure|unknown).",
        "workflow_memory.intents[].failure_reason: optional failure cause.",
        "workflow_memory.intents[].confidence: confidence score in [0.0, 1.0].",
        "workflow_memory.intents[].support_count: supporting observation count.",
        "workflow_memory.intents[].last_seen_at: RFC3339 last-observed timestamp.",
        "workflow_memory.intents[].evidence_refs[]: references backing the pattern.",
        "links.mem_palace_refs[]: retrieval alignment references to mem-palace artifacts.",
    ]
    .join("\n");
    if out.len() > MEMORY_FIELD_LEGEND_MAX_CHARS {
        out.truncate(MEMORY_FIELD_LEGEND_MAX_CHARS);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn test_memory_manager() -> (MemoryManager, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_mem_test_{}",
            uuid::Uuid::new_v4()
        ));
        let dir_str = dir.to_str().unwrap();
        let mm = MemoryManager::new(dir_str, dir_str);
        (mm, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_global_memory_path() {
        let (mm, dir) = test_memory_manager();
        let path = mm.global_memory_path();
        assert!(
            path.ends_with(Path::new("shared").join("AGENTS.md")),
            "path = {}",
            path.display()
        );
        cleanup(&dir);
    }

    #[test]
    fn test_chat_memory_path() {
        let (mm, dir) = test_memory_manager();
        let path = mm.chat_memory_path(12345);
        assert!(path.ends_with(Path::new("groups").join("12345").join("AGENTS.md")));
        cleanup(&dir);
    }

    #[test]
    fn test_persona_memory_path() {
        let (mm, dir) = test_memory_manager();
        let path = mm.persona_memory_path(997894126, 1);
        assert!(path.ends_with(Path::new("997894126").join("1").join("MEMORY.md")));
        cleanup(&dir);
    }

    #[test]
    fn test_groups_root_memory_path_display() {
        let (mm, dir) = test_memory_manager();
        let s = mm.groups_root_memory_path_display();
        assert!(s.contains("AGENTS.md"));
        cleanup(&dir);
    }

    #[test]
    fn test_groups_root_memory_path() {
        let (mm, dir) = test_memory_manager();
        let path = mm.groups_root_memory_path();
        assert!(path.ends_with("AGENTS.md"));
        cleanup(&dir);
    }

    #[test]
    fn test_read_nonexistent_memory() {
        let (mm, dir) = test_memory_manager();
        assert!(mm.read_global_memory().is_none());
        assert!(mm.read_chat_memory(100).is_none());
        assert!(mm.read_persona_memory(100, 1).is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_read_persona_memory_migrates_legacy_markdown() {
        let (mm, dir) = test_memory_manager();
        let legacy_path = mm.persona_memory_path(42, 2);
        if let Some(p) = legacy_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        std::fs::write(
            &legacy_path,
            "# Memory\n\n## Tier 1 — Long term\n\n- old fact\n\n## Tier 2 — Mid term\n\n- project alpha\n\n## Tier 3 — Short term\n\n- recent focus\n",
        )
        .unwrap();

        let content = mm.read_persona_memory(42, 2).unwrap();
        assert!(content.contains("old fact"));
        assert!(content.contains("project alpha"));
        assert!(content.contains("recent focus"));

        let state_path = mm.persona_memory_state_path(42, 2);
        assert!(state_path.exists());
        cleanup(&dir);
    }

    #[test]
    fn test_write_and_read_global_memory() {
        let (mm, dir) = test_memory_manager();
        mm.write_global_memory("global notes").unwrap();
        let content = mm.read_global_memory().unwrap();
        assert_eq!(content, "global notes");
        cleanup(&dir);
    }

    #[test]
    fn test_write_and_read_chat_memory() {
        let (mm, dir) = test_memory_manager();
        mm.write_chat_memory(42, "chat 42 notes").unwrap();
        let content = mm.read_chat_memory(42).unwrap();
        assert_eq!(content, "chat 42 notes");

        // Different chat should be empty
        assert!(mm.read_chat_memory(99).is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_empty() {
        let (mm, dir) = test_memory_manager();
        let ctx = mm.build_memory_context(100, 1);
        assert!(ctx.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_with_global_only() {
        let (mm, dir) = test_memory_manager();
        mm.write_global_memory("I am global memory").unwrap();
        let ctx = mm.build_memory_context(100, 1);
        // Global AGENTS.md is not part of memory_context; it is loaded separately as principles
        assert!(ctx.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_persona_and_daily_only() {
        let (mm, dir) = test_memory_manager();
        mm.write_global_memory("global stuff").unwrap();
        let mut state = PersonaMemoryState::default();
        state.tier1.stable_facts = vec!["persona memory".to_string()];
        mm.write_persona_memory_state(100, 1, state).unwrap();
        let ctx = mm.build_memory_context_with_options(
            100,
            1,
            MemoryPromptBuildOptions::all_sections_no_workflow_cap(),
        );
        assert!(ctx.contains("<memory_field_legend>"));
        assert!(ctx.contains("meta.version: schema version; normalized to current version."));
        assert!(
            ctx.contains("workflow_memory.intents[].confidence: confidence score in [0.0, 1.0].")
        );
        assert!(ctx.contains("<memory_this_persona>"));
        assert!(ctx.contains("persona memory"));
        assert!(ctx.contains("<memory_state_json>"));
        assert!(!ctx.contains("global stuff"));
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_ignores_whitespace_only_persona() {
        let (mm, dir) = test_memory_manager();
        let mut state = PersonaMemoryState::default();
        state.tier1.stable_facts = vec![];
        state.tier2.user_terminology = vec![];
        state.tier2.sops = vec![];
        state.tier2.preferences = vec![];
        state.tier3.recent_focus = vec![];
        mm.write_persona_memory_state(100, 1, state).unwrap();
        let ctx = mm.build_memory_context_with_options(
            100,
            1,
            MemoryPromptBuildOptions::all_sections_no_workflow_cap(),
        );
        assert!(ctx.contains("<memory_field_legend>"));
        assert!(ctx.contains("<memory_this_persona>"));
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_json_mode_omits_markdown_and_legend_by_default() {
        let (mm, dir) = test_memory_manager();
        let mut state = PersonaMemoryState::default();
        state.tier1.stable_facts = vec!["fact a".into()];
        mm.write_persona_memory_state(7, 2, state).unwrap();
        let ctx = mm.build_memory_context_with_options(
            7,
            2,
            MemoryPromptBuildOptions {
                mode: MemoryPromptMode::Json,
                include_legend: false,
                workflow_principles_prompt_max: 0,
            },
        );
        assert!(ctx.contains("<memory_state_json>"));
        assert!(!ctx.contains("<memory_this_persona>"));
        assert!(!ctx.contains("<memory_field_legend>"));
        assert!(ctx.contains("fact a"));
        cleanup(&dir);
    }

    #[test]
    fn test_build_memory_context_truncates_workflow_principles_in_prompt() {
        let (mm, dir) = test_memory_manager();
        let mut state = PersonaMemoryState::default();
        state.tier1.workflow_principles = (0..12).map(|i| format!("wf-{i}")).collect();
        mm.write_persona_memory_state(3, 4, state).unwrap();
        let ctx = mm.build_memory_context_with_options(
            3,
            4,
            MemoryPromptBuildOptions {
                mode: MemoryPromptMode::Json,
                include_legend: false,
                workflow_principles_prompt_max: 5,
            },
        );
        assert!(ctx.contains("<memory_prompt_note>"));
        assert!(ctx.contains("wf-4"));
        assert!(!ctx.contains("wf-11"));
        let disk = mm.read_persona_memory_state(3, 4).unwrap();
        assert_eq!(disk.tier1.workflow_principles.len(), 12);
        cleanup(&dir);
    }

    #[test]
    fn test_memory_field_legend_compact_is_deterministic_and_bounded() {
        let legend_a = render_memory_field_legend_compact();
        let legend_b = render_memory_field_legend_compact();
        assert_eq!(legend_a, legend_b);
        assert!(legend_a.contains("meta.version: schema version; normalized to current version."));
        assert!(legend_a.contains(
            "links.mem_palace_refs[]: retrieval alignment references to mem-palace artifacts."
        ));
        assert!(legend_a.len() <= MEMORY_FIELD_LEGEND_MAX_CHARS);
    }

    #[test]
    fn test_groups_dir() {
        let (mm, dir) = test_memory_manager();
        assert!(mm.groups_dir().ends_with("groups"));
        cleanup(&dir);
    }

    #[test]
    fn test_daily_log_append_and_read() {
        let (mm, dir) = test_memory_manager();
        mm.append_daily_log(100, 1, "2025-01-15", "Note from day one.\n")
            .unwrap();
        mm.append_daily_log(100, 1, "2025-01-15", "Second line.")
            .unwrap();
        let content = mm.read_daily_log(100, 1, "2025-01-15").unwrap();
        assert!(content.contains("Note from day one."));
        assert!(content.contains("Second line."));
        assert!(mm.read_daily_log(100, 1, "2025-01-14").is_none());
        cleanup(&dir);
    }

    #[test]
    fn test_write_and_read_persona_memory_state() {
        let (mm, dir) = test_memory_manager();
        let mut state = PersonaMemoryState::default();
        state.identity.display_name = "Assistant".into();
        state.tier3.recent_focus = vec!["- one".into(), "- one".into(), "- two".into()];
        mm.write_persona_memory_state(10, 3, state).unwrap();
        let read_back = mm.read_persona_memory_state(10, 3).unwrap();
        assert_eq!(read_back.identity.display_name, "Assistant");
        assert_eq!(read_back.tier3.recent_focus.len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn test_read_persona_memory_state_recovers_from_backup_on_parse_error() {
        let (mm, dir) = test_memory_manager();
        let mut valid = PersonaMemoryState::default();
        valid.identity.display_name = "Recovered".into();
        mm.write_persona_memory_state(22, 4, valid).unwrap();

        let path = mm.persona_memory_state_path(22, 4);
        let backup_path = path.with_extension("json.bak");
        let original = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&backup_path, &original).unwrap();
        std::fs::write(&path, "{ invalid json").unwrap();

        let recovered = mm.read_persona_memory_state(22, 4).unwrap();
        assert_eq!(recovered.identity.display_name, "Recovered");
        let repaired = std::fs::read_to_string(&path).unwrap();
        assert!(repaired.contains("\"display_name\": \"Recovered\""));

        let events_path = mm.persona_memory_events_path(22, 4);
        let events = std::fs::read_to_string(events_path).unwrap();
        assert!(events.contains("\"event_type\":\"memory_parse_error_recovered\""));
        cleanup(&dir);
    }

    #[test]
    fn test_read_persona_memory_state_reports_parse_error_without_backup() {
        let (mm, dir) = test_memory_manager();
        let path = mm.persona_memory_state_path(22, 5);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, "{ bad json").unwrap();

        let state = mm.read_persona_memory_state(22, 5);
        assert!(state.is_none());

        let events_path = mm.persona_memory_events_path(22, 5);
        let events = std::fs::read_to_string(events_path).unwrap();
        assert!(events.contains("\"event_type\":\"memory_parse_error\""));
        cleanup(&dir);
    }

    #[test]
    fn test_append_persona_memory_event() {
        let (mm, dir) = test_memory_manager();
        mm.append_persona_memory_event(
            10,
            1,
            "manual_edit",
            "user",
            json!({"field":"tier1.stable_facts"}),
        )
        .unwrap();
        let events_path = mm.persona_memory_events_path(10, 1);
        let content = std::fs::read_to_string(events_path).unwrap();
        assert!(content.contains("\"event_type\":\"manual_edit\""));
        cleanup(&dir);
    }

    #[test]
    fn test_validate_memory_state_confidence_range() {
        let (mm, dir) = test_memory_manager();
        let mut state = PersonaMemoryState::default();
        // A valid schema version is required so validation reaches the confidence check.
        state.meta.version = 1;
        state.workflow_memory.intents.push(WorkflowMemoryEntry {
            intent_signature: "test".into(),
            confidence: 2.0,
            ..WorkflowMemoryEntry::default()
        });
        let err = mm.validate_memory_state(&state).unwrap_err();
        assert!(err.contains("confidence"));
        cleanup(&dir);
    }

    #[test]
    fn test_render_memory_for_llm_empty_state() {
        let state = PersonaMemoryState::default();
        assert!(render_memory_for_llm(&state, 25).is_empty());
    }

    #[test]
    fn test_render_persona_context_memory_omits_tier1() {
        let mut state = PersonaMemoryState::default();
        state.tier3.recent_focus = vec!["finish report".to_string()];
        let out = render_persona_context_memory(&state);
        assert!(out.contains("Recent focus"));
        assert!(out.contains("finish report"));
        assert!(!out.contains("### Identity"));
        assert!(!out.contains("Tier 1"));
    }

    #[test]
    fn test_enrich_persona_memory_for_prompt_uses_persona_name() {
        let state = PersonaMemoryState::default();
        let enriched = enrich_persona_memory_for_prompt(state, Some("PZ"), None);
        let out = render_identity_and_tier1_for_system(&enriched, 25);
        assert!(out.contains("PZ"));
    }

    #[test]
    fn test_render_identity_and_tier1_for_system() {
        let mut state = PersonaMemoryState::default();
        state.identity.display_name = "Milla".into();
        state.tier1.stable_facts = vec!["likes Rust".into()];
        let out = render_identity_and_tier1_for_system(&state, 25);
        assert!(out.contains("### Identity"));
        assert!(out.contains("Milla"));
        assert!(out.contains("Tier 1"));
        assert!(out.contains("likes Rust"));
        assert!(!out.contains("Recent focus"));
    }

    #[test]
    fn test_render_memory_for_llm_combines_system_and_context() {
        let mut state = PersonaMemoryState::default();
        state.identity.display_name = "Milla".into();
        state.tier3.recent_focus = vec!["finish report".to_string()];
        let out = render_memory_for_llm(&state, 25);
        assert!(out.contains("Milla"));
        assert!(out.contains("finish report"));
    }

    #[test]
    fn test_render_persona_context_memory_renders_tier2_knowledge() {
        let mut state = PersonaMemoryState::default();
        state.tier2.user_terminology = vec!["PZ means Persona Zero".into()];
        state.tier2.sops.push(SopPointer {
            id: "publish-gate".into(),
            vault_path: "ORIGIN/Operations/SOPs/Publish.md".into(),
            summary: "Collect approval before publishing".into(),
        });
        state.tier2.preferences = vec!["Use V4 face reference by default".into()];
        let out = render_persona_context_memory(&state);
        assert!(out.contains("### SOPs (Tier 2)"));
        assert!(out.contains("### Tier 2 knowledge"));
        assert!(out.contains("Terminology: PZ means Persona Zero"));
        assert!(out.contains("**publish-gate**"));
        assert!(out.contains("ORIGIN/Operations/SOPs/Publish.md"));
        assert!(out.contains("Preference: Use V4 face reference by default"));
    }

    #[test]
    fn test_read_persona_memory_state_migrates_legacy_active_projects() {
        let (mm, dir) = test_memory_manager();
        let path = mm.persona_memory_state_path(300, 2);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{
  "meta":{"version":1,"revision":1,"updated_at":"2026-05-28T00:00:00Z"},
  "identity":{},
  "tier1":{},
  "tier2":{
    "active_projects":[
      {"id":"a","status":"active","summary":"Term: PZ means Persona Zero","updated_at":"2026-05-28T00:00:00Z"},
      {"id":"b","status":"active","summary":"Preference: Use V4 default","updated_at":"2026-05-28T00:00:00Z"},
      {"id":"c","status":"active","summary":"Rainy Cafe: Published","updated_at":"2026-05-28T00:00:00Z"}
    ]
  },
  "tier3":{},
  "workflow_memory":{},
  "links":{}
}"#,
        )
        .unwrap();
        let state = mm.read_persona_memory_state(300, 2).unwrap();
        assert!(state
            .tier2
            .user_terminology
            .iter()
            .any(|s| s.contains("PZ means Persona Zero")));
        assert!(state
            .tier2
            .preferences
            .iter()
            .any(|s| s.contains("Use V4 default")));
        assert!(!state
            .tier2
            .sops
            .iter()
            .any(|s| s.summary.contains("Published")));
        let events = std::fs::read_to_string(mm.persona_memory_events_path(300, 2)).unwrap();
        assert!(events.contains("memory_state_migrated_tier2_knowledge"));
        cleanup(&dir);
    }

    #[test]
    fn test_render_persona_context_memory_with_options_can_hide_recent_focus() {
        let mut state = PersonaMemoryState::default();
        state.tier3.recent_focus = vec!["finish report".to_string()];
        let out = render_persona_context_memory_with_options(&state, true);
        assert!(!out.contains("### Recent focus"));
        assert!(!out.contains("finish report"));
    }

    #[test]
    fn test_render_memory_for_llm_truncation_note() {
        let mut state = PersonaMemoryState::default();
        state.tier1.workflow_principles = (0..30).map(|i| format!("principle {i}")).collect();
        let out = render_memory_for_llm(&state, 5);
        assert!(out.contains("Workflow principles"));
        assert!(out.contains("showing 5 of 30"));
    }
}
