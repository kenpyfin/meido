//! Shared progress / discovery classification for Classic and Cursor tool loops.
//!
//! Keeps `discovery_streak_count` accounting consistent so narrow file reads/greps do not
//! trip `builtin_loop_guard` during legitimate investigation (e.g. board todo stages).

use std::path::{Component, Path};

/// Tools that reset the discovery streak (the run made forward progress).
pub fn is_progress_tool_use(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "spawn_background_command"
            | "write_file"
            | "edit_file"
            | "apply_search_replace"
            | "symbol_edit"
            | "read_file"
            | "read_tiered_memory"
            | "read_memory"
            | "write_tiered_memory"
            | "write_memory_state"
            | "schedule_task"
            | "register_tracked_job"
            | "update_bulletin_focus"
            | "add_todo"
            | "list_todos"
            | "complete_todo"
            | "finish_pipeline_stage"
    )
}

/// Tools / inputs that count toward the discovery streak (status archaeology / broad search).
pub fn is_discovery_tool_use(tool_name: &str, input: &serde_json::Value) -> bool {
    match tool_name {
        "list_scheduled_tasks" | "list_cursor_agent_runs" => true,
        "grep" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .trim();
            let has_glob = input
                .get("glob")
                .and_then(|v| v.as_str())
                .is_some_and(|g| !g.trim().is_empty());
            // Only broad roots without a glob count as discovery. Narrow file/subdir greps
            // (and any grep with a glob under a specific tree) are investigation, not a stall.
            is_broad_discovery_path(path) && !has_glob
        }
        "glob" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .trim();
            is_broad_discovery_path(path)
        }
        "bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            crate::tools::bash_safety::is_expensive_shell_search(cmd)
                || cmd.to_ascii_lowercase().contains("find ")
        }
        _ => false,
    }
}

/// Paths that are "scan the tree" targets for discovery accounting.
pub fn is_broad_discovery_path(path: &str) -> bool {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." || trimmed == "./" || trimmed == "./." {
        return true;
    }
    let normalized = trimmed.trim_start_matches("./");
    if normalized.is_empty() || normalized == "." {
        return true;
    }
    // `shared`, `./shared`, `workspace/shared`, `./workspace/shared/.`
    let as_path = Path::new(normalized.trim_end_matches("/."));
    let comps: Vec<&str> = as_path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    if comps.is_empty() {
        return true;
    }
    if comps.len() == 1 && comps[0].eq_ignore_ascii_case("shared") {
        return true;
    }
    if comps.len() == 2
        && comps[0].eq_ignore_ascii_case("workspace")
        && comps[1].eq_ignore_ascii_case("shared")
    {
        return true;
    }
    // Trailing `shared/.` style after trim_end_matches("/.") already covered above.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_includes_stage_and_read_tools() {
        assert!(is_progress_tool_use("finish_pipeline_stage"));
        assert!(is_progress_tool_use("complete_todo"));
        assert!(is_progress_tool_use("read_file"));
        assert!(is_progress_tool_use("read_tiered_memory"));
        assert!(!is_progress_tool_use("list_scheduled_tasks"));
    }

    #[test]
    fn discovery_ignores_narrow_grep() {
        assert!(is_discovery_tool_use(
            "list_cursor_agent_runs",
            &serde_json::json!({})
        ));
        let broad = serde_json::json!({"pattern":"LoRA","path":"./workspace/shared/."});
        assert!(is_discovery_tool_use("grep", &broad));
        let narrow_dir = serde_json::json!({"pattern":"finviz","path":"skills/rh-main-book-risk"});
        assert!(!is_discovery_tool_use("grep", &narrow_dir));
        let narrow_file = serde_json::json!({
            "pattern":"etf",
            "path":"skills/rh-main-book-risk/sector_taxonomy.py"
        });
        assert!(!is_discovery_tool_use("grep", &narrow_file));
        let broad_with_glob = serde_json::json!({"pattern":"finviz","path":".","glob":"*.py"});
        assert!(!is_discovery_tool_use("grep", &broad_with_glob));
        let bash = serde_json::json!({"command":"grep -r PZ ./shared/ | head"});
        assert!(is_discovery_tool_use("bash", &bash));
    }

    #[test]
    fn broad_discovery_path_helpers() {
        assert!(is_broad_discovery_path("."));
        assert!(is_broad_discovery_path("shared"));
        assert!(is_broad_discovery_path("./workspace/shared/."));
        assert!(!is_broad_discovery_path("skills/rh-main-book-risk"));
        assert!(!is_broad_discovery_path(
            "/home/ken/big_storage/airflowProd/notebook/My_Trader2.0"
        ));
    }
}
