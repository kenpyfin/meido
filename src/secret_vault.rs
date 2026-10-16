//! Per-chat secret vault. The agent calls `vault_secret`; values live in
//! `runtime/secrets/<chat_id>.env` and history is rewritten to a placeholder.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::db::Database;
use crate::safety_redaction::{shannon_entropy, EnvSecretRedactor};

pub const MIN_CHAT_WIDE_SCRUB_LEN: usize = 6;
const HIGH_ENTROPY: f64 = 3.0;
const LONG_TOKEN_LEN: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretMeta {
    pub name: String,
    pub kind: String,
    pub skill: Option<String>,
    pub created_at: String,
    /// False until the post-turn scrub has rewritten history.
    pub scrubbed: bool,
    /// Chat-wide substring replace is safe (high entropy or very long).
    pub chat_wide: bool,
    /// Persona that stored this secret. `None` means a row written before
    /// persona tagging (shown as shared/legacy in the UI).
    #[serde(default)]
    pub persona_id: Option<i64>,
}

pub fn secrets_dir(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("secrets")
}

pub fn vault_file(data_dir: &str, chat_id: i64) -> PathBuf {
    secrets_dir(data_dir).join(format!("{chat_id}.env"))
}

fn meta_file(data_dir: &str, chat_id: i64) -> PathBuf {
    secrets_dir(data_dir).join(format!("{chat_id}.meta.json"))
}

pub fn placeholder_for(name: &str, chat_id: i64) -> String {
    format!("[SECRET:{name} stored in runtime/secrets/{chat_id}.env]")
}

pub fn validate_secret_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if ok && name.len() >= 2 {
        Ok(())
    } else {
        Err("Secret name must match ^[A-Z][A-Z0-9_]{1,63}$ (example: NOTION_TOKEN).".to_string())
    }
}

pub fn validate_skill_name(skill: &str) -> Result<(), String> {
    let ok = !skill.is_empty()
        && skill.len() <= 64
        && !skill.contains('.')
        && !skill.contains('/')
        && !skill.contains('\\')
        && skill
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && skill
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err("Skill name must be a single directory name under skills/.".to_string())
    }
}

fn ensure_secrets_dir(data_dir: &str) -> io::Result<PathBuf> {
    let dir = secrets_dir(data_dir);
    fs::create_dir_all(&dir)?;
    set_owner_only_dir(&dir);
    Ok(dir)
}

fn set_owner_only_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn set_owner_only_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_owner_only_dir(parent);
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    set_owner_only_file(&tmp);
    fs::rename(&tmp, path)?;
    set_owner_only_file(path);
    Ok(())
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

fn upsert_env_line(content: &str, name: &str, value: &str) -> String {
    let line = format!("{name}={}", quote_env_value(value));
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in content.lines() {
        let trimmed = raw.trim().strip_prefix("export ").unwrap_or(raw.trim());
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim() == name {
                out.push(line.clone());
                replaced = true;
                continue;
            }
        }
        out.push(raw.to_string());
    }
    if !replaced {
        out.push(line);
    }
    let mut joined = out.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn remove_env_line(content: &str, name: &str) -> String {
    let mut out = Vec::new();
    for raw in content.lines() {
        let trimmed = raw.trim().strip_prefix("export ").unwrap_or(raw.trim());
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim() == name {
                continue;
            }
        }
        out.push(raw.to_string());
    }
    let mut joined = out.join("\n");
    if !joined.is_empty() && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn load_meta(data_dir: &str, chat_id: i64) -> Vec<SecretMeta> {
    let path = meta_file(data_dir, chat_id);
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_meta(data_dir: &str, chat_id: i64, meta: &[SecretMeta]) -> io::Result<()> {
    let raw = serde_json::to_string_pretty(meta).unwrap_or_else(|_| "[]".to_string());
    atomic_write(&meta_file(data_dir, chat_id), &format!("{raw}\n"))
}

pub fn list_meta(data_dir: &str, chat_id: i64) -> Vec<SecretMeta> {
    load_meta(data_dir, chat_id)
}

pub fn load_env_pairs(data_dir: &str, chat_id: i64) -> Vec<(String, String)> {
    let Ok(content) = fs::read_to_string(vault_file(data_dir, chat_id)) else {
        return Vec::new();
    };
    crate::safety_redaction::parse_env_assignments(&content)
}

pub fn get_value(data_dir: &str, chat_id: i64, name: &str) -> Option<String> {
    load_env_pairs(data_dir, chat_id)
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
}

#[derive(Debug)]
pub struct VaultedSecret {
    pub name: String,
    pub skill: Option<String>,
    pub chat_wide: bool,
}

/// Store a value in the per-chat vault. Optionally copy it into `skills/<skill>/.env`.
pub fn upsert(
    data_dir: &str,
    skills_dir: &Path,
    chat_id: i64,
    name: &str,
    value: &str,
    kind: &str,
    skill: Option<&str>,
    persona_id: Option<i64>,
    redactor: &EnvSecretRedactor,
) -> Result<VaultedSecret, String> {
    validate_secret_name(name)?;
    if value.is_empty() || value.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err("Secret value must be a single non-empty line.".into());
    }
    if let Some(skill) = skill {
        validate_skill_name(skill)?;
        promote_to_skill(skills_dir, skill, name, value)?;
    }
    ensure_secrets_dir(data_dir).map_err(|e| format!("Failed to create secrets dir: {e}"))?;
    let path = vault_file(data_dir, chat_id);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let next = upsert_env_line(&existing, name, value);
    atomic_write(&path, &next).map_err(|e| format!("Failed to write vault: {e}"))?;

    let chat_wide = value.chars().count() >= MIN_CHAT_WIDE_SCRUB_LEN;
    let mut meta = load_meta(data_dir, chat_id);
    if let Some(row) = meta.iter_mut().find(|m| m.name == name) {
        row.kind = kind.to_string();
        row.skill = skill.map(str::to_string);
        row.scrubbed = false;
        row.chat_wide = chat_wide;
        if persona_id.is_some() {
            row.persona_id = persona_id;
        }
    } else {
        meta.push(SecretMeta {
            name: name.to_string(),
            kind: kind.to_string(),
            skill: skill.map(str::to_string),
            created_at: chrono::Utc::now().to_rfc3339(),
            scrubbed: false,
            chat_wide,
            persona_id,
        });
    }
    save_meta(data_dir, chat_id, &meta)
        .map_err(|e| format!("Failed to write vault metadata: {e}"))?;
    redactor.register_trusted(value);
    Ok(VaultedSecret {
        name: name.to_string(),
        skill: skill.map(str::to_string),
        chat_wide,
    })
}

pub fn promote_to_skill(
    skills_dir: &Path,
    skill: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    validate_skill_name(skill)?;
    validate_secret_name(name)?;
    let dir = skills_dir.join(skill);
    if !dir.is_dir() {
        return Err(format!(
            "Skill directory skills/{skill} does not exist. Omit `skill` or create that directory first."
        ));
    }
    let path = dir.join(".env");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let next = upsert_env_line(&existing, name, value);
    atomic_write(&path, &next).map_err(|e| format!("Failed to write skill .env: {e}"))?;
    Ok(())
}

pub fn forget(
    data_dir: &str,
    skills_dir: &Path,
    chat_id: i64,
    name: &str,
    also_skill: bool,
) -> Result<(), String> {
    validate_secret_name(name)?;
    let meta = load_meta(data_dir, chat_id);
    let skill = meta
        .iter()
        .find(|m| m.name == name)
        .and_then(|m| m.skill.clone());
    let path = vault_file(data_dir, chat_id);
    if path.is_file() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let next = remove_env_line(&existing, name);
        atomic_write(&path, &next).map_err(|e| format!("Failed to update vault: {e}"))?;
    }
    let kept: Vec<SecretMeta> = meta.into_iter().filter(|m| m.name != name).collect();
    save_meta(data_dir, chat_id, &kept)
        .map_err(|e| format!("Failed to update vault metadata: {e}"))?;
    if also_skill {
        if let Some(skill) = skill {
            let env_path = skills_dir.join(&skill).join(".env");
            if env_path.is_file() {
                let existing = fs::read_to_string(&env_path).unwrap_or_default();
                let next = remove_env_line(&existing, name);
                atomic_write(&env_path, &next)
                    .map_err(|e| format!("Failed to update skill .env: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn apply_chat_secret_env(cmd: &mut tokio::process::Command, data_dir: &str, chat_id: i64) {
    for (key, value) in load_env_pairs(data_dir, chat_id) {
        cmd.env(key, value);
    }
}

pub fn vault_source_lines(data_dir: &str, chat_id: i64) -> String {
    let path = vault_file(data_dir, chat_id);
    if !path.is_file() {
        return String::new();
    }
    format!(
        "set -a\n. {}\nset +a\n",
        shell_single_quote(&path.to_string_lossy())
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn placeholder_regex() -> Regex {
    Regex::new(r"\[SECRET:([A-Z][A-Z0-9_]{1,63})(?: stored in [^\]]*)?\]")
        .expect("secret placeholder regex")
}

/// Replace `[SECRET:NAME ...]` in string values. Unknown names are an error.
pub fn resolve_placeholders(data_dir: &str, chat_id: i64, input: Value) -> Result<Value, String> {
    let pairs = load_env_pairs(data_dir, chat_id);
    let re = placeholder_regex();
    resolve_value(&pairs, &re, input)
}

fn resolve_value(pairs: &[(String, String)], re: &Regex, value: Value) -> Result<Value, String> {
    match value {
        Value::String(s) => Ok(Value::String(resolve_string(pairs, re, &s)?)),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(resolve_value(pairs, re, item)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k, resolve_value(pairs, re, v)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other),
    }
}

fn resolve_string(pairs: &[(String, String)], re: &Regex, text: &str) -> Result<String, String> {
    if !text.contains("[SECRET:") {
        return Ok(text.to_string());
    }
    let mut missing: Vec<String> = Vec::new();
    let replaced = re.replace_all(text, |caps: &regex::Captures| {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some((_, value)) = pairs.iter().find(|(k, _)| k == name) {
            value.clone()
        } else {
            missing.push(name.to_string());
            caps.get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        }
    });
    if missing.is_empty() {
        Ok(replaced.into_owned())
    } else {
        let known: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        Err(format!(
            "Unknown secret placeholder(s): {}. Known names: {}.",
            missing.join(", "),
            if known.is_empty() {
                "(none)".to_string()
            } else {
                known.join(", ")
            }
        ))
    }
}

pub fn tool_input_is_sensitive(tool_name: &str) -> bool {
    tool_name == "vault_secret"
}

/// Strip secret values from a `vault_secret` tool payload before it is logged or sent to hooks.
pub fn mask_tool_input_for_log(tool_name: &str, input: &Value) -> Value {
    if !tool_input_is_sensitive(tool_name) {
        return input.clone();
    }
    mask_secret_fields(input)
}

fn mask_secret_fields(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "value" {
                    out.insert(k.clone(), Value::String("[REDACTED_SECRET]".into()));
                } else if k == "__meido_auth" {
                    out.insert(k.clone(), v.clone());
                } else {
                    out.insert(k.clone(), mask_secret_fields(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(mask_secret_fields).collect()),
        other => other.clone(),
    }
}

/// Rewrite this chat's stored messages and agent_history files for every pending vault entry.
pub fn drain_pending_scrubs(db: &Database, data_dir: &str, chat_id: i64) -> usize {
    let meta = load_meta(data_dir, chat_id);
    let mut changed = 0usize;
    let mut next = meta.clone();
    for row in &mut next {
        if row.scrubbed {
            continue;
        }
        let Some(value) = get_value(data_dir, chat_id, &row.name) else {
            row.scrubbed = true;
            continue;
        };
        if value.chars().count() < MIN_CHAT_WIDE_SCRUB_LEN {
            row.scrubbed = true;
            continue;
        }
        let replacement = placeholder_for(&row.name, chat_id);
        let token_boundary = shannon_entropy(&value) < HIGH_ENTROPY && value.len() < LONG_TOKEN_LEN;
        match db.replace_literal_in_chat_messages(chat_id, &value, &replacement, token_boundary) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    target: "redaction",
                    chat_id,
                    name = %row.name,
                    "Failed to scrub secret from messages: {e}"
                );
                continue;
            }
        }
        scrub_agent_history_files(data_dir, chat_id, &value, &replacement, token_boundary);
        row.scrubbed = true;
        changed += 1;
        tracing::info!(
            target: "redaction",
            chat_id,
            name = %row.name,
            "Scrubbed vaulted secret from chat history"
        );
    }
    if next != meta {
        let _ = save_meta(data_dir, chat_id, &next);
    }
    changed
}

/// Rewrite this chat's agent_history files for every vaulted value, including ones
/// already marked scrubbed. The PostDelivery hook runs before this turn's history
/// file is written, so the writer calls this again afterward.
pub fn scrub_vaulted_history_files(data_dir: &str, chat_id: i64) {
    for row in load_meta(data_dir, chat_id) {
        let Some(value) = get_value(data_dir, chat_id, &row.name) else {
            continue;
        };
        if value.chars().count() < MIN_CHAT_WIDE_SCRUB_LEN {
            continue;
        }
        let replacement = placeholder_for(&row.name, chat_id);
        let token_boundary = shannon_entropy(&value) < HIGH_ENTROPY && value.len() < LONG_TOKEN_LEN;
        scrub_agent_history_files(data_dir, chat_id, &value, &replacement, token_boundary);
    }
}

fn scrub_agent_history_files(
    data_dir: &str,
    chat_id: i64,
    needle: &str,
    replacement: &str,
    token_boundary: bool,
) {
    let groups = Path::new(data_dir).join("groups").join(chat_id.to_string());
    let Ok(personas) = fs::read_dir(&groups) else {
        return;
    };
    for persona in personas.flatten() {
        let dir = persona.path().join("agent_history");
        let Ok(files) = fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if !content.contains(needle) {
                continue;
            }
            let updated = replace_secret_text(&content, needle, replacement, token_boundary);
            if updated != content {
                let _ = atomic_write(&path, &updated);
            }
        }
    }
}

pub(crate) fn replace_secret_text(
    text: &str,
    needle: &str,
    replacement: &str,
    token_boundary: bool,
) -> String {
    if !token_boundary {
        return text.replace(needle, replacement);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(needle) {
        let before_ok = rest[..idx]
            .chars()
            .next_back()
            .map(|c| !is_token_char(c))
            .unwrap_or(true);
        let after_start = idx + needle.len();
        let after_ok = rest[after_start..]
            .chars()
            .next()
            .map(|c| !is_token_char(c))
            .unwrap_or(true);
        out.push_str(&rest[..idx]);
        if before_ok && after_ok {
            out.push_str(replacement);
        } else {
            out.push_str(needle);
        }
        rest = &rest[after_start..];
    }
    out.push_str(rest);
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub fn secrets_command_reply(
    data_dir: &str,
    skills_dir: &Path,
    chat_id: i64,
    text: &str,
) -> String {
    let rest = secrets_command_args(text);
    if rest.is_empty() || rest.eq_ignore_ascii_case("list") {
        return format_list(data_dir, chat_id);
    }
    let mut parts = rest.split_whitespace();
    let verb = parts.next().unwrap_or("");
    if verb.eq_ignore_ascii_case("forget") {
        let name = parts.next().unwrap_or("").to_ascii_uppercase();
        let also_skill = parts.any(|p| p == "--skill");
        if let Err(e) = validate_secret_name(&name) {
            return e;
        }
        return match forget(data_dir, skills_dir, chat_id, &name, also_skill) {
            Ok(()) => {
                if also_skill {
                    format!("Forgot {name} from this chat's vault and its skill .env.")
                } else {
                    format!(
                        "Forgot {name} from this chat's vault. The skill .env copy was left in place."
                    )
                }
            }
            Err(e) => e,
        };
    }
    "Usage: /secrets  or  /secrets forget NAME [--skill]".to_string()
}

fn secrets_command_args(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.get(1..) else {
        return String::new();
    };
    let lower = rest.to_ascii_lowercase();
    let after = if let Some(idx) = lower.find("secrets") {
        rest[idx + "secrets".len()..].trim()
    } else {
        ""
    };
    after
        .split_once('@')
        .map(|(head, tail)| {
            if head.trim().is_empty() {
                tail.trim_start_matches(|c: char| c != ' ').trim()
            } else {
                after
            }
        })
        .unwrap_or(after)
        .trim()
        .to_string()
}

fn format_list(data_dir: &str, chat_id: i64) -> String {
    let meta = list_meta(data_dir, chat_id);
    if meta.is_empty() {
        return "No vaulted secrets for this chat. Values are never shown.".to_string();
    }
    let mut lines = vec!["Vaulted secrets (names only):".to_string()];
    for row in meta {
        let skill = row.skill.unwrap_or_else(|| "-".into());
        lines.push(format!(
            "- {}  kind={}  skill={}  stored={}",
            row.name, row.kind, skill, row.created_at
        ));
    }
    lines.push(format!(
        "File: runtime/secrets/{chat_id}.env. /secrets forget NAME removes a name."
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::StoredMessage;
    use tempfile::TempDir;

    fn sample_message(chat_id: i64, content: &str) -> StoredMessage {
        StoredMessage {
            id: format!("m-{chat_id}-{content}"),
            chat_id,
            persona_id: 1,
            session_id: None,
            sender_name: "user".into(),
            content: content.to_string(),
            is_from_bot: false,
            timestamp: "2026-09-29T00:00:00Z".into(),
            origin: "interactive".into(),
            speaker_persona_id: None,
        }
    }

    #[test]
    fn upsert_writes_env_and_meta_without_values() {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("runtime");
        let skills = tmp.path().join("skills");
        fs::create_dir_all(skills.join("notion")).unwrap();
        let redactor = EnvSecretRedactor::empty();
        let data_dir = data.to_string_lossy();
        upsert(
            &data_dir,
            &skills,
            42,
            "NOTION_TOKEN",
            "supersecret12345678",
            "token",
            Some("notion"),
            Some(9),
            &redactor,
        )
        .unwrap();
        let env = fs::read_to_string(vault_file(&data_dir, 42)).unwrap();
        assert!(env.contains("NOTION_TOKEN=supersecret12345678"));
        let meta_raw = fs::read_to_string(meta_file(&data_dir, 42)).unwrap();
        assert!(!meta_raw.contains("supersecret12345678"));
        assert_eq!(list_meta(&data_dir, 42)[0].persona_id, Some(9));
        let skill_env = fs::read_to_string(skills.join("notion").join(".env")).unwrap();
        assert!(skill_env.contains("NOTION_TOKEN="));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(vault_file(&data_dir, 42))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(redactor
            .redact("leak supersecret12345678")
            .contains("[REDACTED_SECRET]"));
    }

    #[test]
    fn rejects_bad_names_and_missing_skill() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(&skills).unwrap();
        let redactor = EnvSecretRedactor::empty();
        let err = upsert(
            tmp.path().to_str().unwrap(),
            &skills,
            1,
            "notion_token",
            "supersecret12345678",
            "token",
            None,
            None,
            &redactor,
        )
        .unwrap_err();
        assert!(err.contains("Secret name"));
        let err = upsert(
            tmp.path().to_str().unwrap(),
            &skills,
            1,
            "NOTION_TOKEN",
            "supersecret12345678",
            "token",
            Some("missing"),
            None,
            &redactor,
        )
        .unwrap_err();
        assert!(err.contains("does not exist"));
        assert!(promote_to_skill(&skills, "..", "NOTION_TOKEN", "x").is_err());
    }

    #[test]
    fn placeholder_resolution_is_scoped_per_chat() {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().to_string_lossy().to_string();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(&skills).unwrap();
        let redactor = EnvSecretRedactor::empty();
        upsert(
            &data,
            &skills,
            7,
            "NOTION_TOKEN",
            "supersecret12345678",
            "token",
            None,
            None,
            &redactor,
        )
        .unwrap();
        let resolved = resolve_placeholders(
            &data,
            7,
            serde_json::json!({"header": "Bearer [SECRET:NOTION_TOKEN stored in runtime/secrets/7.env]"}),
        )
        .unwrap();
        assert_eq!(resolved["header"], "Bearer supersecret12345678");
        let err =
            resolve_placeholders(&data, 7, serde_json::json!("[SECRET:OTHER_TOKEN]")).unwrap_err();
        assert!(err.contains("OTHER_TOKEN"));
        let other =
            resolve_placeholders(&data, 8, serde_json::json!("[SECRET:NOTION_TOKEN]")).unwrap_err();
        assert!(other.contains("NOTION_TOKEN"));
    }

    #[test]
    fn mask_hides_vault_secret_values() {
        let input = serde_json::json!({"secrets": [{"name": "A_B", "value": "hunter2-blue"}]});
        let masked = mask_tool_input_for_log("vault_secret", &input);
        assert!(!masked.to_string().contains("hunter2"));
        assert!(masked.to_string().contains("A_B"));
        let plain = mask_tool_input_for_log("bash", &serde_json::json!({"command": "echo hi"}));
        assert_eq!(plain["command"], "echo hi");
    }

    #[test]
    fn drain_rewrites_messages_and_leaves_other_chats() {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("runtime");
        fs::create_dir_all(&data).unwrap();
        let db = Database::new(data.to_str().unwrap()).unwrap();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(&skills).unwrap();
        let redactor = EnvSecretRedactor::empty();
        let data_dir = data.to_string_lossy().to_string();
        let secret = "supersecret12345678";
        upsert(
            &data_dir,
            &skills,
            3,
            "NOTION_TOKEN",
            secret,
            "token",
            None,
            None,
            &redactor,
        )
        .unwrap();
        db.store_message(&sample_message(3, &format!("token is {secret}")))
            .unwrap();
        db.store_message(&sample_message(9, &format!("keep {secret}")))
            .unwrap();
        let history = data
            .join("groups")
            .join("3")
            .join("1")
            .join("agent_history");
        fs::create_dir_all(&history).unwrap();
        fs::write(
            history.join("20260929-000000.md"),
            format!("saw {secret}\n"),
        )
        .unwrap();
        let n = drain_pending_scrubs(&db, &data_dir, 3);
        assert_eq!(n, 1);
        let msgs = db.get_recent_messages(3, 1, 10, false).unwrap();
        assert!(msgs[0].content.contains("[SECRET:NOTION_TOKEN"));
        assert!(!msgs[0].content.contains(secret));
        let other = db.get_recent_messages(9, 1, 10, false).unwrap();
        assert!(other[0].content.contains(secret));
        let hist = fs::read_to_string(history.join("20260929-000000.md")).unwrap();
        assert!(!hist.contains(secret));
        assert!(list_meta(&data_dir, 3)[0].scrubbed);
    }

    #[test]
    fn short_values_are_not_rewritten_chat_wide() {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("runtime");
        fs::create_dir_all(&data).unwrap();
        let db = Database::new(data.to_str().unwrap()).unwrap();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(&skills).unwrap();
        let data_dir = data.to_string_lossy().to_string();
        upsert(
            &data_dir,
            &skills,
            3,
            "PIN_CODE",
            "1234",
            "password",
            None,
            None,
            &EnvSecretRedactor::empty(),
        )
        .unwrap();
        db.store_message(&sample_message(3, "pin 1234")).unwrap();
        assert_eq!(drain_pending_scrubs(&db, &data_dir, 3), 0);
        let msgs = db.get_recent_messages(3, 1, 10, false).unwrap();
        assert!(msgs[0].content.contains("1234"));
    }

    #[test]
    fn low_entropy_token_does_not_eat_longer_words() {
        let updated = replace_secret_text(
            "the password is password and password1 stays",
            "password",
            "[SECRET:PW]",
            true,
        );
        assert_eq!(
            updated,
            "the [SECRET:PW] is [SECRET:PW] and password1 stays"
        );
    }
}
