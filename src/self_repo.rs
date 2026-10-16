//! Identify and protect the finally-a-value-bot install/source checkout.
//!
//! Persona Tier-1 target repos (e.g. `Repo: /home/.../sourdough`) remain fully allowed.
//! Only this bot's own checkout is banned as a Cursor/agent git project.

use std::path::{Path, PathBuf};

/// Operator override: absolute path to the bot source/install git root.
pub const ENV_SELF_REPO: &str = "FINALLY_A_VALUE_BOT_SELF_REPO";

/// Env var applied to agent shells so git does not walk into the bot checkout.
pub const ENV_GIT_CEILING: &str = "GIT_CEILING_DIRECTORIES";

const PACKAGE_NAME: &str = "finally-a-value-bot";

/// True when `dir` looks like this crate's source/install root.
pub fn looks_like_self_repo(dir: &Path) -> bool {
    let cargo = dir.join("Cargo.toml");
    if cargo.is_file() {
        if let Ok(text) = std::fs::read_to_string(&cargo) {
            if cargo_toml_names_package(&text, PACKAGE_NAME) {
                return true;
            }
        }
    }
    (dir.join("scripts").join("cursor-sdk-runner.py").is_file()
        || dir.join("scripts").join("cursor-sdk-runner.mjs").is_file())
        && dir.join("src").join("lib.rs").is_file()
}

fn cargo_toml_names_package(toml: &str, name: &str) -> bool {
    // Minimal parse: look for `name = "finally-a-value-bot"` near the package table.
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"').trim_matches('\'');
                return value == name;
            }
        }
    }
    false
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve the bot's own git/source root, if detectable.
pub fn resolve_self_repo_root(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(raw) = std::env::var(ENV_SELF_REPO) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_dir() {
                return Some(canonicalize_best_effort(&p));
            }
        }
    }

    let ws = canonicalize_best_effort(workspace_root);
    if let Some(parent) = ws.parent() {
        if looks_like_self_repo(parent) {
            return Some(canonicalize_best_effort(parent));
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if manifest.is_dir() && looks_like_self_repo(&manifest) {
        return Some(canonicalize_best_effort(&manifest));
    }

    None
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path = canonicalize_best_effort(path);
    let root = canonicalize_best_effort(root);
    path.starts_with(&root)
}

/// Paths under `WORKSPACE_DIR` are data (persona files, skills, runtime) — not "self-repo source".
pub fn is_under_workspace_data(workspace_root: &Path, path: &Path) -> bool {
    path_is_under(path, workspace_root)
}

/// True when `path` is inside the bot checkout but outside the workspace data root.
pub fn is_self_repo_source_path(workspace_root: &Path, path: &Path) -> bool {
    let Some(self_repo) = resolve_self_repo_root(workspace_root) else {
        return false;
    };
    if is_under_workspace_data(workspace_root, path) {
        return false;
    }
    path_is_under(path, &self_repo)
}

/// Refuse Cursor/agent cwd when it is the self-repo root or other source paths outside workspace.
pub fn check_agent_cwd_allowed(workspace_root: &Path, cwd: &Path) -> Result<(), String> {
    if is_self_repo_source_path(workspace_root, cwd) {
        let self_repo = resolve_self_repo_root(workspace_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "finally-a-value-bot".into());
        return Err(format!(
            "Blocked: Cursor/agent working directory '{}' is inside the bot's own checkout ({self_repo}). \
             Use the persona workspace under WORKSPACE_DIR, or cd to a Tier-1 target repo \
             (persona `Repo:` path). The bot source repo is never a project workspace.",
            cwd.display()
        ));
    }
    Ok(())
}

/// Value for `GIT_CEILING_DIRECTORIES`: stop upward git discovery at the workspace root.
///
/// Explicit `cd /path/to/tier1-project && git …` still works — ceiling only blocks walking
/// from persona cwd into the parent bot checkout. Target repos outside WORKSPACE_DIR are
/// unaffected.
pub fn git_ceiling_value(workspace_root: &Path) -> String {
    canonicalize_best_effort(workspace_root)
        .to_string_lossy()
        .into_owned()
}

/// Apply git ceiling env to an agent subprocess. Does not remove an existing operator override.
pub fn apply_git_ceiling_env(cmd: &mut tokio::process::Command, workspace_root: &Path) {
    let value = git_ceiling_value(workspace_root);
    if value.is_empty() {
        return;
    }
    // Prefer our ceiling; merge if already set so operators can add extra ceilings.
    match std::env::var(ENV_GIT_CEILING) {
        Ok(existing) if !existing.trim().is_empty() => {
            let sep = if cfg!(windows) { ";" } else { ":" };
            let mut merged = existing;
            for part in value.split(sep) {
                if !part.is_empty() && !merged.split(sep).any(|e| e == part) {
                    merged.push_str(sep);
                    merged.push_str(part);
                }
            }
            cmd.env(ENV_GIT_CEILING, merged);
        }
        _ => {
            cmd.env(ENV_GIT_CEILING, value);
        }
    }
}

/// Absolute path display for prompts (empty when unknown).
pub fn self_repo_display(workspace_root: &Path) -> Option<String> {
    resolve_self_repo_root(workspace_root).map(|p| p.display().to_string())
}

/// Prompt fragment: ban self-repo; allow Tier-1 target repos.
pub fn self_repo_ban_prompt_section(workspace_root: &Path) -> String {
    let self_path = self_repo_display(workspace_root)
        .unwrap_or_else(|| "(bot install/source checkout)".to_string());
    format!(
        "## Self-repo ban (mandatory)\n\
         - **Never** treat the finally-a-value-bot install/source git checkout as a project repo.\n\
         - Banned root: `{self_path}` (and its `.git`). Do not `cd` there for development, \
           branch deletes, force-push, reset, or other git/destructive work.\n\
         - Persona **Tier-1 target repos** (`Repo: /absolute/path/...` in identity/memory) are \
           **fully allowed** — `cd` to that path and run git/file/shell work there.\n\
         - Persona cwd under WORKSPACE_DIR is not a git project root; do not rely on git \
           discovering a parent checkout.\n"
    )
}

/// True if a shell command appears to run git (or cd+git) against the self-repo source tree.
/// Tier-1 project paths outside the self-repo are not matched.
pub fn command_targets_self_repo_git(command: &str, workspace_root: &Path) -> bool {
    let Some(self_repo) = resolve_self_repo_root(workspace_root) else {
        return false;
    };
    let self_str = self_repo.to_string_lossy();
    if self_str.is_empty() {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    let mentions_git = lower.contains("git ")
        || lower.starts_with("git\t")
        || lower.starts_with("git ")
        || lower.contains("|git ")
        || lower.contains("&&git ")
        || lower.contains(";git ")
        || lower.contains("`git ")
        || lower.contains("$(git ");
    if !mentions_git {
        return false;
    }
    command_mentions_self_repo_source(command, &self_str)
}

fn command_mentions_self_repo_source(command: &str, self_str: &str) -> bool {
    let mut rest = command;
    while let Some(idx) = rest.find(self_str) {
        let before_ok = idx == 0
            || rest[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || matches!(c, '=' | '"' | '\'' | ':'));
        let after = &rest[idx + self_str.len()..];
        let next = after.chars().next();
        let boundary_ok = next.is_none()
            || next.is_some_and(|c| {
                c.is_whitespace() || matches!(c, '/' | '\\' | '"' | '\'' | ';' | '&' | '|' | ')')
            });
        if before_ok && boundary_ok {
            // Paths under `…/<self>/workspace/…` are data roots — not banned source.
            if after.starts_with('/') || after.starts_with('\\') {
                let rel = after.trim_start_matches(['/', '\\']);
                let first = rel.split(['/', '\\']).next().unwrap_or("");
                if first == "workspace" {
                    rest = &rest[idx + self_str.len()..];
                    continue;
                }
            }
            return true;
        }
        rest = &rest[idx + self_str.len()..];
    }
    false
}

/// Block message when bash tries to git the self-repo.
pub fn self_repo_git_block_message(workspace_root: &Path) -> String {
    let self_path =
        self_repo_display(workspace_root).unwrap_or_else(|| "finally-a-value-bot".to_string());
    format!(
        "Blocked: git against the bot's own checkout ({self_path}) is not allowed. \
         Persona Tier-1 target repos (`Repo: …` paths) are fully allowed — cd there first. \
         Do not use the finally-a-value-bot source repo as a project."
    )
}

/// Normalize path components for tests / display helpers.
#[cfg(test)]
pub fn path_has_normal_component(path: &Path, name: &str) -> bool {
    use std::path::Component;
    path.components().any(|c| match c {
        Component::Normal(s) => s == name,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_tree(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "finally_a_value_bot_self_repo_{label}_{}",
            uuid::Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn detects_cargo_package_name() {
        let root = temp_tree("cargo");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"finally-a-value-bot\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        assert!(looks_like_self_repo(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_paths_not_self_source() {
        let root = temp_tree("ws");
        let ws = root.join("workspace");
        fs::create_dir_all(ws.join("shared")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"finally-a-value-bot\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();

        assert!(is_self_repo_source_path(&ws, &root.join("src")));
        assert!(!is_self_repo_source_path(
            &ws,
            &ws.join("shared").join("personas").join("1").join("2")
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_command_blocks_self_allows_other_project() {
        let root = temp_tree("gitcmd");
        let ws = root.join("workspace");
        fs::create_dir_all(&ws).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"finally-a-value-bot\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(root.join("scripts").join("cursor-sdk-runner.py"), "").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();

        let self_abs = canonicalize_best_effort(&root);
        let other = "/home/ken/big_storage/projects/sourdough";
        assert!(command_targets_self_repo_git(
            &format!("cd {} && git branch -D main", self_abs.display()),
            &ws
        ));
        assert!(!command_targets_self_repo_git(
            &format!("cd {other} && git push origin HEAD"),
            &ws
        ));
        assert!(!command_targets_self_repo_git("git status", &ws));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn check_agent_cwd_allows_persona_dir() {
        let root = temp_tree("cwd");
        let ws = root.join("workspace");
        let persona = ws.join("shared").join("personas").join("1").join("2");
        fs::create_dir_all(&persona).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"finally-a-value-bot\"\n",
        )
        .unwrap();
        assert!(check_agent_cwd_allowed(&ws, &persona).is_ok());
        assert!(check_agent_cwd_allowed(&ws, &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
