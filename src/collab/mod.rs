//! Persona collaboration: guest replies, groups, and the coordinator digest.
//!
//! Model calls are capped. Nothing here runs unless a workspace or an opt-in job asks for it.

pub mod coordinator;
pub mod groups;
pub mod guest;

use crate::claude::ToolDefinition;

pub const COORDINATOR_NAME: &str = "coordinator";
pub const BRIEF_PREFIX: &str = "[[coordinator_brief]]";
pub const GROUP_TOOL_CAP: usize = 4;
pub const GUEST_HISTORY_TURNS: usize = 6;
pub const GUEST_HISTORY_CHARS: usize = 6_000;
/// One older line cannot spend the whole guest window.
pub const GUEST_HISTORY_LINE_CHARS: usize = 1_500;
/// Matches `GUEST_MAX_PER_TURN` in `web/src/lib/guest-mention.ts`.
pub const GUEST_MAX_PER_TURN: usize = 4;
pub const GROUP_WINDOW: usize = 10;
pub const GROUP_SUMMARY_EVERY: i64 = 30;

const COORDINATOR_ONLY_TOOLS: &[&str] = &[
    "list_persona_activity",
    "list_board",
    "propose_board_change",
];

pub fn is_coordinator_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(COORDINATOR_NAME)
}

/// Drop cross-persona tools unless this run is the Coordinator.
pub fn retain_tools_for_persona(defs: &mut Vec<ToolDefinition>, persona_name: &str) {
    if is_coordinator_name(persona_name) {
        return;
    }
    defs.retain(|def| !COORDINATOR_ONLY_TOOLS.contains(&def.name.as_str()));
}
