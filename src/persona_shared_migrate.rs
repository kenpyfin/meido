use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::Database;

const MIGRATION_SETTING_KEY: &str = "PERSONA_SHARED_MIGRATED";

#[derive(Debug, Default, Clone, Copy)]
pub struct PersonaSharedMigrationStats {
    pub moved_to_persona: usize,
    pub moved_to_skills: usize,
    pub moved_to_unmigrated: usize,
}

#[derive(Debug, Clone)]
struct PersonaIndex {
    chat_id: i64,
    persona_id: i64,
    corpus: String,
}

pub fn maybe_run(config: &Config, db: &Database) -> Result<PersonaSharedMigrationStats, String> {
    if migration_done(db)? {
        return Ok(PersonaSharedMigrationStats::default());
    }
    let stats = run_migration(config)?;
    db.set_app_setting(MIGRATION_SETTING_KEY, "1")
        .map_err(|e| format!("Failed to write migration app setting: {e}"))?;
    Ok(stats)
}

fn migration_done(db: &Database) -> Result<bool, String> {
    let settings = db
        .list_app_settings()
        .map_err(|e| format!("Failed to read app settings: {e}"))?;
    Ok(settings
        .iter()
        .any(|s| s.key == MIGRATION_SETTING_KEY && s.value == "1"))
}

fn run_migration(config: &Config) -> Result<PersonaSharedMigrationStats, String> {
    let workspace_root = config.workspace_root_absolute();
    let shared_root = workspace_root.join("shared");
    if !shared_root.is_dir() {
        return Ok(PersonaSharedMigrationStats::default());
    }

    let persona_index = collect_persona_index(Path::new(&config.runtime_data_dir()));
    let known_skills = collect_known_skills(&workspace_root);
    let mut stats = PersonaSharedMigrationStats::default();

    let skip_top_level: HashSet<&str> = [
        "ORIGIN",
        "vault_db",
        ".venv-vault",
        "skills",
        "personas",
        "upload",
        "workspace",
        "_unmigrated",
    ]
    .into_iter()
    .collect();

    let entries = std::fs::read_dir(&shared_root)
        .map_err(|e| format!("Failed to scan shared dir '{}': {e}", shared_root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(v) => v.to_string(),
            None => continue,
        };
        if skip_top_level.contains(name.as_str()) {
            continue;
        }
        if name == "scripts" && path.is_dir() {
            migrate_scripts_dir(
                &shared_root,
                &workspace_root,
                &path,
                &persona_index,
                &known_skills,
                &mut stats,
            )?;
            continue;
        }
        migrate_flat_entry(
            &shared_root,
            &workspace_root,
            &path,
            &persona_index,
            &known_skills,
            &mut stats,
        )?;
    }

    Ok(stats)
}

fn collect_persona_index(runtime_root: &Path) -> Vec<PersonaIndex> {
    let groups_root = runtime_root.join("groups");
    let mut out = Vec::new();
    let Ok(chat_dirs) = std::fs::read_dir(groups_root) else {
        return out;
    };
    for chat in chat_dirs.flatten() {
        let chat_id = match chat.file_name().to_string_lossy().parse::<i64>().ok() {
            Some(v) => v,
            None => continue,
        };
        let Ok(persona_dirs) = std::fs::read_dir(chat.path()) else {
            continue;
        };
        for persona in persona_dirs.flatten() {
            let persona_id = match persona.file_name().to_string_lossy().parse::<i64>().ok() {
                Some(v) => v,
                None => continue,
            };
            let history_dir = persona.path().join("agent_history");
            let corpus = read_history_corpus(&history_dir);
            out.push(PersonaIndex {
                chat_id,
                persona_id,
                corpus,
            });
        }
    }
    out
}

fn read_history_corpus(history_dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(history_dir) else {
        return String::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    files.sort();
    files.reverse();
    let mut out = String::new();
    for p in files.into_iter().take(20) {
        if let Ok(s) = std::fs::read_to_string(&p) {
            out.push_str(&s);
            out.push('\n');
            if out.len() > 2_000_000 {
                break;
            }
        }
    }
    out
}

fn collect_known_skills(workspace_root: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    for dir in [
        workspace_root.join("skills"),
        workspace_root.join("shared").join("skills"),
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            if e.path().is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    names.insert(name.to_ascii_lowercase());
                }
            }
        }
    }
    names
}

fn migrate_scripts_dir(
    shared_root: &Path,
    workspace_root: &Path,
    scripts_dir: &Path,
    personas: &[PersonaIndex],
    known_skills: &HashSet<String>,
    stats: &mut PersonaSharedMigrationStats,
) -> Result<(), String> {
    let files = collect_files_recursively(scripts_dir)?;
    for src in files {
        let rel = src
            .strip_prefix(shared_root)
            .map_err(|e| format!("strip_prefix failed for '{}': {e}", src.display()))?
            .to_path_buf();
        if let Some(dst) = classify_skill_destination(workspace_root, known_skills, &rel) {
            move_path(&src, &dst)?;
            stats.moved_to_skills += 1;
            continue;
        }
        if let Some((chat_id, persona_id)) = choose_persona_for_path(personas, &rel) {
            let dst =
                crate::tools::persona_shared_dir(workspace_root, chat_id, persona_id).join(&rel);
            move_path(&src, &dst)?;
            stats.moved_to_persona += 1;
            continue;
        }
        let dst = shared_root.join("_unmigrated").join(&rel);
        move_path(&src, &dst)?;
        stats.moved_to_unmigrated += 1;
    }
    remove_empty_dirs(scripts_dir);
    Ok(())
}

fn migrate_flat_entry(
    shared_root: &Path,
    workspace_root: &Path,
    src: &Path,
    personas: &[PersonaIndex],
    known_skills: &HashSet<String>,
    stats: &mut PersonaSharedMigrationStats,
) -> Result<(), String> {
    let rel = src
        .strip_prefix(shared_root)
        .map_err(|e| format!("strip_prefix failed for '{}': {e}", src.display()))?
        .to_path_buf();
    if let Some(dst) = classify_skill_destination(workspace_root, known_skills, &rel) {
        move_path(src, &dst)?;
        stats.moved_to_skills += 1;
        return Ok(());
    }
    if let Some((chat_id, persona_id)) = choose_persona_for_path(personas, &rel) {
        let dst = crate::tools::persona_shared_dir(workspace_root, chat_id, persona_id).join(&rel);
        move_path(src, &dst)?;
        stats.moved_to_persona += 1;
        return Ok(());
    }
    let dst = shared_root.join("_unmigrated").join(rel);
    move_path(src, &dst)?;
    stats.moved_to_unmigrated += 1;
    Ok(())
}

fn classify_skill_destination(
    workspace_root: &Path,
    known_skills: &HashSet<String>,
    rel: &Path,
) -> Option<PathBuf> {
    let rel_str = rel.to_string_lossy().to_ascii_lowercase();
    for skill in known_skills {
        if rel_str.contains(skill) {
            let file_name = rel
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("helper.txt");
            return Some(workspace_root.join("skills").join(skill).join(file_name));
        }
    }
    None
}

fn choose_persona_for_path(personas: &[PersonaIndex], rel: &Path) -> Option<(i64, i64)> {
    if personas.is_empty() {
        return None;
    }
    if personas.len() == 1 {
        return Some((personas[0].chat_id, personas[0].persona_id));
    }
    let rel_str = rel.to_string_lossy();
    let basename = rel
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let mut best: Option<(usize, i64, i64)> = None;
    for p in personas {
        let mut score = 0usize;
        if !rel_str.is_empty() && p.corpus.contains(rel_str.as_ref()) {
            score += 3;
        }
        if !basename.is_empty() && p.corpus.contains(&basename) {
            score += 1;
        }
        if score == 0 {
            continue;
        }
        match best {
            Some((best_score, _, _)) if best_score >= score => {}
            _ => best = Some((score, p.chat_id, p.persona_id)),
        }
    }
    best.map(|(_, c, p)| (c, p))
}

fn collect_files_recursively(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("Failed to scan '{}': {e}", dir.display()))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(collect_files_recursively(&p)?);
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(out)
}

fn move_path(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create '{}': {e}", parent.display()))?;
    }
    let mut target = dst.to_path_buf();
    if target.exists() {
        target = unique_suffixed_path(dst);
    }
    match std::fs::rename(src, &target) {
        Ok(_) => Ok(()),
        Err(_) => {
            if src.is_dir() {
                copy_dir_all(src, &target)?;
                std::fs::remove_dir_all(src)
                    .map_err(|e| format!("Failed to remove '{}': {e}", src.display()))?;
            } else {
                std::fs::copy(src, &target).map_err(|e| {
                    format!(
                        "Failed to copy '{}' -> '{}': {e}",
                        src.display(),
                        target.display()
                    )
                })?;
                std::fs::remove_file(src)
                    .map_err(|e| format!("Failed to remove '{}': {e}", src.display()))?;
            }
            Ok(())
        }
    }
}

fn unique_suffixed_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    for i in 1..1000 {
        let name = if ext.is_empty() {
            format!("{stem}.migrated-{i}")
        } else {
            format!("{stem}.migrated-{i}.{ext}")
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.with_file_name(format!("{stem}.migrated-overflow"))
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create '{}': {e}", dst.display()))?;
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("Failed to read '{}': {e}", src.display()))?;
    for entry in entries.flatten() {
        let child_src = entry.path();
        let child_dst = dst.join(entry.file_name());
        if child_src.is_dir() {
            copy_dir_all(&child_src, &child_dst)?;
        } else {
            std::fs::copy(&child_src, &child_dst).map_err(|e| {
                format!(
                    "Failed to copy '{}' -> '{}': {e}",
                    child_src.display(),
                    child_dst.display()
                )
            })?;
        }
    }
    Ok(())
}

fn remove_empty_dirs(dir: &Path) {
    let _ = std::fs::remove_dir(dir);
}
