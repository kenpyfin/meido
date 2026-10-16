//! Shared pre-delivery artifact resolution and bare-filename normalization for all channels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactResolveKind {
    /// PNG/JPEG/GIF/WebP/BMP only (Telegram auto-photos, bare-line normalization).
    ImageOnly,
    /// Any existing file (web link materialization).
    AnyFile,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn trim_artifact_ref(raw: &str) -> &str {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>')
}

/// Normalize markdown link targets and path refs for local artifact resolution.
///
/// Strips `file://` (including `file://localhost/…`) and URL query/fragment suffixes
/// so delivery materialization can resolve workspace files.
pub fn normalize_local_artifact_ref(raw: &str) -> String {
    let mut t = trim_artifact_ref(raw).to_string();
    if let Some(rest) = t.strip_prefix("file://") {
        t = if let Some(path) = rest.strip_prefix("localhost/") {
            format!("/{path}")
        } else if rest.starts_with('/') {
            rest.to_string()
        } else if let Some(path) = rest.strip_prefix("localhost") {
            let path = path.trim_start_matches('/');
            if path.is_empty() {
                rest.to_string()
            } else {
                format!("/{path}")
            }
        } else {
            rest.to_string()
        };
    }
    if let Some(idx) = t.find('#') {
        t.truncate(idx);
    }
    // Strip URL query (?preview=1) but preserve Windows extended-path prefix \\?\.
    if !t.starts_with(r"\\?\") {
        if let Some(idx) = t.find('?') {
            t.truncate(idx);
        }
    }
    t
}

fn web_delivery_copy_basename_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{8}-\d{6}-bot-(.+)$").unwrap())
}

fn hash_prefixed_upload_basename_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^[0-9a-f]{16}-(.+)$").unwrap())
}

/// Basename variants to try when a delivery URL or `-bot-` copy path does not exist on disk.
pub fn artifact_basename_fallback_candidates(basename: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |value: String| {
        if !value.is_empty() && !out.iter().any(|existing| existing == &value) {
            out.push(value);
        }
    };
    push(basename.to_string());
    if let Some(caps) = web_delivery_copy_basename_regex().captures(basename) {
        if let Some(rest) = caps.get(1) {
            push(rest.as_str().to_string());
        }
    }
    if let Some(caps) = hash_prefixed_upload_basename_regex().captures(basename) {
        if let Some(rest) = caps.get(1) {
            push(rest.as_str().to_string());
        }
    }
    out
}

pub fn is_deliverable_image_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some(ext) if IMAGE_EXTENSIONS.contains(&ext)
    )
}

fn path_matches_kind(path: &Path, kind: ArtifactResolveKind) -> bool {
    if !path.is_file() {
        return false;
    }
    match kind {
        ArtifactResolveKind::AnyFile => true,
        ArtifactResolveKind::ImageOnly => is_deliverable_image_extension(path),
    }
}

fn path_under_workspace(
    candidate: &Path,
    workspace_root: &Path,
    kind: ArtifactResolveKind,
) -> Option<PathBuf> {
    let cand = candidate.canonicalize().ok()?;
    let root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    if !cand.starts_with(&root) || !path_matches_kind(&cand, kind) {
        return None;
    }
    Some(cand)
}

fn collect_artifact_candidates(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
    trimmed: &str,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let as_path = PathBuf::from(trimmed);
    if as_path.is_absolute() {
        candidates.push(as_path);
    } else {
        candidates.push(workspace_root.join(trimmed));
        candidates.push(workspace_root.join("shared").join(trimmed));

        if let Some(stripped) = trimmed.strip_prefix("./") {
            candidates.push(workspace_root.join(stripped));
            candidates.push(workspace_root.join("shared").join(stripped));
        }
        if let Some(stripped) = trimmed.strip_prefix("shared/") {
            candidates.push(workspace_root.join("shared").join(stripped));
        }

        let persona_dir = workspace_root
            .join("shared")
            .join("personas")
            .join(chat_id.to_string())
            .join(persona_id.to_string());
        candidates.push(persona_dir.join(trimmed));
        candidates.push(
            workspace_root
                .join("runtime")
                .join("groups")
                .join(chat_id.to_string())
                .join(persona_id.to_string())
                .join(trimmed),
        );

        if let Some(repo_root) = workspace_root.parent() {
            candidates.push(repo_root.join(trimmed));
            if let Some(stripped) = trimmed.strip_prefix("workspace/") {
                candidates.push(repo_root.join(stripped));
            }
        }
    }
    candidates
}

/// Depth-limited search for a unique basename under the persona directory.
fn unique_persona_basename_match(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
    basename: &str,
    kind: ArtifactResolveKind,
) -> Option<PathBuf> {
    let persona_root = workspace_root
        .join("shared")
        .join("personas")
        .join(chat_id.to_string())
        .join(persona_id.to_string());
    if !persona_root.is_dir() {
        return None;
    }
    let mut matches: Vec<PathBuf> = Vec::new();
    collect_basename_matches(&persona_root, basename, kind, 0, 4, &mut matches);
    if matches.len() == 1 {
        path_under_workspace(&matches[0], workspace_root, kind)
    } else {
        None
    }
}

fn collect_basename_matches(
    dir: &Path,
    basename: &str,
    kind: ArtifactResolveKind,
    depth: u32,
    max_depth: u32,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() > 1 {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_file() {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == basename)
                && path_matches_kind(&path, kind)
            {
                out.push(path);
            }
        } else if path.is_dir() {
            collect_basename_matches(&path, basename, kind, depth + 1, max_depth, out);
        }
    }
}

/// Resolve a local workspace artifact path (persona-scoped when `chat_id` / `persona_id` are set).
pub fn resolve_workspace_artifact_path(
    workspace_root: &Path,
    chat_id: Option<i64>,
    persona_id: Option<i64>,
    raw: &str,
    kind: ArtifactResolveKind,
) -> Option<PathBuf> {
    let t = normalize_local_artifact_ref(raw);
    if let Some(id) = t
        .strip_prefix("/api/artifacts/")
        .and_then(|v| v.strip_suffix("/content"))
    {
        let a = crate::artifacts::Catalog::open(workspace_root)
            .ok()?
            .get(id)
            .ok()?;
        if a.status != "active"
            || chat_id.is_some_and(|c| c != a.chat_id)
            || persona_id.is_some_and(|p| p != a.persona_id)
        {
            return None;
        }
        if kind == ArtifactResolveKind::ImageOnly && a.kind != "image" {
            return None;
        }
        return a.blob_path.is_file().then_some(a.blob_path);
    }
    if t.is_empty()
        || t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("data:")
        || t.starts_with("/api/uploads/")
    {
        return None;
    }

    let candidate = Path::new(&t);
    if candidate.is_absolute() {
        return path_under_workspace(candidate, workspace_root, kind);
    }

    if let (Some(cid), Some(pid)) = (chat_id, persona_id) {
        // A bare name belongs to this persona. Never prefer a coincidental shared file.
        if !t.contains('/') && !t.contains('\\') {
            return unique_persona_basename_match(workspace_root, cid, pid, &t, kind);
        }
        let mut resolved = std::collections::HashSet::new();
        for candidate in collect_artifact_candidates(workspace_root, cid, pid, &t) {
            if let Some(path) = path_under_workspace(&candidate, workspace_root, kind) {
                resolved.insert(path);
            }
        }
        return if resolved.len() == 1 {
            resolved.into_iter().next()
        } else {
            None
        };
    }

    // Legacy resolution without persona scope.
    let shared = workspace_root.join("shared").join(&t);
    let p = if shared.exists() {
        shared
    } else {
        workspace_root.join(&t)
    };
    path_under_workspace(&p, workspace_root, kind)
}

fn bare_image_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*(?P<name>[^\s/\\]+\.(?:png|jpg|jpeg|gif|webp|bmp))(?:[ \t]+\([^)]+\))?[ \t]*$",
        )
        .unwrap()
    })
}

fn inline_image_backtick_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`\n]+\.(?:png|jpg|jpeg|gif|webp|bmp))`").unwrap())
}

fn line_should_skip_bare_normalize(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.contains("![")
        || t.contains("/api/uploads/")
        || t.starts_with("http://")
        || t.starts_with("https://")
}

fn markdown_image_for_basename(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
    basename: &str,
) -> Option<String> {
    let path = resolve_workspace_artifact_path(
        workspace_root,
        Some(chat_id),
        Some(persona_id),
        basename,
        ArtifactResolveKind::ImageOnly,
    )?;
    Some(format!("![{basename}]({})", path.display()))
}

/// Rewrite bare image filename lines into markdown images with canonical absolute paths.
pub fn normalize_assistant_artifact_references(
    text: &str,
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
) -> String {
    let mut out = text.to_string();
    for line in text.lines() {
        if line_should_skip_bare_normalize(line) {
            continue;
        }
        let Some(caps) = bare_image_line_regex().captures(line.trim()) else {
            continue;
        };
        let Some(name_m) = caps.name("name") else {
            continue;
        };
        let basename = name_m.as_str();
        let Some(md) = markdown_image_for_basename(workspace_root, chat_id, persona_id, basename)
        else {
            continue;
        };
        if line == line.trim() {
            out = out.replace(line, &md);
        } else {
            out = out.replace(line.trim(), &md);
        }
    }

    let mut backtick_rewrites: Vec<(String, String)> = Vec::new();
    for caps in inline_image_backtick_regex().captures_iter(&out) {
        let Some(full_match) = caps.get(0) else {
            continue;
        };
        let Some(name_m) = caps.get(1) else {
            continue;
        };
        let token = full_match.as_str();
        let basename = name_m.as_str();
        if out.contains(&format!("![{basename}]")) {
            continue;
        }
        let Some(md) = markdown_image_for_basename(workspace_root, chat_id, persona_id, basename)
        else {
            continue;
        };
        if backtick_rewrites.iter().any(|(from, _)| from == token) {
            continue;
        }
        backtick_rewrites.push((token.to_string(), md));
    }
    for (from, to) in backtick_rewrites {
        out = out.replace(&from, &to);
    }

    out
}

fn extract_upload_urls_from_text(text: &str) -> Vec<String> {
    let Some(re) = Regex::new(r#"/api/uploads/[^\s\)\]\(<>"']+"#).ok() else {
        return Vec::new();
    };
    re.find_iter(text).map(|m| m.as_str().to_string()).collect()
}

fn upload_rel_url_exists(workspace_root: &Path, working_dir: Option<&str>, rel_url: &str) -> bool {
    let Some(rel) = rel_url.strip_prefix("/api/uploads/") else {
        return true;
    };
    let shared_path = workspace_root.join("shared").join("upload").join(rel);
    if shared_path.is_file() {
        return true;
    }
    if let Some(dir) = working_dir {
        let legacy_path = Path::new(dir).join("uploads").join(rel);
        if legacy_path.is_file() {
            return true;
        }
    }
    false
}

fn resolve_delivery_local_file_path(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
    raw: &str,
) -> Option<PathBuf> {
    let trimmed = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    if trimmed.starts_with('#') || trimmed.starts_with("mailto:") {
        return None;
    }
    if let Some(path) = resolve_workspace_artifact_path(
        workspace_root,
        Some(chat_id),
        Some(persona_id),
        trimmed,
        ArtifactResolveKind::AnyFile,
    ) {
        return Some(path);
    }
    None
}

async fn persist_file_for_web_delivery(
    workspace_root: &Path,
    chat_id: i64,
    persona_id: i64,
    source_path: &Path,
) -> Result<String, String> {
    crate::artifacts::publish(
        workspace_root.to_path_buf(),
        chat_id,
        persona_id,
        source_path.to_path_buf(),
        "generated",
    )
    .await
    .map(|a| a.url)
    .map_err(|e| e.to_string())
}

/// Rewrite local file references to stable, verified artifact URLs.
///
/// Interactive web `/api/send` and scheduled/background delivery both need this so
/// markdown images render in the browser (absolute `/home/...` paths are not fetchable).
pub async fn materialize_web_delivery_file_links(
    workspace_root: &Path,
    working_dir: Option<&str>,
    chat_id: i64,
    persona_id: i64,
    response: &str,
) -> Result<String, String> {
    let Some(markdown_link_re) = Regex::new(r#"\]\(([^)\n]+)\)"#).ok() else {
        return Ok(response.to_string());
    };
    let Some(parenthesized_target_re) = Regex::new(r#"\(([^()\n]+)\)"#).ok() else {
        return Ok(response.to_string());
    };
    let mut rewrites: HashMap<String, String> = HashMap::new();
    for caps in markdown_link_re.captures_iter(response) {
        let Some(target) = caps.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        if target.starts_with("/api/artifacts/") || rewrites.contains_key(&target) {
            continue;
        }
        if let Some(local_path) =
            resolve_delivery_local_file_path(workspace_root, chat_id, persona_id, &target)
        {
            let rel =
                persist_file_for_web_delivery(workspace_root, chat_id, persona_id, &local_path)
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error=%e,"artifact_publication_failed");
                        "#artifact-unavailable".into()
                    });
            rewrites.insert(target, rel);
        } else if target.starts_with('/') && !target.starts_with("/api/")
            || target.starts_with("file://")
        {
            rewrites.insert(target, "#artifact-unavailable".into());
        }
    }
    for caps in parenthesized_target_re.captures_iter(response) {
        let Some(target) = caps.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        if target.starts_with("/api/artifacts/") || rewrites.contains_key(&target) {
            continue;
        }
        if let Some(local_path) =
            resolve_delivery_local_file_path(workspace_root, chat_id, persona_id, &target)
        {
            let rel =
                persist_file_for_web_delivery(workspace_root, chat_id, persona_id, &local_path)
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error=%e,"artifact_publication_failed");
                        "#artifact-unavailable".into()
                    });
            rewrites.insert(target, rel);
        }
    }

    let mut updated = response.to_string();
    let code_path = Regex::new(r"`(/[^`\n]+)`").map_err(|e| e.to_string())?;
    for cap in code_path.captures_iter(response) {
        let raw = &cap[1];
        if let Some(path) =
            resolve_delivery_local_file_path(workspace_root, chat_id, persona_id, raw)
        {
            let url = persist_file_for_web_delivery(workspace_root, chat_id, persona_id, &path)
                .await
                .unwrap_or_else(|e| {
                    warn!(error=%e,"artifact_publication_failed");
                    "#artifact-unavailable".into()
                });
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(['[', ']'], "_");
            updated = updated.replace(&cap[0], &format!("[{label}]({url})"));
        }
    }
    for (target, rel) in &rewrites {
        updated = updated.replace(&format!("({target})"), &format!("({rel})"));
    }

    let upload_urls = extract_upload_urls_from_text(&updated);
    for url in upload_urls {
        if upload_rel_url_exists(workspace_root, working_dir, &url) {
            continue;
        }
        warn!("assistant referenced an unavailable legacy upload");
        updated = updated.replace(&url, "#artifact-unavailable");
    }

    if updated.contains("#artifact-unavailable") {
        // A failed publication is a status, never a clickable fragment URL.
        let failed_link = Regex::new(r"!?\[([^\]\n]*)\]\([^\)\n]*#artifact-unavailable\)")
            .map_err(|e| e.to_string())?;
        updated = failed_link
            .replace_all(&updated, "$1 — file unavailable")
            .into_owned();
        updated = updated.replace("#artifact-unavailable", "file unavailable");
        updated.push_str("\n\nA referenced file could not be published. No replacement file was selected. Retry publishing the exact completed output to obtain a working link.");
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fdm_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("shared").join("personas").join("1").join("2")).unwrap();
        root
    }

    #[test]
    fn resolve_persona_scoped_basename() {
        let root = temp_workspace();
        let img = root.join("shared/personas/1/2/PZ-foo.png");
        fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let root = root.canonicalize().unwrap();

        let resolved = resolve_workspace_artifact_path(
            &root,
            Some(1),
            Some(2),
            "PZ-foo.png",
            ArtifactResolveKind::ImageOnly,
        );
        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with(Path::new("PZ-foo.png")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_bare_filename_line() {
        let root = temp_workspace();
        let img = root.join("shared/personas/1/2/PZ-foo.png");
        fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let root = root.canonicalize().unwrap();

        let input = "Hello\n\nPZ-foo.png (Refined Identity)\n\nBye";
        let out = normalize_assistant_artifact_references(&input, &root, 1, 2);
        assert!(out.contains("!["));
        assert!(out.contains("PZ-foo.png"));
        assert!(!out.contains("PZ-foo.png (Refined Identity)"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_skips_existing_markdown() {
        let root = temp_workspace();
        let input = "![alt](https://example.com/a.png)";
        let out = normalize_assistant_artifact_references(&input, &root, 1, 2);
        assert_eq!(out, input);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_backtick_wrapped_filename() {
        let root = temp_workspace();
        let img = root.join("shared/personas/1/2/PZ-20260709-LANDSEND-HOTIFY.png");
        fs::write(&img, [137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let root = root.canonicalize().unwrap();

        let input =
            "**Lands End (#196)**\n\n`PZ-20260709-LANDSEND-HOTIFY.png`\n\nCoastal overlook.";
        let out = normalize_assistant_artifact_references(&input, &root, 1, 2);
        assert!(out.contains("![PZ-20260709-LANDSEND-HOTIFY.png]"));
        assert!(!out.contains("`PZ-20260709-LANDSEND-HOTIFY.png`"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_local_artifact_ref_strips_file_url() {
        assert_eq!(
            normalize_local_artifact_ref(
                "file:///home/user/workspace/shared/personas/1/2/report.md?preview=1"
            ),
            "/home/user/workspace/shared/personas/1/2/report.md"
        );
        assert_eq!(
            normalize_local_artifact_ref("file://localhost/home/user/workspace/report.md#section"),
            "/home/user/workspace/report.md"
        );
        assert_eq!(
            normalize_local_artifact_ref("ORIGIN/Projects/spec.md"),
            "ORIGIN/Projects/spec.md"
        );
    }

    #[test]
    fn normalize_local_artifact_ref_preserves_windows_verbatim_prefix() {
        let path = r"\\?\C:\workspace\shared\mark.png";
        assert_eq!(normalize_local_artifact_ref(path), path);
    }

    #[test]
    fn resolve_file_url_markdown_target() {
        let root = temp_workspace();
        let spec = root.join("shared/personas/997894126/3/ORIGIN/Projects/spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        fs::write(&spec, b"# spec").unwrap();
        let root = root.canonicalize().unwrap();

        let file_url = format!("file://{}", spec.display());
        let resolved = resolve_workspace_artifact_path(
            &root,
            Some(997894126),
            Some(3),
            &file_url,
            ArtifactResolveKind::AnyFile,
        );
        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with(Path::new("spec.md")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_basename_fallback_candidates_strips_delivery_copy_prefix() {
        let candidates = artifact_basename_fallback_candidates(
            "20260604-045735-bot-PZ-20260608-PARK-HOTIFY-MEDIUM.png",
        );
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[1], "PZ-20260608-PARK-HOTIFY-MEDIUM.png");
    }

    #[test]
    fn artifact_basename_fallback_candidates_strips_hash_prefix() {
        let candidates =
            artifact_basename_fallback_candidates("78bc936e4388a4ae-PZ-20260603-EMBARCADERO.png");
        assert!(candidates.contains(&"PZ-20260603-EMBARCADERO.png".to_string()));
    }
}
