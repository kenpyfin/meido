//! Gateway API connections that are not in `.env`.
//!
//! Values live in `runtime/secrets/connections.env` (owner-only). `.env` wins when
//! the same variable is set. Responses never include the stored value.

use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Secret,
    Url,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionDef {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ConnectionKind,
    pub secret_name: Option<&'static str>,
    pub provider_id: Option<&'static str>,
    pub featured: bool,
    pub used_by: &'static str,
}

pub const CONNECTIONS: &[ConnectionDef] = &[
    ConnectionDef {
        id: "local_server",
        label: "Local model server",
        kind: ConnectionKind::Url,
        secret_name: None,
        provider_id: None,
        featured: true,
        used_by: "Topic titles and Classic · Cost routing",
    },
    ConnectionDef {
        id: "anthropic",
        label: "Anthropic",
        kind: ConnectionKind::Secret,
        secret_name: Some("ANTHROPIC_API_KEY"),
        provider_id: Some("anthropic"),
        featured: true,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "openai",
        label: "OpenAI",
        kind: ConnectionKind::Secret,
        secret_name: Some("OPENAI_API_KEY"),
        provider_id: Some("openai"),
        featured: true,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "google",
        label: "Google (Gemini)",
        kind: ConnectionKind::Secret,
        secret_name: Some("GEMINI_API_KEY"),
        provider_id: Some("google"),
        featured: true,
        used_by: "Chat model and Gemini ADK",
    },
    ConnectionDef {
        id: "xai",
        label: "xAI",
        kind: ConnectionKind::Secret,
        secret_name: Some("XAI_API_KEY"),
        provider_id: Some("xai"),
        featured: true,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "cursor",
        label: "Cursor",
        kind: ConnectionKind::Secret,
        secret_name: Some("CURSOR_API_KEY"),
        provider_id: None,
        featured: true,
        used_by: "Cursor engine",
    },
    ConnectionDef {
        id: "perplexity",
        label: "Perplexity",
        kind: ConnectionKind::Secret,
        secret_name: Some("PERPLEXITY_API_KEY"),
        provider_id: None,
        featured: true,
        used_by: "Quality checks when no local model is configured",
    },
    ConnectionDef {
        id: "openrouter",
        label: "OpenRouter",
        kind: ConnectionKind::Secret,
        secret_name: Some("OPENROUTER_API_KEY"),
        provider_id: Some("openrouter"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "deepseek",
        label: "DeepSeek",
        kind: ConnectionKind::Secret,
        secret_name: Some("DEEPSEEK_API_KEY"),
        provider_id: Some("deepseek"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "mistral",
        label: "Mistral",
        kind: ConnectionKind::Secret,
        secret_name: Some("MISTRAL_API_KEY"),
        provider_id: Some("mistral"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "cohere",
        label: "Cohere",
        kind: ConnectionKind::Secret,
        secret_name: Some("COHERE_API_KEY"),
        provider_id: Some("cohere"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "huggingface",
        label: "Hugging Face",
        kind: ConnectionKind::Secret,
        secret_name: Some("HF_TOKEN"),
        provider_id: Some("huggingface"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "together",
        label: "Together AI",
        kind: ConnectionKind::Secret,
        secret_name: Some("TOGETHER_API_KEY"),
        provider_id: Some("together"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "moonshot",
        label: "Moonshot",
        kind: ConnectionKind::Secret,
        secret_name: Some("MOONSHOT_API_KEY"),
        provider_id: Some("moonshot"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "alibaba",
        label: "Alibaba (DashScope)",
        kind: ConnectionKind::Secret,
        secret_name: Some("DASHSCOPE_API_KEY"),
        provider_id: Some("alibaba"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "minimax",
        label: "MiniMax",
        kind: ConnectionKind::Secret,
        secret_name: Some("MINIMAX_API_KEY"),
        provider_id: Some("minimax"),
        featured: false,
        used_by: "Single turn chat model",
    },
    ConnectionDef {
        id: "zhipu",
        label: "Zhipu",
        kind: ConnectionKind::Secret,
        secret_name: Some("ZHIPU_API_KEY"),
        provider_id: Some("zhipu"),
        featured: false,
        used_by: "Single turn chat model",
    },
];

pub fn find_connection(id: &str) -> Option<&'static ConnectionDef> {
    CONNECTIONS.iter().find(|c| c.id == id)
}

pub fn connections_file(data_dir: &str) -> std::path::PathBuf {
    Path::new(data_dir).join("secrets").join("connections.env")
}

pub fn saved_secret(data_dir: &str, name: &str) -> Option<String> {
    let content = fs::read_to_string(connections_file(data_dir)).ok()?;
    crate::safety_redaction::parse_env_assignments(&content)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .filter(|value| !value.trim().is_empty())
}

pub fn secret_is_saved(data_dir: &str, name: &str) -> bool {
    saved_secret(data_dir, name).is_some()
}

pub fn write_secret(data_dir: &str, name: &str, value: &str) -> Result<(), String> {
    crate::secret_vault::validate_secret_name(name)?;
    let value = value.trim();
    if value.is_empty() || value.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err("Enter a single-line API key.".into());
    }
    if value.chars().count() > 512 {
        return Err("API key is too long.".into());
    }
    let path = connections_file(data_dir);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let next = upsert_line(&existing, name, value);
    atomic_write(&path, &next)
}

pub fn clear_secret(data_dir: &str, name: &str) -> Result<(), String> {
    crate::secret_vault::validate_secret_name(name)?;
    let path = connections_file(data_dir);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let next = remove_line(&existing, name);
    if next.trim().is_empty() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Could not remove saved key: {e}"))?;
        }
        return Ok(());
    }
    atomic_write(&path, &next)
}

fn upsert_line(content: &str, name: &str, value: &str) -> String {
    let line = format!("{name}={}", quote_value(value));
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in content.lines() {
        let trimmed = raw.trim().strip_prefix("export ").unwrap_or(raw.trim());
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim() == name {
                if !replaced {
                    out.push(line.clone());
                    replaced = true;
                }
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

fn remove_line(content: &str, name: &str) -> String {
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
    if out.is_empty() {
        String::new()
    } else {
        let mut joined = out.join("\n");
        if !joined.ends_with('\n') {
            joined.push('\n');
        }
        joined
    }
}

fn quote_value(value: &str) -> String {
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

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create secrets directory: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file =
            fs::File::create(&tmp).map_err(|e| format!("Could not write saved key: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Could not write saved key: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Could not write saved key: {e}"))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path).map_err(|e| format!("Could not write saved key: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_secret_round_trip_does_not_keep_cleared_values() {
        let dir = std::env::temp_dir().join(format!("favb-connections-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_secret(dir.to_str().unwrap(), "GEMINI_API_KEY", "test-key-value").unwrap();
        assert_eq!(
            saved_secret(dir.to_str().unwrap(), "GEMINI_API_KEY").as_deref(),
            Some("test-key-value")
        );
        clear_secret(dir.to_str().unwrap(), "GEMINI_API_KEY").unwrap();
        assert!(saved_secret(dir.to_str().unwrap(), "GEMINI_API_KEY").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
