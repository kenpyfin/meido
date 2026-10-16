//! Built-in skills live in the repository `builtin_skills/` tree. They are discovered from disk
//! at runtime (no copy into `WORKSPACE_DIR/skills/`).

use crate::config::Config;
use std::path::PathBuf;

/// Resolve the on-disk `builtin_skills/` directory used for skill discovery and bundled scripts.
///
/// Precedence:
/// 1. `FINALLY_A_VALUE_BOT_BUILTIN_SKILLS` if set and the path exists
/// 2. Parent of the workspace data root + `builtin_skills` (typical layout: repo contains `workspace/` and `builtin_skills/`)
/// 3. Current working directory + `builtin_skills`
/// 4. Parent of the current executable + `builtin_skills` (deployment: ship folder next to the binary)
/// 5. Compile-time `CARGO_MANIFEST_DIR/builtin_skills` if it exists (e.g. `cargo run` from the crate root)
pub fn resolve_builtin_skills_dir(config: &Config) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FINALLY_A_VALUE_BOT_BUILTIN_SKILLS") {
        let pb = PathBuf::from(p.trim());
        if pb.is_dir() {
            return Some(pb);
        }
    }

    let parent_builtin = config
        .workspace_root_absolute()
        .parent()
        .map(|p| p.join("builtin_skills"));
    if let Some(ref p) = parent_builtin {
        if p.is_dir() {
            return Some(p.clone());
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("builtin_skills");
        if p.is_dir() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let p = parent.join("builtin_skills");
            if p.is_dir() {
                return Some(p);
            }
        }
    }

    let manifest_builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("builtin_skills");
    if manifest_builtin.is_dir() {
        return Some(manifest_builtin);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use std::fs;

    #[test]
    fn resolve_finds_sibling_of_workspace() {
        let tmp =
            std::env::temp_dir().join(format!("fab_builtin_skills_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(tmp.join("workspace")).unwrap();
        fs::create_dir_all(tmp.join("builtin_skills").join("demo")).unwrap();
        fs::write(
            tmp.join("builtin_skills").join("demo").join("SKILL.md"),
            "---\nname: demo\ndescription: x\n---\n",
        )
        .unwrap();

        let mut config = test_config();
        config.workspace_dir = tmp.join("workspace").to_string_lossy().to_string();

        let got = resolve_builtin_skills_dir(&config).expect("expected builtin_skills");
        assert_eq!(got, tmp.join("builtin_skills"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
