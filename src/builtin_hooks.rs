//! Shipped hook catalog under repository `builtin_hooks/` (`*.hook.json` manifests).
//!
//! Policy hooks use Rust `builtin_*` action types (see `hook_runtime.rs`). PZ terminal cleanup is
//! not shipped here; operators add optional command hooks under `{WORKSPACE_DIR}/hooks/`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use tracing::warn;

use crate::config::Config;
use crate::error::FinallyAValueBotError;

const SHIPPED_HOOK_MANIFEST_SUFFIX: &str = ".hook.json";

/// Resolve the on-disk `builtin_hooks/` directory.
///
/// Precedence:
/// 1. `FINALLY_A_VALUE_BOT_BUILTIN_HOOKS` if set and path exists
/// 2. Parent of workspace root + `builtin_hooks`
/// 3. Current working directory + `builtin_hooks`
/// 4. Parent of current executable + `builtin_hooks`
/// 5. Compile-time `CARGO_MANIFEST_DIR/builtin_hooks`
pub fn resolve_builtin_hooks_dir(config: &Config) -> Option<PathBuf> {
    resolve_builtin_hooks_dir_from_workspace(config.workspace_root_absolute().as_path())
}

/// Resolve `builtin_hooks/` without a full config (e.g. during DB migration).
pub fn resolve_builtin_hooks_dir_fallback() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("builtin_hooks");
        if p.is_dir() {
            return Some(p);
        }
    }
    let manifest_builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("builtin_hooks");
    if manifest_builtin.is_dir() {
        return Some(manifest_builtin);
    }
    None
}

fn resolve_builtin_hooks_dir_from_workspace(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FINALLY_A_VALUE_BOT_BUILTIN_HOOKS") {
        let pb = PathBuf::from(p.trim());
        if pb.is_dir() {
            return Some(pb);
        }
    }

    if let Some(parent) = workspace_root.parent() {
        let p = parent.join("builtin_hooks");
        if p.is_dir() {
            return Some(p);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("builtin_hooks");
        if p.is_dir() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let p = parent.join("builtin_hooks");
            if p.is_dir() {
                return Some(p);
            }
        }
    }

    let manifest_builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("builtin_hooks");
    if manifest_builtin.is_dir() {
        return Some(manifest_builtin);
    }

    None
}

#[derive(Debug, Deserialize)]
struct ShippedHookManifest {
    name: String,
    event_name: String,
    #[serde(default)]
    matcher: Option<String>,
    action_type: String,
    #[serde(default)]
    action_payload: serde_json::Value,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

fn load_shipped_manifests(dir: &Path) -> Result<Vec<ShippedHookManifest>, FinallyAValueBotError> {
    let mut manifests = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| {
        FinallyAValueBotError::ToolExecution(format!(
            "read builtin_hooks dir '{}': {e}",
            dir.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!("read builtin_hooks entry: {e}"))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file_name.ends_with(SHIPPED_HOOK_MANIFEST_SUFFIX) {
            continue;
        }
        let raw = fs::read_to_string(&path).map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!(
                "read shipped hook manifest '{}': {e}",
                path.display()
            ))
        })?;
        let manifest: ShippedHookManifest = serde_json::from_str(&raw).map_err(|e| {
            FinallyAValueBotError::ToolExecution(format!(
                "parse shipped hook manifest '{}': {e}",
                path.display()
            ))
        })?;
        manifests.push(manifest);
    }
    manifests.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(manifests)
}

/// Upsert shipped hook rows from `*.hook.json` in `hooks_dir` into SQLite `hook_definitions`.
pub fn sync_shipped_hook_definitions(
    conn: &Connection,
    hooks_dir: &Path,
) -> Result<usize, FinallyAValueBotError> {
    let manifests = load_shipped_manifests(hooks_dir)?;
    if manifests.is_empty() {
        warn!(
            "builtin_hooks dir '{}' has no {} manifests",
            hooks_dir.display(),
            SHIPPED_HOOK_MANIFEST_SUFFIX
        );
        return Ok(0);
    }

    let now = Utc::now().to_rfc3339();
    let mut synced = 0usize;
    for manifest in manifests {
        let name = manifest.name.trim();
        let event_name = manifest.event_name.trim();
        let action_type = manifest.action_type.trim().to_ascii_lowercase();
        if name.is_empty() || event_name.is_empty() || action_type.is_empty() {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "invalid shipped hook manifest in '{}': name, event_name, and action_type are required",
                hooks_dir.display()
            )));
        }
        if !action_type.starts_with("builtin_") {
            return Err(FinallyAValueBotError::ToolExecution(format!(
                "shipped hook '{name}' must use a builtin_* action_type (got '{action_type}'); non-builtin hooks belong under WORKSPACE_DIR/hooks/"
            )));
        }
        let payload_json = if manifest.action_payload.is_null() {
            "{}".to_string()
        } else {
            serde_json::to_string(&manifest.action_payload).map_err(|e| {
                FinallyAValueBotError::ToolExecution(format!(
                    "serialize action_payload for shipped hook '{name}': {e}"
                ))
            })?
        };
        let matcher_norm = manifest
            .matcher
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        let enabled_i = if manifest.enabled { 1 } else { 0 };

        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM hook_definitions WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE hook_definitions
                 SET event_name = ?1,
                     matcher = ?2,
                     action_type = ?3,
                     action_payload_json = ?4,
                     scoped_persona_ids_json = NULL,
                     enabled = ?5,
                     updated_at = ?6
                 WHERE id = ?7",
                params![
                    event_name,
                    matcher_norm,
                    action_type,
                    payload_json,
                    enabled_i,
                    now,
                    id
                ],
            )?;
        } else {
            conn.execute(
                "INSERT INTO hook_definitions
                 (name, event_name, matcher, action_type, action_payload_json, scoped_persona_ids_json, enabled, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
                params![
                    name,
                    event_name,
                    matcher_norm,
                    action_type,
                    payload_json,
                    enabled_i,
                    now
                ],
            )?;
        }
        synced += 1;
    }
    Ok(synced)
}

/// Load shipped manifests from the resolved catalog dir and upsert into `hook_definitions`.
pub fn ensure_shipped_hooks_in_db(
    conn: &Connection,
    config: &Config,
) -> Result<(), FinallyAValueBotError> {
    let dir = resolve_builtin_hooks_dir(config)
        .or_else(resolve_builtin_hooks_dir_fallback)
        .ok_or_else(|| {
            FinallyAValueBotError::ToolExecution(
                "builtin_hooks catalog directory not found (expected repository builtin_hooks/ with *.hook.json)".into(),
            )
        })?;
    let n = sync_shipped_hook_definitions(conn, &dir)?;
    if n == 0 {
        return Err(FinallyAValueBotError::ToolExecution(format!(
            "no shipped hook manifests synced from '{}'",
            dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use std::fs;

    #[test]
    fn resolve_finds_sibling_of_workspace() {
        let tmp =
            std::env::temp_dir().join(format!("fab_builtin_hooks_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(tmp.join("workspace")).expect("workspace dir");
        fs::create_dir_all(tmp.join("builtin_hooks")).expect("builtin hooks dir");

        let mut config = test_config();
        config.workspace_dir = tmp.join("workspace").to_string_lossy().to_string();

        let got = resolve_builtin_hooks_dir(&config).expect("expected builtin_hooks");
        assert_eq!(got, tmp.join("builtin_hooks"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn repo_catalog_has_five_shipped_manifests() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("builtin_hooks");
        let manifests = load_shipped_manifests(&dir).expect("load repo manifests");
        assert_eq!(manifests.len(), 7);
        assert!(manifests
            .iter()
            .all(|m| m.action_type.starts_with("builtin_")));
        assert!(
            !manifests
                .iter()
                .any(|m| m.name.contains("pz") || m.name.contains("PZ")),
            "PZ must not be in shipped catalog"
        );
    }

    #[test]
    fn sync_shipped_manifests_upserts_rows() {
        let tmp = std::env::temp_dir().join(format!("fab_hook_sync_test_{}", uuid::Uuid::new_v4()));
        let hooks_dir = tmp.join("builtin_hooks");
        fs::create_dir_all(&hooks_dir).expect("hooks dir");
        fs::write(
            hooks_dir.join("sample.hook.json"),
            r#"{
              "name": "sample-builtin-hook",
              "event_name": "BeforeTurn",
              "action_type": "builtin_scheduler_policy_context",
              "enabled": true
            }"#,
        )
        .expect("write manifest");

        let db_dir = tmp.join("runtime");
        fs::create_dir_all(&db_dir).expect("runtime dir");
        let conn = Connection::open(db_dir.join("test.db")).expect("open db");
        conn.execute_batch(
            "CREATE TABLE hook_definitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                event_name TEXT NOT NULL,
                matcher TEXT,
                action_type TEXT NOT NULL,
                action_payload_json TEXT NOT NULL DEFAULT '{}',
                scoped_persona_ids_json TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL
            );",
        )
        .expect("schema");

        let n = sync_shipped_hook_definitions(&conn, &hooks_dir).expect("sync");
        assert_eq!(n, 1);
        let action_type: String = conn
            .query_row(
                "SELECT action_type FROM hook_definitions WHERE name = 'sample-builtin-hook'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(action_type, "builtin_scheduler_policy_context");

        let _ = fs::remove_dir_all(&tmp);
    }
}
