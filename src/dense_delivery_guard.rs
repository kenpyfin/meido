//! Persona-gated dense delivery: spill over-limit replies to PDF + public URL.
//!
//! v1 is a **length gate only**. Research/tables under the cap stay LLM/SOP until a later
//! always-spill policy. Runs after PDQE so quality eval sees the full assistant text.
//! The chat reply is a natural LLM summary; the PDF is the full original (minus env
//! secrets) uploaded to a public HTTPS host — never offered as a local/internal path.

use std::ffi::OsStr;
use std::net::IpAddr;
use std::path::Path;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{info, warn};

use crate::claude::{Message, MessageContent, ResponseContentBlock};
use crate::config::Config;
use crate::db::{Database, Persona};
use crate::llm;
use crate::safety_redaction::EnvSecretRedactor;

pub const DEFAULT_MESSAGING_MAX_CHARS: usize = 2000;
pub const DEFAULT_WEB_MAX_CHARS: usize = 1000;
pub const DEFAULT_SUMMARY_CHARS: usize = 1200;

const CATBOX_UPLOAD_URL: &str = "https://catbox.moe/user/api.php";
const LITTERBOX_UPLOAD_URL: &str = "https://litterbox.catbox.moe/resources/internals/api.php";
const TMPFILES_UPLOAD_URL: &str = "https://tmpfiles.org/api/v1/upload";
const PIXELDRAIN_UPLOAD_URL: &str = "https://pixeldrain.com/api/file";
const PDF_RENDER_TIMEOUT_SECS: u64 = 60;
const UPLOAD_TIMEOUT_SECS: u64 = 45;
const UPLOAD_ATTEMPTS: u32 = 2;
const SUMMARY_LLM_TIMEOUT_SECS: u64 = 45;
const SUMMARY_SOURCE_MAX_CHARS: usize = 12_000;
const SUMMARY_MAX_TOKENS: u32 = 700;
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[async_trait]
pub trait DeliveryUploader: Send + Sync {
    async fn upload_file(&self, path: &Path) -> Result<String, String>;
}

#[async_trait]
pub trait PdfRenderer: Send + Sync {
    async fn render_pdf(
        &self,
        persona_cwd: &Path,
        md_path: &Path,
        pdf_path: &Path,
    ) -> Result<(), String>;
}

#[async_trait]
pub trait DeliverySummarizer: Send + Sync {
    async fn summarize(
        &self,
        full_text: &str,
        summary_chars: usize,
        public_url: Option<&str>,
    ) -> Result<String, String>;
}

pub struct LlmDeliverySummarizer<'a> {
    config: &'a Config,
    env_redactor: &'a EnvSecretRedactor,
}

impl<'a> LlmDeliverySummarizer<'a> {
    pub fn new(config: &'a Config, env_redactor: &'a EnvSecretRedactor) -> Self {
        Self {
            config,
            env_redactor,
        }
    }
}

#[async_trait]
impl DeliverySummarizer for LlmDeliverySummarizer<'_> {
    async fn summarize(
        &self,
        full_text: &str,
        summary_chars: usize,
        _public_url: Option<&str>,
    ) -> Result<String, String> {
        summarize_with_llm(self.config, self.env_redactor, full_text, summary_chars).await
    }
}

pub struct CatboxUploader {
    client: reqwest::Client,
}

fn public_upload_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(UPLOAD_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent(BROWSER_USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl CatboxUploader {
    pub fn new() -> Self {
        Self {
            client: public_upload_http_client(),
        }
    }
}

impl Default for CatboxUploader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeliveryUploader for CatboxUploader {
    async fn upload_file(&self, path: &Path) -> Result<String, String> {
        let (name, mime, bytes) = read_upload_bytes(path).await?;
        upload_catbox(&self.client, &name, mime, bytes).await
    }
}

pub struct NoopUploader;

#[async_trait]
impl DeliveryUploader for NoopUploader {
    async fn upload_file(&self, _path: &Path) -> Result<String, String> {
        Err("DELIVERY_UPLOAD_PROVIDER=none".into())
    }
}

/// Tries several anonymous public hosts so a single Cloudflare/403 cannot drop the link.
pub struct PublicHostUploader {
    client: reqwest::Client,
}

impl PublicHostUploader {
    pub fn new() -> Self {
        Self {
            client: public_upload_http_client(),
        }
    }
}

impl Default for PublicHostUploader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeliveryUploader for PublicHostUploader {
    async fn upload_file(&self, path: &Path) -> Result<String, String> {
        let (name, mime, bytes) = read_upload_bytes(path).await?;
        let mut errors: Vec<String> = Vec::new();

        match upload_catbox(&self.client, &name, mime, bytes.clone()).await {
            Ok(url) => {
                info!(
                    host = "catbox",
                    "dense_delivery: public HTTPS upload succeeded"
                );
                return Ok(url);
            }
            Err(e) => {
                warn!(host = "catbox", error = %e, "dense_delivery: public host rejected upload");
                errors.push(format!("catbox: {e}"));
            }
        }
        match upload_litterbox(&self.client, &name, mime, bytes.clone()).await {
            Ok(url) => {
                info!(
                    host = "litterbox",
                    "dense_delivery: public HTTPS upload succeeded"
                );
                return Ok(url);
            }
            Err(e) => {
                warn!(host = "litterbox", error = %e, "dense_delivery: public host rejected upload");
                errors.push(format!("litterbox: {e}"));
            }
        }
        match upload_tmpfiles(&self.client, &name, mime, bytes.clone()).await {
            Ok(url) => {
                info!(
                    host = "tmpfiles",
                    "dense_delivery: public HTTPS upload succeeded"
                );
                return Ok(url);
            }
            Err(e) => {
                warn!(host = "tmpfiles", error = %e, "dense_delivery: public host rejected upload");
                errors.push(format!("tmpfiles: {e}"));
            }
        }
        match upload_pixeldrain(&self.client, &name, mime, bytes).await {
            Ok(url) => {
                info!(
                    host = "pixeldrain",
                    "dense_delivery: public HTTPS upload succeeded"
                );
                return Ok(url);
            }
            Err(e) => {
                warn!(host = "pixeldrain", error = %e, "dense_delivery: public host rejected upload");
                errors.push(format!("pixeldrain: {e}"));
            }
        }
        Err(format!("all public hosts failed: {}", errors.join(" | ")))
    }
}

async fn read_upload_bytes(path: &Path) -> Result<(String, &'static str, Vec<u8>), String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("delivery.bin")
        .to_string();
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("failed to read spill file: {e}"))?;
    Ok((file_name, mime_for_path(path), bytes))
}

fn file_part(
    name: String,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<reqwest::multipart::Part, String> {
    reqwest::multipart::Part::bytes(bytes)
        .file_name(name)
        .mime_str(mime)
        .map_err(|e| format!("invalid multipart mime: {e}"))
}

async fn send_multipart(
    client: &reqwest::Client,
    url: &str,
    form: reqwest::multipart::Form,
    label: &str,
) -> Result<String, String> {
    let resp = client
        .post(url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("{label} request failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("{label} response body: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "{label} HTTP {status}: {}",
            truncate_chars(body.trim(), 180)
        ));
    }
    extract_public_https_url(&body)
}

async fn upload_catbox(
    client: &reqwest::Client,
    name: &str,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let mut form = reqwest::multipart::Form::new().text("reqtype", "fileupload");
    if let Ok(hash) = std::env::var("CATBOX_USERHASH") {
        let hash = hash.trim().to_string();
        if !hash.is_empty() {
            form = form.text("userhash", hash);
        }
    }
    let form = form.part("fileToUpload", file_part(name.to_string(), mime, bytes)?);
    send_multipart(client, CATBOX_UPLOAD_URL, form, "catbox").await
}

async fn upload_litterbox(
    client: &reqwest::Client,
    name: &str,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let form = reqwest::multipart::Form::new()
        .text("reqtype", "fileupload")
        .text("time", "72h")
        .part("fileToUpload", file_part(name.to_string(), mime, bytes)?);
    send_multipart(client, LITTERBOX_UPLOAD_URL, form, "litterbox").await
}

async fn upload_tmpfiles(
    client: &reqwest::Client,
    name: &str,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let form =
        reqwest::multipart::Form::new().part("file", file_part(name.to_string(), mime, bytes)?);
    send_multipart(client, TMPFILES_UPLOAD_URL, form, "tmpfiles").await
}

async fn upload_pixeldrain(
    client: &reqwest::Client,
    name: &str,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let form =
        reqwest::multipart::Form::new().part("file", file_part(name.to_string(), mime, bytes)?);
    send_multipart(client, PIXELDRAIN_UPLOAD_URL, form, "pixeldrain").await
}

pub struct RealPdfRenderer;

#[async_trait]
impl PdfRenderer for RealPdfRenderer {
    async fn render_pdf(
        &self,
        persona_cwd: &Path,
        md_path: &Path,
        pdf_path: &Path,
    ) -> Result<(), String> {
        let script = persona_cwd.join("md2pdf_cjk.py");
        let python_err = if script.is_file() {
            match run_timed_command(
                "python3",
                &[
                    script.as_os_str(),
                    md_path.as_os_str(),
                    pdf_path.as_os_str(),
                ],
                Some(persona_cwd),
            )
            .await
            {
                Ok(()) if pdf_path.is_file() => return Ok(()),
                Ok(()) => Some("md2pdf_cjk.py succeeded but PDF missing".to_string()),
                Err(e) => Some(e),
            }
        } else {
            None
        };
        match run_timed_command(
            "pandoc",
            &[md_path.as_os_str(), OsStr::new("-o"), pdf_path.as_os_str()],
            Some(persona_cwd),
        )
        .await
        {
            Ok(()) if pdf_path.is_file() => Ok(()),
            Ok(()) => Err(match python_err {
                Some(e) => format!("{e}; pandoc succeeded but PDF missing"),
                None => "pandoc succeeded but PDF missing".into(),
            }),
            Err(pandoc_err) => Err(match python_err {
                Some(e) => format!("{e}; pandoc fallback failed: {pandoc_err}"),
                None => pandoc_err,
            }),
        }
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("md") => "text/markdown",
        _ => "application/octet-stream",
    }
}

async fn run_timed_command(
    program: &str,
    args: &[&OsStr],
    cwd: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {program}: {e}"))?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(PDF_RENDER_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("{program} timed out after {PDF_RENDER_TIMEOUT_SECS}s"))?
    .map_err(|e| format!("{program} wait failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{program} exited {}: {}",
            output.status,
            stderr.trim()
        ))
    }
}

pub fn is_messaging_channel(caller_channel: &str) -> bool {
    matches!(
        caller_channel.trim().to_ascii_lowercase().as_str(),
        "telegram" | "discord" | "whatsapp" | "wecom"
    )
}

pub fn effective_max_chars(persona: &Persona, caller_channel: &str) -> usize {
    if is_messaging_channel(caller_channel) {
        positive_or_default(
            persona.dense_delivery_messaging_max_chars,
            DEFAULT_MESSAGING_MAX_CHARS,
        )
    } else {
        positive_or_default(persona.dense_delivery_web_max_chars, DEFAULT_WEB_MAX_CHARS)
    }
}

pub fn effective_summary_chars(persona: &Persona) -> usize {
    positive_or_default(persona.dense_delivery_summary_chars, DEFAULT_SUMMARY_CHARS)
}

fn positive_or_default(value: Option<i64>, default: usize) -> usize {
    value
        .and_then(|n| (n > 0).then_some(n as usize))
        .unwrap_or(default)
}

pub fn upload_provider_from_env() -> &'static str {
    match std::env::var("DELIVERY_UPLOAD_PROVIDER") {
        Ok(v) if v.trim().eq_ignore_ascii_case("none") => "none",
        _ => "catbox",
    }
}

fn uploader_from_env() -> Box<dyn DeliveryUploader> {
    if upload_provider_from_env() == "none" {
        Box::new(NoopUploader)
    } else {
        Box::new(PublicHostUploader::new())
    }
}

/// True when `url` is a public https:// link (not localhost, not this bot's /api/uploads/).
pub fn validate_public_https_url(url: &str) -> Result<&str, &'static str> {
    let trimmed = url.trim();
    if host_is_public_https(trimmed) {
        Ok(trimmed)
    } else {
        Err("upload response was not a public https URL")
    }
}

fn extract_public_https_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty upload response".into());
    }
    if let Some(url) = canonicalize_candidate(trimmed) {
        return Ok(url);
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(url) = json_url_candidate(&v).and_then(|s| canonicalize_candidate(&s)) {
            return Ok(url);
        }
        if let Some(id) = v.get("id").and_then(|x| x.as_str()).map(str::trim) {
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
                let candidate = format!("https://pixeldrain.com/api/file/{id}");
                if let Some(url) = canonicalize_candidate(&candidate) {
                    return Ok(url);
                }
            }
        }
    }
    if let Some(found) = find_https_token(trimmed) {
        if let Some(url) = canonicalize_candidate(&found) {
            return Ok(url);
        }
    }
    Err(format!(
        "upload response was not a public https URL: {}",
        truncate_chars(trimmed, 120)
    ))
}

fn json_url_candidate(v: &serde_json::Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "url",
        "link",
        "direct_link",
        "download_url",
        "downloadUrl",
        "href",
    ];
    if let Some(obj) = v.as_object() {
        for key in KEYS {
            if let Some(s) = obj.get(*key).and_then(|x| x.as_str()) {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        if let Some(data) = obj.get("data") {
            return json_url_candidate(data);
        }
    }
    None
}

fn find_https_token(raw: &str) -> Option<String> {
    let start = raw.find("https://")?;
    let rest = &raw[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']' | '\\'))
        .unwrap_or(rest.len());
    let token = rest[..end].trim_end_matches(['.', ',', ';', ':']);
    token.starts_with("https://").then(|| token.to_string())
}

fn canonicalize_candidate(raw: &str) -> Option<String> {
    let mut s = raw
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']'))
        .to_string();
    if let Some(rest) = s.strip_prefix("http://") {
        s = format!("https://{rest}");
    }
    if let Some(rewritten) = rewrite_tmpfiles_direct(&s) {
        s = rewritten;
    }
    if host_is_public_https(&s) {
        Some(s)
    } else {
        None
    }
}

fn rewrite_tmpfiles_direct(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let is_tmp = lower.starts_with("https://tmpfiles.org/")
        || lower.starts_with("https://www.tmpfiles.org/");
    if !is_tmp || lower.contains("/dl/") {
        return None;
    }
    let after = url
        .split_once("tmpfiles.org/")
        .map(|(_, rest)| rest.trim_start_matches('/'))?;
    if after.is_empty() {
        return None;
    }
    Some(format!("https://tmpfiles.org/dl/{after}"))
}

fn host_is_public_https(url: &str) -> bool {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") || trimmed.contains(char::is_whitespace) {
        return false;
    }
    if trimmed.contains("/api/uploads/") {
        return false;
    }
    let Some(host) = https_host(trimmed) else {
        return false;
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
        || host.ends_with(".local")
        || host.ends_with(".localhost")
    {
        return false;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !(ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || is_private_or_link_local(ip));
    }
    true
}

fn is_private_or_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80
        }
    }
}

fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    if let Some(inner) = hostport.strip_prefix('[') {
        return inner.split(']').next().filter(|s| !s.is_empty());
    }
    let host = hostport.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Apply dense delivery when the persona toggle is on and text exceeds the channel cap.
/// Returns `Some(summary)` when the delivered text should be replaced.
pub async fn maybe_apply_dense_delivery(
    config: &Config,
    db: &Database,
    chat_id: i64,
    persona_id: i64,
    caller_channel: &str,
    text: &str,
    env_redactor: &EnvSecretRedactor,
) -> Option<String> {
    let persona = db.get_persona(persona_id).ok().flatten()?;
    if persona.chat_id != chat_id {
        return None;
    }
    let summarizer = LlmDeliverySummarizer::new(config, env_redactor);
    maybe_apply_dense_delivery_with(
        config,
        &persona,
        caller_channel,
        text,
        uploader_from_env().as_ref(),
        &RealPdfRenderer,
        &summarizer,
        env_redactor,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn maybe_apply_dense_delivery_with(
    config: &Config,
    persona: &Persona,
    caller_channel: &str,
    text: &str,
    uploader: &dyn DeliveryUploader,
    pdf: &dyn PdfRenderer,
    summarizer: &dyn DeliverySummarizer,
    env_redactor: &EnvSecretRedactor,
) -> Option<String> {
    if !persona.dense_delivery_enabled {
        return None;
    }
    let cleaned = text.trim();
    let max_chars = effective_max_chars(persona, caller_channel);
    let char_count = cleaned.chars().count();
    if char_count <= max_chars {
        return None;
    }

    let messaging = is_messaging_channel(caller_channel);
    let summary_chars = effective_summary_chars(persona);
    let pdf_body = env_redactor.redact(cleaned);
    log_spill_preservation(persona, cleaned, &pdf_body);

    let workspace_root = config.workspace_root_absolute();
    let persona_cwd =
        crate::tools::persona_shared_dir(&workspace_root, persona.chat_id, persona.id);
    let delivery_dir = persona_cwd.join("delivery");
    if let Err(e) = std::fs::create_dir_all(&delivery_dir) {
        warn!(
            chat_id = persona.chat_id,
            persona_id = persona.id,
            error = %e,
            "dense_delivery: failed to create delivery dir; summarizing without file"
        );
        return Some(
            deliver_natural_summary(summarizer, cleaned, None, summary_chars, messaging).await,
        );
    }

    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let stem = format!("response-{stamp}");
    let md_path = delivery_dir.join(format!("{stem}.md"));
    if let Err(e) = std::fs::write(&md_path, pdf_body.as_bytes()) {
        warn!(
            chat_id = persona.chat_id,
            persona_id = persona.id,
            error = %e,
            "dense_delivery: failed to write markdown spill"
        );
        return Some(
            deliver_natural_summary(summarizer, cleaned, None, summary_chars, messaging).await,
        );
    }

    let pdf_path = delivery_dir.join(format!("{stem}.pdf"));
    let upload_path = match pdf.render_pdf(&persona_cwd, &md_path, &pdf_path).await {
        Ok(()) if pdf_path.is_file() => pdf_path,
        Ok(()) => {
            warn!(
                chat_id = persona.chat_id,
                persona_id = persona.id,
                "dense_delivery: PDF renderer succeeded but file missing; uploading markdown"
            );
            md_path.clone()
        }
        Err(e) => {
            warn!(
                chat_id = persona.chat_id,
                persona_id = persona.id,
                error = %e,
                "dense_delivery: PDF render failed; uploading markdown"
            );
            md_path.clone()
        }
    };

    let uploaded_pdf = upload_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let public_url =
        upload_public_with_retry(uploader, &upload_path, persona.chat_id, persona.id).await;

    let summary = deliver_natural_summary(
        summarizer,
        cleaned,
        public_url.as_deref(),
        summary_chars,
        messaging,
    )
    .await;
    info!(
        chat_id = persona.chat_id,
        persona_id = persona.id,
        channel = caller_channel,
        chars = char_count,
        max_chars,
        has_public_url = public_url.is_some(),
        uploaded_pdf,
        "dense_delivery_spill"
    );
    Some(summary)
}

async fn upload_public_with_retry(
    uploader: &dyn DeliveryUploader,
    path: &Path,
    chat_id: i64,
    persona_id: i64,
) -> Option<String> {
    let mut last_err: Option<String> = None;
    for attempt in 1..=UPLOAD_ATTEMPTS {
        match uploader.upload_file(path).await {
            Ok(url) => match validate_public_https_url(&url) {
                Ok(u) => return Some(u.to_string()),
                Err(e) => {
                    last_err = Some(e.to_string());
                    warn!(
                        chat_id,
                        persona_id,
                        attempt,
                        error = e,
                        "dense_delivery: upload URL rejected"
                    );
                }
            },
            Err(e) => {
                last_err = Some(e.clone());
                warn!(
                    chat_id,
                    persona_id,
                    attempt,
                    error = %e,
                    "dense_delivery: upload failed"
                );
            }
        }
    }
    if let Some(error) = last_err {
        warn!(
            chat_id,
            persona_id,
            error = %error,
            "dense_delivery: public HTTPS upload exhausted; not offering an internal path"
        );
    }
    None
}

fn log_spill_preservation(persona: &Persona, original: &str, spilled: &str) {
    let orig_chars = original.chars().count();
    let spill_chars = spilled.chars().count();
    let distinctive = distinctive_payload_snippets(original, 6);
    let missing: Vec<&str> = distinctive
        .iter()
        .map(String::as_str)
        .filter(|s| !spilled.contains(s))
        .collect();
    if !missing.is_empty() {
        warn!(
            chat_id = persona.chat_id,
            persona_id = persona.id,
            orig_chars,
            spill_chars,
            missing = missing.len(),
            "dense_delivery: PDF source dropped distinctive report content"
        );
        return;
    }
    info!(
        chat_id = persona.chat_id,
        persona_id = persona.id,
        orig_chars,
        spill_chars,
        distinctive = distinctive.len(),
        "dense_delivery: PDF source preserves full report payload"
    );
}

/// Keep tables, figures, and other dense payload that must travel with the PDF.
fn distinctive_payload_snippets(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let keep = t.contains('|')
            || t.chars().any(|c| c.is_ascii_digit())
            || t.starts_with("```")
            || t.len() >= 80;
        if !keep {
            continue;
        }
        let snippet = truncate_chars(t, 80);
        if !snippet.is_empty() && !out.iter().any(|s: &String| s == &snippet) {
            out.push(snippet);
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

async fn deliver_natural_summary(
    summarizer: &dyn DeliverySummarizer,
    full_text: &str,
    public_url: Option<&str>,
    summary_chars: usize,
    messaging: bool,
) -> String {
    let llm_text = match summarizer
        .summarize(full_text, summary_chars, public_url)
        .await
    {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => {
            warn!("dense_delivery: LLM summary was empty; using extractive fallback");
            fallback_natural_summary(full_text, public_url)
        }
        Err(e) => {
            warn!(error = %e, "dense_delivery: LLM summary failed; using extractive fallback");
            fallback_natural_summary(full_text, public_url)
        }
    };
    finalize_delivery_message(&llm_text, public_url, summary_chars, messaging)
}

async fn summarize_with_llm(
    config: &Config,
    env_redactor: &EnvSecretRedactor,
    full_text: &str,
    summary_chars: usize,
) -> Result<String, String> {
    let mut llm_config = config.clone();
    if !config.hook_prompt_model.trim().is_empty() {
        llm_config.model = config.hook_prompt_model.trim().to_string();
    } else if !config.orchestrator_model.trim().is_empty() {
        llm_config.model = config.orchestrator_model.trim().to_string();
    }
    llm_config.max_tokens = SUMMARY_MAX_TOKENS.min(summary_chars as u32 + 200).max(256);

    let source = truncate_chars(&env_redactor.redact(full_text), SUMMARY_SOURCE_MAX_CHARS);
    let url_line = "Do not include URLs, markdown links, or file paths. A public HTTPS PDF URL will be appended after your reply. Do not invent a link.";
    let system = "You write the short chat reply that accompanies a longer report. Sound like a helpful colleague, not a template, status dump, or table of contents. Match the language of the report. Do not use headings like '— summary'. Do not mention character counts, 'spilled to PDF', or local file paths. Do not invent facts or URLs.";
    let user = format!(
        "Write a natural reply of at most {summary_chars} characters.\n\
         Cover the main takeaway and a few key points. Keep specific numbers/names that matter.\n\
         Tell the reader the detailed/sensitive analysis is in the PDF, not omitted.\n\
         {url_line}\n\n\
         Full report:\n{source}"
    );
    let messages = vec![Message {
        role: "user".into(),
        content: MessageContent::Text(user),
    }];
    let provider = llm::create_provider(&llm_config);
    let timeout = std::time::Duration::from_secs(
        config
            .hook_prompt_timeout_secs
            .max(SUMMARY_LLM_TIMEOUT_SECS),
    );
    let response = tokio::time::timeout(timeout, provider.send_message(system, messages, None))
        .await
        .map_err(|_| format!("summary LLM timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("summary LLM failed: {e}"))?;
    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            ResponseContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        Err("summary LLM returned empty text".into())
    } else {
        Ok(text)
    }
}

fn fallback_natural_summary(full_text: &str, public_url: Option<&str>) -> String {
    let topic = extract_topic(full_text);
    let excerpt = first_plain_excerpt(full_text, 420);
    let bullets = extract_heading_bullets(full_text, 4);
    let mut out = String::new();
    if !topic.is_empty() && topic != "Report" {
        out.push_str(&topic);
        out.push_str(".\n\n");
    }
    if !excerpt.is_empty() {
        out.push_str(&excerpt);
        out.push_str("\n\n");
    }
    if !bullets.is_empty() {
        for b in bullets {
            out.push_str("- ");
            out.push_str(&b);
            out.push('\n');
        }
        out.push('\n');
    }
    match public_url {
        Some(url) => {
            out.push_str("I put the full write-up — including the detailed figures — here:\n");
            out.push_str(url);
        }
        None => {
            out.push_str(
                "I have the full write-up ready, but the public PDF link did not go through. Ask me to resend the file if you need it.",
            );
        }
    }
    out
}

fn finalize_delivery_message(
    body: &str,
    public_url: Option<&str>,
    summary_chars: usize,
    _messaging: bool,
) -> String {
    let mut text = strip_local_path_leaks(body.trim());
    if let Some(url) = public_url {
        if !host_is_public_https(url) {
            warn!("dense_delivery: refusing to attach non-public URL to user message");
        } else {
            if text.contains(url) {
                text = text.replace(url, "");
            }
            text = strip_local_path_leaks(text.trim());
            let stanza = format!("\n\n{url}");
            let stanza_chars = stanza.chars().count();
            let body_budget = summary_chars.saturating_sub(stanza_chars);
            if text.chars().count() > body_budget {
                text = truncate_chars(&text, body_budget.saturating_sub(1));
                text.push('…');
            }
            text.push_str(&stanza);
        }
    } else if text.chars().count() > summary_chars {
        text = truncate_chars(&text, summary_chars.saturating_sub(1));
        text.push('…');
    }
    strip_local_path_leaks(&text)
}

fn extract_topic(text: &str) -> String {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let heading = t.trim_start_matches('#').trim();
        if !heading.is_empty() {
            return truncate_chars(heading, 80);
        }
    }
    "Report".into()
}

fn extract_heading_bullets(text: &str, max: usize) -> Vec<String> {
    let mut bullets = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let item = rest.trim();
            if !item.is_empty() {
                bullets.push(truncate_chars(item, 120));
            }
        }
        if bullets.len() >= max {
            break;
        }
    }
    bullets
}

fn first_plain_excerpt(text: &str, max_chars: usize) -> String {
    let mut buf = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('|') {
            continue;
        }
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(t);
        if buf.chars().count() >= max_chars {
            break;
        }
    }
    truncate_chars(&buf, max_chars)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        input.to_string()
    } else {
        input.chars().take(max_chars).collect()
    }
}

fn strip_local_path_leaks(text: &str) -> String {
    let mut out = text.to_string();
    for needle in [
        "/home/",
        "/api/uploads/",
        "file://",
        "http://127.0.0.1",
        "https://127.0.0.1",
        "http://localhost",
        "https://localhost",
    ] {
        while let Some(idx) = out.find(needle) {
            let rest = &out[idx..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '>')
                .unwrap_or(rest.len());
            out.replace_range(idx..idx + end, "[local path omitted]");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn sample_persona(enabled: bool) -> Persona {
        Persona {
            id: 28,
            chat_id: 997894126,
            name: "selling_oversea".into(),
            model_override: None,
            recent_history_min_user: None,
            recent_history_min_assistant: None,
            operator_memo: None,
            dense_delivery_enabled: enabled,
            dense_delivery_messaging_max_chars: None,
            dense_delivery_web_max_chars: None,
            dense_delivery_summary_chars: None,
            agent_engine_override: None,
            gemini_adk_topology: None,
            cursor_sdk_model: None,
            cursor_sdk_model_params: None,
            llm_provider_override: None,
            llm_base_url_override: None,
        }
    }

    struct StubUploader {
        url: String,
        calls: AtomicUsize,
    }

    impl StubUploader {
        fn new(url: &str) -> Self {
            Self {
                url: url.to_string(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl DeliveryUploader for StubUploader {
        async fn upload_file(&self, _path: &Path) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.url.clone())
        }
    }

    struct StubPdf;

    #[async_trait]
    impl PdfRenderer for StubPdf {
        async fn render_pdf(
            &self,
            _persona_cwd: &Path,
            _md_path: &Path,
            pdf_path: &Path,
        ) -> Result<(), String> {
            tokio::fs::write(pdf_path, b"%PDF-1.4\n")
                .await
                .map_err(|e| e.to_string())
        }
    }

    struct StubSummarizer;

    #[async_trait]
    impl DeliverySummarizer for StubSummarizer {
        async fn summarize(
            &self,
            full_text: &str,
            _summary_chars: usize,
            public_url: Option<&str>,
        ) -> Result<String, String> {
            let topic = extract_topic(full_text);
            let mut out =
                format!("Here's a concise take on {topic}. The detailed figures stay in the PDF.");
            if let Some(url) = public_url {
                out.push_str(" Open it here: ");
                out.push_str(url);
            }
            Ok(out)
        }
    }

    struct FailingSummarizer;

    #[async_trait]
    impl DeliverySummarizer for FailingSummarizer {
        async fn summarize(
            &self,
            _full_text: &str,
            _summary_chars: usize,
            _public_url: Option<&str>,
        ) -> Result<String, String> {
            Err("llm down".into())
        }
    }

    struct SilentSummarizer;

    #[async_trait]
    impl DeliverySummarizer for SilentSummarizer {
        async fn summarize(
            &self,
            _full_text: &str,
            _summary_chars: usize,
            _public_url: Option<&str>,
        ) -> Result<String, String> {
            Ok("Here's the short take. Details are in the write-up.".into())
        }
    }

    struct FlakyUploader {
        url: String,
        calls: AtomicUsize,
    }

    impl FlakyUploader {
        fn new(url: &str) -> Self {
            Self {
                url: url.to_string(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl DeliveryUploader for FlakyUploader {
        async fn upload_file(&self, _path: &Path) -> Result<String, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("transient 403".into())
            } else {
                Ok(self.url.clone())
            }
        }
    }

    struct CapturingUploader {
        url: String,
        calls: AtomicUsize,
        last_name: Mutex<Option<String>>,
    }

    impl CapturingUploader {
        fn new(url: &str) -> Self {
            Self {
                url: url.to_string(),
                calls: AtomicUsize::new(0),
                last_name: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl DeliveryUploader for CapturingUploader {
        async fn upload_file(&self, path: &Path) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            *self.last_name.lock().unwrap() = Some(name);
            Ok(self.url.clone())
        }
    }

    fn test_cfg(root: &Path) -> Config {
        let mut cfg = crate::config::test_config();
        cfg.workspace_dir = root.to_string_lossy().into_owned();
        cfg
    }

    fn empty_redactor() -> EnvSecretRedactor {
        EnvSecretRedactor::empty()
    }

    async fn apply(
        cfg: &Config,
        persona: &Persona,
        channel: &str,
        text: &str,
        up: &StubUploader,
    ) -> Option<String> {
        maybe_apply_dense_delivery_with(
            cfg,
            persona,
            channel,
            text,
            up,
            &StubPdf,
            &StubSummarizer,
            &empty_redactor(),
        )
        .await
    }

    fn long_report() -> String {
        let mut s = String::from("# Trade memo\n\n## Finding one\n\n");
        s.push_str(&"lorem ipsum dolor sit amet. ".repeat(120));
        s.push_str("\n\n## Finding two\n\nMore analysis.\n");
        s.push_str("\n\n## Finding three\n\n| SKU | Price |\n| --- | --- |\n| confidential-lot-42 | $18,400 |\n");
        assert!(s.chars().count() > DEFAULT_MESSAGING_MAX_CHARS);
        s
    }

    #[tokio::test]
    async fn toggle_off_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(false);
        let up = StubUploader::new("https://files.catbox.moe/example.pdf");
        let out = apply(&cfg, &persona, "wecom", &long_report(), &up).await;
        assert!(out.is_none());
        assert_eq!(up.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn under_limit_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/example.pdf");
        let out = apply(&cfg, &persona, "wecom", "short reply", &up).await;
        assert!(out.is_none());
        assert_eq!(up.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn over_limit_summary_contains_https() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/example.pdf");
        let out = apply(&cfg, &persona, "wecom", &long_report(), &up)
            .await
            .expect("spill");
        assert!(out.contains("https://"));
        assert!(out.contains("files.catbox.moe"));
        assert!(out.contains("Trade memo"));
        assert!(!out.contains("— summary"));
        assert!(!out.contains("spilled to PDF"));
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
        let delivery = crate::tools::persona_shared_dir(tmp.path(), persona.chat_id, persona.id)
            .join("delivery");
        let entries: Vec<_> = std::fs::read_dir(&delivery).unwrap().collect();
        assert!(!entries.is_empty());
    }

    #[tokio::test]
    async fn messaging_output_never_contains_local_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/example.pdf");
        let mut body = long_report();
        body.push_str("\nSee /home/operator/secret.md and /api/uploads/x.pdf\n");
        let out = apply(&cfg, &persona, "wecom", &body, &up)
            .await
            .expect("spill");
        assert!(!out.contains("/home/"));
        assert!(!out.contains("/api/uploads/"));
        assert!(out.contains("https://"));
    }

    #[tokio::test]
    async fn web_channel_offers_public_https_not_internal_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/web.pdf");
        let out = apply(&cfg, &persona, "web", &long_report(), &up)
            .await
            .expect("spill");
        assert!(out.contains("https://files.catbox.moe/web.pdf"));
        assert!(!out.contains("/home/"));
        assert!(!out.contains("/api/uploads/"));
        let delivery = crate::tools::persona_shared_dir(tmp.path(), persona.chat_id, persona.id)
            .join("delivery");
        assert!(!out.contains(&delivery.to_string_lossy().into_owned()));
    }

    #[tokio::test]
    async fn pdf_source_keeps_sensitive_report_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/sensitive.pdf");
        let _ = apply(&cfg, &persona, "wecom", &long_report(), &up)
            .await
            .expect("spill");
        let delivery = crate::tools::persona_shared_dir(tmp.path(), persona.chat_id, persona.id)
            .join("delivery");
        let md = std::fs::read_dir(&delivery)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .expect("markdown spill");
        let body = std::fs::read_to_string(md.path()).unwrap();
        assert!(body.contains("confidential-lot-42"));
        assert!(body.contains("$18,400"));
        assert!(body.contains("Trade memo"));
    }

    #[tokio::test]
    async fn env_secret_redaction_keeps_sensitive_report_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/redacted.pdf");
        let redactor = EnvSecretRedactor::from_needles(vec!["sk-super-secret-key".into()]);
        let mut body = long_report();
        body.push_str("\nContact token sk-super-secret-key\n");
        let _ = maybe_apply_dense_delivery_with(
            &cfg,
            &persona,
            "wecom",
            &body,
            &up,
            &StubPdf,
            &StubSummarizer,
            &redactor,
        )
        .await
        .expect("spill");
        let delivery = crate::tools::persona_shared_dir(tmp.path(), persona.chat_id, persona.id)
            .join("delivery");
        let md = std::fs::read_dir(&delivery)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .expect("markdown spill");
        let spilled = std::fs::read_to_string(md.path()).unwrap();
        assert!(spilled.contains("confidential-lot-42"));
        assert!(spilled.contains("$18,400"));
        assert!(!spilled.contains("sk-super-secret-key"));
    }

    #[tokio::test]
    async fn uploads_pdf_file_not_internal_markdown_only() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = CapturingUploader::new("https://files.catbox.moe/captured.pdf");
        let out = maybe_apply_dense_delivery_with(
            &cfg,
            &persona,
            "wecom",
            &long_report(),
            &up,
            &StubPdf,
            &StubSummarizer,
            &empty_redactor(),
        )
        .await
        .expect("spill");
        assert!(out.contains("https://files.catbox.moe/captured.pdf"));
        let name = up.last_name.lock().unwrap().clone().expect("uploaded");
        assert!(
            name.rsplit('.')
                .next()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf")),
            "expected PDF upload, got {name}"
        );
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn llm_failure_falls_back_to_natural_summary_with_url() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/fallback.pdf");
        let out = maybe_apply_dense_delivery_with(
            &cfg,
            &persona,
            "wecom",
            &long_report(),
            &up,
            &StubPdf,
            &FailingSummarizer,
            &empty_redactor(),
        )
        .await
        .expect("spill");
        assert!(out.contains("https://files.catbox.moe/fallback.pdf"));
        assert!(!out.contains("— summary"));
        assert!(!out.contains("spilled to PDF"));
    }

    #[tokio::test]
    async fn persona_limit_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let mut persona = sample_persona(true);
        persona.dense_delivery_messaging_max_chars = Some(10);
        let up = StubUploader::new("https://files.catbox.moe/tiny.pdf");
        let out = apply(&cfg, &persona, "telegram", "this is more than ten", &up).await;
        assert!(out.is_some());
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn silent_summary_still_gets_forced_public_url() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = StubUploader::new("https://files.catbox.moe/forced.pdf");
        let out = maybe_apply_dense_delivery_with(
            &cfg,
            &persona,
            "wecom",
            &long_report(),
            &up,
            &StubPdf,
            &SilentSummarizer,
            &empty_redactor(),
        )
        .await
        .expect("spill");
        assert!(out.contains("https://files.catbox.moe/forced.pdf"));
        assert!(out
            .trim_end()
            .ends_with("https://files.catbox.moe/forced.pdf"));
        assert!(!out.contains("/api/uploads/"));
    }

    #[tokio::test]
    async fn upload_retries_then_attaches_public_url() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_cfg(tmp.path());
        let persona = sample_persona(true);
        let up = FlakyUploader::new("https://litterbox.catbox.moe/retry.pdf");
        let out = maybe_apply_dense_delivery_with(
            &cfg,
            &persona,
            "wecom",
            &long_report(),
            &up,
            &StubPdf,
            &SilentSummarizer,
            &empty_redactor(),
        )
        .await
        .expect("spill");
        assert!(out.contains("https://litterbox.catbox.moe/retry.pdf"));
        assert_eq!(up.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalid_upload_url_rejected() {
        assert!(validate_public_https_url("http://files.catbox.moe/x.pdf").is_err());
        assert!(validate_public_https_url("/api/uploads/x.pdf").is_err());
        assert!(validate_public_https_url("https://127.0.0.1/x.pdf").is_err());
        assert!(validate_public_https_url("https://localhost/x.pdf").is_err());
        assert!(validate_public_https_url("https://192.168.1.9/x.pdf").is_err());
        assert!(validate_public_https_url("https://example.com/api/uploads/x.pdf").is_err());
        assert!(validate_public_https_url("https://files.catbox.moe/x.pdf").is_ok());
        assert!(validate_public_https_url("https://pixeldrain.com/api/file/abc").is_ok());
    }

    #[test]
    fn extract_public_url_from_messy_host_responses() {
        assert_eq!(
            extract_public_https_url("https://files.catbox.moe/abc.pdf\n").unwrap(),
            "https://files.catbox.moe/abc.pdf"
        );
        assert_eq!(
            extract_public_https_url(
                r#"{"status":"success","data":{"url":"http://tmpfiles.org/12/r.pdf"}}"#
            )
            .unwrap(),
            "https://tmpfiles.org/dl/12/r.pdf"
        );
        assert_eq!(
            extract_public_https_url(r#"{"id":"Ab1Cd2"}"#).unwrap(),
            "https://pixeldrain.com/api/file/Ab1Cd2"
        );
        let html = "<html>see https://files.catbox.moe/zz.pdf</html>";
        assert_eq!(
            extract_public_https_url(html).unwrap(),
            "https://files.catbox.moe/zz.pdf"
        );
        assert!(extract_public_https_url("https://127.0.0.1/secret.pdf").is_err());
        assert!(extract_public_https_url("/api/uploads/web/1/x.pdf").is_err());
    }

    #[test]
    fn finalize_always_appends_public_url() {
        let url = "https://files.catbox.moe/forced.pdf";
        let out = finalize_delivery_message(
            "Here's the take. See /api/uploads/web/1/old.pdf",
            Some(url),
            200,
            true,
        );
        assert!(out.contains(url));
        assert!(!out.contains("/api/uploads/"));
        assert!(out.trim_end().ends_with(url));
    }

    #[test]
    fn finalize_keeps_public_url_when_over_budget() {
        let url = "https://files.catbox.moe/keep.pdf";
        let body = "word ".repeat(80);
        let out = finalize_delivery_message(&body, Some(url), 60, true);
        assert!(out.contains(url));
        assert!(out.chars().count() <= 60);
        assert!(out.trim_end().ends_with(url));
    }

    #[test]
    fn messaging_channel_classification() {
        assert!(is_messaging_channel("wecom"));
        assert!(is_messaging_channel("Telegram"));
        assert!(!is_messaging_channel("web"));
        assert!(!is_messaging_channel("scheduler"));
    }
}
