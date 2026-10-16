//! Literal secret redaction from env-like files, plus strict known-format patterns for disk sinks.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tracing::info;

use crate::builtin_skills;
use crate::config::Config;
use crate::tools::path_guard::is_env_like_name;

pub const REDACTED: &str = "[REDACTED_SECRET]";

const DEFAULT_MIN_VALUE_LEN: usize = 8;
const MAX_WALK_DEPTH: usize = 12;
const MAX_ENV_FILES: usize = 200;
const MAX_NEEDLES: usize = 500;
const MAX_ENV_FILE_BYTES: u64 = 256 * 1024;

const SKIP_DIR_NAMES: &[&str] = &[".git", "node_modules", "target", "runtime"];

/// Config / operational keys whose values are not credentials (exact match, uppercase).
const NON_SECRET_ENV_KEYS: &[&str] = &[
    "WORKSPACE_DIR",
    "FINALLY_A_VALUE_BOT_WORKSPACE_DIR",
    "FINALLY_A_VALUE_BOT_CONFIG",
    "FINALLY_A_VALUE_BOT_BUILTIN_SKILLS",
    "FINALLY_A_VALUE_BOT_BUILTIN_HOOKS",
    "TIMEZONE",
    "WEB_HOST",
    "WEB_PORT",
    "WEB_ENABLED",
    "BOT_USERNAME",
    "AGENT_DISPLAY_NAME",
    "UNIVERSAL_CHAT_ID",
    "LLM_BASE_URL",
    "EVALUATOR_BASE_URL",
    "SEARXNG_URL",
    "SOCIAL_BASE_URL",
    "CURSOR_AGENT_RUNNER_URL",
    "VAULT_EMBEDDING_SERVER_URL",
    "VAULT_VECTOR_DB_URL",
    "VAULT_ORIGIN_VAULT_PATH",
    "VAULT_VECTOR_DB_PATH",
    "VAULT_ORIGIN_VAULT_REPO",
    "VAULT_GIT_URL",
    "VAULT_PRINCIPLES_PATH",
    "VAULT_SEARCH_COMMAND",
    "VAULT_INDEX_COMMAND",
    "VAULT_VECTOR_DB_COLLECTION",
    "CURSOR_AGENT_CLI_PATH",
    "BROWSER_EXECUTABLE_PATH",
    "POST_EDIT_VALIDATION_COMMANDS",
    "GIT_USERNAME",
    "WHATSAPP_PHONE_NUMBER_ID",
    "DISCORD_ALLOWED_CHANNELS",
    "WECOM_CORP_ID",
    "WECOM_AGENT_ID",
    "WECOM_WEBHOOK_PORT",
    "WECOM_ALLOWED_CHATS",
    "WECOM_BOT_ID",
    "WECOM_MODE",
    "QUALITY_EVAL_CHANNELS",
    "ORCHESTRATOR_MODEL",
    "CURSOR_AGENT_MODEL",
    "HOOK_PROMPT_MODEL",
    "EVALUATOR_MODEL",
    "RESPONSE_QUALITY_EVALUATOR_MODEL",
    "POST_TOOL_EVALUATOR_MODEL",
    "RUNTIME_RELIABILITY_PROFILE",
    "PROJECT_AUTO_ASSOCIATION_STRICTNESS",
    "SAFETY_OUTPUT_GUARD_MODE",
    "SAFETY_EXECUTION_MODE",
    "SAFETY_RISKY_CATEGORIES",
    "MAX_TOKENS",
    "MAX_TOOL_ITERATIONS",
    "MAX_HISTORY_MESSAGES",
    "MAX_DOCUMENT_SIZE_MB",
    "WEB_MAX_INFLIGHT_PER_SESSION",
    "WEB_MAX_REQUESTS_PER_WINDOW",
    "WEB_RATE_WINDOW_SECONDS",
    "WEB_RUN_HISTORY_LIMIT",
    "WEB_SESSION_IDLE_TTL_SECONDS",
    "SCHEDULER_TASK_TIMEOUT_SECS",
    "SCHEDULER_STALE_RUNNING_RECLAIM_SECS",
    "SCHEDULER_MAX_CONCURRENT_TASKS",
    "SCHEDULER_POLL_INTERVAL_SECS",
    "BACKGROUND_JOB_LEASE_TTL_SECS",
    "BACKGROUND_JOB_LEASE_FALLBACK_RENEW_SECS",
    "BACKGROUND_JOB_PENDING_START_TIMEOUT_SECS",
    "BACKGROUND_SHELL_MONITOR_POLL_SECS",
    "BACKGROUND_SHELL_TMUX_SESSION_PREFIX",
    "CURSOR_AGENT_TMUX_SESSION_PREFIX",
    "CURSOR_AGENT_TIMEOUT_SECS",
    "BROWSER_CDP_PORT_BASE",
    "BROWSER_HEADLESS",
    "BROWSER_MANAGED",
    "HOOK_COMMAND_TIMEOUT_SECS",
    "HOOK_PROMPT_TIMEOUT_SECS",
    "QUALITY_EVAL_MAX_NUDGES_PER_RUN",
    "QUALITY_EVAL_MIN_CONFIDENCE",
    "SAFETY_MAX_EMOJIS_PER_RESPONSE",
    "SAFETY_TAIL_REPEAT_LIMIT",
    "ENV_REDACT_MIN_VALUE_LEN",
    "RECENT_HISTORY_MIN_USER_MESSAGES",
    "RECENT_HISTORY_MIN_ASSISTANT_MESSAGES",
    "SHOW_THINKING",
    "TOOL_OUTPUT_DEBUG",
    "ORCHESTRATOR_ENABLED",
    "POST_TOOL_EVALUATOR_ENABLED",
    "RESPONSE_QUALITY_EVALUATOR_ENABLED",
    "BACKGROUND_SHELL_TMUX_ENABLED",
    "CURSOR_AGENT_TMUX_ENABLED",
    "BACKGROUND_SHELL_AUTO_RETRY_ON_FAILURE",
    "BACKGROUND_SHELL_AUTO_RETRY_MAX",
    "BACKGROUND_SHELL_AUTO_AGENT_ON_SUCCESS",
];

const PLACEHOLDER_VALUES: &[&str] = &[
    "changeme",
    "change_me",
    "your_api_key",
    "your_api_key_here",
    "your-api-key",
    "insert_key_here",
    "replace_me",
    "placeholder",
    "xxx",
    "todo",
    "fixme",
    "example",
    "dummy",
    "test",
    "secret",
    "password",
];

const MIN_NEEDLE_ENTROPY: f64 = 3.0;

/// Redacts literal values parsed from env-like files. Needles can be added after startup.
#[derive(Debug, Clone)]
pub struct EnvSecretRedactor {
    needles: Arc<RwLock<Vec<String>>>,
}

impl EnvSecretRedactor {
    pub fn empty() -> Self {
        Self {
            needles: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn discover(config: &Config) -> Self {
        let min_value_len = std::env::var("ENV_REDACT_MIN_VALUE_LEN")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .filter(|&n| (4..=128).contains(&n))
            .unwrap_or(DEFAULT_MIN_VALUE_LEN);

        let mut allowed_roots: Vec<PathBuf> = vec![config.workspace_root_absolute()];
        if let Ok(Some(config_env)) = Config::resolve_config_path() {
            if let Some(parent) = config_env.parent() {
                allowed_roots.push(parent.to_path_buf());
            }
            allowed_roots.push(config_env);
        }
        if let Some(builtin) = builtin_skills::resolve_builtin_skills_dir(config) {
            allowed_roots.push(builtin);
        }
        allowed_roots.sort();
        allowed_roots.dedup();

        let mut env_files: Vec<PathBuf> = Vec::new();
        let mut files_scanned = 0usize;

        if let Ok(Some(config_env)) = Config::resolve_config_path() {
            if config_env.is_file() && should_load_env_file(&config_env) {
                push_env_file(&mut env_files, config_env, &mut files_scanned);
            }
        }

        for root in &allowed_roots {
            if root.is_dir() {
                walk_for_env_files(root, &allowed_roots, 0, &mut env_files, &mut files_scanned);
            }
        }
        // `runtime` is skipped by the workspace walk, so load the per-chat vault explicitly.
        let secrets_dir = PathBuf::from(config.runtime_data_dir()).join("secrets");
        if secrets_dir.is_dir() {
            walk_for_env_files(
                &secrets_dir,
                &allowed_roots,
                0,
                &mut env_files,
                &mut files_scanned,
            );
        }

        let mut value_set: HashSet<String> = HashSet::new();
        for path in &env_files {
            if value_set.len() >= MAX_NEEDLES {
                break;
            }
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    for (key, value) in parse_env_content(&content) {
                        if value_set.len() >= MAX_NEEDLES {
                            break;
                        }
                        if !should_redact_env_key(&key) {
                            continue;
                        }
                        if should_redact_value(&value, min_value_len) {
                            value_set.insert(value);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "redaction",
                        path = %path.display(),
                        "Failed to read env file for redaction catalog: {e}"
                    );
                }
            }
        }

        let mut needles: Vec<String> = value_set.into_iter().collect();
        expand_url_encoded_needles(&mut needles);
        needles.sort_by_key(|s| std::cmp::Reverse(s.len()));
        needles.dedup();

        info!(
            target: "redaction",
            env_files = env_files.len(),
            needles = needles.len(),
            min_value_len = min_value_len,
            "Env-only secret redaction catalog built"
        );

        Self {
            needles: Arc::new(RwLock::new(needles)),
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let needles = self.needles_snapshot();
        redact_with_needles(text, &needles)
    }

    /// Needle redaction plus strict vendor key formats. Disk sinks only.
    pub fn redact_with_patterns(&self, text: &str) -> String {
        redact_known_key_formats(&self.redact(text))
    }

    /// Register a value the agent already judged sensitive. Low-entropy and short
    /// values are skipped so ordinary words do not become global needles.
    pub fn register_trusted(&self, value: &str) {
        let trimmed = value.trim();
        if trimmed.len() < DEFAULT_MIN_VALUE_LEN || is_non_secret_config_value(trimmed) {
            return;
        }
        if shannon_entropy(trimmed) < MIN_NEEDLE_ENTROPY {
            return;
        }
        self.insert_needle(trimmed);
    }

    /// Re-read an env-like file under `skills/` and register credential values.
    pub fn register_skill_env_file(&self, path: &Path) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !is_env_like_name(name) {
            return;
        }
        let under_skills = path
            .components()
            .any(|c| matches!(c, std::path::Component::Normal(part) if part == "skills"));
        if !under_skills {
            return;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let min_value_len = min_value_len();
        for (key, value) in parse_env_content(&content) {
            if should_redact_env_key(&key) && should_redact_value(&value, min_value_len) {
                self.insert_needle(&value);
            }
        }
    }

    fn insert_needle(&self, value: &str) {
        let mut needles = self.needles.write().unwrap_or_else(|e| e.into_inner());
        if needles.iter().any(|n| n == value) || needles.len() >= MAX_NEEDLES {
            return;
        }
        needles.push(value.to_string());
        let encoded = urlencoding::encode(value);
        if encoded != value && !needles.iter().any(|n| n == encoded.as_ref()) {
            needles.push(encoded.into_owned());
        }
        needles.sort_by_key(|s| std::cmp::Reverse(s.len()));
        needles.dedup();
    }

    fn needles_snapshot(&self) -> Vec<String> {
        self.needles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    #[cfg(test)]
    pub fn from_needles(needles: Vec<String>) -> Self {
        let mut needles = needles;
        needles.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Self {
            needles: Arc::new(RwLock::new(needles)),
        }
    }
}

fn redact_with_needles(text: &str, needles: &[String]) -> String {
    if needles.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        if out.contains(needle) {
            out = out.replace(needle, REDACTED);
        }
    }
    out
}

fn min_value_len() -> usize {
    std::env::var("ENV_REDACT_MIN_VALUE_LEN")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&n| (4..=128).contains(&n))
        .unwrap_or(DEFAULT_MIN_VALUE_LEN)
}

pub(crate) fn parse_env_assignments(content: &str) -> Vec<(String, String)> {
    parse_env_content(content)
}

/// Shannon entropy in bits per character.
pub fn shannon_entropy(value: &str) -> f64 {
    let n = value.chars().count();
    if n == 0 {
        return 0.0;
    }
    let mut counts = std::collections::HashMap::<char, usize>::new();
    for c in value.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let n = n as f64;
    counts
        .values()
        .map(|count| {
            let p = *count as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn redact_known_key_formats(text: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            r"(?<![A-Za-z0-9_-])sk-ant-[A-Za-z0-9_-]{20,}",
            r"(?<![A-Za-z0-9_-])sk-[A-Za-z0-9]{20,}",
            r"(?<![A-Za-z0-9_-])AIza[0-9A-Za-z_-]{30,}",
            r"(?<![A-Za-z0-9_-])ghp_[A-Za-z0-9]{20,}",
            r"(?<![A-Za-z0-9_-])github_pat_[A-Za-z0-9_]{20,}",
            r"(?<![A-Za-z0-9_-])xox[baprs]-[A-Za-z0-9-]{10,}",
            r"(?<![A-Za-z0-9_-])crsr_[A-Za-z0-9]{16,}",
            r"(?<![A-Za-z0-9])[0-9]{8,10}:[A-Za-z0-9_-]{30,}",
        ]
        .into_iter()
        .filter_map(|pat| regex::Regex::new(pat).ok())
        .collect()
    });
    let mut out = text.to_string();
    for re in res {
        out = re.replace_all(&out, REDACTED).into_owned();
    }
    out
}

fn should_load_env_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| is_env_like_name(name) && !is_sample_or_example_env(name))
}

fn is_sample_or_example_env(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.contains(".example") || lower.contains(".sample")
}

fn push_env_file(files: &mut Vec<PathBuf>, path: PathBuf, count: &mut usize) {
    if *count >= MAX_ENV_FILES {
        return;
    }
    if files.iter().any(|p| p == &path) {
        return;
    }
    files.push(path);
    *count += 1;
}

fn path_under_allowed_roots(path: &Path, allowed_roots: &[PathBuf]) -> bool {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    allowed_roots.iter().any(|root| {
        let root_resolved = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        resolved.starts_with(&root_resolved)
    })
}

fn walk_for_env_files(
    dir: &Path,
    allowed_roots: &[PathBuf],
    depth: usize,
    out: &mut Vec<PathBuf>,
    files_scanned: &mut usize,
) {
    if depth > MAX_WALK_DEPTH || *files_scanned >= MAX_ENV_FILES {
        return;
    }
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        if *files_scanned >= MAX_ENV_FILES {
            return;
        }
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if path.is_dir() {
            if SKIP_DIR_NAMES.iter().any(|s| name == *s) {
                continue;
            }
            walk_for_env_files(&path, allowed_roots, depth + 1, out, files_scanned);
        } else if path.is_file() {
            if !is_env_like_name(&name) || is_sample_or_example_env(&name) {
                continue;
            }
            if std::fs::metadata(&path)
                .map(|m| m.len() <= MAX_ENV_FILE_BYTES)
                .unwrap_or(false)
                && path_under_allowed_roots(&path, allowed_roots)
            {
                push_env_file(out, path, files_scanned);
            }
        }
    }
}

fn parse_env_content(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        let value = parse_env_value(raw_value.trim());
        if !value.is_empty() {
            pairs.push((key, value));
        }
    }
    pairs
}

fn parse_env_value(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
    {
        let inner = &raw[1..raw.len().saturating_sub(1)];
        return inner.to_string();
    }
    raw.to_string()
}

/// True when an env key names a credential-like setting (not paths, models, limits, etc.).
fn should_redact_env_key(key: &str) -> bool {
    let k = key.trim().to_ascii_uppercase();
    if k.is_empty() || NON_SECRET_ENV_KEYS.contains(&k.as_str()) {
        return false;
    }
    if k.ends_with("_CLIENT_ID") {
        return false;
    }
    if k.contains("SECRET")
        || k.contains("PASSWORD")
        || k.contains("PASSWD")
        || k.contains("TOKEN")
        || k.contains("API_KEY")
        || k.ends_with("_KEY")
        || k.contains("PRIVATE_KEY")
        || k.contains("CREDENTIAL")
        || k.contains("DATABASE_URL")
        || k.contains("USERHASH")
        || k == "DSN"
        || k.ends_with("_DSN")
    {
        return true;
    }
    false
}

fn should_redact_value(value: &str, min_len: usize) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < min_len {
        return false;
    }
    if is_non_secret_config_value(trimmed) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if PLACEHOLDER_VALUES.iter().any(|p| lower == *p) {
        return false;
    }
    if lower.starts_with("your_") || lower.starts_with("replace_") || lower.contains("example.com")
    {
        return false;
    }
    if shannon_entropy(trimmed) < MIN_NEEDLE_ENTROPY {
        return false;
    }
    true
}

fn is_non_secret_config_value(value: &str) -> bool {
    if value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('/')
        || value.starts_with("~/")
    {
        return true;
    }
    if value.len() >= 3 {
        let bytes = value.as_bytes();
        if bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
    }
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "on" | "off" | "yes" | "no"
    ) {
        return true;
    }
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if (value.starts_with("http://") || value.starts_with("https://")) && !value.contains('@') {
        return true;
    }
    false
}

fn expand_url_encoded_needles(needles: &mut Vec<String>) {
    let mut extra = Vec::new();
    for needle in needles.iter() {
        let encoded = urlencoding::encode(needle);
        if encoded != needle.as_str() {
            extra.push(encoded.into_owned());
        }
    }
    needles.extend(extra);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_env(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn redacts_literal_env_value() {
        let redactor = EnvSecretRedactor::from_needles(vec!["supersecret12345678".to_string()]);
        let out = redactor.redact("token=supersecret12345678 done");
        assert!(!out.contains("supersecret"));
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn preserves_linkedin_url_when_not_in_env() {
        let redactor = EnvSecretRedactor::empty();
        let url = "https://www.linkedin.com/jobs/view/4123789234567890123";
        assert_eq!(redactor.redact(url), url);
    }

    #[test]
    fn preserves_token_assignment_without_env_value() {
        let redactor = EnvSecretRedactor::empty();
        let input = "token=not_a_real_secret_value_abcdef";
        assert_eq!(redactor.redact(input), input);
    }

    #[test]
    fn preserves_long_filename_without_env_value() {
        let redactor = EnvSecretRedactor::empty();
        let name = "Capital_One_Senior_PM_Resume_v2_ABC987xyzABCDEFGHIJ_KLMNOP";
        let input = format!("Generated {name}.pdf — review when ready.");
        assert_eq!(redactor.redact(&input), input);
    }

    #[test]
    fn longest_needle_first_avoids_partial_leak() {
        let redactor = EnvSecretRedactor::from_needles(vec![
            "shortsec12".to_string(),
            "shortsec12345678".to_string(),
        ]);
        let out = redactor.redact("value=shortsec12345678");
        assert!(!out.contains("shortsec12"));
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn discover_loads_env_like_file() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join("skills").join("foo");
        fs::create_dir_all(&skills).unwrap();
        write_env(&skills, ".env", "API_KEY=supersecret12345678\nSHORT=abc\n");

        let mut config = crate::config::test_config();
        config.workspace_dir = tmp.path().to_string_lossy().to_string();

        let redactor = EnvSecretRedactor::discover(&config);
        let out = redactor.redact("leak supersecret12345678 end");
        assert!(out.contains(REDACTED));
        assert!(!out.contains("supersecret12345678"));
        assert!(redactor.redact("SHORT=abc").contains("abc"));
    }

    #[test]
    fn discover_skips_non_secret_config_keys() {
        let tmp = TempDir::new().unwrap();
        write_env(
            tmp.path(),
            ".env",
            "WORKSPACE_DIR=/home/operator/projects/bot/workspace\nTELEGRAM_BOT_TOKEN=telegramsecret123456\n",
        );

        let mut config = crate::config::test_config();
        config.workspace_dir = tmp.path().to_string_lossy().to_string();

        let redactor = EnvSecretRedactor::discover(&config);
        let path_msg = "Successfully wrote to /home/operator/projects/bot/workspace/shared/personas/1/2/output.md";
        assert_eq!(redactor.redact(path_msg), path_msg);
        assert!(redactor
            .redact("token telegramsecret123456")
            .contains(REDACTED));
    }

    #[test]
    fn secret_env_key_detection() {
        assert!(should_redact_env_key("TELEGRAM_BOT_TOKEN"));
        assert!(should_redact_env_key("OPENAI_API_KEY"));
        assert!(should_redact_env_key("SOCIAL_TIKTOK_CLIENT_SECRET"));
        assert!(should_redact_env_key("CATBOX_USERHASH"));
        assert!(should_redact_env_key("CURSOR_API_KEY"));
        assert!(!should_redact_env_key("WORKSPACE_DIR"));
        assert!(!should_redact_env_key("SOCIAL_TIKTOK_CLIENT_ID"));
        assert!(!should_redact_env_key("ORCHESTRATOR_MODEL"));
    }

    #[test]
    fn register_trusted_redacts_immediately_and_skips_words() {
        let redactor = EnvSecretRedactor::empty();
        redactor.register_trusted("supersecret12345678");
        assert!(redactor
            .redact("leak supersecret12345678")
            .contains(REDACTED));
        redactor.register_trusted("password");
        assert_eq!(redactor.redact("password"), "password");
    }

    #[test]
    fn pattern_redaction_is_separate_from_chat_redaction() {
        let redactor = EnvSecretRedactor::empty();
        let key = "sk-abcdefghijklmnopqrstuvwxyz";
        let input = format!("key {key} and task-and-risk");
        let disk = redactor.redact_with_patterns(&input);
        assert!(!disk.contains(key));
        assert!(disk.contains("task-and-risk"));
        assert!(redactor.redact(&input).contains(key));
    }

    #[test]
    fn skips_path_and_url_config_values() {
        assert!(is_non_secret_config_value("./workspace"));
        assert!(is_non_secret_config_value("/var/data/workspace"));
        assert!(is_non_secret_config_value("https://api.example.com/v1"));
        assert!(!is_non_secret_config_value("supersecret12345678"));
    }

    #[test]
    fn parse_env_handles_quotes_and_export() {
        let pairs = parse_env_content(
            r#"
# comment
export FOO="quoted value here"
BAR=plain
"#,
        );
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].1, "quoted value here");
        assert_eq!(pairs[1].1, "plain");
    }
}
