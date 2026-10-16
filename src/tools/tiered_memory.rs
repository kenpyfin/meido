//! Per-persona tiered memory backed by canonical memory_state.json.

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashSet;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::memory::{dedupe_sops, MemoryManager, PersonaMemoryState, SopPointer};

use super::{
    auth_context_from_input, authorize_chat_persona_access, schema_object, Tool, ToolResult,
};

fn parse_tier_content(state: &PersonaMemoryState, tier: u8) -> String {
    match tier {
        1 => {
            let mut lines = Vec::new();
            if !state.identity.display_name.trim().is_empty() {
                lines.push(format!(
                    "- Identity|display_name={}",
                    state.identity.display_name.trim()
                ));
            }
            if !state.identity.self_model.trim().is_empty() {
                lines.push(format!(
                    "- Identity|self_model={}",
                    state.identity.self_model.trim()
                ));
            }
            if !state.identity.voice_style.trim().is_empty() {
                lines.push(format!(
                    "- Identity|voice_style={}",
                    state.identity.voice_style.trim()
                ));
            }
            lines.extend(
                state
                    .identity
                    .non_negotiables
                    .iter()
                    .map(|v| format!("- IdentityConstraint|{v}")),
            );
            lines.extend(state.tier1.stable_facts.clone());
            lines.extend(
                state
                    .tier1
                    .workflow_principles
                    .iter()
                    .map(|v| format!("- WorkflowPrinciple|{v}")),
            );
            lines.join("\n").trim().to_string()
        }
        2 => {
            let mut lines = Vec::new();
            lines.extend(
                state
                    .tier2
                    .user_terminology
                    .iter()
                    .map(|v| format!("- Terminology|{}", v.trim())),
            );
            lines.extend(state.tier2.sops.iter().map(|sop| {
                let id = if sop.id.trim().is_empty() {
                    SopPointer::derive_id_from_vault_path(&sop.vault_path)
                } else {
                    sop.id.trim().to_string()
                };
                format!(
                    "- SOP|{id}|{}|{}",
                    sop.vault_path.trim(),
                    sop.summary.trim()
                )
            }));
            lines.extend(
                state
                    .tier2
                    .preferences
                    .iter()
                    .map(|v| format!("- Preference|{}", v.trim())),
            );
            lines.join("\n").trim().to_string()
        }
        3 => state.tier3.recent_focus.join("\n").trim().to_string(),
        _ => String::new(),
    }
}

fn parse_tier2_knowledge_lines(content: &str) -> (Vec<String>, Vec<SopPointer>, Vec<String>) {
    let mut terminology = Vec::new();
    let mut sops = Vec::new();
    let mut preferences = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("- Terminology|") {
            terminology.push(v.trim().to_string());
            continue;
        }
        if let Some(sop) = SopPointer::from_legacy_line(line) {
            sops.push(sop);
            continue;
        }
        if let Some(v) = line.strip_prefix("- Preference|") {
            preferences.push(v.trim().to_string());
            continue;
        }
        if let Some(v) = line.strip_prefix("- ProjectState|") {
            if let Some(sop) = SopPointer::from_legacy_line(v) {
                sops.push(sop);
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("- TaskState|") {
            if let Some(sop) = SopPointer::from_legacy_line(v) {
                sops.push(sop);
            }
        }
    }
    (
        dedupe_lines(&terminology, 40),
        dedupe_sops(sops),
        dedupe_lines(&preferences, 40),
    )
}

fn dedupe_lines(lines: &[String], max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let key = t.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(t.to_string());
        }
    }
    out.into_iter().take(max).collect()
}

fn normalize_tier3_recent_focus(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out.into_iter().take(15).collect()
}

fn apply_tier_write(state: &mut PersonaMemoryState, tier: u8, content: &str) {
    match tier {
        1 => {
            let mut stable_facts = Vec::new();
            let mut workflow_principles = Vec::new();
            let mut identity_constraints = Vec::new();
            for raw_line in content.lines() {
                let line = raw_line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(v) = line.strip_prefix("- Identity|display_name=") {
                    state.identity.display_name = v.trim().to_string();
                    continue;
                }
                if let Some(v) = line.strip_prefix("- Identity|self_model=") {
                    state.identity.self_model = v.trim().to_string();
                    continue;
                }
                if let Some(v) = line.strip_prefix("- Identity|voice_style=") {
                    state.identity.voice_style = v.trim().to_string();
                    continue;
                }
                if let Some(v) = line.strip_prefix("- IdentityConstraint|") {
                    identity_constraints.push(v.trim().to_string());
                    continue;
                }
                if let Some(v) = line.strip_prefix("- WorkflowPrinciple|") {
                    workflow_principles.push(v.trim().to_string());
                    continue;
                }
                stable_facts.push(line.to_string());
            }
            state.identity.non_negotiables = identity_constraints;
            state.tier1.stable_facts = stable_facts;
            state.tier1.workflow_principles = workflow_principles;
        }
        2 => {
            let (terminology, sops, preferences) = parse_tier2_knowledge_lines(content);
            state.tier2.user_terminology = terminology;
            state.tier2.sops = sops;
            state.tier2.preferences = preferences;
            state.tier2.legacy_known_steps.clear();
        }
        3 => {
            state.tier3.recent_focus = normalize_tier3_recent_focus(content);
        }
        _ => {}
    }
}

pub struct ReadTieredMemoryTool {
    memory: MemoryManager,
}

impl ReadTieredMemoryTool {
    pub fn new(data_dir: &str) -> Self {
        ReadTieredMemoryTool {
            memory: MemoryManager::new(data_dir, data_dir),
        }
    }
}

#[async_trait]
impl Tool for ReadTieredMemoryTool {
    fn name(&self) -> &str {
        "read_tiered_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_tiered_memory",
            "Read this persona's tiered memory from canonical memory_state.json (legacy MEMORY.md auto-migrates). Optional tier (1, 2, or 3) returns only that section. Tier 2 contains user terminology, SOPs (`tier2.sops` with ORIGIN vault paths), and durable preferences.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (default: current chat from context)"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Persona ID (default: current persona from context)"
                    },
                    "tier": {
                        "type": "integer",
                        "description": "Optional: 1, 2, or 3 to return only that tier's content"
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => return ToolResult::error("Missing auth context".into()),
        };
        let chat_id = input
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_chat_id);
        let persona_id = input
            .get("persona_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_persona_id);
        if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
            return ToolResult::error(e);
        }

        let state = match self
            .memory
            .read_or_migrate_persona_memory_state(chat_id, persona_id)
        {
            Some(s) => s,
            None => {
                return ToolResult::success(
                    "No canonical memory state found (not yet created).".into(),
                )
            }
        };
        let state_path = self.memory.persona_memory_state_path(chat_id, persona_id);
        info!("Reading tiered memory: {}", state_path.display());

        let tier_opt = input.get("tier").and_then(|v| v.as_i64()).map(|n| n as u8);
        let result = if let Some(t) = tier_opt {
            if !(1..=3).contains(&t) {
                return ToolResult::error("tier must be 1, 2, or 3".into());
            }
            let section = parse_tier_content(&state, t);
            if section.is_empty() {
                format!("(Tier {} is empty.)", t)
            } else {
                section
            }
        } else {
            crate::memory::render_memory_markdown(&state)
        };

        ToolResult::success(result)
    }
}

pub struct WriteTieredMemoryTool {
    memory: MemoryManager,
}

impl WriteTieredMemoryTool {
    pub fn new(data_dir: &str) -> Self {
        WriteTieredMemoryTool {
            memory: MemoryManager::new(data_dir, data_dir),
        }
    }
}

#[async_trait]
impl Tool for WriteTieredMemoryTool {
    fn name(&self) -> &str {
        "write_tiered_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_tiered_memory",
            "Write one tier of canonical memory_state.json. Tier 1 = long-term (only on explicit user ask); Tier 2 = terminology + SOPs + preferences. SOP line format: `- SOP|<id>|ORIGIN/path/to/doc.md|<summary>`. Tier 3 = short-lived scratch focus. Replaces only that tier's section.",
            schema_object(
                json!({
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID"
                    },
                    "persona_id": {
                        "type": "integer",
                        "description": "Persona ID"
                    },
                    "tier": {
                        "type": "integer",
                        "description": "Tier to write: 1 (long-term), 2 (mid-term), or 3 (short-term)",
                        "enum": [1, 2, 3]
                    },
                    "content": {
                        "type": "string",
                        "description": "Text content for this tier (replaces existing content in that tier)"
                    }
                }),
                &["tier", "content"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let auth = match auth_context_from_input(&input) {
            Some(a) => a,
            None => return ToolResult::error("Missing auth context".into()),
        };
        let chat_id = input
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_chat_id);
        let persona_id = input
            .get("persona_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(auth.caller_persona_id);
        let tier = match input
            .get("tier")
            .and_then(|v| v.as_i64())
            .filter(|&n| (1..=3).contains(&n))
        {
            Some(n) => n as u8,
            None => {
                return ToolResult::error("Missing or invalid 'tier' (must be 1, 2, or 3)".into())
            }
        };
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if let Err(e) = authorize_chat_persona_access(&input, chat_id, persona_id) {
            return ToolResult::error(e);
        }

        let mut state = self
            .memory
            .read_or_migrate_persona_memory_state(chat_id, persona_id)
            .unwrap_or_default();
        apply_tier_write(&mut state, tier, content);
        let state_path = self.memory.persona_memory_state_path(chat_id, persona_id);
        info!(
            "Writing tiered memory tier {} to canonical JSON: {}",
            tier,
            state_path.display()
        );
        match self
            .memory
            .write_persona_memory_state(chat_id, persona_id, state)
        {
            Ok(()) => {
                let _ = self.memory.append_persona_memory_event(
                    chat_id,
                    persona_id,
                    "tier_write",
                    "agent_auto",
                    json!({
                        "tier": tier,
                        "state_path": state_path.to_string_lossy().to_string(),
                    }),
                );
                ToolResult::success(format!("Tier {} updated in canonical memory state.", tier))
            }
            Err(e) => ToolResult::error(format!("Failed to write canonical memory state: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tier_content_from_state() {
        let mut state = PersonaMemoryState::default();
        state.identity.display_name = "Nova".into();
        state.tier1.stable_facts = vec!["- stable fact".into()];
        state.tier2.user_terminology = vec!["PZ = persona zero".into()];
        state.tier2.sops.push(SopPointer {
            id: "approve-hotify".into(),
            vault_path: "ORIGIN/Operations/SOPs/Approve-Hotify.md".into(),
            summary: "Approve base image, then hotify".into(),
        });
        state.tier2.preferences = vec!["Prefer Bay Area weather context".into()];
        state.tier3.recent_focus = vec!["- recent".into()];
        assert!(parse_tier_content(&state, 1).contains("Identity|display_name=Nova"));
        let t2 = parse_tier_content(&state, 2);
        assert!(t2.contains("Terminology|PZ = persona zero"));
        assert!(t2.contains("SOP|approve-hotify|ORIGIN/Operations/SOPs/Approve-Hotify.md"));
        assert!(t2.contains("Preference|Prefer Bay Area weather context"));
        assert_eq!(parse_tier_content(&state, 3), "- recent");
    }

    #[test]
    fn test_apply_tier_write_identity_and_principles() {
        let mut state = PersonaMemoryState::default();
        let content = "\
- Identity|display_name=KenAssistant
- Identity|voice_style=concise
- IdentityConstraint|Do not hallucinate
- WorkflowPrinciple|Check run history before claiming no memory
- Stable fact line";
        apply_tier_write(&mut state, 1, content);
        assert_eq!(state.identity.display_name, "KenAssistant");
        assert_eq!(state.identity.voice_style, "concise");
        assert_eq!(state.identity.non_negotiables.len(), 1);
        assert_eq!(state.tier1.workflow_principles.len(), 1);
        assert_eq!(state.tier1.stable_facts.len(), 1);
    }

    #[test]
    fn test_parse_tier2_knowledge_lines_supports_new_and_legacy_formats() {
        let input = r#"
- Terminology|PZ = Persona Zero
- SOP|gate|ORIGIN/Operations/SOPs/Gate.md|Approve base image before hotify
- Preference|Use V4 ref by default
"#;
        let (terminology, sops, preferences) = parse_tier2_knowledge_lines(input);
        assert_eq!(terminology.len(), 1);
        assert_eq!(preferences.len(), 1);
        assert_eq!(sops.len(), 1);
        assert_eq!(sops[0].vault_path, "ORIGIN/Operations/SOPs/Gate.md");
    }

    #[test]
    fn test_normalize_tier3_recent_focus_dedupes_lines() {
        let input = r#"
- monitoring queue
- monitoring queue
- checking output
"#;
        let out = normalize_tier3_recent_focus(input);
        assert_eq!(
            out.iter()
                .filter(|l| l.contains("monitoring queue"))
                .count(),
            1
        );
        assert_eq!(
            out.iter().filter(|l| l.contains("checking output")).count(),
            1
        );
    }

    #[test]
    fn test_apply_tier_write_tier2_knowledge() {
        let mut state = PersonaMemoryState::default();
        let content = "\
- Terminology|IG = Instagram
- SOP|publish|ORIGIN/Operations/SOPs/Publish.md|Collect user approval before publishing
- Preference|Avoid strap-slip prompts";
        apply_tier_write(&mut state, 2, content);
        assert_eq!(state.tier2.user_terminology.len(), 1);
        assert_eq!(state.tier2.preferences.len(), 1);
        assert_eq!(state.tier2.sops.len(), 1);
        assert_eq!(
            state.tier2.sops[0].vault_path,
            "ORIGIN/Operations/SOPs/Publish.md"
        );
    }
}
