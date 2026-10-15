use serde::Deserialize;
use std::path::PathBuf;

/// How much routing detail is included in `<available_skills>` (`SKILLS_CATALOG_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsCatalogMode {
    /// Description + `when_to_use` excerpt + meta (default).
    Full,
    /// Name + one-line description + meta only (no `when_to_use` in the system prompt).
    Compact,
}

impl SkillsCatalogMode {
    pub fn from_env() -> Self {
        match std::env::var("SKILLS_CATALOG_MODE").ok().as_deref() {
            Some(s) if s.trim().eq_ignore_ascii_case("compact") => Self::Compact,
            _ => Self::Full,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    /// Routing hints for the system prompt catalog (full procedures stay in SKILL.md body).
    pub when_to_use: Option<String>,
    pub dir_path: PathBuf,
    pub platforms: Vec<String>,
    pub deps: Vec<String>,
    pub source: String,
    pub version: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SkillFrontmatter {
    name: Option<String>,
    /// Alternative to name (e.g. some skills use title)
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: String,
    /// When the agent should call `activate_skill` for this skill (catalog only; optional for third-party skills).
    #[serde(default)]
    when_to_use: Option<String>,
    #[serde(default)]
    platforms: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    compatibility: SkillCompatibility,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SkillCompatibility {
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
}

pub struct SkillManager {
    /// Primary and additional skills directories. Primary (first) is workspace/skills;
    /// additional may include workspace/shared/skills so personas that create skills
    /// in the shared workspace still have them discoverable by all personas.
    skills_dirs: Vec<PathBuf>,
}

impl SkillManager {
    /// Create a SkillManager that scans a single directory.
    pub fn from_skills_dir(skills_dir: &str) -> Self {
        SkillManager {
            skills_dirs: vec![PathBuf::from(skills_dir)],
        }
    }

    /// Create a SkillManager that scans multiple directories. Skills are merged and
    /// deduped by name (earlier directories take precedence). Typical order: workspace
    /// `skills/`, `shared/skills/`, then repository `builtin_skills/` (see
    /// [`Config::skill_discovery_dirs`](crate::config::Config::skill_discovery_dirs)).
    pub fn from_skills_dirs(dirs: impl IntoIterator<Item = impl AsRef<std::path::Path>>) -> Self {
        let skills_dirs: Vec<PathBuf> =
            dirs.into_iter().map(|p| p.as_ref().to_path_buf()).collect();
        SkillManager { skills_dirs }
    }

    #[allow(dead_code)]
    pub fn new(data_dir: &str) -> Self {
        let skills_dir = PathBuf::from(data_dir).join("skills");
        SkillManager {
            skills_dirs: vec![skills_dir],
        }
    }

    /// Discover skills for the agent catalog: valid frontmatter and supported on this platform.
    /// Does not require local `deps` binaries (API/remote skills may list optional tools in frontmatter).
    pub fn discover_skills(&self) -> Vec<SkillMetadata> {
        self.discover_skills_internal(false)
    }

    /// Discover every skill with valid `SKILL.md` frontmatter (including other platforms).
    pub fn discover_all_skills(&self) -> Vec<SkillMetadata> {
        self.discover_skills_internal(true)
    }

    /// API or cross-platform skill (declared deps and/or not targeted at this OS).
    pub fn skill_is_remote(&self, skill: &SkillMetadata) -> bool {
        !skill.deps.is_empty() || !platform_allowed(&skill.platforms)
    }

    /// Discover available skills filtered by an optional allowlist.
    /// `None` => allow all. `Some(set)` => only skills in the set (case-insensitive).
    pub fn discover_skills_for_allowed(
        &self,
        allowed_skill_names: Option<&std::collections::HashSet<String>>,
    ) -> Vec<SkillMetadata> {
        let mut skills = self.discover_skills();
        if let Some(allowed) = allowed_skill_names {
            skills.retain(|s| allowed.iter().any(|a| a.eq_ignore_ascii_case(&s.name)));
        }
        skills
    }

    fn discover_skills_internal(&self, include_unavailable: bool) -> Vec<SkillMetadata> {
        let mut seen_names = std::collections::HashSet::new();
        let mut skills = Vec::new();

        for skills_dir in &self.skills_dirs {
            let entries = match std::fs::read_dir(skills_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let skill_md = {
                    let p = path.join("SKILL.md");
                    if p.exists() {
                        p
                    } else {
                        let p = path.join("skill.md");
                        if p.exists() {
                            p
                        } else {
                            continue;
                        }
                    }
                };
                if let Ok(content) = std::fs::read_to_string(&skill_md) {
                    if let Some((meta, _body)) = parse_skill_md(&content, &path) {
                        if seen_names.contains(&meta.name) {
                            continue;
                        }
                        if include_unavailable || self.skill_is_available(&meta).is_ok() {
                            seen_names.insert(meta.name.clone());
                            skills.push(meta);
                        }
                    }
                }
            }
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// Load a skill by name if it is available on the current platform.
    pub fn load_skill(&self, name: &str) -> Option<(SkillMetadata, String)> {
        self.load_skill_checked(name).ok()
    }

    /// Load a skill with availability diagnostics.
    pub fn load_skill_checked(&self, name: &str) -> Result<(SkillMetadata, String), String> {
        let all_skills = self.discover_skills_internal(true);

        for skill in all_skills {
            if skill.name != name {
                continue;
            }

            self.skill_is_available(&skill)?;

            for filename in ["SKILL.md", "skill.md"] {
                let skill_md = skill.dir_path.join(filename);
                if skill_md.exists() {
                    if let Ok(content) = std::fs::read_to_string(&skill_md) {
                        if let Some((meta, body)) = parse_skill_md(&content, &skill.dir_path) {
                            return Ok((meta, body));
                        }
                    }
                }
            }
            return Err(format!("Skill '{name}' exists but could not be loaded."));
        }

        let available = self.discover_skills();
        if available.is_empty() {
            Err(format!(
                "Skill '{name}' not found. No skills are currently available."
            ))
        } else {
            let names: Vec<&str> = available.iter().map(|s| s.name.as_str()).collect();
            Err(format!(
                "Skill '{name}' not found. Available skills: {}",
                names.join(", ")
            ))
        }
    }

    fn skill_is_available(&self, skill: &SkillMetadata) -> Result<(), String> {
        if !platform_allowed(&skill.platforms) {
            return Err(format!(
                "Skill '{}' is a remote skill for this platform (current: {}, supported: {}).",
                skill.name,
                current_platform(),
                skill.platforms.join(", ")
            ));
        }
        Ok(())
    }

    /// Build a compact skills catalog for the system prompt.
    /// Returns empty string if no skills are available.
    /// YAML frontmatter only (name, description, when_to_use, constraints). Full SKILL.md body is loaded via `activate_skill`.
    pub fn build_skills_catalog(&self) -> String {
        self.build_skills_catalog_with_mode(SkillsCatalogMode::from_env())
    }

    /// Same as [`Self::build_skills_catalog`] but filtered by an optional allowlist.
    pub fn build_skills_catalog_for_allowed(
        &self,
        allowed_skill_names: Option<&std::collections::HashSet<String>>,
    ) -> String {
        self.build_skills_catalog_with_mode_for_allowed(
            SkillsCatalogMode::from_env(),
            allowed_skill_names,
        )
    }

    /// Same as [`Self::build_skills_catalog`] with an explicit mode (tests; overrides env).
    pub fn build_skills_catalog_with_mode(&self, mode: SkillsCatalogMode) -> String {
        self.build_skills_catalog_with_mode_for_allowed(mode, None)
    }

    /// Same as [`Self::build_skills_catalog_for_allowed`] with an explicit catalog mode.
    pub fn build_skills_catalog_for_allowed_with_mode(
        &self,
        mode: SkillsCatalogMode,
        allowed_skill_names: Option<&std::collections::HashSet<String>>,
    ) -> String {
        self.build_skills_catalog_with_mode_for_allowed(mode, allowed_skill_names)
    }

    fn build_skills_catalog_with_mode_for_allowed(
        &self,
        mode: SkillsCatalogMode,
        allowed_skill_names: Option<&std::collections::HashSet<String>>,
    ) -> String {
        const WHEN_TO_USE_MAX_CHARS: usize = 800;
        const COMPACT_DESCRIPTION_MAX_CHARS: usize = 280;

        let skills = self.discover_skills_for_allowed(allowed_skill_names);
        if skills.is_empty() {
            return String::new();
        }
        let mut catalog = String::from("<available_skills>\n");
        for skill in &skills {
            let description_for_catalog = match mode {
                SkillsCatalogMode::Full => skill.description.clone(),
                SkillsCatalogMode::Compact => {
                    let collapsed: String = skill
                        .description
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if collapsed.len() <= COMPACT_DESCRIPTION_MAX_CHARS {
                        collapsed
                    } else {
                        let boundary =
                            collapsed.floor_char_boundary(COMPACT_DESCRIPTION_MAX_CHARS - 3);
                        format!("{}...", &collapsed[..boundary])
                    }
                }
            };
            catalog.push_str(&format!(
                "- **{}**: {}\n",
                skill.name, description_for_catalog
            ));
            if matches!(mode, SkillsCatalogMode::Full) {
                if let Some(w) = skill
                    .when_to_use
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    let excerpt = if w.len() > WHEN_TO_USE_MAX_CHARS {
                        let boundary = w.floor_char_boundary(WHEN_TO_USE_MAX_CHARS);
                        format!("{}...", &w[..boundary])
                    } else {
                        w.to_string()
                    };
                    for line in excerpt.lines() {
                        catalog.push_str(&format!("  {}\n", line));
                    }
                }
            }
            let mut meta_parts: Vec<String> = Vec::new();
            if !skill.platforms.is_empty() {
                meta_parts.push(format!("platforms={}", skill.platforms.join(",")));
            }
            if !skill.deps.is_empty() {
                meta_parts.push(format!("deps={}", skill.deps.join(",")));
            }
            if skill.source != "local" {
                meta_parts.push(format!("source={}", skill.source));
            }
            if let Some(v) = &skill.version {
                meta_parts.push(format!("version={v}"));
            }
            if let Some(u) = &skill.updated_at {
                meta_parts.push(format!("updated_at={u}"));
            }
            if !meta_parts.is_empty() {
                catalog.push_str(&format!("  Meta: {}\n", meta_parts.join("; ")));
            }
        }
        catalog.push_str("</available_skills>");
        catalog
    }

    /// Build a user-facing formatted list of available skills.
    /// Shows the skills directory path(s) and, if any skills are remote (other platform / API), lists them too.
    pub fn list_skills_formatted(&self) -> String {
        let skills_dir_display = self
            .skills_dirs
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ");
        let all = self.discover_skills_internal(true);
        let available: Vec<_> = self.discover_skills();

        if available.is_empty() && all.is_empty() {
            return format!(
                "No skills found in:\n{}\n(Add skill folders with SKILL.md or skill.md.)",
                skills_dir_display
            );
        }

        let mut output = format!("Loaded from: {}\n\n", skills_dir_display);
        if available.is_empty() {
            output.push_str("No skills available on this platform/runtime (see below).\n\n");
        } else {
            output.push_str(&format!("Available skills ({}):\n\n", available.len()));
            for skill in &available {
                output.push_str(&format!(
                    "• {} — {} [{}]\n",
                    skill.name, skill.description, skill.source
                ));
            }
        }

        let unavailable: Vec<_> = all
            .iter()
            .filter(|s| !available.iter().any(|a| a.name == s.name))
            .collect();
        if !unavailable.is_empty() {
            output.push_str(&format!(
                "\nRemote skills (API or other platform) ({}):\n",
                unavailable.len()
            ));
            for skill in &unavailable {
                let note = if !platform_allowed(&skill.platforms) {
                    self.skill_is_available(skill)
                        .err()
                        .unwrap_or_else(|| "other platform".into())
                } else if !skill.deps.is_empty() {
                    format!("API skill (declared deps: {})", skill.deps.join(", "))
                } else {
                    "remote skill".to_string()
                };
                output.push_str(&format!("  • {} — {}\n", skill.name, note));
            }
        }

        output
    }

    #[allow(dead_code)]
    pub fn skills_dir(&self) -> &PathBuf {
        self.skills_dirs
            .first()
            .expect("SkillManager has at least one directory")
    }
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

fn normalize_platform(value: &str) -> String {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "macos" | "osx" => "darwin".to_string(),
        _ => v,
    }
}

fn platform_allowed(platforms: &[String]) -> bool {
    if platforms.is_empty() {
        return true;
    }

    let current = current_platform();
    platforms.iter().any(|p| {
        let p = normalize_platform(p);
        p == "all" || p == "*" || p == current
    })
}

/// Parse a SKILL.md file, extracting frontmatter via YAML and body.
/// Returns None if the file lacks valid frontmatter with a name field.
fn parse_skill_md(content: &str, dir_path: &std::path::Path) -> Option<(SkillMetadata, String)> {
    let trimmed = content.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---\n") && !trimmed.starts_with("---\r\n") {
        return None;
    }

    let mut lines = trimmed.lines();
    let _ = lines.next()?; // opening ---

    let mut yaml_block = String::new();
    let mut consumed = 0usize;
    for line in lines {
        consumed += line.len() + 1;
        if line.trim() == "---" || line.trim() == "..." {
            break;
        }
        yaml_block.push_str(line);
        yaml_block.push('\n');
    }

    if yaml_block.trim().is_empty() {
        return None;
    }

    let fm: SkillFrontmatter = serde_yaml::from_str(&yaml_block).ok()?;
    let name = fm
        .name
        .or(fm.title)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let name = name?;

    let mut platforms: Vec<String> = fm
        .platforms
        .into_iter()
        .chain(fm.compatibility.os)
        .map(|p| normalize_platform(&p))
        .filter(|p| !p.is_empty())
        .collect();
    platforms.sort();
    platforms.dedup();

    let mut deps: Vec<String> = fm
        .deps
        .into_iter()
        .chain(fm.compatibility.deps)
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();
    deps.sort();
    deps.dedup();

    let header_len = if let Some(idx) = trimmed.find("\n---\n") {
        idx + 5
    } else if let Some(idx) = trimmed.find("\n...\n") {
        idx + 5
    } else {
        // fallback to consumed length from line-by-line scan
        4 + consumed
    };

    let body = trimmed
        .get(header_len..)
        .unwrap_or_default()
        .trim()
        .to_string();

    let when_to_use = fm
        .when_to_use
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Some((
        SkillMetadata {
            name,
            description: fm.description,
            when_to_use,
            dir_path: dir_path.to_path_buf(),
            platforms,
            deps,
            source: fm
                .source
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "local".to_string()),
            version: fm
                .version
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            updated_at: fm
                .updated_at
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        },
        body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_md_valid() {
        let content = r#"---
name: pdf
description: Convert documents to PDF
when_to_use: "User asks to create or modify PDFs."
platforms: [linux, darwin]
deps: [pandoc]
---
Use this skill to convert documents.
"#;
        let dir = PathBuf::from("/tmp/skills/pdf");
        let result = parse_skill_md(content, &dir);
        assert!(result.is_some());
        let (meta, body) = result.unwrap();
        assert_eq!(meta.name, "pdf");
        assert_eq!(meta.description, "Convert documents to PDF");
        assert_eq!(
            meta.when_to_use.as_deref(),
            Some("User asks to create or modify PDFs.")
        );
        assert_eq!(meta.platforms, vec!["darwin", "linux"]);
        assert_eq!(meta.deps, vec!["pandoc"]);
        assert_eq!(meta.source, "local");
        assert!(body.contains("Use this skill"));
    }

    #[test]
    fn test_parse_skill_md_compatibility_os() {
        let content = r#"---
name: apple-notes
description: Apple Notes
compatibility:
  os:
    - darwin
  deps:
    - memo
---
Instructions.
"#;
        let dir = PathBuf::from("/tmp/skills/apple-notes");
        let (meta, _) = parse_skill_md(content, &dir).unwrap();
        assert_eq!(meta.platforms, vec!["darwin"]);
        assert_eq!(meta.deps, vec!["memo"]);
    }

    #[test]
    fn test_parse_skill_md_no_frontmatter() {
        let content = "Just some markdown without frontmatter.";
        let dir = PathBuf::from("/tmp/skills/test");
        assert!(parse_skill_md(content, &dir).is_none());
    }

    #[test]
    fn test_platform_allowed_empty_means_all() {
        assert!(platform_allowed(&[]));
    }

    #[test]
    fn test_build_skills_catalog_empty() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_skills_test_{}",
            uuid::Uuid::new_v4()
        ));
        let sm = SkillManager::new(dir.to_str().unwrap());
        let catalog = sm.build_skills_catalog();
        assert!(catalog.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_skills_catalog_excludes_body() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_skills_catalog_body_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("demo")).unwrap();
        let unique = "UNIQUE_BODY_TOKEN_XYZ789";
        let md = format!(
            "---\nname: demo\ndescription: Demo skill\nwhen_to_use: |\n  When testing catalog.\n---\n{unique}\n"
        );
        std::fs::write(dir.join("demo").join("SKILL.md"), md).unwrap();
        let sm = SkillManager::from_skills_dir(dir.to_str().unwrap());
        let catalog = sm.build_skills_catalog_with_mode(SkillsCatalogMode::Full);
        assert!(catalog.contains("demo"));
        assert!(catalog.contains("Demo skill"));
        assert!(catalog.contains("When testing catalog."));
        assert!(!catalog.contains(unique));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_skills_catalog_compact_omits_when_to_use() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_skills_compact_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("tiny")).unwrap();
        let md = "---\nname: tiny\ndescription: Short desc\nwhen_to_use: |\n  HIDDEN_WHEN_TO_USE_LINE\n---\nbody\n";
        std::fs::write(dir.join("tiny").join("SKILL.md"), md).unwrap();
        let sm = SkillManager::from_skills_dir(dir.to_str().unwrap());
        let catalog = sm.build_skills_catalog_with_mode(SkillsCatalogMode::Compact);
        assert!(catalog.contains("tiny"));
        assert!(catalog.contains("Short desc"));
        assert!(!catalog.contains("HIDDEN_WHEN_TO_USE_LINE"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_skills_catalog_truncates_long_when_to_use() {
        let dir = std::env::temp_dir().join(format!(
            "finally_a_value_bot_skills_when_long_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("long")).unwrap();
        let long = "a".repeat(900);
        let md = format!("---\nname: long\ndescription: x\nwhen_to_use: \"{long}\"\n---\nb\n");
        std::fs::write(dir.join("long").join("SKILL.md"), md).unwrap();
        let sm = SkillManager::from_skills_dir(dir.to_str().unwrap());
        let catalog = sm.build_skills_catalog_with_mode(SkillsCatalogMode::Full);
        assert!(catalog.contains("..."));
        assert!(!catalog.contains(&long));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
