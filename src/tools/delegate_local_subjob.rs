//! Tool definition for strategy-delegated bounded local read-only sub-jobs.
//! Execution is handled in the Classic agent loop (`telegram.rs`).

use crate::claude::ToolDefinition;
use crate::tools::schema_object;
use serde_json::json;

pub const DELEGATE_LOCAL_SUBJOB_NAME: &str = "delegate_local_subjob";

pub fn delegate_local_subjob_definition() -> ToolDefinition {
    ToolDefinition::new(
        DELEGATE_LOCAL_SUBJOB_NAME,
        "Delegate a bounded read-only discovery task to the local model. \
Use for grep/read/glob/search chains that do not require mutations. \
Returns a summary for you to act on with mutation tools.",
        schema_object(
            json!({
                "brief": {
                    "type": "string",
                    "description": "Clear instructions for what to discover (paths, patterns, questions)."
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Max local tool rounds (default 3, max 5)."
                },
                "include_paths": {
                    "type": "string",
                    "description": "Optional path hints for the sub-job."
                }
            }),
            &["brief"],
        ),
    )
}
