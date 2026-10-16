//! ADK agent kinds and synthetic tools (`transfer_to_agent`, `exit_loop`).

use crate::claude::ToolDefinition;
use crate::gemini_adk::profile::{AdkAgent, AdkAgentKind};
use serde_json::json;

pub const TRANSFER_TOOL: &str = "transfer_to_agent";
pub const EXIT_LOOP_TOOL: &str = "exit_loop";

pub fn transfer_tool_definition(sub_agent_ids: &[String]) -> ToolDefinition {
    let names = sub_agent_ids.join(", ");
    ToolDefinition::new(
        TRANSFER_TOOL,
        format!(
            "Delegate the remainder of this turn to a specialist sub-agent. Allowed agent names: {names}. Pass a short handoff brief in `reason`."
        ),
        json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Id of the sub-agent to transfer to"
                },
                "reason": {
                    "type": "string",
                    "description": "Brief handoff context for the sub-agent"
                }
            },
            "required": ["agent_name"]
        }),
    )
}

pub fn exit_loop_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        EXIT_LOOP_TOOL,
        "Signal that the enclosing LoopAgent should stop after this turn.",
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Why the loop can stop"
                }
            }
        }),
    )
}

pub fn resolve_agent_model(agent: &AdkAgent, topology_default: &str) -> String {
    let m = agent.model.trim();
    if m.is_empty() {
        topology_default.trim().to_string()
    } else {
        m.to_string()
    }
}

pub fn is_workflow(kind: AdkAgentKind) -> bool {
    matches!(
        kind,
        AdkAgentKind::Sequential | AdkAgentKind::Parallel | AdkAgentKind::Loop
    )
}
