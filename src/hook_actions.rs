use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use tracing::warn;

use crate::hook_runtime::HookMemoryEffects;
use crate::memory::MemoryManager;

fn is_terminal_focus_line(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("published")
        || lower.contains("scrapped")
        || lower.contains("completed")
        || lower.contains("cancelled")
        || lower.contains("canceled")
}

pub fn extract_pz_post_ids(text: &str) -> HashSet<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"PZ-\d{8}(?:-[A-Za-z0-9_]+)?").expect("valid PZ post id regex"))
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

pub fn extract_terminal_pz_post_ids(
    tool_name: &str,
    input: &Value,
    result_content: &str,
    is_error: bool,
) -> HashSet<String> {
    if is_error {
        return HashSet::new();
    }
    let input_s = serde_json::to_string(input).unwrap_or_default();
    let mut combined = String::new();
    combined.push_str(tool_name);
    combined.push('\n');
    combined.push_str(&input_s);
    combined.push('\n');
    combined.push_str(result_content);
    if !is_terminal_focus_line(&combined) {
        return HashSet::new();
    }
    extract_pz_post_ids(&combined)
}

pub fn apply_persona_memory_hygiene(
    memory: &MemoryManager,
    chat_id: i64,
    persona_id: i64,
    terminal_post_ids: &HashSet<String>,
    bulletin_present: bool,
) {
    let Some(mut memory_state) = memory.read_persona_memory_state(chat_id, persona_id) else {
        return;
    };
    let mut changed = false;

    if bulletin_present || !terminal_post_ids.is_empty() {
        let before_tier3_len = memory_state.tier3.recent_focus.len();
        memory_state.tier3.recent_focus.retain(|line| {
            let post_match = terminal_post_ids.iter().any(|id| line.contains(id));
            !is_terminal_focus_line(line) && !post_match
        });
        changed |= memory_state.tier3.recent_focus.len() != before_tier3_len;
    }

    if changed {
        if let Err(e) = memory.write_persona_memory_state(chat_id, persona_id, memory_state) {
            warn!("Persona focus hygiene write failed: {e}");
        }
    }
}

/// Applies validated hook memory effects.
pub fn apply_hook_memory_effects(
    memory: &MemoryManager,
    chat_id: i64,
    persona_id: i64,
    effects: &HookMemoryEffects,
) {
    if effects.terminal_pz_post_ids.is_empty() {
        return;
    }
    let terminal_post_ids: HashSet<String> = effects.terminal_pz_post_ids.iter().cloned().collect();
    apply_persona_memory_hygiene(memory, chat_id, persona_id, &terminal_post_ids, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_terminal_pz_post_ids_requires_terminal_keyword() {
        let ids = extract_terminal_pz_post_ids(
            "bash",
            &json!({"cmd": "echo PZ-20260101-abc"}),
            "PZ-20260101-abc",
            false,
        );
        assert!(ids.is_empty());
    }

    #[test]
    fn extract_terminal_pz_post_ids_on_published_output() {
        let ids = extract_terminal_pz_post_ids(
            "bash",
            &json!({}),
            "Post PZ-20260101-abc published successfully",
            false,
        );
        assert_eq!(ids.len(), 1);
        assert!(ids.contains("PZ-20260101-abc"));
    }

    #[test]
    fn extract_terminal_pz_post_ids_skips_tool_errors() {
        let ids = extract_terminal_pz_post_ids(
            "bash",
            &json!({}),
            "Post PZ-20260101-abc published",
            true,
        );
        assert!(ids.is_empty());
    }
}
