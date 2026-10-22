//! Four-column board pipeline: todo, plan, build, verify.
//!
//! Dragging a card only changes its column. A stage runs when the operator
//! presses Deploy, or when that card's Auto flag advances after an explicit pass.

use crate::db::PersonaTodo;
use crate::db_collab::clip_chars;

pub const PIPELINE: [&str; 4] = ["todo", "plan", "build", "verify"];

/// Hops Auto may take after the stage that was just started.
/// todo → plan → build → verify is three hops.
pub const AUTO_CHAIN_BUDGET: u8 = 3;

pub fn next_stage(status: &str) -> Option<&'static str> {
    let idx = PIPELINE.iter().position(|stage| *stage == status)?;
    PIPELINE.get(idx + 1).copied()
}

pub fn is_runnable_stage(status: &str) -> bool {
    matches!(status, "todo" | "plan" | "build" | "verify")
}

pub fn stage_brief(status: &str) -> &'static str {
    match status {
        "todo" => {
            "Capture the work: restate the outcome, the constraints, and what done looks like. Put that in the finish note."
        }
        "plan" => {
            "Write an execution plan: the problem, the files or systems involved, a checklist, and open questions. Put that plan in the finish note."
        }
        "build" => {
            "Do the work from the plan. Keep the card's scope. Record what changed in the finish note."
        }
        "verify" => {
            "Verify the work against the plan. Run the project's checks. Record the result in the finish note. On a failure, finish as blocked and name the failing check. Passing this stage closes the card."
        }
        _ => "This column does not run a stage.",
    }
}

pub fn build_stage_prompt(item: &PersonaTodo) -> String {
    let instruction = item
        .deploy_prompt
        .as_deref()
        .map(|text| clip_chars(text, 4000))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| stage_brief(&item.status).to_string());
    let history = item
        .description
        .as_deref()
        .map(|text| clip_chars(text, 2000))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(none)".to_string());
    let blocked = item
        .blocked_reason
        .as_deref()
        .filter(|text| !text.is_empty())
        .unwrap_or("(none)");
    let source = match item.source_ref.as_deref().filter(|text| !text.is_empty()) {
        Some(reference) => format!("{} {reference}", item.source_kind),
        None => item.source_kind.clone(),
    };
    format!(
        "[board_stage] card #{id} stage {stage}\n\n\
Title: {title}\n\
Operator instruction:\n{instruction}\n\
Stage notes (read only):\n{history}\n\
Blocked reason: {blocked}\n\
Source: {source}\n\n\
You are running the {stage} stage for board card #{id} in your own thread.\n\n\
When the stage work is finished, call finish_pipeline_stage with item_id {id} and outcome \"pass\".\n\
If you cannot finish, call finish_pipeline_stage with outcome \"blocked\" and a blocked_reason.\n\
Do not change the card status yourself. A status change during this run is ignored.\n\
Passing the stage does not move the card unless Auto is on for it.\n\
A note passed to finish_pipeline_stage is appended to the stage notes. It does not replace the deploy prompt.\n",
        id = item.id,
        stage = item.status,
        title = item.title,
        instruction = instruction,
        history = history,
        blocked = blocked,
        source = source,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PersonaTodo;

    fn card(status: &str, deploy_prompt: Option<&str>, description: Option<&str>) -> PersonaTodo {
        PersonaTodo {
            id: 7,
            chat_id: 1,
            persona_id: 2,
            title: "Fix the latch".into(),
            status: status.into(),
            source_hint: None,
            created_at: "t".into(),
            updated_at: "t".into(),
            completed_at: None,
            description: description.map(str::to_string),
            owner_persona_id: Some(2),
            priority: 0,
            due_at: None,
            blocked_reason: None,
            position: 1.0,
            source_kind: "manual".into(),
            source_ref: None,
            auto_advance: 0,
            stage_run_id: None,
            stage_outcome: None,
            deploy_prompt: deploy_prompt.map(str::to_string),
        }
    }

    #[test]
    fn pipeline_advances_until_verify() {
        assert_eq!(next_stage("todo"), Some("plan"));
        assert_eq!(next_stage("plan"), Some("build"));
        assert_eq!(next_stage("build"), Some("verify"));
        assert_eq!(next_stage("verify"), None);
        assert!(is_runnable_stage("todo"));
        assert!(is_runnable_stage("plan"));
        assert!(is_runnable_stage("build"));
        assert!(is_runnable_stage("verify"));
        assert!(!is_runnable_stage("design"));
        assert_eq!(AUTO_CHAIN_BUDGET, 3);
    }

    #[test]
    fn deploy_prompt_is_the_instruction_and_description_is_history() {
        let prompt = build_stage_prompt(&card(
            "build",
            Some("Keep the brass hinge."),
            Some("Plan: replace the latch."),
        ));
        assert!(prompt.contains("Operator instruction:\nKeep the brass hinge."));
        assert!(prompt.contains("Stage notes (read only):\nPlan: replace the latch."));
        assert!(!prompt.contains("Do the work from the plan."));

        let fallback = build_stage_prompt(&card("plan", None, None));
        assert!(fallback.contains("Write an execution plan"));
        assert!(fallback.contains("Stage notes (read only):\n(none)"));
    }
}
