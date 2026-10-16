//! Cursor-only prompt shaping: slim system text for sidecar delegation.
//!
//! Every message is a new Cursor session (FullSlim). Resume-delta helpers remain
//! for tests/compat but are not selected by the engine.
//!
//! Does not alter `prepare_agent_run` / `build_system_prompt`; Classic and Gemini ADK
//! engines are unaffected.

use crate::agent_turn_context::{
    is_non_interactive_history_text, parse_current_request_content, parse_quoted_message_block,
    QuotedMessageRef,
};
use crate::claude::{ContentBlock, Message, MessageContent};
use crate::cursor_mcp_bridge::MCP_SERVER_NAME;

const TOOL_GROUPS_HEADING: &str = "## Tool groups";
const CONVERSATION_MEMORY_HEADING: &str = "## Conversation Memory";
const AGENT_SKILLS_HEADING: &str = "# Agent Skills";

const MCP_TOOLS_DELEGATION_SECTION: &str = r#"## Tool groups (MCP delegation)
Bot tools are exposed via MCP server `meido` (loopback). Tool names and parameter schemas come from MCP `tools/list` — do not rely on prose catalogs here.
- **Prefer MCP** for persona-scoped files (`read_file`, `write_file`, `grep`, …), recall (`recall`) and known vault paths (`read_file`, `locate_file`), scheduler, bulletin (`update_bulletin_focus`), operator todos (`add_todo` / `list_todos` / `complete_todo`), channel delivery, and skills (`activate_skill`, `run_skill_script`).
- Cursor built-ins (shell, search) exist; use MCP file tools for paths under the persona cwd and for vault/scheduler/bulletin/todo workflows.
- **Skills:** metadata in the Agent Skills section below is routing only; call `activate_skill` before following procedural skill steps.
- **Secrets:** if the user message contains a password, login, API key, or token, call MCP `vault_secret` before using it. Then use `$NAME` via the bot `bash` tool (not Cursor's native shell) or `[SECRET:NAME]` in other tool inputs. Never repeat the value.

"#;

const CURSOR_FILE_DELIVERY_REMINDER: &str = r#"## File links (web delivery)
When sharing files with the user, put markdown links in your **final reply** using **plain absolute local paths** (e.g. `[Spec](/home/.../personas/{chat_id}/{persona_id}/ORIGIN/Projects/spec.md)`).
- Never use `file://` URLs — the web UI cannot open them.
- Never fabricate `/api/uploads/...` URLs — the platform materializes absolute paths at delivery.
- After writing or updating a file, include a markdown link to the **current** absolute path when the user asks for a link.

"#;

const AGENT_SKILLS_INTRO_SLIM: &str = "\n# Agent Skills\n\nMetadata catalog for routing — call **`activate_skill`** to load full `SKILL.md` before procedural steps.\n\n";

const MAX_PROMPT_LEN: usize = 120_000;
const TIER1_HEADING: &str = "# Identity and long-term memory (Tier 1)";
const MAX_TIER1_ANCHOR_CHARS: usize = 4_096;
const GIT_DISCIPLINE_LINE: &str = "Git discipline: persona cwd is not the project git root. \
For commit/push/merge, cd to the Tier 1 project repo path first (`Repo: …` — fully allowed). \
Never use git against the Meido install/source checkout.\n";

const SELF_REPO_BAN_LINE: &str = "Self-repo ban: never treat the Meido source/install \
git checkout as a development project (no branch deletes, force-push, reset, or edits there). \
Persona Tier-1 target repos (`Repo: /absolute/path`) are fully allowed.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationPromptMode {
    /// Slim system prompt + full conversation messages.
    FullSlim,
    /// Minimal runtime header + delta messages only (resumed Cursor session).
    ResumeDelta,
}

impl DelegationPromptMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullSlim => "full_slim",
            Self::ResumeDelta => "resume_delta",
        }
    }
}

pub fn select_delegation_prompt_mode(
    _resume_agent_id: Option<&str>,
    _is_scheduled_task: bool,
    _delegation_resume_delta: bool,
) -> DelegationPromptMode {
    // Every user message is a new Cursor session; gateway always sends FullSlim.
    DelegationPromptMode::FullSlim
}

/// When MCP is live and slim is enabled, replace the long tool-groups block with MCP delegation prose.
pub fn slim_delegation_system_prompt(full: &str, slim_enabled: bool) -> String {
    if !slim_enabled {
        return ensure_self_repo_ban_in_prompt(full.to_string());
    }
    let mut out = replace_tool_groups_section(full);
    out = shorten_agent_skills_intro(&out);
    if !out.contains("## File links (web delivery)") {
        out.push('\n');
        out.push_str(CURSOR_FILE_DELIVERY_REMINDER);
    }
    ensure_self_repo_ban_in_prompt(out)
}

fn ensure_self_repo_ban_in_prompt(mut out: String) -> String {
    if out.contains("## Self-repo ban") || out.contains("Self-repo ban:") {
        return out;
    }
    out.push('\n');
    out.push_str("## Self-repo ban (mandatory)\n");
    out.push_str(SELF_REPO_BAN_LINE);
    out.push('\n');
    out
}

fn replace_tool_groups_section(full: &str) -> String {
    let Some(start) = full.find(TOOL_GROUPS_HEADING) else {
        return full.to_string();
    };
    let after_start = &full[start + TOOL_GROUPS_HEADING.len()..];
    let Some(rel_end) = after_start.find(CONVERSATION_MEMORY_HEADING) else {
        return full.to_string();
    };
    let end = start + TOOL_GROUPS_HEADING.len() + rel_end;
    let mut out = String::with_capacity(full.len());
    out.push_str(&full[..start]);
    out.push_str(MCP_TOOLS_DELEGATION_SECTION);
    out.push_str(&full[end..]);
    out
}

fn shorten_agent_skills_intro(full: &str) -> String {
    let Some(start) = full.find(AGENT_SKILLS_HEADING) else {
        return full.to_string();
    };
    let after_heading = &full[start + AGENT_SKILLS_HEADING.len()..];
    let Some(rel_end) = after_heading.find("<available_skills>") else {
        return full.to_string();
    };
    let end = start + AGENT_SKILLS_HEADING.len() + rel_end;
    let mut out = String::with_capacity(full.len());
    out.push_str(&full[..start]);
    out.push_str(AGENT_SKILLS_INTRO_SLIM);
    out.push_str(&full[end..]);
    out
}

pub struct DelegationRuntimeHeader {
    pub chat_id: i64,
    pub persona_id: i64,
    pub mcp_enabled: bool,
}

fn build_minimal_runtime_header(
    header: &DelegationRuntimeHeader,
    tier1_anchor: Option<&str>,
) -> String {
    let mcp_line = if header.mcp_enabled {
        format!("Tools: MCP server `{MCP_SERVER_NAME}` (authoritative schemas via tools/list).\n")
    } else {
        String::new()
    };
    let mut out = format!(
        "## Cursor delegation (resume turn)\n\
         chat_id={} persona_id={}\n\
         {mcp_line}\
         Path discipline: never use `workspace/` prefixes; cwd is persona-scoped under \
         `shared/personas/{{chat_id}}/{{persona_id}}/`.\n\
         {self_repo_ban}\
         Primary task: the `[current_request]` below.\n\n\
         {file_delivery}",
        header.chat_id,
        header.persona_id,
        self_repo_ban = SELF_REPO_BAN_LINE,
        file_delivery = CURSOR_FILE_DELIVERY_REMINDER,
    );
    if let Some(tier1) = tier1_anchor.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(tier1);
        out.push_str("\n\n");
        if tier1_has_project_repo_path(tier1) {
            out.push_str(GIT_DISCIPLINE_LINE);
        }
    }
    out
}

/// Slice Tier 1 identity/facts from the full delegation system prompt for resume-delta anchoring.
pub fn extract_tier1_anchor(delegation_system: &str) -> Option<String> {
    let start = delegation_system.find(TIER1_HEADING)?;
    let after = &delegation_system[start..];
    let rest = &after[TIER1_HEADING.len()..];
    let end = rest
        .find("\n# ")
        .map(|i| TIER1_HEADING.len() + i)
        .unwrap_or(after.len());
    let slice = after[..end].trim();
    if slice.is_empty() {
        return None;
    }
    if slice.chars().count() <= MAX_TIER1_ANCHOR_CHARS {
        return Some(slice.to_string());
    }
    let truncated: String = slice.chars().take(MAX_TIER1_ANCHOR_CHARS).collect();
    Some(format!(
        "{truncated}\n\n(Tier 1 anchor truncated for resume delta)"
    ))
}

fn tier1_has_project_repo_path(tier1: &str) -> bool {
    tier1.contains("Repo:") || tier1.contains("/home/")
}

fn is_prior_turn_message(text: &str) -> bool {
    text.contains("context=\"prior_turn\"")
}

fn is_trusted_delta_message(text: &str) -> bool {
    let t = text.trim();
    t.contains("[system_runtime_context")
        || t.starts_with("[persona_context]")
        || t.starts_with("[session_context")
        || t.starts_with("[current_request")
        || t.starts_with("[hook_context]")
        || t.starts_with("[continuation_context]")
}

fn current_request_index(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == "user" && message_text(m).trim().starts_with("[current_request"))
}

fn already_has_continuation(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|m| message_text(m).trim().starts_with("[continuation_context]"))
}

/// Last interactive `prior_turn` user message and a following assistant reply before `[current_request]`.
/// Skips scheduler/shell plumbing so short follow-ups latch onto the live chat, not a cron dump.
pub fn extract_last_prior_turn_pair(messages: &[Message]) -> Option<(Message, Message)> {
    let current_idx = current_request_index(messages)?;
    let before = &messages[..current_idx];
    let mut search_end = before.len();
    while search_end > 0 {
        let user_idx = before[..search_end].iter().rposition(|m| {
            m.role == "user"
                && is_prior_turn_message(&message_text(m))
                && !is_non_interactive_history_text(&message_text(m))
        })?;
        if let Some(assistant_msg) = before[user_idx + 1..]
            .iter()
            .find(|m| m.role == "assistant" && !is_non_interactive_history_text(&message_text(m)))
        {
            return Some((before[user_idx].clone(), assistant_msg.clone()));
        }
        search_end = user_idx;
    }
    None
}

fn build_continuation_context_message(user: &Message, assistant: &Message) -> Message {
    let body = format!(
        "[continuation_context]\n\
         Immediately preceding turn — use for disambiguation when [current_request] is a follow-up; \
         [current_request] remains primary.\n\n\
         ## user\n{}\n\n\
         ## assistant\n{}\n\
         [/continuation_context]",
        message_text(user),
        message_text(assistant),
    );
    Message {
        role: "user".into(),
        content: MessageContent::Text(body),
    }
}

fn build_quoted_continuation_message(quote: &QuotedMessageRef) -> Message {
    let follow = if quote.follow_up.trim().is_empty() {
        "(no extra text — respond to the quoted message)".to_string()
    } else {
        quote.follow_up.clone()
    };
    let body = format!(
        "[continuation_context]\n\
         The user quoted a prior message. Treat [current_request] as a follow-up to that quote; \
         [current_request] remains primary.\n\n\
         ## quoted_{} (sender={} id={})\n{}\n\n\
         ## user follow-up\n{}\n\
         [/continuation_context]",
        quote.role, quote.sender, quote.id, quote.content, follow
    );
    Message {
        role: "user".into(),
        content: MessageContent::Text(body),
    }
}

fn continuation_message_for(messages: &[Message]) -> Option<Message> {
    let current_idx = current_request_index(messages)?;
    let current_body = parse_current_request_content(&message_text(&messages[current_idx]))?;
    if let Some(quote) = parse_quoted_message_block(&current_body) {
        return Some(build_quoted_continuation_message(&quote));
    }
    let (user, assistant) = extract_last_prior_turn_pair(messages)?;
    Some(build_continuation_context_message(&user, &assistant))
}

/// FullSlim still uses a fresh Cursor session; this only highlights the live exchange in the prompt.
pub fn attach_full_slim_continuation(messages: &[Message]) -> Vec<Message> {
    let mut out = messages.to_vec();
    if already_has_continuation(&out) {
        return out;
    }
    if let Some(continuation) = continuation_message_for(&out) {
        insert_continuation_before_current_request(&mut out, continuation);
    }
    out
}

fn insert_continuation_before_current_request(delta: &mut Vec<Message>, continuation: Message) {
    if let Some(idx) = delta
        .iter()
        .position(|m| message_text(m).trim().starts_with("[current_request"))
    {
        delta.insert(idx, continuation);
    } else {
        delta.push(continuation);
    }
}

/// Trusted delta messages plus the last prior turn pair for continuation disambiguation.
pub fn build_resume_delta_messages(messages: &[Message]) -> Vec<Message> {
    let mut delta = extract_resume_delta_messages(messages);
    if let Some((user, assistant)) = extract_last_prior_turn_pair(messages) {
        insert_continuation_before_current_request(
            &mut delta,
            build_continuation_context_message(&user, &assistant),
        );
    }
    delta
}

/// Messages for a resumed Cursor session: runtime/persona/session context, hook steering, current request.
pub fn extract_resume_delta_messages(messages: &[Message]) -> Vec<Message> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.role == "assistant" {
            continue;
        }
        let text = message_text(msg);
        if text.trim().is_empty() || is_prior_turn_message(&text) {
            continue;
        }
        if is_trusted_delta_message(&text) {
            out.push(msg.clone());
        }
    }
    out
}

pub fn build_cursor_delegation_prompt(
    mode: DelegationPromptMode,
    delegation_system: &str,
    messages: &[Message],
    header: &DelegationRuntimeHeader,
    is_scheduled_task: bool,
    has_image_input: bool,
) -> String {
    let mut prompt = match mode {
        DelegationPromptMode::FullSlim => {
            let shaped = attach_full_slim_continuation(messages);
            flatten_preserving_recent(delegation_system, &shaped, MAX_PROMPT_LEN)
        }
        DelegationPromptMode::ResumeDelta => {
            let tier1_anchor = extract_tier1_anchor(delegation_system);
            let delta_messages = build_resume_delta_messages(messages);
            let runtime_header = build_minimal_runtime_header(header, tier1_anchor.as_deref());
            flatten_preserving_recent(&runtime_header, &delta_messages, MAX_PROMPT_LEN)
        }
    };

    if is_scheduled_task {
        prompt = format!(
            "[scheduled_task]\nAutomated scheduled job — not interactive chat. \
             Complete the task below and return a concise result.\n\n{prompt}"
        );
    }
    if has_image_input {
        prompt.push_str(
            "\n\n(Note: image input was attached but the Cursor engine currently supports text only. \
             Describe any needed visual context in follow-up if required.)\n",
        );
    }
    if prompt.len() > MAX_PROMPT_LEN {
        prompt = clamp_string_keeping_current_request(prompt, MAX_PROMPT_LEN);
    }
    prompt
}

fn is_prefix_context_message(text: &str) -> bool {
    let t = text.trim();
    t.contains("[system_runtime_context")
        || t.starts_with("[persona_context]")
        || t.starts_with("[session_context")
}

fn split_messages_for_budget(messages: &[Message]) -> (Vec<Message>, Vec<Message>, Vec<Message>) {
    let Some(current_idx) = current_request_index(messages) else {
        return (messages.to_vec(), Vec::new(), Vec::new());
    };
    let suffix_start = messages[..current_idx]
        .iter()
        .rposition(|m| message_text(m).trim().starts_with("[continuation_context]"))
        .unwrap_or(current_idx);
    let mut prefix_end = 0usize;
    for (i, msg) in messages[..suffix_start].iter().enumerate() {
        if is_prefix_context_message(&message_text(msg)) {
            prefix_end = i + 1;
        } else {
            break;
        }
    }
    (
        messages[..prefix_end].to_vec(),
        messages[prefix_end..suffix_start].to_vec(),
        messages[suffix_start..].to_vec(),
    )
}

fn flatten_preserving_recent(system_prompt: &str, messages: &[Message], max: usize) -> String {
    let (prefix, mut middle, suffix) = split_messages_for_budget(messages);
    loop {
        let combined = concat_message_slices(&prefix, &middle, &suffix);
        let prompt = flatten_turn_prompt(system_prompt, &combined);
        if prompt.len() <= max {
            return prompt;
        }
        if middle.is_empty() {
            break;
        }
        middle.remove(0);
    }

    const NOTICE: &str = "\n\n(older conversation trimmed for Cursor engine limit)";
    let mut sys = system_prompt.to_string();
    while sys.len() > 2048 {
        let combined = concat_message_slices(&prefix, &[], &suffix);
        let mut prompt = flatten_turn_prompt(&sys, &combined);
        if prompt.len() + NOTICE.len() <= max {
            prompt.push_str(NOTICE);
            return prompt;
        }
        let new_len = sys.len().saturating_sub(4096).max(2048);
        sys.truncate(sys.floor_char_boundary(new_len));
    }

    let combined = concat_message_slices(&prefix, &[], &suffix);
    let prompt = flatten_turn_prompt(&sys, &combined);
    if prompt.len() <= max {
        return prompt;
    }
    clamp_string_keeping_current_request(prompt, max)
}

fn concat_message_slices(
    prefix: &[Message],
    middle: &[Message],
    suffix: &[Message],
) -> Vec<Message> {
    let mut out = Vec::with_capacity(prefix.len() + middle.len() + suffix.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(middle);
    out.extend_from_slice(suffix);
    out
}

fn clamp_string_keeping_current_request(prompt: String, max: usize) -> String {
    if prompt.len() <= max {
        return prompt;
    }
    const NOTICE: &str = "\n\n(older conversation trimmed for Cursor engine limit)\n\n";
    let Some(req_at) = prompt.rfind("[current_request") else {
        let keep = max.saturating_sub(NOTICE.len()).max(1);
        let mut out = prompt;
        out.truncate(out.floor_char_boundary(keep));
        out.push_str(NOTICE.trim_end());
        return out;
    };
    let tail = &prompt[req_at..];
    let budget = max.saturating_sub(tail.len() + NOTICE.len());
    if budget == 0 {
        let mut out = tail.to_string();
        if out.len() > max {
            out.truncate(out.floor_char_boundary(max));
        }
        return out;
    }
    let mut head = prompt[..req_at].to_string();
    if head.len() > budget {
        head.truncate(head.floor_char_boundary(budget));
    }
    format!("{head}{NOTICE}{tail}")
}

fn flatten_turn_prompt(system_prompt: &str, messages: &[Message]) -> String {
    let mut out = String::new();
    out.push_str("# System\n\n");
    out.push_str(system_prompt);
    out.push_str("\n\n# Conversation\n\n");
    for msg in messages {
        let text = message_text(msg);
        if text.trim().is_empty() {
            continue;
        }
        out.push_str("## ");
        out.push_str(msg.role.as_str());
        out.push('\n');
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out
}

fn message_text(msg: &Message) -> String {
    match &msg.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_full_system() -> String {
        format!(
            "# Principles\n\nBe helpful.\n\n{TOOL_GROUPS_HEADING} (names only)\n\
             - **Shell:** bash\n- **Files:** read_file\n\n{CONVERSATION_MEMORY_HEADING}\n\
             - **Primary task**: answer [current_request]\n\n{AGENT_SKILLS_HEADING}\n\n\
             Long intro about metadata and SKILLS_CATALOG_MODE compact full modes.\n\n\
             <available_skills>\n- name: foo\n</available_skills>"
        )
    }

    #[test]
    fn slim_appends_file_delivery_reminder() {
        let full = fixture_full_system();
        let slim = slim_delegation_system_prompt(&full, true);
        assert!(slim.contains("## File links (web delivery)"));
        assert!(slim.contains("Never use `file://` URLs"));
        assert!(slim.contains("Never fabricate `/api/uploads/...` URLs"));
    }

    #[test]
    fn slim_strips_tool_groups_preserves_conversation_memory() {
        let full = fixture_full_system();
        let slim = slim_delegation_system_prompt(&full, true);
        assert!(!slim.contains("- **Shell:** bash"));
        assert!(slim.contains(CONVERSATION_MEMORY_HEADING));
        assert!(slim.contains(MCP_SERVER_NAME));
        assert!(slim.contains("# Principles"));
    }

    #[test]
    fn slim_disabled_preserves_body_and_ensures_self_repo_ban() {
        let full = fixture_full_system();
        let out = slim_delegation_system_prompt(&full, false);
        assert!(
            out.starts_with(&full),
            "slim-disabled path must keep the full system prompt body"
        );
        assert!(out.contains("Self-repo ban"));
        // Already-present ban must stay identity (no duplicate append).
        assert_eq!(slim_delegation_system_prompt(&out, false), out);
    }

    #[test]
    fn slim_shortens_agent_skills_intro() {
        let full = fixture_full_system();
        let slim = slim_delegation_system_prompt(&full, true);
        assert!(!slim.contains("SKILLS_CATALOG_MODE"));
        assert!(slim.contains("<available_skills>"));
    }

    #[test]
    fn delta_includes_trusted_messages_only() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[system_runtime_context timezone=\"UTC\"]now[/system_runtime_context]".into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\">old</user_message>".into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("prior reply".into()),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text("[persona_context]\nmemo\n[/persona_context]".into()),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\ndo it\n[/current_request]".into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text("[hook_context]\npolicy\n[/hook_context]".into()),
            },
        ];
        let delta = extract_resume_delta_messages(&messages);
        assert_eq!(delta.len(), 4);
        let joined: String = delta
            .iter()
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("[persona_context]"));
        assert!(joined.contains("[current_request"));
        assert!(joined.contains("[hook_context]"));
        assert!(!joined.contains("prior_turn"));
        assert!(!joined.contains("prior reply"));
    }

    #[test]
    fn mode_selection_always_full_slim() {
        assert_eq!(
            select_delegation_prompt_mode(Some("agent-1"), true, true),
            DelegationPromptMode::FullSlim
        );
        assert_eq!(
            select_delegation_prompt_mode(Some("agent-1"), false, true),
            DelegationPromptMode::FullSlim
        );
        assert_eq!(
            select_delegation_prompt_mode(Some("agent-1"), false, false),
            DelegationPromptMode::FullSlim
        );
        assert_eq!(
            select_delegation_prompt_mode(None, false, true),
            DelegationPromptMode::FullSlim
        );
    }

    #[test]
    fn resume_delta_prompt_uses_minimal_header() {
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text(
                "[current_request sender=\"u\"]\nhi\n[/current_request]".into(),
            ),
        }];
        let header = DelegationRuntimeHeader {
            chat_id: 1,
            persona_id: 2,
            mcp_enabled: true,
        };
        let prompt = build_cursor_delegation_prompt(
            DelegationPromptMode::ResumeDelta,
            "unused",
            &messages,
            &header,
            false,
            false,
        );
        assert!(prompt.contains("resume turn"));
        assert!(prompt.contains("chat_id=1"));
        assert!(prompt.contains("## File links (web delivery)"));
        assert!(prompt.contains("Self-repo ban"));
        assert!(prompt.contains("fully allowed"));
        assert!(!prompt.contains("# Principles"));
        assert!(prompt.contains("[current_request]"));
    }

    fn fixture_tier1_system() -> String {
        format!(
            "# Principles\n\nBe helpful.\n\n{TIER1_HEADING}\n\n\
             ### Identity\n\n**Name:** sourdough\n\n\
             ### Long-term context (Tier 1)\n\n\
             - Sourdough and Bread website Repo: /home/ken/big_storage/projects/sourdough\n\n\
             # Vault and Vector DB Paths\n\n- path: /tmp\n"
        )
    }

    #[test]
    fn resume_delta_header_includes_tier1_anchor() {
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text(
                "[current_request sender=\"u\"]\nhi\n[/current_request]".into(),
            ),
        }];
        let header = DelegationRuntimeHeader {
            chat_id: 997894126,
            persona_id: 26,
            mcp_enabled: true,
        };
        let prompt = build_cursor_delegation_prompt(
            DelegationPromptMode::ResumeDelta,
            &fixture_tier1_system(),
            &messages,
            &header,
            false,
            false,
        );
        assert!(prompt.contains(TIER1_HEADING));
        assert!(prompt.contains("/home/ken/big_storage/projects/sourdough"));
        assert!(prompt.contains("Git discipline"));
    }

    #[test]
    fn resume_delta_always_includes_last_prior_pair() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\" sender=\"u\" at=\"t1\">old task</user_message>"
                        .into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\" at=\"t2\">old reply</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\" sender=\"u\" at=\"t3\">fix index.html on dev</user_message>"
                        .into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\" at=\"t4\">Fixed /home/ken/big_storage/projects/sourdough/index.html</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\nplease commit and push to origin for dev branch\n[/current_request]"
                        .into(),
                ),
            },
        ];
        let delta = build_resume_delta_messages(&messages);
        let joined: String = delta
            .iter()
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("[continuation_context]"));
        assert!(joined.contains("fix index.html on dev"));
        assert!(joined.contains("sourdough/index.html"));
        assert!(!joined.contains("old task"));
    }

    #[test]
    fn resume_delta_continuation_before_current_request() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\">prior</user_message>".into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("done".into()),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\ngo\n[/current_request]".into(),
                ),
            },
        ];
        let delta = build_resume_delta_messages(&messages);
        let texts: Vec<String> = delta.iter().map(message_text).collect();
        let cont_idx = texts
            .iter()
            .position(|t| t.starts_with("[continuation_context]"))
            .expect("continuation");
        let req_idx = texts
            .iter()
            .position(|t| t.starts_with("[current_request"))
            .expect("current_request");
        assert!(cont_idx < req_idx);
    }

    fn test_header() -> DelegationRuntimeHeader {
        DelegationRuntimeHeader {
            chat_id: 1,
            persona_id: 24,
            mcp_enabled: false,
        }
    }

    #[test]
    fn full_slim_injects_interactive_continuation() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\">for the next one, use a more direct prompt</user_message>"
                        .into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">FILL V13 keeps the whole-skirt mask.</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">Scheduled daily PZ pipeline is the task.</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\nV6\n[/current_request]".into(),
                ),
            },
        ];
        let prompt = build_cursor_delegation_prompt(
            DelegationPromptMode::FullSlim,
            "sys",
            &messages,
            &test_header(),
            false,
            false,
        );
        assert!(prompt.contains("[continuation_context]"));
        assert!(prompt.contains("FILL V13"));
        let cont_at = prompt.find("[continuation_context]").unwrap();
        let cont_end = prompt.find("[/continuation_context]").unwrap();
        let continuation = &prompt[cont_at..cont_end];
        assert!(!continuation.contains("Scheduled daily"));
        assert!(prompt.contains("[current_request"));
        assert!(prompt.contains("V6"));
    }

    #[test]
    fn full_slim_quoted_message_is_continuation() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "<user_message context=\"prior_turn\">unrelated</user_message>".into(),
                ),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(
                    "<assistant_message context=\"prior_turn\">Alamo HOTIFY-V2</assistant_message>"
                        .into(),
                ),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\n[quoted_message id=\"v5\" role=\"assistant\" sender=\"bot\"]\nFort Mason FILL V5 is not a keep.\n[/quoted_message]\n\nV6\n[/current_request]"
                        .into(),
                ),
            },
        ];
        let prompt = build_cursor_delegation_prompt(
            DelegationPromptMode::FullSlim,
            "sys",
            &messages,
            &test_header(),
            false,
            false,
        );
        let cont_at = prompt.find("[continuation_context]").unwrap();
        let cont_end = prompt.find("[/continuation_context]").unwrap();
        let continuation = &prompt[cont_at..cont_end];
        assert!(continuation.contains("Fort Mason FILL V5"));
        assert!(continuation.contains("V6"));
        assert!(!continuation.contains("Alamo HOTIFY-V2"));
    }

    #[test]
    fn flatten_preserves_current_request_when_over_budget() {
        let old = "x".repeat(4000);
        let messages = vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text(format!(
                    "<user_message context=\"prior_turn\">{old}</user_message>"
                )),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text(format!(
                    "<assistant_message context=\"prior_turn\">{old}</assistant_message>"
                )),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text(
                    "[current_request sender=\"u\"]\nUNIQUE_LIVE_ASK\n[/current_request]".into(),
                ),
            },
        ];
        let prompt =
            flatten_preserving_recent("sys", &attach_full_slim_continuation(&messages), 1500);
        assert!(prompt.contains("UNIQUE_LIVE_ASK"));
        assert!(prompt.contains("[current_request"));
        assert!(
            prompt.contains("older conversation trimmed") || !prompt.contains(&old),
            "expected oldest history dropped or trim notice"
        );
    }
}
