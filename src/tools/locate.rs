//! Context-first file locator: resolve against declared roots before any walk.

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::claude::ToolDefinition;
use crate::memory::{MemoryManager, PersonaMemoryState};

use super::{
    auth_context_from_input, persona_shared_dir, schema_object, shared_global_prefixes,
    workspace_shared_root, Tool, ToolAuthContext, ToolResult,
};

/// Max directory depth for rung-2 walks (relative to each registry root).
pub const LOCATE_MAX_DEPTH: u32 = 4;
/// Soft inode budget for rung-2 walks across all roots.
pub const LOCATE_INODE_BUDGET: usize = 8_000;

const PRUNE_DIR_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "site-packages",
    "target",
    "__pycache__",
    ".venv-vault",
    ".venv-comfy-mcp",
    ".venv-steel",
];

/// A declared location the agent may search or resolve into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    pub label: String,
    pub path: PathBuf,
    /// Higher wins ties; persona-local > shared > workspace.
    pub specificity: u8,
    /// When true, this entry is an exact file pointer (SOP vault_path), not a directory root.
    pub exact_file: bool,
    /// `ORIGIN/…` queries join the remainder onto this root (the directory may not be named ORIGIN).
    pub strips_origin_prefix: bool,
    /// `vault_db/…` queries join the remainder onto this root when `VAULT_VECTOR_DB_PATH` is set.
    pub strips_vault_db_prefix: bool,
}

/// Parse `Repo: /absolute/path` (or trailing absolute path) from Tier 1 stable facts.
pub fn parse_tier1_repo_paths(stable_facts: &[String]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for fact in stable_facts {
        let t = fact.trim().trim_start_matches('-').trim();
        let lower = t.to_ascii_lowercase();
        let abs = if let Some(idx) = lower.find("repo:") {
            let after = t[idx + "repo:".len()..].trim();
            extract_absolute_path(after)
        } else {
            extract_absolute_path(t)
        };
        if let Some(path) = abs {
            let label = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("repo")
                .to_string();
            out.push((format!("Tier 1 repo: {label}"), path));
        }
    }
    out
}

fn extract_absolute_path(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    // Prefer Unix absolute; also accept Windows drive paths when present.
    let start = s.find('/').or_else(|| {
        s.char_indices().find_map(|(i, c)| {
            if c.is_ascii_alphabetic() && s[i..].chars().nth(1) == Some(':') {
                Some(i)
            } else {
                None
            }
        })
    })?;
    let rest = &s[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let candidate = rest[..end].trim_end_matches(['/', '\\']);
    if candidate.is_empty() {
        return None;
    }
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        Some(path)
    } else {
        None
    }
}

fn push_existing_dir(out: &mut Vec<SearchRoot>, label: &str, path: PathBuf, specificity: u8) {
    if path.is_dir() {
        out.push(SearchRoot {
            label: label.to_string(),
            path,
            specificity,
            exact_file: false,
            strips_origin_prefix: false,
            strips_vault_db_prefix: false,
        });
    }
}

fn push_origin_vault(out: &mut Vec<SearchRoot>, path: PathBuf) {
    if path.is_file() {
        return;
    }
    out.push(SearchRoot {
        label: "ORIGIN vault".to_string(),
        path,
        specificity: 80,
        exact_file: false,
        strips_origin_prefix: true,
        strips_vault_db_prefix: false,
    });
}

fn push_existing_file(out: &mut Vec<SearchRoot>, label: &str, path: PathBuf, specificity: u8) {
    if path.is_file() {
        out.push(SearchRoot {
            label: label.to_string(),
            path,
            specificity,
            exact_file: true,
            strips_origin_prefix: false,
            strips_vault_db_prefix: false,
        });
    }
}

/// Build the ordered root registry from workspace layout + persona memory.
///
/// The ORIGIN root is `{workspace}/shared/ORIGIN`. Use [`build_registry_with_origin`]
/// when `VAULT_ORIGIN_VAULT_PATH` points somewhere else.
///
/// Nonexistent roots are dropped so the candidate list is always live. The
/// configured ORIGIN vault is kept even when the directory is not created yet.
pub fn build_registry(
    workspace_root: &Path,
    auth: Option<&ToolAuthContext>,
    memory: Option<&PersonaMemoryState>,
) -> Vec<SearchRoot> {
    build_registry_with_origin(
        workspace_root,
        auth,
        memory,
        &workspace_shared_root(workspace_root).join("ORIGIN"),
    )
}

/// Same as [`build_registry`], with an explicit ORIGIN vault root.
pub fn build_registry_with_origin(
    workspace_root: &Path,
    auth: Option<&ToolAuthContext>,
    memory: Option<&PersonaMemoryState>,
    origin_vault: &Path,
) -> Vec<SearchRoot> {
    let configured_vector_db = auth.and_then(|a| a.vector_db_root.clone());
    let mut out = Vec::new();
    let shared = workspace_shared_root(workspace_root);

    if let Some(auth) = auth.filter(|a| a.caller_chat_id > 0 && a.caller_persona_id > 0) {
        let persona =
            persona_shared_dir(workspace_root, auth.caller_chat_id, auth.caller_persona_id);
        push_existing_dir(&mut out, "persona cwd", persona.clone(), 100);
        let persona_origin = persona.join("ORIGIN");
        if persona_origin.is_dir() {
            out.push(SearchRoot {
                label: "persona ORIGIN".into(),
                path: persona_origin,
                specificity: 95,
                exact_file: false,
                strips_origin_prefix: true,
                strips_vault_db_prefix: false,
            });
        }
        for prefix in shared_global_prefixes() {
            if prefix == "ORIGIN" || (prefix == "vault_db" && configured_vector_db.is_some()) {
                continue;
            }
            push_existing_dir(
                &mut out,
                &format!("persona {prefix}"),
                persona.join(prefix),
                90,
            );
        }
    }

    push_origin_vault(&mut out, origin_vault.to_path_buf());
    if let Some(path) = configured_vector_db {
        if !path.is_file() {
            out.push(SearchRoot {
                label: "vector db".into(),
                path,
                specificity: 75,
                exact_file: false,
                strips_origin_prefix: false,
                strips_vault_db_prefix: true,
            });
        }
    }
    if let Some(auth) = auth {
        for path in &auth.external_roots {
            if path.as_os_str().is_empty() {
                continue;
            }
            out.push(SearchRoot {
                label: "external path".into(),
                path: path.clone(),
                specificity: 70,
                exact_file: false,
                strips_origin_prefix: false,
                strips_vault_db_prefix: false,
            });
        }
    }
    for prefix in shared_global_prefixes() {
        if prefix == "ORIGIN"
            || (prefix == "vault_db" && out.iter().any(|r| r.strips_vault_db_prefix))
        {
            continue;
        }
        push_existing_dir(
            &mut out,
            &format!("shared {prefix}"),
            shared.join(prefix),
            75,
        );
    }
    push_existing_dir(
        &mut out,
        "workspace skills",
        workspace_root.join("skills"),
        70,
    );
    push_existing_dir(&mut out, "shared skills", shared.join("skills"), 70);
    push_existing_dir(
        &mut out,
        "workspace runtime",
        workspace_root.join("runtime"),
        65,
    );

    if let Some(mem) = memory {
        for (label, path) in parse_tier1_repo_paths(&mem.tier1.stable_facts) {
            push_existing_dir(&mut out, &label, path, 85);
        }
        for sop in &mem.tier2.sops {
            let vp = sop.vault_path.trim();
            if !vp.starts_with("ORIGIN/") {
                continue;
            }
            let rest = &vp["ORIGIN/".len()..];
            // Prefer persona ORIGIN file when present, else shared.
            if let Some(auth) = auth.filter(|a| a.caller_chat_id > 0 && a.caller_persona_id > 0) {
                let persona_origin =
                    persona_shared_dir(workspace_root, auth.caller_chat_id, auth.caller_persona_id)
                        .join("ORIGIN")
                        .join(rest);
                push_existing_file(&mut out, &format!("SOP {}", sop.id), persona_origin, 110);
            }
            if let Some(shared_sop) = crate::tools::join_within(origin_vault, Path::new(rest)) {
                push_existing_file(
                    &mut out,
                    &format!("SOP {} (shared)", sop.id),
                    shared_sop,
                    105,
                );
            }
        }
    }

    // Deduplicate by canonical path, keep highest specificity.
    out.sort_by(|a, b| {
        b.specificity
            .cmp(&a.specificity)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut seen = HashSet::new();
    out.retain(|r| {
        let key = std::fs::canonicalize(&r.path)
            .unwrap_or_else(|_| r.path.clone())
            .to_string_lossy()
            .to_string();
        seen.insert(key)
    });
    out
}

/// Directory roots only (for path allowlists and bounded walks).
pub fn directory_roots(registry: &[SearchRoot]) -> Vec<&SearchRoot> {
    registry.iter().filter(|r| !r.exact_file).collect()
}

/// Absolute paths of directory roots (for bash_safety scoping).
pub fn allowed_search_root_paths(registry: &[SearchRoot]) -> Vec<PathBuf> {
    directory_roots(registry)
        .into_iter()
        .map(|r| r.path.clone())
        .collect()
}

/// Resolve a relative `ORIGIN/...` (or other shared-global prefix) preferring persona-local.
pub fn resolve_prefixed_path(
    workspace_root: &Path,
    tool_working_dir: &Path,
    normalized: &str,
) -> PathBuf {
    resolve_prefixed_path_with_origin(
        workspace_root,
        tool_working_dir,
        normalized,
        &workspace_shared_root(workspace_root).join("ORIGIN"),
        None,
    )
}

/// Same as [`resolve_prefixed_path`], with an explicit ORIGIN vault root.
pub fn resolve_prefixed_path_with_origin(
    workspace_root: &Path,
    tool_working_dir: &Path,
    normalized: &str,
    origin_vault: &Path,
    vector_db: Option<&Path>,
) -> PathBuf {
    let shared_root = workspace_shared_root(workspace_root);
    let first = Path::new(normalized)
        .components()
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .unwrap_or_default();

    if !shared_global_prefixes().contains(&first) {
        return shared_root.join(normalized);
    }

    let shared_path = if first == "ORIGIN" {
        let rest = Path::new(normalized)
            .strip_prefix("ORIGIN")
            .unwrap_or(Path::new(normalized));
        match crate::tools::join_within(origin_vault, rest) {
            Some(path) => path,
            None => return PathBuf::from(normalized),
        }
    } else if first == "vault_db" {
        if let Some(root) = vector_db {
            let rest = Path::new(normalized)
                .strip_prefix("vault_db")
                .unwrap_or(Path::new(normalized));
            return match crate::tools::join_within(root, rest) {
                Some(path) => path,
                None => PathBuf::from(normalized),
            };
        }
        shared_root.join(normalized)
    } else {
        shared_root.join(normalized)
    };
    let persona_prefix = tool_working_dir.join(first);
    if is_persona_tool_cwd(workspace_root, tool_working_dir) && persona_prefix.is_dir() {
        let persona_path = tool_working_dir.join(normalized);
        if persona_path.exists() || !shared_path.exists() {
            return persona_path;
        }
    }
    shared_path
}

fn is_persona_tool_cwd(workspace_root: &Path, tool_working_dir: &Path) -> bool {
    let personas = workspace_shared_root(workspace_root).join("personas");
    crate::tools::path_under_root(tool_working_dir, &personas)
}

/// Format registry for system prompt injection.
pub fn format_registry_for_prompt(registry: &[SearchRoot]) -> String {
    let mut lines = Vec::new();
    lines.push("## Declared file roots (locate first)".to_string());
    lines.push(
        "Use `locate_file` (or `read_file` on a known `ORIGIN/…` / Tier-1 `Repo:` path) before any \
         filesystem search. On a miss, **ask the user** — do not run `find`/`grep -r` over large trees."
            .to_string(),
    );
    for root in registry.iter().filter(|r| !r.exact_file).take(16) {
        lines.push(format!("- **{}:** `{}`", root.label, root.path.display()));
    }
    let sop_ptrs: Vec<_> = registry.iter().filter(|r| r.exact_file).take(8).collect();
    if !sop_ptrs.is_empty() {
        lines.push("### Exact SOP pointers".to_string());
        for p in sop_ptrs {
            lines.push(format!("- **{}:** `{}`", p.label, p.path.display()));
        }
    }
    lines.join("\n")
}

#[derive(Debug, Clone)]
pub struct LocateCandidate {
    pub path: PathBuf,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct LocateResult {
    pub found: bool,
    pub path: Option<PathBuf>,
    pub rung: u8,
    pub roots_tried: Vec<String>,
    pub truncated: bool,
    pub message: String,
    /// Partial filename matches. Empty when `found` is true. These are not the file.
    pub candidates: Vec<LocateCandidate>,
}

/// Run the four-rung resolution ladder for a query path or filename.
pub fn locate_in_registry(query: &str, registry: &[SearchRoot]) -> LocateResult {
    let query = query.trim().trim_start_matches("./");
    let mut roots_tried = Vec::new();

    if query.is_empty() {
        return LocateResult {
            found: false,
            path: None,
            rung: 3,
            roots_tried,
            truncated: false,
            message: "Empty query. Provide a relative path (e.g. ORIGIN/…) or a filename.".into(),
            candidates: Vec::new(),
        };
    }

    // Rung 0 — exact declared pointer (SOP paths / exact files).
    for root in registry.iter().filter(|r| r.exact_file) {
        roots_tried.push(format!("{} ({})", root.label, root.path.display()));
        let matches_query = root.path.ends_with(query)
            || root
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(query))
            || path_suffix_eq(&root.path, query);
        if matches_query && root.path.is_file() {
            return LocateResult {
                found: true,
                path: Some(root.path.clone()),
                rung: 0,
                roots_tried,
                truncated: false,
                message: format!("Found via declared pointer ({})", root.label),
                candidates: Vec::new(),
            };
        }
    }

    // Rung 1 — prefix join under each directory root (persona ORIGIN before shared).
    let dirs = directory_roots(registry);
    for root in &dirs {
        roots_tried.push(format!("{} ({})", root.label, root.path.display()));
        let candidate = join_query_under_root(root, query);
        if candidate.is_file() || candidate.is_dir() {
            return LocateResult {
                found: true,
                path: Some(candidate),
                rung: 1,
                roots_tried,
                truncated: false,
                message: format!("Found under {}", root.label),
                candidates: Vec::new(),
            };
        }
    }

    // Rung 2 — bounded walk for basename / glob-ish name inside registry dirs only.
    let needle = Path::new(query)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(query);
    let mut truncated = false;
    let mut visited = 0usize;
    let mut candidates = Vec::new();
    for root in &dirs {
        match walk_for_name(root.path.as_path(), needle, LOCATE_MAX_DEPTH, &mut visited) {
            WalkOutcome::Hit(path) => {
                return LocateResult {
                    found: true,
                    path: Some(path),
                    rung: 2,
                    roots_tried,
                    truncated,
                    message: format!("Found via bounded walk under {}", root.label),
                    candidates: Vec::new(),
                };
            }
            WalkOutcome::None {
                truncated: walk_truncated,
                candidates: found,
            } => {
                truncated = truncated || walk_truncated;
                candidates.extend(found);
            }
        }
        if visited >= LOCATE_INODE_BUDGET {
            truncated = true;
            break;
        }
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
    candidates.truncate(5);

    // Rung 3 — ask, do not widen. Partial names are hints, not a found file.
    let tried = roots_tried.join("; ");
    let candidate_note = if candidates.is_empty() {
        String::new()
    } else {
        " Partial filename candidates are included; they are not an exact match.".into()
    };
    LocateResult {
        found: false,
        path: None,
        rung: 3,
        roots_tried,
        truncated,
        message: format!(
            "Not found in declared roots (rung 3). Tried: {tried}. \
             Ask the user for the correct path; do not run find/grep -r over large trees.{}{}",
            if truncated {
                " (bounded walk hit inode budget — results may be incomplete within roots.)"
            } else {
                ""
            },
            candidate_note
        ),
        candidates,
    }
}

fn path_suffix_eq(path: &Path, query: &str) -> bool {
    let q = Path::new(query);
    path.ends_with(q)
}

fn join_query_under_root(root: &SearchRoot, query: &str) -> PathBuf {
    let q = query.trim_start_matches('/');
    // ORIGIN/foo under a root already named ORIGIN → join foo only.
    if let Some(rest) = q.strip_prefix("ORIGIN/") {
        if root.strips_origin_prefix
            || root
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "ORIGIN")
        {
            return crate::tools::join_within(&root.path, Path::new(rest))
                .unwrap_or_else(|| root.path.clone());
        }
    }
    if let Some(rest) = q.strip_prefix("vault_db/") {
        if root.strips_vault_db_prefix
            || root
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "vault_db")
        {
            return crate::tools::join_within(&root.path, Path::new(rest))
                .unwrap_or_else(|| root.path.clone());
        }
    }
    // Bare filename under root.
    if !q.contains('/') {
        return root.path.join(q);
    }
    root.path.join(q)
}

enum WalkOutcome {
    Hit(PathBuf),
    None {
        truncated: bool,
        candidates: Vec<LocateCandidate>,
    },
}

fn partial_score(name: &str, needle: &str) -> Option<f32> {
    if needle.chars().count() < 3 {
        return None;
    }
    if name.eq_ignore_ascii_case(needle) {
        return None;
    }
    if name.starts_with(needle) {
        Some(0.9)
    } else if name.contains(needle) {
        Some(0.5)
    } else {
        None
    }
}

fn walk_for_name(root: &Path, needle: &str, max_depth: u32, visited: &mut usize) -> WalkOutcome {
    let needle_lower = needle.to_ascii_lowercase();
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    let mut truncated = false;
    let mut candidates = Vec::new();

    while let Some((dir, depth)) = stack.pop() {
        if *visited >= LOCATE_INODE_BUDGET {
            return WalkOutcome::None {
                truncated: true,
                candidates,
            };
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            *visited += 1;
            if *visited >= LOCATE_INODE_BUDGET {
                truncated = true;
                break;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "." || name_str == ".." {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if PRUNE_DIR_NAMES
                    .iter()
                    .any(|p| name_str.eq_ignore_ascii_case(p))
                    || name_str.starts_with(".venv")
                {
                    continue;
                }
                if depth < max_depth {
                    stack.push((entry.path(), depth + 1));
                }
            } else if ft.is_file() {
                let lowered = name_str.to_ascii_lowercase();
                if lowered == needle_lower {
                    return WalkOutcome::Hit(entry.path());
                }
                if let Some(score) = partial_score(&lowered, &needle_lower) {
                    candidates.push(LocateCandidate {
                        path: entry.path(),
                        score,
                    });
                    if candidates.len() > 32 {
                        candidates.sort_by(|left, right| {
                            right
                                .score
                                .partial_cmp(&left.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        candidates.truncate(8);
                    }
                }
            }
        }
    }
    WalkOutcome::None {
        truncated,
        candidates,
    }
}

/// Load persona memory when auth is present (best-effort).
pub fn load_memory_for_auth(
    workspace_root: &Path,
    runtime_data_dir: &Path,
    auth: Option<&ToolAuthContext>,
) -> Option<PersonaMemoryState> {
    let auth = auth.filter(|a| a.caller_chat_id > 0 && a.caller_persona_id > 0)?;
    let mm = MemoryManager::new(
        runtime_data_dir.to_string_lossy().as_ref(),
        workspace_root.to_string_lossy().as_ref(),
    );
    mm.read_persona_memory_state(auth.caller_chat_id, auth.caller_persona_id)
}

pub struct LocateFileTool {
    workspace_root: PathBuf,
    runtime_data_dir: PathBuf,
    origin_vault: PathBuf,
}

impl LocateFileTool {
    pub fn new(workspace_root: &str, runtime_data_dir: &str) -> Self {
        let workspace_root = PathBuf::from(workspace_root);
        let origin_vault = workspace_root.join("shared").join("ORIGIN");
        Self {
            workspace_root,
            runtime_data_dir: PathBuf::from(runtime_data_dir),
            origin_vault,
        }
    }

    pub fn with_origin_vault(mut self, origin_vault: PathBuf) -> Self {
        self.origin_vault = origin_vault;
        self
    }
}

#[async_trait]
impl Tool for LocateFileTool {
    fn name(&self) -> &str {
        "locate_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "locate_file",
            "Locate a file using declared persona/workspace roots (persona ORIGIN, the configured \
             ORIGIN vault, skills, runtime, Tier-1 Repo paths, SOP pointers) before any wide \
             filesystem scan. On miss, returns roots tried and instructs you to ask the user — \
             do not escalate to find.",
            schema_object(
                json!({
                    "query": {
                        "type": "string",
                        "description": "Relative path (e.g. ORIGIN/Operations/SOPs/Foo.md) or a basename to find under declared roots"
                    }
                }),
                &["query"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return ToolResult::error("Missing 'query' parameter".into()),
        };
        let auth = auth_context_from_input(&input);
        let memory =
            load_memory_for_auth(&self.workspace_root, &self.runtime_data_dir, auth.as_ref());
        let registry = build_registry_with_origin(
            &self.workspace_root,
            auth.as_ref(),
            memory.as_ref(),
            &self.origin_vault,
        );
        info!("locate_file query={} roots={}", query, registry.len());
        let result = locate_in_registry(query, &registry);
        let body = json!({
            "found": result.found,
            "path": result.path.as_ref().map(|p| p.display().to_string()),
            "rung": result.rung,
            "roots_tried": result.roots_tried,
            "truncated": result.truncated,
            "message": result.message,
            "candidates": result.candidates.iter().map(|candidate| json!({
                "path": candidate.path.display().to_string(),
                "score": candidate.score,
            })).collect::<Vec<_>>(),
        });
        if result.found {
            ToolResult::success(body.to_string())
        } else {
            ToolResult::error(body.to_string()).with_error_type("not_found")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{PersonaMemoryState, SopPointer, Tier1Memory};

    fn auth(chat: i64, persona: i64) -> ToolAuthContext {
        ToolAuthContext {
            run_id: None,
            caller_channel: "web".into(),
            caller_chat_id: chat,
            caller_persona_id: persona,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        }
    }

    #[test]
    fn parse_tier1_repo_extracts_absolute_path() {
        let facts = vec![
            "- Sourdough and Bread website Repo: /home/ken/big_storage/projects/sourdough"
                .to_string(),
        ];
        let repos = parse_tier1_repo_paths(&facts);
        assert_eq!(repos.len(), 1);
        assert_eq!(
            repos[0].1,
            PathBuf::from("/home/ken/big_storage/projects/sourdough")
        );
    }

    #[test]
    fn persona_origin_wins_over_shared_in_resolve() {
        let root =
            std::env::temp_dir().join(format!("finally_locate_origin_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        let shared_origin = workspace_shared_root(&root).join("ORIGIN").join("a.md");
        let persona_origin = persona.join("ORIGIN").join("a.md");
        std::fs::create_dir_all(persona_origin.parent().unwrap()).unwrap();
        std::fs::create_dir_all(shared_origin.parent().unwrap()).unwrap();
        std::fs::write(&persona_origin, "persona").unwrap();
        std::fs::write(&shared_origin, "shared").unwrap();

        let resolved = resolve_prefixed_path(&root, &persona, "ORIGIN/a.md");
        assert_eq!(resolved, persona_origin);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_rung0_hits_sop_pointer() {
        let root =
            std::env::temp_dir().join(format!("finally_locate_sop_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        let sop = persona
            .join("ORIGIN")
            .join("Operations")
            .join("SOPs")
            .join("Sourdough-Journal-Post-Pipeline.md");
        std::fs::create_dir_all(sop.parent().unwrap()).unwrap();
        std::fs::write(&sop, "# sop").unwrap();

        let mut mem = PersonaMemoryState::default();
        mem.tier2.sops.push(SopPointer {
            id: "sourdough-journal".into(),
            vault_path: "ORIGIN/Operations/SOPs/Sourdough-Journal-Post-Pipeline.md".into(),
            summary: "journal".into(),
        });
        let a = auth(10, 2);
        let registry = build_registry(&root, Some(&a), Some(&mem));
        let result = locate_in_registry(
            "ORIGIN/Operations/SOPs/Sourdough-Journal-Post-Pipeline.md",
            &registry,
        );
        assert!(result.found);
        assert_eq!(result.rung, 0);
        assert_eq!(result.path.as_ref().unwrap(), &sop);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_rung3_lists_roots_and_asks() {
        let root =
            std::env::temp_dir().join(format!("finally_locate_miss_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        std::fs::create_dir_all(persona.join("ORIGIN")).unwrap();
        std::fs::create_dir_all(workspace_shared_root(&root).join("ORIGIN")).unwrap();
        let a = auth(10, 2);
        let registry = build_registry(&root, Some(&a), None);
        let result = locate_in_registry("ORIGIN/no-such-file-zzzz.md", &registry);
        assert!(!result.found);
        assert_eq!(result.rung, 3);
        assert!(result.message.contains("Ask the user"));
        assert!(!result.roots_tried.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_includes_tier1_repo_when_present() {
        let root =
            std::env::temp_dir().join(format!("finally_locate_repo_{}", uuid::Uuid::new_v4()));
        let repo = root.join("external_repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(persona_shared_dir(&root, 1, 1)).unwrap();
        let mem = PersonaMemoryState {
            tier1: Tier1Memory {
                stable_facts: vec![format!("Repo: {}", repo.display())],
                workflow_principles: vec![],
            },
            ..Default::default()
        };
        let a = auth(1, 1);
        let registry = build_registry(&root, Some(&a), Some(&mem));
        assert!(registry.iter().any(|r| r.path == repo && !r.exact_file));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_refuses_to_leave_registry_root() {
        // Bounded walk only searches under given root — outside files are invisible.
        let root =
            std::env::temp_dir().join(format!("finally_locate_walk_{}", uuid::Uuid::new_v4()));
        let inside = root.join("inside");
        let outside = root.join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        let mut visited = 0;
        let outcome = walk_for_name(&inside, "secret.txt", 4, &mut visited);
        assert!(matches!(
            outcome,
            WalkOutcome::None {
                truncated: false,
                ..
            }
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn partial_filename_is_ranked_and_not_treated_as_found() {
        let root =
            std::env::temp_dir().join(format!("finally_locate_partial_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("AlphaPlan.md"), "x").unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();
        let registry = vec![SearchRoot {
            label: "temp".into(),
            path: root.clone(),
            specificity: 1,
            exact_file: false,
            strips_origin_prefix: false,
            strips_vault_db_prefix: false,
        }];
        let result = locate_in_registry("alpha", &registry);
        assert!(!result.found);
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.path.ends_with("AlphaPlan.md")));
        assert!(result
            .candidates
            .iter()
            .all(|candidate| candidate.score < 1.0));
        let exact = locate_in_registry("AlphaPlan.md", &registry);
        assert!(exact.found);
        assert!(exact.candidates.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
