//! Shared bash safety checks for `bash` and `spawn_background_command`.

use std::path::{Path, PathBuf};

use super::ToolResult;

/// Max runtime for expensive shell searches when explicitly confirmed via `CONFIRM_EXECUTE`.
pub const EXPENSIVE_SHELL_SEARCH_TIMEOUT_SECS: u64 = 120;

const EXPENSIVE_SEARCH_BLOCK_MESSAGE: &str = "Blocked expensive shell search. Recursive `grep -r` / unbounded `find` over large trees can run for many minutes and block the chat. \
Prefer: `locate_file` (declared persona/ORIGIN/Repo roots), `glob` (file names), the `grep` tool (contents; skips binaries and huge dirs), `read_tiered_memory` / `list_cursor_agent_runs` (job status), or `read_file` on a known path. \
On a locate miss, ask the user — do not widen the search. \
Do not retry with CONFIRM_EXECUTE — unscoped recursive shell searches stay blocked. Call the `grep` tool (or `locate_file`) on one declared root instead.";

pub fn parse_confirmation_prefix(command: &str) -> (bool, String) {
    const PREFIX: &str = "CONFIRM_EXECUTE ";
    if let Some(rest) = command.strip_prefix(PREFIX) {
        (true, rest.trim().to_string())
    } else {
        (false, command.to_string())
    }
}

fn command_is_risky_for_category(command_lower: &str, category: &str) -> bool {
    match category {
        "destructive" => {
            command_lower.contains("rm -rf")
                || command_lower.contains("rm -fr")
                || command_lower.contains("mkfs")
                || command_lower.contains("shred ")
                || command_lower.contains(" dd if=")
                || command_lower.starts_with("dd if=")
        }
        "system" => {
            command_lower.contains("systemctl ")
                || command_lower.contains(" service ")
                || command_lower.starts_with("service ")
                || command_lower.contains("shutdown")
                || command_lower.contains("reboot")
                || command_lower.contains("killall ")
                || command_lower.contains("pkill ")
                || command_lower.contains("launchctl ")
                || command_lower.contains("sudo ")
        }
        "network" => {
            (command_lower.contains("curl ")
                && (command_lower.contains(" -x post")
                    || command_lower.contains(" --request post")
                    || command_lower.contains(" -x put")
                    || command_lower.contains(" --request put")
                    || command_lower.contains(" -x patch")
                    || command_lower.contains(" --request patch")
                    || command_lower.contains(" -x delete")
                    || command_lower.contains(" --request delete")))
                || (command_lower.contains("wget ")
                    && (command_lower.contains(" --post")
                        || command_lower.contains(" --method=post")
                        || command_lower.contains(" --method=put")
                        || command_lower.contains(" --method=patch")
                        || command_lower.contains(" --method=delete")))
        }
        "package" => {
            command_lower.contains("apt-get ")
                || command_lower.starts_with("apt ")
                || command_lower.contains(" yum ")
                || command_lower.starts_with("yum ")
                || command_lower.contains(" dnf ")
                || command_lower.starts_with("dnf ")
                || command_lower.contains(" pacman ")
                || command_lower.starts_with("pacman ")
                || command_lower.contains("brew install")
                || command_lower.contains("brew uninstall")
                || command_lower.contains("pip install")
                || command_lower.contains("pip uninstall")
                || command_lower.contains("npm install")
                || command_lower.contains("npm uninstall")
                || command_lower.contains("cargo install")
                || command_lower.contains("cargo uninstall")
        }
        _ => false,
    }
}

pub fn detect_risky_categories(command: &str, configured_categories: &[String]) -> Vec<String> {
    let command_lower = command.to_ascii_lowercase();
    let mut matched = Vec::new();
    for category in configured_categories {
        let c = category.trim().to_ascii_lowercase();
        if c.is_empty() {
            continue;
        }
        if command_is_risky_for_category(&command_lower, &c) {
            matched.push(c);
        }
    }
    matched.sort();
    matched.dedup();
    matched
}

/// Returns `None` when execution may proceed, or a blocked/confirmation `ToolResult`.
pub fn check_bash_safety(
    command: &str,
    confirmed: bool,
    safety_execution_mode: &str,
    safety_risky_categories: &[String],
) -> Option<ToolResult> {
    let safety_mode = safety_execution_mode.trim().to_ascii_lowercase();
    let risky_categories = detect_risky_categories(command, safety_risky_categories);
    if risky_categories.is_empty() || safety_mode == "off" {
        return None;
    }
    if safety_mode == "strict" {
        return Some(
            ToolResult::error(format!(
                "Blocked by safety_execution_mode=strict. Risky categories detected: [{}]. Command was not executed.",
                risky_categories.join(", ")
            ))
            .with_error_type("blocked_by_policy"),
        );
    }
    if safety_mode == "warn_confirm" && !confirmed {
        return Some(
            ToolResult::error(format!(
                "Execution paused by safety policy. Risky categories detected: [{}]. \
Add explicit confirmation by re-running with prefix: CONFIRM_EXECUTE <your command>",
                risky_categories.join(", ")
            ))
            .with_error_type("confirmation_required"),
        );
    }
    None
}

/// True when a shell command is likely to scan huge directory trees (e.g. `grep -r` on `shared/`).
///
/// When `allowed_roots` is non-empty, name-filtered `find` is still expensive unless every find
/// root is under one of those declared roots (or is a relative cwd-scoped path).
pub fn is_expensive_shell_search(command: &str) -> bool {
    is_expensive_shell_search_scoped(command, &[])
}

pub fn is_expensive_shell_search_scoped(command: &str, allowed_roots: &[PathBuf]) -> bool {
    let cmd = command.trim();
    if cmd.is_empty() {
        return false;
    }
    is_recursive_grep_command_scoped(cmd, allowed_roots)
        || is_unbounded_find_command(cmd, allowed_roots)
        || is_unbounded_ripgrep_command(cmd)
}

/// Returns `None` when execution may proceed, or a blocked `ToolResult`.
/// `spawn_background_command` does not call this — long scans may be intentional in background.
///
/// Unscoped recursive shell searches (`grep -r` / broad `find`) stay blocked even when the
/// command is prefixed with `CONFIRM_EXECUTE`. Confirmation only applies to other safety categories.
pub fn check_expensive_shell_search(command: &str, confirmed: bool) -> Option<ToolResult> {
    check_expensive_shell_search_scoped(command, confirmed, &[], &[])
}

/// Like [`check_expensive_shell_search`], with declared roots for scoping and optional hint labels
/// shown in the block message (from the locate registry).
pub fn check_expensive_shell_search_scoped(
    command: &str,
    _confirmed: bool,
    allowed_roots: &[PathBuf],
    root_hints: &[String],
) -> Option<ToolResult> {
    // Unscoped recursive searches are never confirmable — models treated CONFIRM_EXECUTE as the
    // default retry and burned multi-minute timeouts on /home tree walks.
    if !is_expensive_shell_search_scoped(command, allowed_roots) {
        return None;
    }
    let mut msg = EXPENSIVE_SEARCH_BLOCK_MESSAGE.to_string();
    if !root_hints.is_empty() {
        msg.push_str("\n\nDeclared roots to try with locate_file / the grep tool:\n");
        for h in root_hints.iter().take(12) {
            msg.push_str("- ");
            msg.push_str(h);
            msg.push('\n');
        }
        if let Some(pattern) = extract_find_name_pattern(command) {
            msg.push_str(&format!(
                "Suggested: locate_file with query=\"{pattern}\" (or the ORIGIN-relative path you expected).\n"
            ));
        } else {
            msg.push_str(
                "Suggested: call the grep tool with path set to one declared root (and an optional glob), or locate_file with the basename you expected.\n",
            );
        }
    }
    Some(ToolResult::error(msg).with_error_type("blocked_by_policy"))
}

fn extract_find_name_pattern(command: &str) -> Option<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    for i in 0..parts.len() {
        if parts[i] == "-name" || parts[i] == "-iname" {
            let pat = parts.get(i + 1)?;
            return Some(pat.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

/// Block git that explicitly targets the bot's own checkout.
/// An external path that covers the checkout root lifts the block.
pub fn check_self_repo_git(
    command: &str,
    workspace_root: &Path,
    external_roots: &[PathBuf],
) -> Option<ToolResult> {
    if !crate::self_repo::command_targets_self_repo_git(command, workspace_root) {
        return None;
    }
    if crate::self_repo::self_repo_git_allowed_by_external_roots(workspace_root, external_roots) {
        return None;
    }
    Some(
        ToolResult::error(crate::self_repo::self_repo_git_block_message(
            workspace_root,
        ))
        .with_error_type("blocked_by_policy"),
    )
}

/// Recursive `grep -r` / `grep -rn` is expensive unless every search root is under a declared
/// allowed root (or is a relative cwd-scoped path). Narrow greps like
/// `grep -rn pattern /allowed/root/*.py` stay allowed.
fn is_recursive_grep_command_scoped(command: &str, allowed_roots: &[PathBuf]) -> bool {
    let lower = command.to_ascii_lowercase();
    if !lower.contains("grep") {
        return false;
    }
    let recursive = lower.contains(" -r")
        || lower.contains(" -rn")
        || lower.contains(" -r ")
        || lower.contains(" -rn ")
        || lower.contains(" --recursive")
        || lower.starts_with("grep -r")
        || lower.starts_with("grep -rn")
        || lower.starts_with("grep -R")
        || lower.contains("grep -r")
        || lower.contains("grep -rn")
        || lower.contains("grep -R");
    if !recursive {
        return false;
    }
    let tokens = grep_path_tokens(command);
    if tokens.is_empty() {
        // `grep -rn pattern` with implicit `.` — relative/cwd scoped when allowed_roots empty
        // still counts as expensive (shared/ workspace walks). With allowed roots, cwd is
        // persona-scoped and relative `.` is treated as allowed.
        return allowed_roots.is_empty();
    }
    for token in tokens {
        if is_broad_search_root(&token) {
            return true;
        }
        if !find_root_is_allowed(&token, allowed_roots) {
            return true;
        }
    }
    false
}

/// Extract path-like arguments after the pattern in a grep invocation.
fn grep_path_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut parts = command.split_whitespace().peekable();
    while let Some(part) = parts.next() {
        let base = part.rsplit('/').next().unwrap_or(part);
        if base != "grep" && base != "egrep" && base != "fgrep" && !base.ends_with("grep") {
            continue;
        }
        // Skip options until we reach the pattern, then collect paths.
        let mut seen_pattern = false;
        while let Some(arg) = parts.peek().copied() {
            if !seen_pattern {
                if arg.starts_with('-') {
                    let opt = parts.next().unwrap_or(arg);
                    if grep_option_takes_value(opt) {
                        let _ = parts.next();
                    }
                    continue;
                }
                // Pattern token.
                let _ = parts.next();
                seen_pattern = true;
                continue;
            }
            let path = parts.next().unwrap_or(arg);
            if path == "|" || path.starts_with('|') {
                break;
            }
            // Strip simple shell redirections glued to the token.
            let cleaned = path.trim_matches('"').trim_matches('\'');
            if cleaned.is_empty() {
                continue;
            }
            tokens.push(cleaned.to_string());
        }
    }
    tokens
}

fn grep_option_takes_value(opt: &str) -> bool {
    matches!(
        opt,
        "-e" | "-f"
            | "-m"
            | "--regexp"
            | "--file"
            | "--max-count"
            | "--include"
            | "--exclude"
            | "--exclude-dir"
            | "--exclude-from"
            | "-A"
            | "-B"
            | "-C"
            | "--after-context"
            | "--before-context"
            | "--context"
    ) || opt.starts_with("--include=")
        || opt.starts_with("--exclude=")
        || opt.starts_with("--exclude-dir=")
}

fn is_unbounded_find_command(command: &str, allowed_roots: &[PathBuf]) -> bool {
    let lower = command.to_ascii_lowercase();
    if !lower.contains("find ") {
        return false;
    }
    if lower.contains("-maxdepth") {
        return false;
    }
    // Name-filtered finds are usually fast — but only when every root is declared/scoped.
    // `find /home/ken/big_storage -name …` must stay expensive (was the hang vector).
    if lower.contains("-name") || lower.contains("-iname") {
        return find_targets_unscoped_root(command, allowed_roots);
    }
    true
}

/// True when any `find` root is broad (`/`, `/home`, …) or outside declared allowed roots.
fn find_targets_unscoped_root(command: &str, allowed_roots: &[PathBuf]) -> bool {
    let tokens = find_path_tokens(command);
    if tokens.is_empty() {
        // `find -name foo` with implicit `.` — relative/cwd scoped.
        return false;
    }
    for token in tokens {
        if is_broad_search_root(&token) {
            return true;
        }
        if !find_root_is_allowed(&token, allowed_roots) {
            return true;
        }
    }
    false
}

fn find_root_is_allowed(raw: &str, allowed_roots: &[PathBuf]) -> bool {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return false;
    }
    // Relative paths are cwd-scoped (persona tool cwd) — allowed.
    // A leading `/` is absolute in the command text even when `Path::is_absolute`
    // is false on Windows.
    let rooted = Path::new(trimmed).is_absolute()
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.starts_with('~')
        || trimmed.eq_ignore_ascii_case("$home")
        || trimmed.to_ascii_lowercase().starts_with("$home/")
        || trimmed.to_ascii_lowercase().starts_with("${home}");
    if !rooted {
        return true;
    }
    if allowed_roots.is_empty() {
        // No registry: keep legacy behavior — only block broad home roots.
        return !is_broad_search_root(trimmed);
    }
    let abs = Path::new(trimmed);
    allowed_roots.iter().any(|root| {
        // Find target must be the allowed root or a descendant — not a parent of it.
        crate::tools::path_under_root(abs, root)
    })
}

fn find_path_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut parts = command.split_whitespace().peekable();
    while let Some(part) = parts.next() {
        if part != "find" && !part.ends_with("/find") {
            continue;
        }
        while let Some(arg) = parts.peek().copied() {
            if arg == "--" {
                parts.next();
                continue;
            }
            if arg.starts_with('-') {
                // Skip option and its value when it takes one.
                let opt = parts.next().unwrap_or(arg);
                if find_option_takes_value(opt) {
                    let _ = parts.next();
                }
                continue;
            }
            tokens.push(arg.to_string());
            parts.next();
            // Keep collecting roots until options resume.
            while let Some(next) = parts.peek().copied() {
                if next.starts_with('-') {
                    break;
                }
                tokens.push(next.to_string());
                parts.next();
            }
            break;
        }
    }
    tokens
}

fn find_option_takes_value(opt: &str) -> bool {
    matches!(
        opt,
        "-name"
            | "-iname"
            | "-path"
            | "-ipath"
            | "-regex"
            | "-iregex"
            | "-type"
            | "-user"
            | "-group"
            | "-size"
            | "-mtime"
            | "-atime"
            | "-ctime"
            | "-mmin"
            | "-amin"
            | "-cmin"
            | "-maxdepth"
            | "-mindepth"
            | "-newer"
            | "-exec"
            | "-execdir"
            | "-ok"
            | "-printf"
            | "-fprintf"
    ) || opt.starts_with("-name")
        || opt.starts_with("-iname")
}

fn is_broad_search_root(raw: &str) -> bool {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "~"
        || lower == "$home"
        || lower == "${home}"
        || lower.starts_with("~/")
        || lower.starts_with("$home/")
        || lower.starts_with("${home}/")
    {
        // `~/project/...` is scoped; bare `~` / `$HOME` is not.
        return lower == "~" || lower == "$home" || lower == "${home}";
    }
    let path = std::path::Path::new(trimmed);
    let components: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::RootDir => Some("/".to_string()),
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return false;
    }
    // `/`
    if components.len() == 1 && components[0] == "/" {
        return true;
    }
    // `/home`, `/Users`, `/home/<user>`, `/Users/<user>`
    if components.first().map(String::as_str) == Some("/") {
        match components.get(1).map(String::as_str) {
            Some("home") | Some("Users") => components.len() <= 3,
            _ => false,
        }
    } else {
        false
    }
}

fn is_unbounded_ripgrep_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let uses_rg = lower.starts_with("rg ") || lower.contains(" rg ");
    if !uses_rg {
        return false;
    }
    let bounded = lower.contains("--max-count")
        || lower.contains(" -m ")
        || lower.contains("| head ")
        || lower.contains("|head ");
    !bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expensive_shell_search_blocks_recursive_grep() {
        assert!(is_expensive_shell_search(
            "grep -r \"PZ\" /home/ken/proj/workspace/shared/ | head -n 10"
        ));
    }

    #[test]
    fn expensive_shell_search_allows_find_with_name_filter_relative() {
        assert!(!is_expensive_shell_search(
            "find ./workspace/shared -name 'resumx' -type f"
        ));
        assert!(!is_expensive_shell_search(
            "find ORIGIN -name 'Sourdough*.md' -type f"
        ));
    }

    #[test]
    fn expensive_shell_search_blocks_broad_home_find_even_with_name() {
        assert!(is_expensive_shell_search(
            "find /home/ken -name 'resumx' -type f"
        ));
        assert!(is_expensive_shell_search("find /home -name foo"));
        assert!(is_expensive_shell_search("find / -name foo"));
        assert!(is_expensive_shell_search("find ~ -name foo"));
        assert!(is_expensive_shell_search("find $HOME -name foo"));
    }

    #[test]
    fn expensive_shell_search_blocks_unregistered_absolute_find_with_name() {
        // Hang vector: name filter under /home/ken/big_storage (not a "broad" home root
        // under the old depth<=3 rule, but outside any declared registry root).
        let allowed = vec![PathBuf::from(
            "/home/ken/big_storage/projects/meido/workspace",
        )];
        assert!(is_expensive_shell_search_scoped(
            "find /home/ken/big_storage -name 'Sourdough-Journal-Post-Pipeline.md' -type f",
            &allowed,
        ));
        // Same find is allowed when the root itself is declared (or an ancestor is).
        let allowed_sourdough = vec![PathBuf::from("/home/ken/big_storage/projects/sourdough")];
        assert!(!is_expensive_shell_search_scoped(
            "find /home/ken/big_storage/projects/sourdough -name 'planner.html' -type f",
            &allowed_sourdough,
        ));
        assert!(!is_expensive_shell_search_scoped(
            "find /home/ken/big_storage/projects/sourdough/assets -name '*.jpeg' -type f",
            &allowed_sourdough,
        ));
    }

    #[test]
    fn expensive_shell_search_blocks_unbounded_find() {
        assert!(is_expensive_shell_search(
            "find ./workspace/shared -mtime -1"
        ));
    }

    #[test]
    fn expensive_shell_search_allows_simple_ls() {
        assert!(!is_expensive_shell_search(
            "ls -lh /tmp/shared/PZ-20260515-CAFE-LoRA-Fixed.png"
        ));
    }

    #[test]
    fn check_expensive_shell_search_blocks_even_when_confirmed() {
        let cmd = "grep -r foo ./shared/";
        assert!(check_expensive_shell_search(cmd, false).is_some());
        // CONFIRM_EXECUTE must not unlock unscoped recursive greps.
        assert!(check_expensive_shell_search(cmd, true).is_some());
        let blocked = check_expensive_shell_search(cmd, true).expect("blocked");
        assert!(blocked
            .content
            .contains("Do not retry with CONFIRM_EXECUTE"));
    }

    #[test]
    fn expensive_shell_search_allows_scoped_recursive_grep() {
        let allowed = vec![PathBuf::from(
            "/home/ken/big_storage/airflowProd/notebook/My_Trader2.0",
        )];
        assert!(!is_expensive_shell_search_scoped(
            "grep -rn \"screener\" /home/ken/big_storage/airflowProd/notebook/My_Trader2.0/*.py",
            &allowed,
        ));
        assert!(is_expensive_shell_search_scoped(
            "grep -rn \"finviz\" /home/ken/big_storage/airflowProd 2>/dev/null",
            &allowed,
        ));
    }

    #[test]
    fn check_expensive_shell_search_scoped_lists_hints() {
        let blocked = check_expensive_shell_search_scoped(
            "find /home/ken/big_storage -name 'foo.md'",
            false,
            &[PathBuf::from("/tmp/workspace")],
            &["persona ORIGIN (`/tmp/workspace/shared/personas/1/2/ORIGIN`)".into()],
        )
        .expect("should block");
        assert!(blocked.content.contains("locate_file"));
        assert!(blocked.content.contains("persona ORIGIN"));
        assert!(blocked.content.contains("foo.md"));
        assert!(blocked
            .content
            .contains("Do not retry with CONFIRM_EXECUTE"));
    }
}
