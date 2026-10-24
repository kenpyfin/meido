pub mod activate_skill;
pub mod agent_history;
pub mod apply_search_replace;
pub mod artifacts;
pub mod bash;
pub mod bash_safety;
pub mod bulletin;
pub mod collab_tools;
pub mod command_runner;
pub mod cursor_agent;
pub mod delegate_local_subjob;
pub mod edit_file;
pub mod export_chat;
pub mod glob;
pub mod grep;
pub mod locate;
pub mod mcp;
pub mod memory;
pub mod memory_state;
pub mod path_guard;
pub mod persona_todo;
pub mod read_file;
pub mod read_repo_map;
pub mod recall;
pub mod register_hook;
pub mod register_tracked_job;
pub mod run_skill_script;
pub mod schedule;
pub mod search_history;
pub mod search_vault;
pub mod send_message;
pub mod social_feed;
pub mod spawn_background_command;
pub mod symbol_edit;
pub mod sync_skills;
pub mod tiered_memory;
pub mod tool_cancel;
pub mod vault_add;
pub mod vault_secret;
pub mod web_fetch;
pub mod web_html;
pub mod web_search;
pub mod write_file;

use std::sync::Arc;
use std::{path::Path, path::PathBuf, time::Instant};

use async_trait::async_trait;
use serde_json::json;
use teloxide::prelude::*;

use crate::claude::ToolDefinition;
use crate::config::Config;
use crate::db::Database;
use crate::safety_redaction::EnvSecretRedactor;

pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub status_code: Option<i32>,
    pub bytes: usize,
    pub duration_ms: Option<u128>,
    pub error_type: Option<String>,
}

impl ToolResult {
    pub fn success(content: String) -> Self {
        let bytes = content.len();
        ToolResult {
            content,
            is_error: false,
            status_code: Some(0),
            bytes,
            duration_ms: None,
            error_type: None,
        }
    }

    pub fn error(content: String) -> Self {
        let bytes = content.len();
        ToolResult {
            content,
            is_error: true,
            status_code: Some(1),
            bytes,
            duration_ms: None,
            error_type: Some("tool_error".to_string()),
        }
    }

    pub fn with_status_code(mut self, status_code: i32) -> Self {
        self.status_code = Some(status_code);
        self
    }

    pub fn with_error_type(mut self, error_type: impl Into<String>) -> Self {
        self.error_type = Some(error_type.into());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRisk {
    Low,
    Medium,
    High,
}

impl ToolRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolRisk::Low => "low",
            ToolRisk::Medium => "medium",
            ToolRisk::High => "high",
        }
    }
}

pub fn tool_risk(name: &str) -> ToolRisk {
    match name {
        "register_tracked_job" => ToolRisk::Low,
        "bash" | "spawn_background_command" => ToolRisk::High,
        "cursor_agent"
        | "write_file"
        | "edit_file"
        | "apply_search_replace"
        | "symbol_edit"
        | "write_memory"
        | "write_tiered_memory"
        | "write_memory_state"
        | "patch_memory_state"
        | "update_bulletin_focus"
        | "add_vault_item"
        | "sync_skills"
        | "vault_secret"
        | "register_hook"
        | "schedule_task"
        | "update_scheduled_task"
        | "pause_scheduled_task"
        | "resume_scheduled_task"
        | "cancel_scheduled_task" => ToolRisk::Medium,
        _ => ToolRisk::Low,
    }
}

#[derive(Clone, Debug)]
pub struct ToolAuthContext {
    /// Trusted identity of the producing agent turn.
    pub run_id: Option<String>,
    pub caller_channel: String,
    pub caller_chat_id: i64,
    pub caller_persona_id: i64,
    pub control_chat_ids: Vec<i64>,
    /// True when the agent run was started by the scheduler (cron / one-shot task).
    pub is_scheduled_task: bool,
    /// Focused web session for this run (`None` = main chat).
    pub session_id: Option<String>,
    /// `VAULT_VECTOR_DB_PATH` when set. Absent means `{workspace}/shared/vault_db`.
    pub vector_db_root: Option<PathBuf>,
    /// Absolute paths this persona may read and write outside the workspace.
    /// Empty means no extra paths. The model cannot set this; `inject_auth_context` overwrites it.
    pub external_roots: Vec<PathBuf>,
}

/// Trim and drop empty session ids so they store as main chat (`NULL`).
pub fn normalize_session_id(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl ToolAuthContext {
    pub fn is_control_chat(&self) -> bool {
        self.control_chat_ids.contains(&self.caller_chat_id)
    }

    pub fn can_access_chat(&self, target_chat_id: i64) -> bool {
        self.is_control_chat() || self.caller_chat_id == target_chat_id
    }

    /// True if the caller can access the given (chat_id, persona_id) for memory/tiered operations.
    pub fn can_access_chat_persona(&self, target_chat_id: i64, target_persona_id: i64) -> bool {
        self.is_control_chat()
            || (self.caller_chat_id == target_chat_id
                && self.caller_persona_id == target_persona_id)
    }

    pub fn chat_session_id(&self) -> Option<String> {
        normalize_session_id(self.session_id.as_deref())
    }
}

const AUTH_CONTEXT_KEY: &str = "__meido_auth";

/// When the tool targets the same chat as the agent run, prefer the run's persona (`caller_persona_id`)
/// over DB active/default (e.g. `get_or_create_default_persona`).
pub fn default_persona_id_for_chat(input: &serde_json::Value, target_chat_id: i64) -> Option<i64> {
    let auth = auth_context_from_input(input)?;
    if auth.caller_chat_id == target_chat_id && auth.caller_persona_id > 0 {
        Some(auth.caller_persona_id)
    } else {
        None
    }
}

pub fn auth_context_from_input(input: &serde_json::Value) -> Option<ToolAuthContext> {
    let ctx = input.get(AUTH_CONTEXT_KEY)?;
    let caller_channel = ctx
        .get("caller_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("telegram")
        .to_string();
    let caller_chat_id = ctx.get("caller_chat_id")?.as_i64()?;
    let caller_persona_id = ctx
        .get("caller_persona_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let control_chat_ids = ctx
        .get("control_chat_ids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default();
    let is_scheduled_task = ctx
        .get("is_scheduled_task")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let session_id = normalize_session_id(ctx.get("session_id").and_then(|v| v.as_str()));
    Some(ToolAuthContext {
        run_id: ctx
            .get("run_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        caller_channel,
        caller_chat_id,
        caller_persona_id,
        control_chat_ids,
        is_scheduled_task,
        session_id,
        vector_db_root: ctx
            .get("vector_db_root")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        external_roots: ctx
            .get("external_roots")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.as_str().and_then(|path| {
                            crate::persona_external_paths::normalize_absolute(path).ok()
                        })
                    })
                    .take(32)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

pub fn authorize_chat_access(input: &serde_json::Value, target_chat_id: i64) -> Result<(), String> {
    if let Some(auth) = auth_context_from_input(input) {
        if !auth.can_access_chat(target_chat_id) {
            return Err(format!(
                "Permission denied: chat {} cannot operate on chat {}",
                auth.caller_chat_id, target_chat_id
            ));
        }
    }
    Ok(())
}

/// Authorize access to (chat_id, persona_id) for tiered memory. Fails if auth is missing or cannot access that pair.
pub fn authorize_chat_persona_access(
    input: &serde_json::Value,
    target_chat_id: i64,
    target_persona_id: i64,
) -> Result<(), String> {
    let auth = auth_context_from_input(input)
        .ok_or_else(|| "Permission denied: no auth context".to_string())?;
    if !auth.can_access_chat_persona(target_chat_id, target_persona_id) {
        return Err(format!(
            "Permission denied: cannot access memory for chat {} persona {}",
            target_chat_id, target_persona_id
        ));
    }
    Ok(())
}

pub fn inject_auth_context(input: serde_json::Value, auth: &ToolAuthContext) -> serde_json::Value {
    let mut obj = match input {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    obj.insert(
        AUTH_CONTEXT_KEY.to_string(),
        json!({
            "caller_channel": auth.caller_channel,
            "caller_chat_id": auth.caller_chat_id,
            "caller_persona_id": auth.caller_persona_id,
            "control_chat_ids": auth.control_chat_ids,
            "is_scheduled_task": auth.is_scheduled_task,
            "session_id": auth.session_id,
            "run_id": auth.run_id,
            "vector_db_root": auth.vector_db_root.as_ref().map(|p| p.to_string_lossy().to_string()),
            "external_roots": auth.external_roots.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        }),
    );
    serde_json::Value::Object(obj)
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, input: serde_json::Value) -> ToolResult;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    env_redactor: Arc<EnvSecretRedactor>,
    runtime_data_dir: String,
    recall: Arc<crate::recall::RecallService>,
}

fn hide_recall_alias(name: &str) -> bool {
    matches!(name, "search_chat_history" | "search_vault")
}

/// Path to the mistaken nested copy agents sometimes create under tool cwd (`shared/workspace/`).
pub fn shadow_workspace_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("shared").join("workspace")
}

/// Returns true if `path` lies under `{workspace_root}/shared/workspace/`.
pub fn is_under_shadow_workspace(workspace_root: &Path, path: &Path) -> bool {
    let shadow = shadow_workspace_path(workspace_root);
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let shadow_resolved = std::fs::canonicalize(&shadow).unwrap_or(shadow);
    resolved.starts_with(&shadow_resolved)
}

/// Reject writes into the shadow workspace tree (see `shadow_workspace_path`).
pub fn check_shadow_workspace_write(workspace_root: &Path, resolved: &Path) -> Result<(), String> {
    if is_under_shadow_workspace(workspace_root, resolved) {
        Err(
            "Path is under shadow workspace `shared/workspace/`; use paths relative to tool cwd \
             (`shared/`) without a `workspace/` prefix."
                .to_string(),
        )
    } else {
        Ok(())
    }
}

pub fn shared_global_prefixes() -> [&'static str; 3] {
    ["ORIGIN", "vault_db", ".venv-vault"]
}

pub fn workspace_shared_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join("shared")
}

pub fn persona_shared_dir(workspace_root: &Path, chat_id: i64, persona_id: i64) -> PathBuf {
    workspace_shared_root(workspace_root)
        .join("personas")
        .join(chat_id.to_string())
        .join(persona_id.to_string())
}

pub fn ensure_persona_shared_dir(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
) -> Result<PathBuf, String> {
    let dir = persona_shared_dir(workspace_root, chat_id, persona_id);
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Failed to create persona shared directory '{}': {e}",
            dir.display()
        )
    })?;
    Ok(dir)
}

fn has_persona_scope(auth: Option<&ToolAuthContext>) -> bool {
    auth.is_some_and(|a| a.caller_chat_id > 0 && a.caller_persona_id > 0)
}

/// Absolutize a path for prefix checks without requiring the path to exist.
///
/// `canonicalize` fails for missing files. On macOS `/tmp` and `/var` are
/// symlinks, and on Windows the canonical form adds a `\\?\` prefix, so a
/// missing file and its existing parent would disagree and `starts_with` would
/// fail closed. A leading `/` stays a root even where `Path::is_absolute` is
/// false (Windows).
pub fn absolutize_for_compare(path: &Path) -> PathBuf {
    if let Some(canonical) = canonicalize_existing_ancestor(path) {
        return strip_verbatim_path_prefix(canonical);
    }
    if path_text_is_rooted(path) {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn path_text_is_rooted(path: &Path) -> bool {
    path.is_absolute()
        || path.starts_with("/")
        || path.components().next().is_some_and(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
}

fn strip_verbatim_path_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path;
    };
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    PathBuf::from(rest.to_string())
}

fn canonicalize_existing_ancestor(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Some(canonical);
    }
    let mut tail = Vec::new();
    let mut cursor = path.to_path_buf();
    while let Some(name) = cursor.file_name().map(|name| name.to_os_string()) {
        tail.push(name);
        if !cursor.pop() || cursor.as_os_str().is_empty() {
            break;
        }
        if let Ok(canonical) = std::fs::canonicalize(&cursor) {
            let mut out = canonical;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
    }
    None
}

/// True when `path` is under `root` (both sides absolutized; existence not required).
pub fn path_under_root(path: &Path, root: &Path) -> bool {
    let resolved_path = absolutize_for_compare(path);
    let resolved_root = absolutize_for_compare(root);
    resolved_path.starts_with(&resolved_root)
}

/// Join `rel` onto `root` and refuse a result that leaves `root` via `..`.
pub fn join_within(root: &Path, rel: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = root.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.as_path() == root || !out.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.starts_with(root) {
        Some(out)
    } else {
        None
    }
}

fn is_under(path: &Path, root: &Path) -> bool {
    path_under_root(path, root)
}

/// Strip redundant prefixes when tool cwd is under `.../shared`.
pub fn normalize_tool_relative_path(working_dir: &Path, path: &str) -> String {
    let mut s = path.trim().to_string();
    if s.is_empty() {
        return s;
    }
    while s.starts_with("./") {
        s = s[2..].to_string();
    }

    let cwd_is_shared_tree = working_dir.components().any(|c| {
        matches!(
            c,
            std::path::Component::Normal(name) if name.to_str().is_some_and(|s| s.eq_ignore_ascii_case("shared"))
        )
    });

    if !cwd_is_shared_tree {
        return s;
    }

    for prefix in ["workspace/shared/", "workspace/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("shared/") {
        s = rest.to_string();
    }

    if let Some(parent_name) = working_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        use std::path::Component;
        let rel = Path::new(&s);
        if let Some(Component::Normal(first)) = rel.components().next() {
            if first == parent_name {
                s = rel
                    .components()
                    .skip(1)
                    .collect::<PathBuf>()
                    .to_string_lossy()
                    .into_owned();
            }
        }
    }

    s
}

fn resolves_under_workspace_data_root(_tool_shared_dir: &Path, normalized: &str) -> bool {
    let rel = Path::new(normalized);
    rel.components().next().is_some_and(|c| {
        matches!(
            c,
            std::path::Component::Normal(name)
                if name == "runtime" || name == "skills"
        )
    })
}

pub fn resolve_tool_path(workspace_root: &Path, tool_working_dir: &Path, path: &str) -> PathBuf {
    resolve_tool_path_with_origin(
        workspace_root,
        tool_working_dir,
        path,
        &workspace_shared_root(workspace_root).join("ORIGIN"),
        None,
    )
}

/// Resolve a tool path.
///
/// `ORIGIN/…` lands in `origin_vault` (`VAULT_ORIGIN_VAULT_PATH`).
/// `vault_db/…` lands in `vector_db` when that root is set (`VAULT_VECTOR_DB_PATH`).
/// When `vector_db` is `None`, `vault_db/…` stays `{workspace}/shared/vault_db`.
pub fn resolve_tool_path_with_origin(
    workspace_root: &Path,
    tool_working_dir: &Path,
    path: &str,
    origin_vault: &Path,
    vector_db: Option<&Path>,
) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return candidate;
    }

    let normalized = normalize_tool_relative_path(tool_working_dir, path);
    if normalized.is_empty() {
        return tool_working_dir.join(path);
    }

    let shared_root = workspace_shared_root(workspace_root);

    if let Some(rest) = normalized.strip_prefix("shared/skills/") {
        return shared_root.join("skills").join(rest);
    }
    if let Some(rest) = normalized.strip_prefix("shared/") {
        let first = std::path::Path::new(rest)
            .components()
            .next()
            .and_then(|c| match c {
                std::path::Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .unwrap_or_default();
        if first == "ORIGIN" || (first == "vault_db" && vector_db.is_some()) {
            return locate::resolve_prefixed_path_with_origin(
                workspace_root,
                tool_working_dir,
                rest,
                origin_vault,
                vector_db,
            );
        }
        if shared_global_prefixes().contains(&first) || first == "skills" {
            return shared_root.join(rest);
        }
        return tool_working_dir.join(rest);
    }

    let first = std::path::Path::new(&normalized)
        .components()
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .unwrap_or_default();
    if shared_global_prefixes().contains(&first) {
        return locate::resolve_prefixed_path_with_origin(
            workspace_root,
            tool_working_dir,
            &normalized,
            origin_vault,
            vector_db,
        );
    }

    if resolves_under_workspace_data_root(tool_working_dir, &normalized) {
        return workspace_root.join(&normalized);
    }

    tool_working_dir.join(&normalized)
}

/// Resolve the tool working directory. Always uses the shared workspace (base/shared).
pub fn resolve_tool_working_dir(base_working_dir: &Path) -> PathBuf {
    let resolved = workspace_shared_root(base_working_dir);
    let _ = std::fs::create_dir_all(&resolved);
    resolved
}

pub fn resolve_tool_working_dir_for_auth(
    base_working_dir: &Path,
    auth: Option<&ToolAuthContext>,
) -> PathBuf {
    if has_persona_scope(auth) {
        let auth = auth.expect("checked above");
        if let Ok(dir) = ensure_persona_shared_dir(
            base_working_dir,
            auth.caller_chat_id,
            auth.caller_persona_id,
        ) {
            return dir;
        }
    }
    resolve_tool_working_dir(base_working_dir)
}

pub fn assert_persona_tool_path_allowed(
    workspace_root: &Path,
    resolved_path: &Path,
    auth: Option<&ToolAuthContext>,
    is_write: bool,
) -> Result<(), String> {
    assert_persona_tool_path_allowed_with_origin(
        workspace_root,
        resolved_path,
        auth,
        is_write,
        &workspace_shared_root(workspace_root).join("ORIGIN"),
    )
}

/// Like [`assert_persona_tool_path_allowed`], using `origin_vault` as the ORIGIN root.
pub fn assert_persona_tool_path_allowed_with_origin(
    workspace_root: &Path,
    resolved_path: &Path,
    auth: Option<&ToolAuthContext>,
    is_write: bool,
    origin_vault: &Path,
) -> Result<(), String> {
    let memory = auth.and_then(|a| {
        locate::load_memory_for_auth(workspace_root, &workspace_root.join("runtime"), Some(a))
    });
    assert_persona_tool_path_allowed_with_memory(
        workspace_root,
        resolved_path,
        auth,
        is_write,
        memory.as_ref(),
        origin_vault,
    )
}

/// Like [`assert_persona_tool_path_allowed`], but accepts preloaded persona memory so Tier-1
/// `Repo:` paths and SOP pointers expand the allowlist.
pub fn assert_persona_tool_path_allowed_with_memory(
    workspace_root: &Path,
    resolved_path: &Path,
    auth: Option<&ToolAuthContext>,
    is_write: bool,
    memory: Option<&crate::memory::PersonaMemoryState>,
    origin_vault: &Path,
) -> Result<(), String> {
    if is_write && crate::self_repo::is_self_repo_source_path(workspace_root, resolved_path) {
        let covered = auth.is_some_and(|a| {
            crate::self_repo::path_covered_by_roots(resolved_path, &a.external_roots)
        });
        if !covered {
            return Err(format!(
                "Write blocked: '{}' is inside the bot's own source checkout and is not on \
                 this persona's external paths. Add that directory under Paths outside the \
                 workspace to allow writes there. Persona Tier-1 target repos (`Repo: …`) \
                 stay allowed.",
                resolved_path.display()
            ));
        }
    }
    if !has_persona_scope(auth) {
        return Ok(());
    }
    let auth = auth.expect("checked above");
    let shared_root = workspace_shared_root(workspace_root);

    if is_write
        && (is_under(resolved_path, &shared_root.join("scripts"))
            || is_under(resolved_path, &shared_root.join("parking")))
    {
        return Err(
            "Write blocked: flat shared paths like `shared/scripts/` and `shared/parking/` are \
             deprecated. Use `skills/<skill-name>/` (or `shared/skills/<skill-name>/`) for reusable \
             scripts, or write persona files under `shared/personas/{chat_id}/{persona_id}/`."
                .to_string(),
        );
    }

    // A data workspace that does not overlap the app checkout is editable by every persona.
    if crate::self_repo::workspace_disjoint_from_app(workspace_root)
        && crate::self_repo::is_under_workspace_data(workspace_root, resolved_path)
    {
        return Ok(());
    }

    let registry =
        locate::build_registry_with_origin(workspace_root, Some(auth), memory, origin_vault);
    let mut allowed_labels: Vec<String> = locate::directory_roots(&registry)
        .iter()
        .map(|r| format!("{} (`{}`)", r.label, r.path.display()))
        .collect();

    for root in locate::directory_roots(&registry) {
        if is_under(resolved_path, &root.path) {
            return Ok(());
        }
    }

    // Exact SOP file pointers.
    for root in registry.iter().filter(|r| r.exact_file) {
        if absolutize_for_compare(resolved_path) == absolutize_for_compare(&root.path) {
            return Ok(());
        }
    }

    if auth.is_control_chat() {
        let chat_personas_root = shared_root
            .join("personas")
            .join(auth.caller_chat_id.to_string());
        allowed_labels.push(format!(
            "control-chat personas (`{}`)",
            chat_personas_root.display()
        ));
        if is_under(resolved_path, &chat_personas_root) {
            return Ok(());
        }
    }

    // Path is outside every declared root.
    Err(format!(
        "Permission denied: path '{}' is outside the allowed persona scope. Allowed roots: {}. \
         Use locate_file to resolve declared paths, or ask the user — do not run find over large trees.",
        resolved_path.display(),
        if allowed_labels.is_empty() {
            "persona folder, shared ORIGIN/vault paths, workspace runtime/skills, and shared/skills"
                .to_string()
        } else {
            allowed_labels.join("; ")
        }
    ))
}

/// Auto-detect the vault search command from the search-vault skill.
/// Prefers `workspace/skills/`; falls back to repository `builtin_skills/search-vault/`.
/// Expects a Python venv at `shared/.venv-vault` when present.
fn detect_vault_search_command(config: &Config) -> Option<String> {
    let workspace = config.workspace_root_absolute();
    let ws_script = workspace
        .join("skills")
        .join("search-vault")
        .join("query_vault.py");
    let script = if ws_script.exists() {
        ws_script
    } else {
        crate::builtin_skills::resolve_builtin_skills_dir(config)
            .map(|b| b.join("search-vault").join("query_vault.py"))
            .filter(|p| p.exists())?
    };

    let venv_python = workspace
        .join("shared")
        .join(".venv-vault")
        .join("bin")
        .join("python");
    let python = if venv_python.exists() {
        venv_python.to_string_lossy().to_string()
    } else {
        "python3".to_string()
    };

    Some(format!("{} {} \"{{query}}\"", python, script.display()))
}

impl ToolRegistry {
    pub fn new(
        config: &Config,
        _bot: Bot,
        db: Arc<Database>,
        runtime_toggles: std::sync::Arc<crate::runtime_toggles::RuntimeToggles>,
        env_redactor: Arc<EnvSecretRedactor>,
    ) -> Self {
        let working_dir = PathBuf::from(config.working_dir());
        if let Err(e) = std::fs::create_dir_all(&working_dir) {
            tracing::warn!(
                "Failed to create working_dir '{}': {}",
                working_dir.display(),
                e
            );
        }
        let skills_data_dir = config.skills_data_dir();
        let origin_vault = config.origin_vault_root();
        let recall = Arc::new(crate::recall::RecallService::new(config, db.clone()));
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(
                bash::BashTool::new_with_safety(
                    config.working_dir(),
                    config.safety_execution_mode.clone(),
                    config.safety_risky_categories.clone(),
                    runtime_toggles.clone(),
                    env_redactor.clone(),
                )
                .with_runtime_data_dir(config.runtime_data_dir())
                .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                read_file::ReadFileTool::new(config.working_dir())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                read_repo_map::ReadRepoMapTool::new(config.working_dir())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                write_file::WriteFileTool::new(config.working_dir())
                    .with_redactor(env_redactor.clone())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                edit_file::EditFileTool::new(config.working_dir())
                    .with_redactor(env_redactor.clone())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                apply_search_replace::ApplySearchReplaceTool::new(
                    config.working_dir(),
                    config.allow_fuzzy_search_replace,
                )
                .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                symbol_edit::SymbolEditTool::new(config.working_dir(), config.symbol_edit_enabled)
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                glob::GlobTool::new(config.working_dir()).with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                grep::GrepTool::new(config.working_dir()).with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                locate::LocateFileTool::new(config.working_dir(), &config.runtime_data_dir())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(memory::ReadMemoryTool::new(
                &config.runtime_data_dir(),
                config.working_dir(),
            )),
            Box::new(memory::WriteMemoryTool::new(
                &config.runtime_data_dir(),
                config.working_dir(),
            )),
            Box::new(memory_state::ReadMemoryStateTool::new(
                &config.runtime_data_dir(),
                config.working_dir(),
            )),
            Box::new(memory_state::ValidateMemoryStateTool::new(
                &config.runtime_data_dir(),
                config.working_dir(),
            )),
            Box::new(
                memory_state::WriteMemoryStateTool::new(
                    &config.runtime_data_dir(),
                    config.working_dir(),
                )
                .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(
                memory_state::PatchMemoryStateTool::new(
                    &config.runtime_data_dir(),
                    config.working_dir(),
                )
                .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(web_fetch::WebFetchTool),
            Box::new(web_search::WebSearchTool::new(
                config.tavily_api_key.clone(),
                config.web_search_searxng_url.clone(),
            )),
            Box::new(register_tracked_job::RegisterTrackedJobTool::new(
                db.clone(),
            )),
            Box::new(register_hook::RegisterHookTool::new(db.clone(), config)),
            Box::new(schedule::ScheduleTaskTool::new(
                db.clone(),
                config.timezone.clone(),
            )),
            Box::new(schedule::UpdateScheduledTaskTool::new(
                db.clone(),
                config.timezone.clone(),
            )),
            Box::new(schedule::ListTasksTool::new(db.clone())),
            Box::new(schedule::PauseTaskTool::new(db.clone())),
            Box::new(schedule::ResumeTaskTool::new(db.clone())),
            Box::new(schedule::CancelTaskTool::new(db.clone())),
            Box::new(schedule::GetTaskHistoryTool::new(db.clone())),
            Box::new(
                export_chat::ExportChatTool::new(db.clone(), &config.runtime_data_dir())
                    .with_redactor(env_redactor.clone()),
            ),
            Box::new(vault_secret::VaultSecretTool::new(
                &config.runtime_data_dir(),
                config.skills_data_dir_absolute(),
                env_redactor.clone(),
            )),
            Box::new(cursor_agent::CursorAgentTool::new(config, db.clone())),
            Box::new(cursor_agent::ListCursorAgentRunsTool::new(db.clone())),
            Box::new(cursor_agent::CursorAgentSendTool::new(config)),
            Box::new(cursor_agent::BuildSkillTool::new(config, db.clone())),
            Box::new(activate_skill::ActivateSkillTool::new_with_dirs_and_db(
                config.skill_discovery_dirs(),
                db.clone(),
            )),
            Box::new(
                run_skill_script::RunSkillScriptTool::new_with_dirs_and_db(
                    config.skill_discovery_dirs(),
                    db.clone(),
                    runtime_toggles.clone(),
                )
                .with_runtime_data_dir(config.runtime_data_dir()),
            ),
            Box::new(sync_skills::SyncSkillsTool::new(&skills_data_dir)),
            Box::new(tiered_memory::ReadTieredMemoryTool::new(
                &config.runtime_data_dir(),
            )),
            Box::new(
                tiered_memory::WriteTieredMemoryTool::new(&config.runtime_data_dir())
                    .with_origin_vault(origin_vault.clone()),
            ),
            Box::new(bulletin::UpdateBulletinFocusTool::new(db.clone())),
            Box::new(persona_todo::AddTodoTool::new(db.clone())),
            Box::new(persona_todo::ListTodosTool::new(db.clone())),
            Box::new(persona_todo::CompleteTodoTool::new(db.clone())),
            Box::new(collab_tools::UpdateBoardItemTool::new(db.clone())),
            Box::new(collab_tools::ListBoardTool::new(db.clone())),
            Box::new(collab_tools::ListPersonaActivityTool::new(db.clone())),
            Box::new(collab_tools::ListScheduledRunsTool::new(db.clone())),
            Box::new(collab_tools::ProposeBoardChangeTool::new(db.clone())),
            Box::new(collab_tools::FinishPipelineStageTool::new(db.clone())),
            Box::new(
                search_history::SearchHistoryTool::new(db.clone()).with_recall(recall.clone()),
            ),
            Box::new(recall::RecallTool::new(recall.clone())),
            Box::new(agent_history::ReadAgentHistoryTool::new(
                &config.runtime_data_dir(),
            )),
        ];

        let mut tools: Vec<Box<dyn Tool>> = tools;

        for operation in ["publish_artifact", "find_artifacts", "get_artifact"] {
            tools.push(Box::new(artifacts::ArtifactTool {
                root: config.workspace_root_absolute(),
                origin_vault: origin_vault.clone(),
                operation,
                db: db.clone(),
            }));
        }

        // Register SearchVaultTool: native mode (embedding + ChromaDB HTTP),
        // explicit command mode (vault_search_command), or auto-detected from built-in skill.
        if let Some(ref vault) = config.vault {
            let use_native = vault.embedding_server_url.is_some() && vault.vector_db_url.is_some();
            let use_command = vault
                .vault_search_command
                .as_ref()
                .is_some_and(|c| !c.trim().is_empty());

            if use_native {
                let embed_url = vault.embedding_server_url.as_ref().unwrap();
                let db_url = vault.vector_db_url.as_ref().unwrap();
                let collection = vault.vector_db_collection.as_deref().unwrap_or("vault");
                let vault_tool =
                    search_vault::SearchVaultTool::new_native(embed_url, db_url, collection)
                        .with_recall(recall.clone());
                recall.set_vault_mode(vault_tool.mode());
                tools.push(Box::new(vault_tool));
                tools.push(Box::new(vault_add::AddVaultItemTool::new(
                    embed_url, db_url, collection,
                )));
                tracing::info!(
                    "search_vault and add_vault_item tools registered (native: collection={}, db={})",
                    collection,
                    db_url
                );
            } else if use_command {
                let cmd = vault.vault_search_command.as_ref().unwrap();
                let vault_tool =
                    search_vault::SearchVaultTool::new_command(cmd, config.working_dir())
                        .with_recall(recall.clone());
                recall.set_vault_mode(vault_tool.mode());
                tools.push(Box::new(vault_tool));
                tracing::info!(
                    "search_vault tool registered (command: {})",
                    cmd.split_whitespace().next().unwrap_or("…")
                );
            } else if let Some(cmd) = detect_vault_search_command(config) {
                let vault_tool =
                    search_vault::SearchVaultTool::new_command(&cmd, config.working_dir())
                        .with_recall(recall.clone());
                recall.set_vault_mode(vault_tool.mode());
                tools.push(Box::new(vault_tool));
                tracing::info!("search_vault tool registered (auto-detected from built-in skill)");
            }
        }

        let mut social_added = Vec::new();
        if let Some(ref social) = config.social {
            if social.is_platform_enabled("tiktok") {
                tools.push(Box::new(social_feed::FetchTiktokFeedTool::new(
                    config,
                    db.clone(),
                )));
                social_added.push("fetch_tiktok_feed");
            }
            if social.is_platform_enabled("instagram") {
                tools.push(Box::new(social_feed::FetchInstagramFeedTool::new(
                    config,
                    db.clone(),
                )));
                social_added.push("fetch_instagram_feed");
            }
            if social.is_platform_enabled("linkedin") {
                tools.push(Box::new(social_feed::FetchLinkedinFeedTool::new(
                    config, db,
                )));
                social_added.push("fetch_linkedin_feed");
            }
        }
        if !social_added.is_empty() {
            tracing::info!("Social feed tools registered: {}", social_added.join(", "));
        }
        ToolRegistry {
            tools,
            env_redactor,
            runtime_data_dir: config.runtime_data_dir(),
            recall,
        }
    }

    pub fn recall(&self) -> Arc<crate::recall::RecallService> {
        self.recall.clone()
    }

    pub fn env_redactor(&self) -> &EnvSecretRedactor {
        &self.env_redactor
    }

    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|tool| !hide_recall_alias(tool.name()))
            .map(|t| t.definition())
            .collect()
    }

    pub fn definitions_filtered(&self, read_only: bool) -> Vec<ToolDefinition> {
        if !read_only {
            return self.definitions();
        }
        self.tools
            .iter()
            .filter_map(|t| {
                let name = t.name();
                let allowed = matches!(
                    name,
                    "read_file"
                        | "read_repo_map"
                        | "glob"
                        | "grep"
                        | "locate_file"
                        | "recall"
                        | "read_memory"
                        | "read_memory_state"
                        | "validate_memory_state"
                        | "read_tiered_memory"
                        | "web_search"
                        | "web_fetch"
                        | "read_agent_history"
                        | "find_artifacts"
                        | "get_artifact"
                        | "list_scheduled_tasks"
                        | "get_task_history"
                        | "list_todos"
                );
                if allowed {
                    Some(t.definition())
                } else {
                    None
                }
            })
            .collect()
    }

    pub async fn execute(&self, name: &str, input: serde_json::Value) -> ToolResult {
        for tool in &self.tools {
            if tool.name() == name {
                let started = Instant::now();
                let input = if name == "vault_secret" {
                    input
                } else if let Some(auth) = auth_context_from_input(&input) {
                    match crate::secret_vault::resolve_placeholders(
                        &self.runtime_data_dir,
                        auth.caller_chat_id,
                        input,
                    ) {
                        Ok(resolved) => resolved,
                        Err(e) => return ToolResult::error(e),
                    }
                } else {
                    input
                };
                let mut result = tool.execute(input).await;
                result.content = self.env_redactor.redact(&result.content);
                result.duration_ms = Some(started.elapsed().as_millis());
                result.bytes = result.content.len();
                if result.is_error && result.error_type.is_none() {
                    result.error_type = Some("tool_error".to_string());
                }
                if result.status_code.is_none() {
                    result.status_code = Some(if result.is_error { 1 } else { 0 });
                }
                return result;
            }
        }
        ToolResult::error(format!("Unknown tool: {name}")).with_error_type("unknown_tool")
    }

    pub async fn execute_with_auth(
        &self,
        name: &str,
        input: serde_json::Value,
        auth: &ToolAuthContext,
    ) -> ToolResult {
        let input = inject_auth_context(input, auth);
        self.execute(name, input).await
    }
}

/// Helper to build a JSON Schema object with required properties.
pub fn schema_object(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_result_success() {
        let r = ToolResult::success("ok".into());
        assert_eq!(r.content, "ok");
        assert!(!r.is_error);
    }

    #[test]
    fn test_tool_result_error() {
        let r = ToolResult::error("fail".into());
        assert_eq!(r.content, "fail");
        assert!(r.is_error);
    }

    #[test]
    fn test_schema_object() {
        let schema = schema_object(
            json!({
                "name": {"type": "string"},
                "age": {"type": "integer"}
            }),
            &["name"],
        );
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["age"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "name");
    }

    #[test]
    fn test_schema_object_empty_required() {
        let schema = schema_object(json!({}), &[]);
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn test_default_persona_id_for_chat_matches_run() {
        let input = json!({
            "__meido_auth": {
                "caller_chat_id": 50,
                "caller_persona_id": 9,
                "control_chat_ids": []
            }
        });
        assert_eq!(default_persona_id_for_chat(&input, 50), Some(9));
        assert_eq!(default_persona_id_for_chat(&input, 51), None);
        let no_pid = json!({
            "__meido_auth": {
                "caller_chat_id": 50,
                "caller_persona_id": 0,
                "control_chat_ids": []
            }
        });
        assert_eq!(default_persona_id_for_chat(&no_pid, 50), None);
    }

    #[test]
    fn test_auth_context_from_input() {
        let input = json!({
            "__meido_auth": {
                "caller_channel": "telegram",
                "caller_chat_id": 123,
                "control_chat_ids": [123, 999]
            }
        });
        let auth = auth_context_from_input(&input).unwrap();
        assert_eq!(auth.caller_channel, "telegram");
        assert_eq!(auth.caller_chat_id, 123);
        assert!(auth.session_id.is_none());
        assert!(auth.is_control_chat());
        assert!(auth.can_access_chat(456));
    }

    #[test]
    fn test_auth_context_session_id_roundtrip() {
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: Some("sess-abc".into()),
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let injected = inject_auth_context(json!({"command": "echo"}), &auth);
        let parsed = auth_context_from_input(&injected).unwrap();
        assert_eq!(parsed.chat_session_id().as_deref(), Some("sess-abc"));
        let blank = json!({
            "__meido_auth": {
                "caller_chat_id": 10,
                "session_id": "  "
            }
        });
        assert!(auth_context_from_input(&blank)
            .unwrap()
            .session_id
            .is_none());
    }

    #[test]
    fn test_authorize_chat_access_denied() {
        let input = json!({
            "__meido_auth": {
                "caller_channel": "telegram",
                "caller_chat_id": 100,
                "control_chat_ids": []
            }
        });
        let err = authorize_chat_access(&input, 200).unwrap_err();
        assert!(err.contains("Permission denied"));
    }

    #[test]
    fn test_normalize_tool_relative_path_strips_redundant_prefixes() {
        let shared = std::path::PathBuf::from("/proj/workspace/shared");
        assert_eq!(
            normalize_tool_relative_path(&shared, "workspace/shared/foo.txt"),
            "foo.txt"
        );
        assert_eq!(
            normalize_tool_relative_path(&shared, "workspace/runtime/groups/1/x"),
            "runtime/groups/1/x"
        );
        assert_eq!(
            normalize_tool_relative_path(&shared, "shared/ORIGIN/x.md"),
            "ORIGIN/x.md"
        );
        assert_eq!(
            normalize_tool_relative_path(&shared, "./foo.txt"),
            "foo.txt"
        );
    }

    #[test]
    fn test_resolve_tool_path_from_shared_cwd() {
        let root = std::path::PathBuf::from("/proj/workspace");
        let shared = root.join("shared");
        assert_eq!(
            resolve_tool_path(&root, &shared, "foo.txt"),
            std::path::PathBuf::from("/proj/workspace/shared/foo.txt")
        );
        assert_eq!(
            resolve_tool_path(&root, &shared, "workspace/shared/foo.txt"),
            std::path::PathBuf::from("/proj/workspace/shared/foo.txt")
        );
        assert_eq!(
            resolve_tool_path(&root, &shared, "workspace/runtime/groups/1/x"),
            std::path::PathBuf::from("/proj/workspace/runtime/groups/1/x")
        );
        assert_eq!(
            resolve_tool_path(&root, &shared, "shared/ORIGIN/x.md"),
            std::path::PathBuf::from("/proj/workspace/shared/ORIGIN/x.md")
        );
    }

    #[test]
    fn test_check_shadow_workspace_write_blocks_nested_tree() {
        let root = std::env::temp_dir().join(format!("meido_shadow_{}", uuid::Uuid::new_v4()));
        let shadow_file = root.join("shared").join("workspace").join("nested.txt");
        std::fs::create_dir_all(shadow_file.parent().unwrap()).unwrap();
        std::fs::write(&shadow_file, "x").unwrap();
        assert!(is_under_shadow_workspace(&root, &shadow_file));
        let err = check_shadow_workspace_write(&root, &shadow_file).unwrap_err();
        assert!(err.contains("shadow workspace"));
        let ok_path = root.join("shared").join("ok.txt");
        assert!(!is_under_shadow_workspace(&root, &ok_path));
        assert!(check_shadow_workspace_write(&root, &ok_path).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_tool_working_dir_shared() {
        let dir = resolve_tool_working_dir(std::path::Path::new("/tmp/work"));
        assert_eq!(dir, std::path::PathBuf::from("/tmp/work/shared"));
    }

    #[test]
    fn test_resolve_tool_working_dir_for_auth_persona() {
        let root = std::env::temp_dir().join(format!("meido_tool_cwd_{}", uuid::Uuid::new_v4()));
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let cwd = resolve_tool_working_dir_for_auth(&root, Some(&auth));
        assert_eq!(
            cwd,
            root.join("shared").join("personas").join("10").join("2")
        );
        assert!(cwd.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_assert_persona_tool_path_allowed_rejects_other_persona() {
        let root = std::env::temp_dir().join(format!("meido_tool_scope_{}", uuid::Uuid::new_v4()));
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let mine = persona_shared_dir(&root, 10, 2).join("x.txt");
        let other = persona_shared_dir(&root, 10, 3).join("x.txt");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&mine, "ok").unwrap();
        std::fs::write(&other, "no").unwrap();
        assert!(assert_persona_tool_path_allowed(&root, &mine, Some(&auth), false).is_ok());
        assert!(assert_persona_tool_path_allowed(&root, &other, Some(&auth), false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_assert_persona_tool_path_allowed_rejects_flat_shared_writes() {
        let root = std::env::temp_dir().join(format!("meido_tool_scope2_{}", uuid::Uuid::new_v4()));
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let flat = root.join("shared").join("scripts").join("a.py");
        std::fs::create_dir_all(flat.parent().unwrap()).unwrap();
        // Create the file so `is_under` canonicalizes the full path (matching the
        // canonicalized root) regardless of symlinked temp dirs.
        std::fs::write(&flat, "x").unwrap();
        let err = assert_persona_tool_path_allowed(&root, &flat, Some(&auth), true).unwrap_err();
        assert!(err.contains("flat shared paths"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_tool_path_prefers_persona_origin() {
        let root = std::env::temp_dir().join(format!("meido_origin_res_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        let persona_file = persona.join("ORIGIN").join("ops").join("sop.md");
        let shared_file = root
            .join("shared")
            .join("ORIGIN")
            .join("ops")
            .join("sop.md");
        std::fs::create_dir_all(persona_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(shared_file.parent().unwrap()).unwrap();
        std::fs::write(&persona_file, "persona").unwrap();
        std::fs::write(&shared_file, "shared").unwrap();
        let resolved = resolve_tool_path(&root, &persona, "ORIGIN/ops/sop.md");
        assert_eq!(resolved, persona_file);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_origin_paths_follow_configured_vault() {
        let root = std::env::temp_dir().join(format!("meido_vault_root_{}", uuid::Uuid::new_v4()));
        let vault = std::env::temp_dir().join(format!("meido_vault_ext_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        std::fs::create_dir_all(&persona).unwrap();
        std::fs::create_dir_all(vault.join("Operations")).unwrap();
        let note = vault.join("Operations").join("note.md");
        std::fs::write(&note, "vault").unwrap();

        let resolved = resolve_tool_path_with_origin(
            &root,
            &persona,
            "ORIGIN/Operations/note.md",
            &vault,
            None,
        );
        assert_eq!(resolved, note);

        let escaped =
            resolve_tool_path_with_origin(&root, &persona, "ORIGIN/../../secret.txt", &vault, None);
        assert!(!path_under_root(&escaped, &vault) || escaped == vault);

        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &note,
            Some(&auth),
            true,
            &vault
        )
        .is_ok());
        let local = root.join("shared").join("ORIGIN").join("note.md");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "local").unwrap();
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &local,
            Some(&auth),
            true,
            &vault
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&vault);
    }

    #[test]
    fn test_vector_db_path_follows_configured_root_only_when_set() {
        let root = std::env::temp_dir().join(format!("meido_vdb_root_{}", uuid::Uuid::new_v4()));
        let db = std::env::temp_dir().join(format!("meido_vdb_ext_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        let origin = root.join("shared").join("ORIGIN");
        std::fs::create_dir_all(&persona).unwrap();
        std::fs::create_dir_all(db.join("chroma")).unwrap();
        let index = db.join("chroma").join("index");
        std::fs::write(&index, "db").unwrap();

        let unset =
            resolve_tool_path_with_origin(&root, &persona, "vault_db/chroma/index", &origin, None);
        assert_eq!(
            unset,
            root.join("shared")
                .join("vault_db")
                .join("chroma")
                .join("index")
        );

        let set = resolve_tool_path_with_origin(
            &root,
            &persona,
            "vault_db/chroma/index",
            &origin,
            Some(db.as_path()),
        );
        assert_eq!(set, index);

        let escaped = resolve_tool_path_with_origin(
            &root,
            &persona,
            "vault_db/../../secret.txt",
            &origin,
            Some(db.as_path()),
        );
        assert!(!path_under_root(&escaped, &db) || escaped == db);

        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: Some(db.clone()),
            external_roots: Vec::new(),
        };
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &index,
            Some(&auth),
            false,
            &origin
        )
        .is_ok());
        let local = root.join("shared").join("vault_db").join("index");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "local").unwrap();
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &local,
            Some(&auth),
            false,
            &origin
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&db);
    }

    #[test]
    fn test_external_roots_allow_only_the_configured_absolute_path() {
        let root = std::env::temp_dir().join(format!("meido_ext_ws_{}", uuid::Uuid::new_v4()));
        let allowed = std::env::temp_dir().join(format!("meido_ext_ok_{}", uuid::Uuid::new_v4()));
        let denied = std::env::temp_dir().join(format!("meido_ext_no_{}", uuid::Uuid::new_v4()));
        let persona = persona_shared_dir(&root, 10, 2);
        let origin = root.join("shared").join("ORIGIN");
        std::fs::create_dir_all(&persona).unwrap();
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();
        let note = allowed.join("note.md");
        let other = denied.join("note.md");
        std::fs::write(&note, "ok").unwrap();
        std::fs::write(&other, "no").unwrap();

        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: vec![allowed.clone()],
        };
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &note,
            Some(&auth),
            true,
            &origin
        )
        .is_ok());
        assert!(assert_persona_tool_path_allowed_with_origin(
            &root,
            &other,
            Some(&auth),
            false,
            &origin
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&allowed);
        let _ = std::fs::remove_dir_all(&denied);
    }

    #[test]
    fn test_external_root_lifts_self_repo_write_ban() {
        let checkout =
            std::env::temp_dir().join(format!("meido_self_allow_{}", uuid::Uuid::new_v4()));
        let workspace = checkout.join("workspace");
        let src = checkout.join("src");
        let persona = persona_shared_dir(&workspace, 10, 2);
        let origin = workspace.join("shared").join("ORIGIN");
        std::fs::create_dir_all(&persona).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(checkout.join("Cargo.toml"), "[package]\nname = \"meido\"\n").unwrap();
        let file = src.join("lib.rs");
        std::fs::write(&file, "x").unwrap();
        let elsewhere = checkout.join("docs").join("note.md");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "x").unwrap();

        let mut auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: vec![src.clone()],
        };
        assert!(
            assert_persona_tool_path_allowed_with_origin(
                &workspace,
                &file,
                Some(&auth),
                true,
                &origin
            )
            .is_ok(),
            "listed subdirectory should be writable"
        );
        let denied = assert_persona_tool_path_allowed_with_origin(
            &workspace,
            &elsewhere,
            Some(&auth),
            true,
            &origin,
        )
        .unwrap_err();
        assert!(denied.contains("external paths"));
        auth.external_roots.clear();
        let blocked = assert_persona_tool_path_allowed_with_origin(
            &workspace,
            &file,
            Some(&auth),
            true,
            &origin,
        )
        .unwrap_err();
        assert!(blocked.contains("external paths"));
        let _ = std::fs::remove_dir_all(&checkout);
    }

    #[test]
    fn test_assert_persona_allows_missing_file_under_allowed_root() {
        let root =
            std::env::temp_dir().join(format!("meido_missing_scope_{}", uuid::Uuid::new_v4()));
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let persona = persona_shared_dir(&root, 10, 2);
        std::fs::create_dir_all(persona.join("ORIGIN")).unwrap();
        // Missing file under persona ORIGIN must NOT be permission-denied.
        let missing = persona.join("ORIGIN").join("no-such-sop.md");
        assert!(
            assert_persona_tool_path_allowed(&root, &missing, Some(&auth), false).is_ok(),
            "missing in-scope path should be allowed (not-found is for read_file)"
        );
        let outside = std::env::temp_dir().join(format!(
            "meido_missing_scope_outside_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("x.md");
        std::fs::write(&outside_file, "x").unwrap();
        let err =
            assert_persona_tool_path_allowed(&root, &outside_file, Some(&auth), false).unwrap_err();
        assert!(err.contains("Permission denied"));
        let loose = root.join("loose.md");
        std::fs::write(&loose, "x").unwrap();
        assert!(
            assert_persona_tool_path_allowed(&root, &loose, Some(&auth), true).is_ok(),
            "a workspace outside the app directory is editable by every persona"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn test_workspace_inside_app_does_not_blanket_approve_loose_files() {
        let app = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = app
            .join("target")
            .join(format!("meido_inside_ws_{}", uuid::Uuid::new_v4()));
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots: Vec::new(),
        };
        let persona = persona_shared_dir(&root, 10, 2);
        std::fs::create_dir_all(&persona).unwrap();
        let loose = root.join("loose.md");
        std::fs::write(&loose, "x").unwrap();
        let err = assert_persona_tool_path_allowed(&root, &loose, Some(&auth), true).unwrap_err();
        assert!(err.contains("Permission denied"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_app_directory_is_editable_when_workspace_is_outside() {
        let root = std::env::temp_dir().join(format!("meido_app_allow_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut external_roots = Vec::new();
        crate::self_repo::append_auto_approved_app_directory(&root, &mut external_roots);
        let app = crate::self_repo::current_app_directory().expect("app checkout");
        let file = app.join("Cargo.toml");
        let auth = ToolAuthContext {
            run_id: None,
            caller_channel: "web".to_string(),
            caller_chat_id: 10,
            caller_persona_id: 2,
            control_chat_ids: vec![],
            is_scheduled_task: false,
            session_id: None,
            vector_db_root: None,
            external_roots,
        };
        let persona = persona_shared_dir(&root, 10, 2);
        std::fs::create_dir_all(&persona).unwrap();
        assert!(
            assert_persona_tool_path_allowed(&root, &file, Some(&auth), true).is_ok(),
            "the app directory is approved for every persona when the workspace is outside it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_path_under_root_handles_nonexistent_relative() {
        let root = std::env::temp_dir().join(format!("meido_under_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("shared").join("ORIGIN")).unwrap();
        let missing = root.join("shared").join("ORIGIN").join("missing.md");
        assert!(path_under_root(
            &missing,
            &root.join("shared").join("ORIGIN")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[allow(dead_code)]
    struct DummyTool {
        tool_name: String,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                self.tool_name.clone(),
                "dummy",
                schema_object(json!({}), &[]),
            )
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult::success("ok".into())
        }
    }

    #[test]
    fn test_tool_risk_levels() {
        assert_eq!(tool_risk("bash"), ToolRisk::High);
        assert_eq!(tool_risk("write_file"), ToolRisk::Medium);
        assert_eq!(tool_risk("apply_search_replace"), ToolRisk::Medium);
        assert_eq!(tool_risk("symbol_edit"), ToolRisk::Medium);
        assert_eq!(tool_risk("pause_scheduled_task"), ToolRisk::Medium);
        assert_eq!(tool_risk("sync_skills"), ToolRisk::Medium);
        assert_eq!(tool_risk("read_file"), ToolRisk::Low);
    }
}
