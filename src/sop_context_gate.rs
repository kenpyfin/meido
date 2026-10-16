//! Steer the agent toward vault SOPs when the user mentions workflows/pipelines.

pub const SOP_VAULT_PATH: &str = "ORIGIN/Operations/SOPs/PZ-Post-Pipeline.md";

const SOP_PHRASES: &[&str] = &[
    "workflow",
    "workflows",
    "pipeline",
    "sop",
    "standard operating",
    "post generation",
    "pz post",
    "schedule job",
    "schedule task #164",
];

/// When the user's `[current_request]` mentions procedures, inject vault SOP guidance.
pub fn sop_request_steer(user_request: &str) -> Option<String> {
    let lower = user_request.to_ascii_lowercase();
    if !SOP_PHRASES.iter().any(|p| lower.contains(p)) {
        return None;
    }
    Some(format!(
        "[hook_context]\n\
         **SOP vocabulary:** \"Workflow\" / \"pipeline\" = vault SOP at `tier2.sops[].vault_path` (ORIGIN markdown), not YAML (`run_workflow` was removed). Use `locate_file` or `read_file` for that SOP path and follow it. Use `recall` only if the note is not already named. PZ default: `{SOP_VAULT_PATH}`. Use `activate_skill` + `run_skill_script`.\n\
         [/hook_context]"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steer_matches_pipeline_phrase() {
        let s = sop_request_steer("run the pz post generation schedule job").expect("steer");
        assert!(s.contains("locate_file"));
        assert!(s.contains("recall"));
        assert!(s.contains("run_workflow"));
        assert!(s.contains("removed"));
    }

    #[test]
    fn steer_skips_unrelated() {
        assert!(sop_request_steer("what time is it").is_none());
    }
}
