use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use crate::claude::ToolDefinition;
use crate::db::Database;
use crate::runtime_toggles::RuntimeToggles;
use crate::skills::SkillManager;

use super::command_runner::{build_command_with_env, run_managed_command, ManagedCommandOutcome};
use super::tool_cancel::current_tool_cancel;
use super::{auth_context_from_input, schema_object, Tool, ToolResult};

pub struct RunSkillScriptTool {
    skill_manager: SkillManager,
    db: Option<Arc<Database>>,
    runtime_toggles: Arc<RuntimeToggles>,
    runtime_data_dir: String,
}

impl RunSkillScriptTool {
    pub fn new_with_dirs_and_db(
        dirs: impl IntoIterator<Item = impl AsRef<Path>>,
        db: Arc<Database>,
        runtime_toggles: Arc<RuntimeToggles>,
    ) -> Self {
        Self {
            skill_manager: SkillManager::from_skills_dirs(dirs),
            db: Some(db),
            runtime_toggles,
            runtime_data_dir: String::new(),
        }
    }

    pub fn with_runtime_data_dir(mut self, runtime_data_dir: impl Into<String>) -> Self {
        self.runtime_data_dir = runtime_data_dir.into();
        self
    }

    #[cfg(test)]
    pub fn new_for_test(dirs: impl IntoIterator<Item = impl AsRef<Path>>) -> Self {
        Self {
            skill_manager: SkillManager::from_skills_dirs(dirs),
            db: None,
            runtime_toggles: RuntimeToggles::new(false),
            runtime_data_dir: String::new(),
        }
    }
}

/// If the skill dir has exactly one `*_tool.py`, return its file name.
pub fn primary_tool_script_name(skill_dir: &Path) -> Option<String> {
    let mut matches = list_tool_py_scripts(skill_dir);
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn list_tool_py_scripts(skill_dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(skill_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with("_tool.py") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    matches.sort();
    matches
}

fn list_cli_scripts(skill_dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(skill_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with("_cli.py") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    matches.sort();
    matches
}

/// Parse runnable script filenames from SKILL.md `## Scripts` section or inline backticks.
pub fn scripts_from_skill_md(skill_dir: &Path) -> Vec<String> {
    let skill_md = skill_dir.join("SKILL.md");
    let Ok(body) = std::fs::read_to_string(&skill_md) else {
        return Vec::new();
    };
    let mut scripts = Vec::new();
    let mut in_scripts = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            in_scripts = trimmed.eq_ignore_ascii_case("## Scripts")
                || trimmed.to_ascii_lowercase().starts_with("## scripts");
            continue;
        }
        if in_scripts {
            if trimmed.starts_with("## ") {
                break;
            }
            for token in extract_script_tokens(trimmed) {
                if !scripts.contains(&token) {
                    scripts.push(token);
                }
            }
        }
    }
    for token in extract_script_tokens(&body) {
        if (token.ends_with(".py") || token.ends_with(".sh") || token.ends_with(".js"))
            && skill_dir.join(&token).is_file()
            && !scripts.contains(&token)
        {
            scripts.push(token);
        }
    }
    scripts.sort();
    scripts
}

fn extract_script_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in line.split(['`', '(', ')', '[', ']', ',']) {
        let p = part.trim();
        if p.contains('/') {
            continue;
        }
        if p.ends_with(".py") || p.ends_with(".sh") || p.ends_with(".js") {
            out.push(p.to_string());
        }
    }
    out
}

/// Collect candidate runnable script names for hints and validation.
pub fn runnable_script_candidates(skill_dir: &Path) -> Vec<String> {
    if let Some(one) = primary_tool_script_name(skill_dir) {
        return vec![one];
    }
    let from_md = scripts_from_skill_md(skill_dir);
    if !from_md.is_empty() {
        return from_md;
    }
    let cli = list_cli_scripts(skill_dir);
    if cli.len() == 1 {
        return cli;
    }
    if !cli.is_empty() {
        return cli;
    }
    Vec::new()
}

pub fn is_shell_like_script_name(script: &str) -> bool {
    matches!(
        script.trim().to_ascii_lowercase().as_str(),
        "ls" | "bash"
            | "sh"
            | "python"
            | "python3"
            | "node"
            | "npm"
            | "npx"
            | "cat"
            | "echo"
            | "find"
            | "grep"
            | "wc"
            | "pwd"
            | "cd"
            | "cp"
            | "mv"
            | "rm"
            | "mkdir"
            | "chmod"
            | "curl"
            | "wget"
            | "."
            | ".."
    )
}

/// True when `script` looks like a skill file name (not a shell command).
pub fn looks_like_skill_script_filename(script: &str) -> bool {
    let trimmed = script.trim();
    if trimmed.is_empty() || trimmed.contains("..") || Path::new(trimmed).is_absolute() {
        return false;
    }
    if is_shell_like_script_name(trimmed) {
        return false;
    }
    Path::new(trimmed)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "py" | "sh" | "js" | "mjs" | "cjs" | "ps1"
            )
        })
        .unwrap_or(false)
}

/// Hint line appended by `activate_skill` when runnable scripts exist.
pub fn format_run_skill_script_hint(skill_name: &str, skill_dir: &Path) -> Option<String> {
    let candidates = runnable_script_candidates(skill_dir);
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        let script = &candidates[0];
        return Some(format!(
            "Run scripts: run_skill_script(skill_name=\"{skill_name}\", script=\"{script}\", args=[\"--help\"])\n"
        ));
    }
    let numbered = candidates
        .iter()
        .enumerate()
        .map(|(i, s)| format!("  {}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "Run scripts (pick one filename):\n{numbered}\nExample: run_skill_script(skill_name=\"{skill_name}\", script=\"{}\", args=[...])\n",
        candidates[0]
    ))
}

/// Resolve a hint for pipeline validation when a skill was activated.
pub fn runnable_script_hint_for_skill(
    state: &crate::telegram::AppState,
    skill_name: &str,
) -> Option<String> {
    let manager = SkillManager::from_skills_dirs(state.config.skill_discovery_dirs());
    let (meta, _body) = manager.load_skill(skill_name)?;
    format_run_skill_script_hint(skill_name, &meta.dir_path)
}

pub fn resolve_script_under_skill_dir(skill_dir: &Path, script: &str) -> Result<PathBuf, String> {
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return Err("Parameter 'script' must not be empty.".into());
    }
    if trimmed.contains("..") || Path::new(trimmed).is_absolute() {
        return Err(format!(
            "Invalid script path '{trimmed}': must be a relative file under the skill directory (no '..')."
        ));
    }

    let joined = skill_dir.join(trimmed);
    if !joined.is_file() {
        return Err(format!(
            "Script not found: {} (skill directory: {})",
            joined.display(),
            skill_dir.display()
        ));
    }

    let skill_canon = std::fs::canonicalize(skill_dir).unwrap_or_else(|_| skill_dir.to_path_buf());
    let resolved = std::fs::canonicalize(&joined)
        .map_err(|e| format!("Failed to resolve script path {}: {e}", joined.display()))?;
    if !resolved.starts_with(&skill_canon) {
        return Err(format!(
            "Script path escapes skill directory: {}",
            resolved.display()
        ));
    }

    let resolved_str = resolved.to_string_lossy().to_string();
    super::path_guard::check_path(&resolved_str)?;

    Ok(resolved)
}

pub fn default_interpreter(script_path: &Path, deps: &[String]) -> Result<String, String> {
    if let Some(interpreter) = interpreter_from_deps(deps) {
        return Ok(interpreter);
    }

    let ext = script_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    match ext {
        "py" => Ok("python3".into()),
        "js" | "mjs" | "cjs" => Ok("node".into()),
        "sh" => Ok("bash".into()),
        "ps1" => Ok("powershell".into()),
        other => Err(format!(
            "Cannot infer interpreter for .{other} script; pass interpreter explicitly or add deps to the skill frontmatter."
        )),
    }
}

fn interpreter_from_deps(deps: &[String]) -> Option<String> {
    for dep in deps {
        let d = dep.trim();
        if d.eq_ignore_ascii_case("python3")
            || d.eq_ignore_ascii_case("python")
            || d.starts_with("python")
        {
            return Some(if d.eq_ignore_ascii_case("python") {
                "python3".into()
            } else {
                d.to_string()
            });
        }
        if d.eq_ignore_ascii_case("node") || d.eq_ignore_ascii_case("nodejs") {
            return Some("node".into());
        }
        if d.eq_ignore_ascii_case("bash") || d.eq_ignore_ascii_case("sh") {
            return Some("bash".into());
        }
    }
    None
}

fn parse_args(input: &serde_json::Value) -> Result<Vec<String>, String> {
    let Some(value) = input.get("args") else {
        return Ok(Vec::new());
    };
    let arr = value
        .as_array()
        .ok_or_else(|| "Parameter 'args' must be an array of strings.".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let s = item
            .as_str()
            .ok_or_else(|| format!("Parameter 'args[{i}]' must be a string."))?;
        out.push(s.to_string());
    }
    Ok(out)
}

async fn check_persona_skill_allowed(
    db: &Database,
    input: &serde_json::Value,
    skill_name: &str,
) -> Option<ToolResult> {
    let auth = auth_context_from_input(input)?;
    match db.is_skill_allowed_for_persona(auth.caller_chat_id, auth.caller_persona_id, skill_name) {
        Ok(false) => Some(ToolResult::error(format!(
            "Skill '{skill_name}' is not allowed for persona {}.",
            auth.caller_persona_id
        ))),
        Ok(true) => None,
        Err(e) => Some(ToolResult::error(format!(
            "Failed to evaluate skill policy for '{skill_name}': {e}"
        ))),
    }
}

fn format_process_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let mut result_text = String::new();
    if !stdout.is_empty() {
        result_text.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result_text.is_empty() {
            result_text.push('\n');
        }
        result_text.push_str("STDERR:\n");
        result_text.push_str(stderr);
    }
    if result_text.is_empty() {
        result_text = format!("Command completed with exit code {exit_code}");
    }
    if result_text.len() > 30000 {
        result_text.truncate(30000);
        result_text.push_str("\n... (output truncated)");
    }
    result_text
}

#[async_trait]
impl Tool for RunSkillScriptTool {
    fn name(&self) -> &str {
        "run_skill_script"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_skill_script",
            "Run a script from an agent skill directory. Prefer this over bash for skill bundled scripts (e.g. read_email_tool.py): paths resolve to the canonical skill folder regardless of persona tool cwd. Activate the skill first when you need its full SKILL.md instructions.",
            schema_object(
                json!({
                    "skill_name": {
                        "type": "string",
                        "description": "Skill name (same as activate_skill / catalog)"
                    },
                    "script": {
                        "type": "string",
                        "description": "Script file under the skill directory, e.g. read_email_tool.py (no .. or absolute paths)"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments passed to the script after the script path"
                    },
                    "interpreter": {
                        "type": "string",
                        "description": "Executable to run the script (default: inferred from extension and skill deps, e.g. python3)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 3600)"
                    }
                }),
                &["skill_name", "script"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let skill_name = match input.get("skill_name").and_then(|v| v.as_str()) {
            Some(n) => n.trim(),
            None => return ToolResult::error("Missing required parameter: skill_name".into()),
        };
        if skill_name.is_empty() {
            return ToolResult::error("Parameter 'skill_name' must not be empty.".into());
        }

        let script = match input.get("script").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing required parameter: script".into()),
        };

        let args = match parse_args(&input) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };

        let timeout_secs = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        if let (Some(db), Some(_)) = (&self.db, auth_context_from_input(&input)) {
            if let Some(blocked) = check_persona_skill_allowed(db, &input, skill_name).await {
                return blocked;
            }
        }

        let (meta, _body) = match self.skill_manager.load_skill_checked(skill_name) {
            Ok(pair) => pair,
            Err(e) => return ToolResult::error(e),
        };

        let script_path = match resolve_script_under_skill_dir(&meta.dir_path, script) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };

        let interpreter = match input.get("interpreter").and_then(|v| v.as_str()) {
            Some(i) if !i.trim().is_empty() => i.trim().to_string(),
            _ => match default_interpreter(&script_path, &meta.deps) {
                Ok(i) => i,
                Err(e) => return ToolResult::error(e),
            },
        };

        info!(
            "run_skill_script: {} {}/{} {:?}",
            interpreter, skill_name, script, args
        );

        let mut cmd = build_command_with_env(
            &super::command_runner::CommandSpec {
                program: interpreter.clone(),
                args: {
                    let mut a = vec![script_path.to_string_lossy().into_owned()];
                    a.extend(args.clone());
                    a
                },
            },
            Some(&meta.dir_path),
            self.runtime_toggles.tool_output_debug(),
            None,
        );
        if let Some(auth) = super::auth_context_from_input(&input) {
            if !self.runtime_data_dir.is_empty() {
                crate::secret_vault::apply_chat_secret_env(
                    &mut cmd,
                    &self.runtime_data_dir,
                    auth.caller_chat_id,
                );
            }
        }

        let cancel = current_tool_cancel();
        let outcome = run_managed_command(cmd, timeout_secs, cancel.as_ref()).await;

        match outcome {
            ManagedCommandOutcome::Completed(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);
                let result_text = format_process_output(&stdout, &stderr, exit_code);

                if exit_code == 0 {
                    ToolResult::success(result_text).with_status_code(exit_code)
                } else {
                    ToolResult::error(format!("Exit code {exit_code}\n{result_text}"))
                        .with_status_code(exit_code)
                        .with_error_type("process_exit")
                }
            }
            ManagedCommandOutcome::SpawnError(e) => {
                ToolResult::error(format!("Failed to execute script: {e}"))
                    .with_error_type("spawn_error")
            }
            ManagedCommandOutcome::TimedOut => {
                ToolResult::error(format!("Script timed out after {timeout_secs} seconds"))
                    .with_error_type("timeout")
            }
            ManagedCommandOutcome::Cancelled => {
                ToolResult::error("Script cancelled because the Cursor run ended".into())
                    .with_error_type("run_cancelled")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "finally_a_value_bot_run_skill_script_test_{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn create_skill_with_script(base_dir: &Path, name: &str) -> PathBuf {
        let skill_dir = base_dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let content = format!(
            "---\nname: {name}\ndescription: Test skill\ndeps:\n  - python3\n---\n# Test\n"
        );
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
        std::fs::write(
            skill_dir.join("demo_tool.py"),
            "#!/usr/bin/env python3\nimport sys\nprint('ok', sys.argv[1] if len(sys.argv) > 1 else '')\n",
        )
        .unwrap();
        skill_dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_resolve_script_rejects_parent_traversal() {
        let dir = test_dir();
        let skill_dir = create_skill_with_script(&dir, "demo");
        let err = resolve_script_under_skill_dir(&skill_dir, "../SKILL.md").unwrap_err();
        assert!(err.contains(".."));
        cleanup(&dir);
    }

    #[test]
    fn test_resolve_script_accepts_file_in_skill_dir() {
        let dir = test_dir();
        let skill_dir = create_skill_with_script(&dir, "demo");
        let resolved = resolve_script_under_skill_dir(&skill_dir, "demo_tool.py").expect("resolve");
        assert!(resolved.ends_with("demo_tool.py"));
        cleanup(&dir);
    }

    #[test]
    fn test_primary_tool_script_name_single_match() {
        let dir = test_dir();
        let skill_dir = create_skill_with_script(&dir, "demo");
        assert_eq!(
            primary_tool_script_name(&skill_dir).as_deref(),
            Some("demo_tool.py")
        );
        cleanup(&dir);
    }

    fn create_hotify_style_skill(base_dir: &Path, name: &str) -> PathBuf {
        let skill_dir = base_dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pz-hotify\ndescription: Hotify\n---\n## Scripts\n- `hotify_cli.py`\n",
        )
        .unwrap();
        std::fs::write(
            skill_dir.join("hotify_cli.py"),
            "#!/usr/bin/env python3\nprint('ok')\n",
        )
        .unwrap();
        skill_dir
    }

    #[test]
    fn test_cli_script_hint_without_tool_py() {
        let dir = test_dir();
        let skill_dir = create_hotify_style_skill(&dir, "pz-hotify");
        let hint = format_run_skill_script_hint("pz-hotify", &skill_dir).expect("hint");
        assert!(hint.contains("hotify_cli.py"));
        cleanup(&dir);
    }

    #[test]
    fn test_is_shell_like_script_name() {
        assert!(is_shell_like_script_name("bash"));
        assert!(is_shell_like_script_name("find"));
        assert!(is_shell_like_script_name("."));
        assert!(!is_shell_like_script_name("hotify_cli.py"));
    }

    #[test]
    fn test_looks_like_skill_script_filename() {
        assert!(looks_like_skill_script_filename("hotify_cli.py"));
        assert!(!looks_like_skill_script_filename("find"));
        assert!(!looks_like_skill_script_filename("."));
    }

    #[test]
    fn test_default_interpreter_python() {
        let dir = test_dir();
        let skill_dir = create_skill_with_script(&dir, "demo");
        let script = skill_dir.join("demo_tool.py");
        assert_eq!(
            default_interpreter(&script, &["python3".into()]).unwrap(),
            "python3"
        );
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_skill_script_executes() {
        let dir = test_dir();
        create_skill_with_script(&dir, "demo");
        let tool = RunSkillScriptTool::new_for_test([&dir]);
        let result = tool
            .execute(json!({
                "skill_name": "demo",
                "script": "demo_tool.py",
                "args": ["--help"],
                "timeout_secs": 30
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("ok"));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_skill_script_missing_skill() {
        let dir = test_dir();
        let tool = RunSkillScriptTool::new_for_test([&dir]);
        let result = tool
            .execute(json!({
                "skill_name": "missing",
                "script": "demo_tool.py"
            }))
            .await;
        assert!(result.is_error);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_skill_script_missing_params() {
        let dir = test_dir();
        let tool = RunSkillScriptTool::new_for_test([&dir]);
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("skill_name"));
        cleanup(&dir);
    }
}
