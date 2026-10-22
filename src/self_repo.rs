//! Identify and protect the Meido install/source checkout.
//!
//! Persona Tier-1 target repos (e.g. `Repo: /home/.../sourdough`) remain fully allowed.
//! Only this bot's own checkout is banned as a Cursor/agent git project.

use std::path::{Path, PathBuf};

/// Operator override: absolute path to the bot source/install git root (`MEIDO_SELF_REPO`).
pub const ENV_SELF_REPO: &str = crate::meido_env::SELF_REPO.preferred;

/// Env var applied to agent shells so git does not walk into the bot checkout.
pub const ENV_GIT_CEILING: &str = "GIT_CEILING_DIRECTORIES";

/// Current Cargo package name (`env!("CARGO_PKG_NAME")` of this crate).
const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");

/// Package names that identify a Meido checkout. Older checkouts still carry the
/// pre-rename name and must stay protected.
const PACKAGE_NAMES: &[&str] = &[PACKAGE_NAME, "finally-a-value-bot"];

/// True when `dir` looks like this crate's source/install root.
pub fn looks_like_self_repo(dir: &Path) -> bool {
    let cargo = dir.join("Cargo.toml");
    if cargo.is_file() {
        if let Ok(text) = std::fs::read_to_string(&cargo) {
            if PACKAGE_NAMES
                .iter()
                .any(|name| cargo_toml_names_package(&text, name))
            {
                return true;
            }
        }
    }
    (dir.join("scripts").join("cursor-sdk-runner.py").is_file()
        || dir.join("scripts").join("cursor-sdk-runner.mjs").is_file())
        && dir.join("src").join("lib.rs").is_file()
}

fn cargo_toml_names_package(toml: &str, name: &str) -> bool {
    // Minimal parse: look for `name = "<name>"` inside the `[package]` table.
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
    if let Some(raw) = crate::meido_env::get(crate::meido_env::SELF_REPO) {
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

/// Checkout this process was built from, when that directory is still on disk.
///
/// Prefers the compile-time crate root over the process cwd so a data workspace
/// that sits inside an older checkout is not mistaken for the running app.
pub fn current_app_directory() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if manifest.is_dir() && looks_like_self_repo(&manifest) {
        return Some(canonicalize_best_effort(&manifest));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = self_repo_ancestor(&exe) {
            return Some(root);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(root) = self_repo_ancestor(&cwd) {
            return Some(root);
        }
    }
    None
}

fn self_repo_ancestor(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().map(Path::to_path_buf)
    } else {
        Some(start.to_path_buf())
    };
    for _ in 0..8 {
        let Some(dir) = current else {
            break;
        };
        if looks_like_self_repo(&dir) {
            return Some(canonicalize_best_effort(&dir));
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    path_is_under(a, b) || path_is_under(b, a)
}

/// True when `WORKSPACE_DIR` is neither inside the app checkout nor an ancestor of it.
pub fn workspace_disjoint_from_app(workspace_root: &Path) -> bool {
    match current_app_directory() {
        Some(app) => !paths_overlap(workspace_root, &app),
        None => false,
    }
}

/// App checkout every persona may edit when the data workspace does not overlap it.
pub fn auto_approved_app_directory(workspace_root: &Path) -> Option<PathBuf> {
    if !workspace_disjoint_from_app(workspace_root) {
        return None;
    }
    current_app_directory()
}

/// Append the app checkout when the workspace sits outside it.
///
/// Already-covered paths are left as the operator saved them.
pub fn append_auto_approved_app_directory(workspace_root: &Path, roots: &mut Vec<PathBuf>) {
    let Some(app) = auto_approved_app_directory(workspace_root) else {
        return;
    };
    if path_covered_by_roots(&app, roots) {
        return;
    }
    roots.push(app);
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

/// True when `path` is the same as `root` or a descendant of it.
pub fn path_covered_by_roots(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_is_under(path, root))
}

/// External paths that overlap the bot checkout: the checkout itself, a parent, or a subdirectory.
pub fn external_roots_overlapping_self_repo(
    workspace_root: &Path,
    external_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let Some(self_repo) = resolve_self_repo_root(workspace_root) else {
        return Vec::new();
    };
    external_roots
        .iter()
        .filter(|root| path_is_under(root, &self_repo) || path_is_under(&self_repo, root))
        .cloned()
        .collect()
}

/// Git against the checkout is allowed only when an external path covers the checkout root.
pub fn self_repo_git_allowed_by_external_roots(
    workspace_root: &Path,
    external_roots: &[PathBuf],
) -> bool {
    let Some(self_repo) = resolve_self_repo_root(workspace_root) else {
        return false;
    };
    path_covered_by_roots(&self_repo, external_roots)
}

/// Refuse Cursor/agent cwd when it is the self-repo root or other source paths outside workspace.
pub fn check_agent_cwd_allowed(workspace_root: &Path, cwd: &Path) -> Result<(), String> {
    if is_self_repo_source_path(workspace_root, cwd) {
        let self_repo = resolve_self_repo_root(workspace_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| PACKAGE_NAME.into());
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

/// When this persona's external paths overlap the checkout, replace the hard ban
/// so the model is allowed to work on the listed paths.
pub fn rewrite_self_repo_ban_for_external_paths(
    prompt: String,
    workspace_root: &Path,
    external_roots: &[PathBuf],
) -> String {
    let listed = external_roots_overlapping_self_repo(workspace_root, external_roots);
    if listed.is_empty() {
        return prompt;
    }
    let Some(self_repo) = resolve_self_repo_root(workspace_root) else {
        return prompt;
    };
    const HEADING: &str = "## Self-repo ban (mandatory)\n";
    let Some(start) = prompt.find(HEADING) else {
        return prompt;
    };
    let after = start + HEADING.len();
    let end = prompt[after..]
        .find("\n\n")
        .map(|i| after + i)
        .unwrap_or(prompt.len());
    let listed_fmt = listed
        .iter()
        .map(|p| format!("`{}`", p.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let git_line = if self_repo_git_allowed_by_external_roots(workspace_root, external_roots) {
        format!(
            "- Git against `{self}` is allowed because that checkout is covered by the external-path list.\n",
            self = self_repo.display()
        )
    } else {
        format!(
            "- Git that names the checkout root `{self}` stays blocked until that root (or a parent) is on the external-path list. File reads and writes under the listed paths are still allowed.\n",
            self = self_repo.display()
        )
    };
    let replacement = format!(
        "## Self-repo ban (mandatory)\n\
         - Banned root: `{self}` (and its `.git`), except paths covered by this persona's external paths.\n\
         - **Allowed:** {listed_fmt}. Reads, writes, and shell under those listed paths are allowed, including inside the Meido checkout.\n\
         {git_line}\
         - Every path in that checkout that is not under a listed path stays banned.\n\
         - Persona **Tier-1 target repos** (`Repo: /absolute/path/...`) outside this checkout stay fully allowed.\n\
         - Persona cwd under WORKSPACE_DIR is not a git project root; do not rely on git discovering a parent checkout.\n",
        self = self_repo.display(),
        git_line = git_line,
    );
    let mut out = String::with_capacity(prompt.len() + replacement.len());
    out.push_str(&prompt[..start]);
    out.push_str(&replacement);
    out.push_str(&prompt[end..]);
    out
}

/// Prompt fragment: ban self-repo; allow Tier-1 target repos.
pub fn self_repo_ban_prompt_section(workspace_root: &Path) -> String {
    let self_path = self_repo_display(workspace_root)
        .unwrap_or_else(|| "(bot install/source checkout)".to_string());
    format!(
        "## Self-repo ban (mandatory)\n\
         - **Never** treat the Meido install/source git checkout as a project repo.\n\
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
    let self_path = self_repo_display(workspace_root).unwrap_or_else(|| PACKAGE_NAME.to_string());
    format!(
        "Blocked: git against the bot's own checkout ({self_path}) is not allowed. \
         Add that checkout to this persona's external paths to allow it. \
         Persona Tier-1 target repos (`Repo: …` paths) are fully allowed — cd there first."
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
        let root =
            std::env::temp_dir().join(format!("meido_self_repo_{label}_{}", uuid::Uuid::new_v4()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn detects_cargo_package_name() {
        let root = temp_tree("cargo");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"meido\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        assert!(looks_like_self_repo(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_legacy_cargo_package_name() {
        let root = temp_tree("cargo_legacy");
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
    fn ignores_unrelated_cargo_package() {
        let root = temp_tree("cargo_other");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"sourdough\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        assert!(!looks_like_self_repo(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_paths_not_self_source() {
        let root = temp_tree("ws");
        let ws = root.join("workspace");
        fs::create_dir_all(ws.join("shared")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
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
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
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
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
        assert!(check_agent_cwd_allowed(&ws, &persona).is_ok());
        assert!(check_agent_cwd_allowed(&ws, &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn external_path_covers_checkout_and_subdir() {
        let root = temp_tree("allow");
        let ws = root.join("workspace");
        let src = root.join("src");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&src).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
        fs::write(src.join("lib.rs"), "").unwrap();
        let file = src.join("lib.rs");
        assert!(path_covered_by_roots(&file, std::slice::from_ref(&root)));
        assert!(!path_covered_by_roots(
            &file,
            &[ws.join("shared").join("notes")]
        ));
        assert!(self_repo_git_allowed_by_external_roots(
            &ws,
            std::slice::from_ref(&root)
        ));
        assert!(!self_repo_git_allowed_by_external_roots(
            &ws,
            std::slice::from_ref(&src)
        ));
        let overlapping = external_roots_overlapping_self_repo(&ws, std::slice::from_ref(&src));
        assert_eq!(overlapping.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_names_allowlisted_checkout() {
        let root = temp_tree("prompt");
        let ws = root.join("workspace");
        fs::create_dir_all(&ws).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
        let prompt = self_repo_ban_prompt_section(&ws);
        let rewritten =
            rewrite_self_repo_ban_for_external_paths(prompt, &ws, std::slice::from_ref(&root));
        assert!(rewritten.contains("**Allowed:**"));
        assert!(rewritten.contains(&root.display().to_string()));
        assert!(rewritten.contains("Git against"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_outside_the_app_auto_approves_the_app_directory() {
        let outside =
            std::env::temp_dir().join(format!("meido_outside_app_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&outside).unwrap();
        let app = current_app_directory().expect("this checkout is the app directory");
        assert!(workspace_disjoint_from_app(&outside));
        assert!(!workspace_disjoint_from_app(&app.join("workspace")));
        let mut roots = Vec::new();
        append_auto_approved_app_directory(&outside, &mut roots);
        assert!(path_covered_by_roots(&app, &roots));
        append_auto_approved_app_directory(&outside, &mut roots);
        assert_eq!(roots.len(), 1);
        let _ = fs::remove_dir_all(&outside);
    }
}
