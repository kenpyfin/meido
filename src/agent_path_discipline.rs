//! Shared strict path-discipline text for agent system prompts and skill-creation tools.

/// Full markdown section injected into `build_system_prompt`.
pub fn strict_path_discipline_section(
    tool_cwd_display: &str,
    workspace_data_root_display: &str,
    skills_dir_display: &str,
) -> String {
    format!(
        r##"
## Path discipline (strict)

Follow these rules on **every** turn for `read_file`, `write_file`, `edit_file`, `apply_search_replace`, `symbol_edit`, `glob`, `grep`, **`locate_file`**, **`bash`**, **`run_skill_script`**, and **`cursor_agent`** / **`build_skill`**.

**Tool cwd:** `{tool_cwd}` (persona-scoped under `{workspace_data_root_display}/shared/personas/...` when auth is present). Relative tool paths resolve from here—not from the configuration root and not by prefixing `WORKSPACE_DIR` as `workspace/...`.
**Locate first:** Prefer `locate_file` or a known `ORIGIN/…` / Tier-1 `Repo:` path before any filesystem walk. On a miss, ask the user — do not run `find`/`grep -r` over large trees.

### Allowed patterns (use these)

| Goal | Path in file/bash tools | Notes |
|------|---------------------------|-------|
| Files in the shared workspace | `ORIGIN/...`, `notes/foo.md`, `./report.pdf` | No `workspace/` prefix |
| Skills (read/write via tools) | `skills/<name>/SKILL.md`, `skills/<name>/script.py` | Resolves to `{skills_dir_display}/` |
| Create a new skill | **`build_skill`** after `activate_skill` `create-skill` | Writes under `{skills_dir_display}/` |
| Update an existing skill | **`build_skill`** or `skills/<name>/...` after `activate_skill` `modify-skill` | Required before mutating existing skills |
| Runtime / DB / groups | `runtime/...` | Not `workspace/runtime/...` |
| Skill credentials | `skills/<name>/.env` | Not project-root `.env` via file tools |
| Run a skill script | **`run_skill_script`**(`skill_name`, `script`, `args`) | Preferred over bash for `*_tool.py` and other bundled scripts; cwd is the skill dir |

### Forbidden patterns (never use)

- `workspace/...`, `workspace/shared/...`, `workspace/skills/...`, `workspace/runtime/...` in tool or bash paths → creates **`shared/workspace/`** (shadow tree; skills there are **not discovered**).
- `shared/skills/...` when cwd is already `shared/` → wrong nesting.
- Bash: `mkdir -p workspace/skills/...`, `cp ... workspace/skills/...`, or any shell path containing `workspace/skills/` while cwd is `shared/`.
- `../` chains to reach skills or runtime (e.g. `../../../../skills/...`) — use `skills/<name>/...` in file tools or **`run_skill_script`** for execution.
- Bash to run skill scripts when **`run_skill_script`** applies (e.g. `python3 ../../../../skills/foo/bar.py`).
- Treating `shared/workspace/` as `WORKSPACE_DIR` or as a canonical skills location.
- **Self-repo:** never use the finally-a-value-bot install/source git checkout as a project (no git branch deletes, force-push, reset, or source edits there). Persona Tier-1 **`Repo: /absolute/path`** target repos are **fully allowed** — `cd` there for commits and development.

### Shadow workspace

`{workspace_data_root_display}/shared/workspace/` is a mistaken duplicate. **Do not create or update files there.** Canonical skills live at `{skills_dir_display}/`. If you find shadow copies, migrate to canonical paths and remove shadow duplicates after verifying content.

### Skills checklist

1. **New skill:** activate `create-skill`, then prefer **`build_skill(name=..., ...)`** over manual `write_file`.
2. **Existing skill:** activate `modify-skill`, read current `SKILL.md`, then `build_skill` or targeted edits under `skills/<name>/`.
3. Prefer **`build_skill`** over manual `write_file` when rewriting most of a skill.
4. If using `write_file`, path must be `skills/<name>/SKILL.md` (tool-relative), never `workspace/skills/...`.
5. Skill shell examples in SKILL.md bodies should document **`run_skill_script`** (or `skills/<name>/script.py` for file reads), not bare `python3 script.py` from persona cwd or `workspace/skills/...`.
6. After creating a skill, it must exist at `{skills_dir_display}/<name>/SKILL.md` to be discoverable by `/skills` and `activate_skill`.
7. After **`activate_skill`**, run bundled scripts with **`run_skill_script`** using the `skill_name` and `script` from the activation output.
"##,
        tool_cwd = tool_cwd_display,
        workspace_data_root_display = workspace_data_root_display,
        skills_dir_display = skills_dir_display,
    )
}

/// Short footer for `build_skill` / cursor-agent skill-creation prompts.
pub fn build_skill_path_discipline_footer(skills_dir_display: &str, skill_name: &str) -> String {
    format!(
        r#"
PATH DISCIPLINE (mandatory):
- Write ONLY under: {skills_dir_display}/{skill_name}/
- Create or overwrite: {skills_dir_display}/{skill_name}/SKILL.md
- Do NOT write under shared/workspace/, workspace/skills/, or any path that prefixes workspace/ when cwd is shared/.
- Use the absolute paths above; do not create a nested shared/workspace/ tree.
"#,
        skills_dir_display = skills_dir_display,
        skill_name = skill_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_section_forbids_workspace_skills_prefix() {
        let s = strict_path_discipline_section("/data/shared", "/data", "/data/skills");
        assert!(s.contains("## Path discipline (strict)"));
        assert!(s.contains("workspace/skills/"));
        assert!(s.contains("shared/workspace/"));
        assert!(s.contains("/data/skills"));
        assert!(s.contains("run_skill_script"));
        assert!(s.contains("../../../../skills"));
        assert!(s.contains("Self-repo"));
        assert!(s.contains("Tier-1"));
        assert!(s.contains("locate_file"));
        assert!(s.contains("ask the user"));
    }

    #[test]
    fn build_skill_footer_uses_absolute_skill_dir() {
        let s = build_skill_path_discipline_footer("/data/skills", "my-skill");
        assert!(s.contains("/data/skills/my-skill/SKILL.md"));
        assert!(s.contains("Do NOT write under shared/workspace/"));
    }
}
