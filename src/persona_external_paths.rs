//! Per-persona absolute paths outside the workspace.
//!
//! Each persona has one environment variable, `MEIDO_PERSONA_<id>_EXTERNAL_PATHS`.
//! The value is a JSON array of absolute paths. An unset or empty variable grants
//! no extra paths. The web UI writes the same variable in the configuration `.env`
//! and updates the in-memory map so the next run sees it.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, RwLock};

const KEY_PREFIX: &str = "MEIDO_PERSONA_";
const KEY_SUFFIX: &str = "_EXTERNAL_PATHS";
const MAX_PATHS: usize = 32;
const MAX_PATH_CHARS: usize = 4096;

/// Environment variable for one persona's external path allowlist.
pub fn env_key(persona_id: i64) -> Option<String> {
    if persona_id <= 0 {
        return None;
    }
    Some(format!("{KEY_PREFIX}{persona_id}{KEY_SUFFIX}"))
}

fn persona_id_from_key(key: &str) -> Option<i64> {
    let rest = key.strip_prefix(KEY_PREFIX)?.strip_suffix(KEY_SUFFIX)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok().filter(|id: &i64| *id > 0)
}

/// Lexically normalize an absolute path. Relative paths and the filesystem root are rejected.
pub fn normalize_absolute(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path is empty".into());
    }
    if trimmed.len() > MAX_PATH_CHARS {
        return Err("path is too long".into());
    }
    if trimmed.contains('\0') {
        return Err("path contains a null byte".into());
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(format!("'{trimmed}' is not an absolute path"));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(format!("'{trimmed}' escapes its root"));
                }
            }
            Component::Normal(name) => out.push(name),
            other => out.push(other.as_os_str()),
        }
    }
    if !out
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        return Err("the filesystem root cannot be an external path".into());
    }
    Ok(out)
}

/// Parse a `MEIDO_PERSONA_<id>_EXTERNAL_PATHS` value.
///
/// A JSON array is the stored form. A hand-edited value may also be one absolute
/// path, or several paths separated by newlines.
pub fn parse_external_paths_value(raw: &str) -> Result<Vec<PathBuf>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<String> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed)
            .map_err(|_| "external paths must be a JSON array of absolute paths".to_string())?
    } else {
        trimmed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    if items.len() > MAX_PATHS {
        return Err(format!("at most {MAX_PATHS} external paths are allowed"));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let path = normalize_absolute(&item)?;
        if !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    }
    Ok(out)
}

pub fn encode_external_paths(paths: &[PathBuf]) -> String {
    let values: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string())
}

fn quote_env_value(value: &str) -> String {
    if value
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '#' | '\\' | '$' | '`'))
    {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

fn newline_of(content: &str) -> &str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim().strip_prefix("export ").unwrap_or(line.trim());
    let (key, _) = trimmed.split_once('=')?;
    Some(key.trim())
}

pub fn upsert_env_line(content: &str, name: &str, value: &str) -> String {
    let newline = newline_of(content);
    let line = format!("{name}={}", quote_env_value(value));
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in content.lines() {
        if assignment_key(raw) == Some(name) {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        out.push(line);
    }
    let mut joined = out.join(newline);
    if !joined.ends_with(newline) {
        joined.push_str(newline);
    }
    joined
}

pub fn remove_env_line(content: &str, name: &str) -> String {
    let newline = newline_of(content);
    let out: Vec<&str> = content
        .lines()
        .filter(|raw| assignment_key(raw) != Some(name))
        .collect();
    if out.is_empty() {
        return String::new();
    }
    let mut joined = out.join(newline);
    if content.ends_with('\n') {
        joined.push_str(newline);
    }
    joined
}

fn atomic_replace(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.persona-paths.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("env")
    ));
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("Could not write .env: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Could not write .env: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Could not write .env: {e}"))?;
    }
    if let Some(permissions) = permissions {
        let _ = fs::set_permissions(&tmp, permissions);
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path).map_err(|e| format!("Could not replace .env: {e}"))?;
    }
    Ok(())
}

/// Live allowlist. File writes and the map stay together so a UI save applies
/// before the next process start.
pub struct PersonaExternalPaths {
    paths: RwLock<HashMap<i64, Vec<PathBuf>>>,
    env_file: Mutex<Option<PathBuf>>,
    file_lock: Mutex<()>,
}

impl PersonaExternalPaths {
    pub fn empty(env_file: Option<PathBuf>) -> Self {
        Self {
            paths: RwLock::new(HashMap::new()),
            env_file: Mutex::new(env_file),
            file_lock: Mutex::new(()),
        }
    }

    /// Load every `MEIDO_PERSONA_<id>_EXTERNAL_PATHS` value already in the process environment.
    ///
    /// A value that does not parse is ignored for that persona. The process still starts.
    pub fn load(env_file: Option<PathBuf>) -> Self {
        let mut paths = HashMap::new();
        for (key, value) in std::env::vars() {
            let Some(persona_id) = persona_id_from_key(&key) else {
                continue;
            };
            match parse_external_paths_value(&value) {
                Ok(parsed) if !parsed.is_empty() => {
                    paths.insert(persona_id, parsed);
                }
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        persona_id,
                        "Ignoring {key}: the value is not a list of absolute paths"
                    );
                }
            }
        }
        Self {
            paths: RwLock::new(paths),
            env_file: Mutex::new(env_file),
            file_lock: Mutex::new(()),
        }
    }

    pub fn paths_for(&self, persona_id: i64) -> Vec<PathBuf> {
        self.paths
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(&persona_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace one persona's allowlist. An empty list removes the environment variable.
    pub fn set(&self, persona_id: i64, raw_paths: &[String]) -> Result<Vec<PathBuf>, String> {
        let key = env_key(persona_id).ok_or_else(|| "invalid persona id".to_string())?;
        let encoded = serde_json::to_string(raw_paths)
            .map_err(|_| "could not encode external paths".to_string())?;
        let paths = parse_external_paths_value(&encoded)?;
        self.write_key(&key, persona_id, &paths)?;
        Ok(paths)
    }

    pub fn clear(&self, persona_id: i64) -> Result<(), String> {
        let Some(key) = env_key(persona_id) else {
            return Ok(());
        };
        self.write_key(&key, persona_id, &[])
    }

    fn write_key(&self, key: &str, persona_id: i64, paths: &[PathBuf]) -> Result<(), String> {
        let _file = self.file_lock.lock().unwrap_or_else(|err| err.into_inner());
        let env_file = self
            .env_file
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        let Some(env_file) = env_file else {
            return Err(
                "No .env file found. Create one at the configuration root before saving paths outside the workspace."
                    .into(),
            );
        };
        if !env_file.is_file() {
            return Err(format!(
                "Configuration file '{}' is missing.",
                env_file.display()
            ));
        }
        let contents = fs::read_to_string(&env_file)
            .map_err(|e| format!("Could not read {}: {e}", env_file.display()))?;
        let next = if paths.is_empty() {
            remove_env_line(&contents, key)
        } else {
            upsert_env_line(&contents, key, &encode_external_paths(paths))
        };
        atomic_replace(&env_file, &next)?;
        let mut map = self.paths.write().unwrap_or_else(|err| err.into_inner());
        if paths.is_empty() {
            map.remove(&persona_id);
        } else {
            map.insert(persona_id, paths.to_vec());
        }
        tracing::info!(
            persona_id,
            count = paths.len(),
            "Updated persona external paths"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_stay_absolute_and_roots_are_rejected() {
        assert!(normalize_absolute("notes/file.md").is_err());
        let root = if cfg!(windows) { r"C:\" } else { "/" };
        assert!(normalize_absolute(root).is_err());

        let allowed = if cfg!(windows) {
            r"C:\repos\other"
        } else {
            "/tmp/repos/other"
        };
        let path = normalize_absolute(allowed).expect("absolute path");
        assert_eq!(path, PathBuf::from(allowed));

        let climbed = if cfg!(windows) {
            r"C:\repos\other\..\secret"
        } else {
            "/tmp/repos/other/../secret"
        };
        let normalized = normalize_absolute(climbed).expect("normalized");
        assert!(normalized.ends_with("secret"));
        assert!(!normalized.ends_with(Path::new("other").join("secret")));
    }

    #[test]
    fn json_value_round_trips_and_env_line_replaces_only_that_key() {
        let raw = if cfg!(windows) {
            r#"["C:\\repos\\other","C:\\repos\\other"]"#
        } else {
            r#"["/tmp/repos/other","/tmp/repos/other"]"#
        };
        let paths = parse_external_paths_value(raw).expect("parse");
        assert_eq!(paths.len(), 1);
        let encoded = encode_external_paths(&paths);
        let again = parse_external_paths_value(&encoded).expect("round trip");
        assert_eq!(again, paths);

        let key = env_key(2).unwrap();
        let file = format!("OTHER=keep\n{key}=[\"/old\"]\nTOKEN=secret\n");
        let updated = upsert_env_line(&file, &key, &encoded);
        assert!(updated.contains("OTHER=keep"));
        assert!(updated.contains("TOKEN=secret"));
        assert!(updated.contains(&format!("{key}=")));
        assert!(!updated.contains("/old"));
        let cleared = remove_env_line(&updated, &key);
        assert!(!cleared.contains(&key));
        assert!(cleared.contains("TOKEN=secret"));
    }

    #[test]
    fn set_writes_the_persona_key_and_clear_removes_it() {
        let dir =
            std::env::temp_dir().join(format!("meido_external_paths_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_file = dir.join(".env");
        std::fs::write(&env_file, "TOKEN=keep\n").unwrap();
        let store = PersonaExternalPaths::empty(Some(env_file.clone()));
        let path = if cfg!(windows) {
            r"C:\repos\other"
        } else {
            "/tmp/repos/other"
        };
        let saved = store.set(4, &[path.to_string()]).expect("set");
        assert_eq!(saved, store.paths_for(4));
        let text = std::fs::read_to_string(&env_file).unwrap();
        let key = env_key(4).unwrap();
        assert!(text.contains(&key));
        assert!(text.contains("TOKEN=keep"));
        store.clear(4).expect("clear");
        assert!(store.paths_for(4).is_empty());
        let text = std::fs::read_to_string(&env_file).unwrap();
        assert!(!text.contains(&key));
        assert!(text.contains("TOKEN=keep"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
