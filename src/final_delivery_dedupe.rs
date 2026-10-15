//! Helpers for splitting agent final replies (e.g. memory tail after horizontal rule).

use crate::db::StoredMessage;

/// User-visible copy when an agent turn ends with no assistant text.
pub const EMPTY_TURN_NOTICE: &str =
    "This run finished with no reply text. Please send your request again.";

/// User-visible copy when the agent errors after the user message was already stored.
pub fn failed_turn_notice(error: &str) -> String {
    let error = error.trim();
    if error.is_empty() {
        "This run failed before a reply was ready. Please send your request again.".to_string()
    } else {
        format!(
            "This run failed before a reply was ready: {error}\nPlease send your request again."
        )
    }
}

/// Replace a blank (or persona-prefix-only) final with a visible notice.
pub fn ensure_visible_turn_text(response: &str) -> String {
    if strip_leading_persona_tokens(response).trim().is_empty() {
        EMPTY_TURN_NOTICE.to_string()
    } else {
        response.to_string()
    }
}

/// After persona normalization, what to actually deliver to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFinalDeliveryPlan {
    /// Deliver the full proposed final (default).
    DeliverFull,
    /// Deliver only this suffix (raw text, no persona prefix).
    DeliverSuffixOnly(String),
    /// Nothing new to show.
    Skip,
}

/// Strip leading `[Tag] ` transport prefixes (same idea as `channel::strip_leading_persona_tokens`).
fn strip_leading_persona_tokens(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        if !rest.starts_with('[') {
            break;
        }
        let Some(close_idx) = rest.find(']') else {
            break;
        };
        let token = &rest[1..close_idx];
        if token.is_empty() || token.len() > 64 || token.contains('\n') {
            break;
        }
        rest = rest[close_idx + 1..].trim_start();
    }
    rest
}

/// Split `(main, memory_tail)` when the final ends with a horizontal rule and Memory heading.
pub fn split_memory_tail(proposed_body: &str) -> (&str, Option<&str>) {
    let trimmed = proposed_body.trim_end();
    let idx_len = trimmed
        .rfind("\n\n---\n")
        .map(|i| (i, "\n\n---\n".len()))
        .or_else(|| trimmed.rfind("\n---\n").map(|i| (i, "\n---\n".len())));
    let Some((idx, needle_len)) = idx_len else {
        return (trimmed, None);
    };
    let tail = trimmed[idx + needle_len..].trim_start();
    let head = trimmed[..idx].trim_end();
    let tl = tail.to_ascii_lowercase();
    if tl.starts_with("**memory") || tl.starts_with("memory update") || tl.starts_with("# memory") {
        if head.is_empty() {
            return ("", Some(tail));
        }
        return (head, Some(tail));
    }
    (trimmed, None)
}

/// Decide how to deliver `proposed_final_indicated`. Mid-run `send_message` dedupe was removed;
/// delivery is always the full final unless the body is empty after stripping persona prefix.
pub fn plan_agent_final_delivery(
    _send_message_anchor: Option<&StoredMessage>,
    proposed_final_indicated: &str,
) -> AgentFinalDeliveryPlan {
    let proposed_body = strip_leading_persona_tokens(proposed_final_indicated).trim();
    if proposed_body.is_empty() {
        AgentFinalDeliveryPlan::Skip
    } else {
        AgentFinalDeliveryPlan::DeliverFull
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_delivers_full_for_non_empty_body() {
        let plan = plan_agent_final_delivery(None, "[pep] Hello there");
        assert_eq!(plan, AgentFinalDeliveryPlan::DeliverFull);
    }

    #[test]
    fn plan_skips_empty_body() {
        let plan = plan_agent_final_delivery(None, "   ");
        assert_eq!(plan, AgentFinalDeliveryPlan::Skip);
    }

    #[test]
    fn ensure_visible_turn_text_replaces_blank_and_persona_prefix() {
        assert_eq!(ensure_visible_turn_text("   "), EMPTY_TURN_NOTICE);
        assert_eq!(
            ensure_visible_turn_text("[Influencer_PZ_3]  \n"),
            EMPTY_TURN_NOTICE
        );
        assert_eq!(ensure_visible_turn_text("hello"), "hello");
    }

    #[test]
    fn failed_turn_notice_includes_error() {
        let text = failed_turn_notice("sidecar HTTP 500");
        assert!(text.contains("sidecar HTTP 500"));
        assert!(text.contains("send your request again"));
        assert!(failed_turn_notice("  ").contains("failed before a reply"));
    }

    #[test]
    fn split_memory_tail_extracts_suffix() {
        let final_txt = "Main answer here.\n\n---\n**Memory Update**:\n- tier3 note";
        let (main, tail) = split_memory_tail(final_txt);
        assert_eq!(main, "Main answer here.");
        assert!(tail.unwrap().contains("tier3"));
    }
}
