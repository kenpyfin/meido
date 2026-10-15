//! Enforce `activate_skill` for `modify-skill` before mutating existing workspace skills.

use std::path::{Component, Path, PathBuf};

pub const REQUIRED_MODIFY_SKILL: &str = "modify-skill";

const SKILL_MUTATION_TOOLS: &[&str] = &[
    "build_skill",
    "write_file",
    "edit_file",
    "apply_search_replace",
    "symbol_edit",
];

/// Tools that may change workspace skills and require `modify-skill` when the target already exists.
pub fn tool_name_can_mutate_skills(tool_name: &str) -> bool {
    SKILL_MUTATION_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tool_name))
}

/// Skill directory name targeted by this tool input, if any.
pub fn extract_skill_name_from_tool_input(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<String> {
    match tool_name {
        "build_skill" => input
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        "write_file" | "edit_file" | "apply_search_replace" | "symbol_edit" => input
            .get("path")
            .and_then(|v| v.as_str())
            .and_then(skill_name_from_path_str),
        _ => None,
    }
}

fn skill_name_from_path_str(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = Path::new(trimmed);
    if let Some(name) = first_component_after_skills_prefix(p) {
        return Some(name);
    }
    // Absolute path: .../skills/<name>/...
    for (idx, component) in p.components().enumerate() {
        if let Component::Normal(name) = component {
            if name.eq_ignore_ascii_case("skills") {
                if let Some(Component::Normal(skill_name)) = p.components().nth(idx + 1) {
                    return Some(skill_name.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

fn first_component_after_skills_prefix(path: &Path) -> Option<String> {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if let Component::Normal(name) = component {
            if name.eq_ignore_ascii_case("skills") {
                if let Some(Component::Normal(skill_name)) = components.next() {
                    return Some(skill_name.to_string_lossy().into_owned());
                }
                return None;
            }
        }
    }
    None
}

pub fn workspace_skill_exists(skills_data_dir: &Path, skill_name: &str) -> bool {
    let dir = skills_data_dir.join(skill_name);
    dir.join("SKILL.md").is_file() || dir.join("skill.md").is_file()
}

/// Resolve tool-relative `skills/...` paths the same way file tools do.
pub fn resolved_path_under_skills_data_dir(
    tool_shared_dir: &Path,
    skills_data_dir: &Path,
    path: &str,
) -> Option<PathBuf> {
    let workspace_root = tool_shared_dir.parent().unwrap_or(tool_shared_dir);
    let resolved = crate::tools::resolve_tool_path(workspace_root, tool_shared_dir, path);
    if resolved.starts_with(skills_data_dir) {
        Some(resolved)
    } else {
        None
    }
}

/// True when this tool call should be blocked until `modify-skill` is activated this turn.
pub fn requires_modify_skill_activation(
    tool_name: &str,
    input: &serde_json::Value,
    skills_data_dir: &Path,
    tool_shared_dir: &Path,
) -> bool {
    if !tool_name_can_mutate_skills(tool_name) {
        return false;
    }

    if tool_name == "build_skill" {
        let Some(skill_name) = extract_skill_name_from_tool_input(tool_name, input) else {
            return false;
        };
        return workspace_skill_exists(skills_data_dir, &skill_name);
    }

    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
        if let Some(resolved) =
            resolved_path_under_skills_data_dir(tool_shared_dir, skills_data_dir, path)
        {
            let skill_name = resolved
                .strip_prefix(skills_data_dir)
                .ok()
                .and_then(|rel| rel.components().next())
                .and_then(|c| match c {
                    Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                });
            if let Some(name) = skill_name {
                return workspace_skill_exists(skills_data_dir, &name)
                    || skills_data_dir.join(&name).is_dir();
            }
        }
        if let Some(name) = skill_name_from_path_str(path) {
            return workspace_skill_exists(skills_data_dir, &name)
                || skills_data_dir.join(&name).is_dir();
        }
    }

    false
}

pub fn modify_skill_required_error_message() -> String {
    format!(
        "Changing an existing skill requires activating `{REQUIRED_MODIFY_SKILL}` first in this turn. \
         Call `activate_skill` with skill_name `{REQUIRED_MODIFY_SKILL}`, read the current SKILL.md, \
         then use `build_skill` or file tools under `skills/<name>/`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn extract_skill_name_from_tool_relative_path() {
        assert_eq!(
            skill_name_from_path_str("skills/my-skill/SKILL.md").as_deref(),
            Some("my-skill")
        );
    }

    #[test]
    fn build_skill_requires_gate_when_skill_exists() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(skills.join("demo")).unwrap();
        fs::write(skills.join("demo").join("SKILL.md"), "---\nname: demo\n").unwrap();
        let shared = tmp.path().join("shared");
        fs::create_dir_all(&shared).unwrap();

        let input = serde_json::json!({ "name": "demo" });
        assert!(requires_modify_skill_activation(
            "build_skill",
            &input,
            &skills,
            &shared,
        ));
    }

    #[test]
    fn build_skill_skips_gate_for_new_skill() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join("skills");
        fs::create_dir_all(&skills).unwrap();
        let shared = tmp.path().join("shared");
        fs::create_dir_all(&shared).unwrap();

        let input = serde_json::json!({ "name": "brand-new" });
        assert!(!requires_modify_skill_activation(
            "build_skill",
            &input,
            &skills,
            &shared,
        ));
    }
}
