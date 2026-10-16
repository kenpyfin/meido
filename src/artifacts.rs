//! Durable, persona-scoped publications. Working files are never served directly.
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub chat_id: i64,
    pub persona_id: i64,
    pub name: String,
    pub kind: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub source: String,
    pub source_path: String,
    pub sha256: String,
    pub status: String,
    pub availability: String,
    pub pinned: bool,
    pub job_id: Option<String>,
    pub session_id: Option<String>,
    pub message_id: Option<String>,
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub trashed_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    pub url: String,
    pub preview_url: String,
    #[serde(skip)]
    pub blob_path: PathBuf,
}

#[derive(Default, Deserialize)]
pub struct Filter {
    pub q: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub job_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct Page {
    pub artifacts: Vec<Artifact>,
    pub next_cursor: Option<String>,
}

pub struct Catalog {
    conn: Connection,
    root: PathBuf,
}

/// Unpinned trash is removed after this many days. Pinned trash is kept.
pub const TRASH_RETENTION_DAYS: i64 = 30;
const TRASH_DUE_BATCH: usize = 100;
const COLS: &str = "id,chat_id,persona_id,name,kind,size_bytes,created_at,source,source_path,sha256,status,pinned,job_id,session_id,message_id,superseded_by,blob_path,trashed_at";
fn stamp(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
fn trash_expires_at(status: &str, trashed_at: Option<&str>) -> Option<String> {
    if status != "trash" {
        return None;
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(trashed_at?).ok()?;
    Some(stamp(
        parsed.with_timezone(&chrono::Utc) + chrono::Duration::days(TRASH_RETENTION_DAYS),
    ))
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    let id: String = r.get(0)?;
    let url = format!("/api/artifacts/{id}/content");
    let status: String = r.get(10)?;
    let trashed_at: Option<String> = r.get(17)?;
    let expires_at = trash_expires_at(&status, trashed_at.as_deref());
    Ok(Artifact {
        id,
        chat_id: r.get(1)?,
        persona_id: r.get(2)?,
        name: r.get(3)?,
        kind: r.get(4)?,
        size_bytes: r.get(5)?,
        created_at: r.get(6)?,
        source: r.get(7)?,
        source_path: r.get(8)?,
        sha256: r.get(9)?,
        status,
        availability: if PathBuf::from(r.get::<_, String>(16)?).is_file() {
            "available"
        } else {
            "missing"
        }
        .into(),
        pinned: r.get(11)?,
        job_id: r.get(12)?,
        session_id: r.get(13)?,
        message_id: r.get(14)?,
        superseded_by: r.get(15)?,
        trashed_at,
        expires_at,
        blob_path: PathBuf::from(r.get::<_, String>(16)?),
        preview_url: format!("{url}?preview=true"),
        url,
    })
}

pub fn kind(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => "image",
        "mp4" | "webm" | "mov" | "m4v" => "video",
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => "audio",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "txt" | "json" | "log" | "csv" | "yml" | "yaml" | "xml" => "text",
        "pdf" => "pdf",
        _ => "other",
    }
}

fn catalog_column(conn: &Connection, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(artifacts)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl Catalog {
    pub fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let dir = root.join("runtime/artifacts");
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join("catalog.sqlite3"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS artifacts (
            id TEXT PRIMARY KEY, chat_id INTEGER NOT NULL, persona_id INTEGER NOT NULL,
            name TEXT NOT NULL, kind TEXT NOT NULL, size_bytes INTEGER NOT NULL,
            created_at TEXT NOT NULL, source TEXT NOT NULL, source_path TEXT NOT NULL,
            sha256 TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'active', pinned INTEGER NOT NULL DEFAULT 0,
            job_id TEXT,session_id TEXT,message_id TEXT,superseded_by TEXT,blob_path TEXT NOT NULL,trashed_at TEXT);
          CREATE INDEX IF NOT EXISTS artifacts_page ON artifacts(chat_id,persona_id,status,created_at DESC,id DESC);
          CREATE INDEX IF NOT EXISTS artifacts_source ON artifacts(chat_id,persona_id,source_path,sha256);
          CREATE INDEX IF NOT EXISTS artifacts_job ON artifacts(chat_id,persona_id,job_id);
          CREATE TABLE IF NOT EXISTS artifact_aliases(url TEXT PRIMARY KEY,artifact_id TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS artifact_migrations(scope TEXT PRIMARY KEY, complete INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS artifact_runs(artifact_id TEXT NOT NULL,run_id TEXT NOT NULL,PRIMARY KEY(artifact_id,run_id));
          CREATE INDEX IF NOT EXISTS artifact_runs_run ON artifact_runs(run_id);
          CREATE TABLE IF NOT EXISTS artifact_jobs(artifact_id TEXT NOT NULL,job_id TEXT NOT NULL,PRIMARY KEY(artifact_id,job_id));
          CREATE INDEX IF NOT EXISTS artifact_jobs_job ON artifact_jobs(job_id);
          CREATE TABLE IF NOT EXISTS artifact_locations(artifact_id TEXT NOT NULL,path TEXT NOT NULL,sha256 TEXT NOT NULL,managed INTEGER NOT NULL,PRIMARY KEY(artifact_id,path));
          CREATE TABLE IF NOT EXISTS artifact_deletions(artifact_id TEXT PRIMARY KEY,delete_original INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS artifact_messages(artifact_id TEXT NOT NULL,message_id TEXT NOT NULL,PRIMARY KEY(artifact_id,message_id));")?;
        if !catalog_column(&conn, "trashed_at")? {
            conn.execute("ALTER TABLE artifacts ADD COLUMN trashed_at TEXT", [])?;
            // Rows already in Trash get a full retention window from this upgrade.
            conn.execute(
                "UPDATE artifacts SET trashed_at=?1 WHERE status='trash' AND trashed_at IS NULL",
                [stamp(chrono::Utc::now())],
            )?;
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS artifacts_trash_due ON artifacts(status, pinned, trashed_at);")?;
        Ok(Self { conn, root })
    }

    pub fn get(&self, id: &str) -> Result<Artifact> {
        Ok(self.conn.query_row(
            &format!("SELECT {COLS} FROM artifacts WHERE id=?1"),
            [id],
            row,
        )?)
    }
    pub fn alias(&self, url: &str) -> Result<Option<Artifact>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT artifact_id FROM artifact_aliases WHERE url=?1",
                [url],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|id| self.get(&id)).transpose()
    }
    pub fn missing_reference(&self, cid: i64, pid: i64, url: &str, time: &str) -> Result<Artifact> {
        if let Some(a) = self.alias(url)? {
            return Ok(a);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let name = url.rsplit('/').next().unwrap_or("Missing file");
        self.conn.execute("INSERT INTO artifacts(id,chat_id,persona_id,name,kind,size_bytes,created_at,source,source_path,sha256,blob_path) VALUES(?1,?2,?3,?4,?5,0,?6,'missing_reference','','','')",params![id,cid,pid,name,kind(name),time])?;
        self.add_alias(url, &id)?;
        self.get(&id)
    }
    pub fn add_alias(&self, url: &str, id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO artifact_aliases VALUES(?1,?2)",
            params![url, id],
        )?;
        Ok(())
    }
    pub fn associate_run(&self, id: &str, run_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO artifact_runs VALUES(?1,?2)",
            params![id, run_id],
        )?;
        Ok(())
    }
    pub fn append_run_outputs(
        &self,
        cid: i64,
        pid: i64,
        run_id: &str,
        text: &str,
    ) -> Result<String> {
        let mut stmt=self.conn.prepare(&format!("SELECT {COLS} FROM artifacts WHERE chat_id=?1 AND persona_id=?2 AND status='active' AND id IN (SELECT artifact_id FROM artifact_runs WHERE run_id=?3) ORDER BY created_at,id"))?;
        let items = stmt
            .query_map(params![cid, pid, run_id], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut output = text.to_owned();
        for a in items {
            if !text.contains(&a.url) {
                output.push_str(&format!(
                    "\n\n[{}]({})",
                    a.name.replace(['[', ']'], "_"),
                    a.url
                ));
            }
        }
        Ok(output)
    }
    pub fn message_artifacts(&self, cid: i64, pid: i64, mid: &str) -> Result<Vec<Artifact>> {
        let mut stmt=self.conn.prepare(&format!("SELECT {COLS} FROM artifacts WHERE chat_id=?1 AND persona_id=?2 AND id IN (SELECT artifact_id FROM artifact_messages WHERE message_id=?3) ORDER BY created_at,id"))?;
        let items = stmt
            .query_map(params![cid, pid, mid], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }
    pub fn associate_text(
        &self,
        cid: i64,
        pid: i64,
        mid: &str,
        session: Option<&str>,
        text: &str,
    ) -> Result<()> {
        let re = regex::Regex::new(r"/api/artifacts/([0-9a-f-]{36})/content")?;
        for cap in re.captures_iter(text) {
            let a = self.get(&cap[1])?;
            if a.chat_id == cid && a.persona_id == pid {
                self.associate(&a.id, None, session, Some(mid))?;
            }
        }
        Ok(())
    }
    pub fn set_created(&self, id: &str, time: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE artifacts SET created_at=min(created_at,?2) WHERE id=?1",
            params![id, time],
        )?;
        Ok(())
    }
    pub fn migration_complete(&self, scope: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT complete FROM artifact_migrations WHERE scope=?1",
                [scope],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
    pub fn finish_migration(&self, scope: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO artifact_migrations VALUES(?1,1)",
            [scope],
        )?;
        Ok(())
    }
    pub fn associate(
        &self,
        id: &str,
        job: Option<&str>,
        session: Option<&str>,
        message: Option<&str>,
    ) -> Result<()> {
        self.conn.execute("UPDATE artifacts SET job_id=COALESCE(job_id,?2),session_id=COALESCE(session_id,?3),message_id=COALESCE(message_id,?4) WHERE id=?1",params![id,job,session,message])?;
        if let Some(job) = job {
            self.conn.execute(
                "INSERT OR IGNORE INTO artifact_jobs VALUES(?1,?2)",
                params![id, job],
            )?;
        }
        if let Some(message) = message {
            self.conn.execute(
                "INSERT OR IGNORE INTO artifact_messages VALUES(?1,?2)",
                params![id, message],
            )?;
        }
        Ok(())
    }
    pub fn publish(&mut self, cid: i64, pid: i64, path: &Path, source: &str) -> Result<Artifact> {
        self.publish_checked(cid, pid, path, source, None)
    }
    pub fn publish_checked(
        &mut self,
        cid: i64,
        pid: i64,
        path: &Path,
        source: &str,
        expected: Option<&str>,
    ) -> Result<Artifact> {
        let path = path.canonicalize()?;
        if !path.starts_with(&self.root) || !path.is_file() {
            bail!("Source must be a file inside the workspace")
        }
        // Never publish another persona's files, runtime secrets, or arbitrary config files.
        let persona = self.root.join(format!("shared/personas/{cid}/{pid}"));
        let uploads = self.root.join(format!("shared/upload/web/{cid}"));
        let groups = self.root.join(format!("runtime/groups/{cid}/{pid}"));
        let legacy = self.root.join(format!("uploads/web/{cid}"));
        let shared = self.root.join("shared");
        let allowed = path.starts_with(&persona)
            || path.starts_with(&groups)
            || (path.starts_with(&uploads)
                && (path.parent() == Some(uploads.as_path())
                    || path.starts_with(uploads.join(pid.to_string()))))
            || (path.starts_with(&legacy)
                && (path.parent() == Some(legacy.as_path())
                    || path.starts_with(legacy.join(pid.to_string()))))
            || (path.parent() == Some(shared.as_path())
                && !path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with('.'));
        if !allowed {
            bail!("File is outside this persona's publication roots")
        }
        let name = path
            .file_name()
            .context("Missing filename")?
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') || name.ends_with(".env") {
            bail!("Configuration files cannot be published")
        }
        let before = std::fs::metadata(&path)?;
        let dir = self
            .root
            .join(format!("runtime/artifacts/blobs/{cid}/{pid}"));
        std::fs::create_dir_all(&dir)?;
        let temp = dir.join(format!("{}.partial", uuid::Uuid::new_v4()));
        let result = (|| -> Result<Artifact> {
            let mut input = std::fs::File::open(&path)?;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            let mut hash = Sha256::new();
            let mut size = 0u64;
            let mut buf = [0u8; 65536];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n])?;
                hash.update(&buf[..n]);
                size += n as u64;
            }
            output.sync_all()?;
            drop(output);
            let after = std::fs::metadata(&path)?;
            if before.len() != after.len()
                || before.modified()? != after.modified()?
                || size != before.len()
            {
                bail!("File changed during publication; retry after the producer finishes")
            }
            let digest = format!("{:x}", hash.finalize());
            if expected.is_some_and(|e| e != digest) {
                bail!("Output checksum mismatch")
            }
            let blob = dir.join(&digest);
            let tx = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing:Option<String>=tx.query_row("SELECT id FROM artifacts WHERE chat_id=?1 AND persona_id=?2 AND source_path=?3 AND sha256=?4 AND status!='deleted' LIMIT 1",params![cid,pid,path.to_string_lossy(),digest],|r|r.get(0)).optional()?;
            if let Some(id) = existing {
                if !blob.exists() {
                    std::fs::rename(&temp, &blob)?;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO artifact_locations VALUES(?1,?2,?3,?4)",
                    params![
                        id,
                        path.to_string_lossy(),
                        digest,
                        (path.starts_with(self.root.join("shared/upload"))
                            || path.starts_with(self.root.join("uploads")))
                    ],
                )?;
                tx.commit()?;
                return self.get(&id);
            }
            if !blob.exists() {
                std::fs::rename(&temp, &blob)?;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let created = chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO artifacts(id,chat_id,persona_id,name,kind,size_bytes,created_at,source,source_path,sha256,blob_path) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,cid,pid,name,kind(&name),size,created,source,path.to_string_lossy(),digest,blob.to_string_lossy()])?;
            tx.execute(
                "INSERT OR IGNORE INTO artifact_locations VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    path.to_string_lossy(),
                    digest,
                    (path.starts_with(self.root.join("shared/upload"))
                        || path.starts_with(self.root.join("uploads")))
                ],
            )?;
            tx.commit()?;
            self.get(&id)
        })();
        let _ = std::fs::remove_file(&temp);
        result
    }
    pub fn list(&self, cid: i64, pid: i64, f: &Filter) -> Result<Page> {
        let limit = f.limit.unwrap_or(50).clamp(1, 100);
        let cursor = f.cursor.as_deref().and_then(|c| c.split_once('|'));
        if f.cursor.is_some() && cursor.is_none() {
            bail!("Invalid cursor")
        }
        let (time, id) = cursor.unwrap_or(("", ""));
        let mut stmt=self.conn.prepare(&format!("SELECT {COLS} FROM artifacts WHERE chat_id=?1 AND persona_id=?2 AND status=?3 AND (?4='' OR kind=?4) AND (?5='' OR instr(lower(name),lower(?5))>0) AND (?6='' OR job_id=?6 OR id IN (SELECT artifact_id FROM artifact_jobs WHERE job_id=?6)) AND (?7='' OR created_at<?7 OR (created_at=?7 AND id<?8)) AND (?9='' OR created_at<?9) AND (?10='' OR created_at>=?10) ORDER BY created_at DESC,id DESC LIMIT ?11"))?;
        let k = f.kind.as_deref().filter(|k| *k != "all").unwrap_or("");
        let mut artifacts = stmt
            .query_map(
                params![
                    cid,
                    pid,
                    f.status.as_deref().unwrap_or("active"),
                    k,
                    f.q.as_deref().unwrap_or(""),
                    f.job_id.as_deref().unwrap_or(""),
                    time,
                    id,
                    f.before.as_deref().unwrap_or(""),
                    f.after.as_deref().unwrap_or(""),
                    limit + 1
                ],
                row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = artifacts.len() > limit;
        artifacts.truncate(limit);
        let next_cursor = if more {
            artifacts
                .last()
                .map(|a| format!("{}|{}", a.created_at, a.id))
        } else {
            None
        };
        Ok(Page {
            artifacts,
            next_cursor,
        })
    }
    pub fn update(
        &self,
        id: &str,
        status: Option<&str>,
        pinned: Option<bool>,
        superseded: Option<&str>,
    ) -> Result<Artifact> {
        let tx = self.conn.unchecked_transaction()?;
        let a = self.get(id)?;
        if self.conn.query_row(
            "SELECT count(*) FROM artifact_deletions WHERE artifact_id=?1",
            [id],
            |r| r.get::<_, i64>(0),
        )? > 0
        {
            bail!("Permanent deletion is in progress; retry deletion to finish")
        }
        if a.status == "deleted" {
            bail!("Permanently deleted artifacts cannot be changed")
        }
        if let Some(status) = status {
            if !matches!(status, "active" | "trash") {
                bail!("Status must be active or trash")
            }
            if status == "trash" && a.pinned {
                bail!("Unpin this artifact before trashing it")
            }
            if status == "trash" && a.status != "trash" {
                self.conn.execute(
                    "UPDATE artifacts SET status=?2, trashed_at=?3 WHERE id=?1",
                    params![id, status, stamp(chrono::Utc::now())],
                )?;
            } else if status == "active" {
                self.conn.execute(
                    "UPDATE artifacts SET status=?2, trashed_at=NULL WHERE id=?1",
                    params![id, status],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE artifacts SET status=?2 WHERE id=?1",
                    params![id, status],
                )?;
            }
        }
        if let Some(pin) = pinned {
            self.conn.execute(
                "UPDATE artifacts SET pinned=?2 WHERE id=?1",
                params![id, pin],
            )?;
        }
        if let Some(other) = superseded {
            let b = self.get(other)?;
            if a.chat_id != b.chat_id || a.persona_id != b.persona_id || a.id == b.id {
                bail!("Invalid replacement artifact")
            }
            self.conn.execute(
                "UPDATE artifacts SET superseded_by=?2 WHERE id=?1",
                params![id, other],
            )?;
        }
        tx.commit()?;
        self.get(id)
    }
    /// Oldest unpinned trash whose retention has elapsed. Callers delete with `original` false.
    pub fn trash_due(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Artifact>> {
        let cutoff = stamp(now - chrono::Duration::days(TRASH_RETENTION_DAYS));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM artifacts WHERE status='trash' AND pinned=0 AND trashed_at IS NOT NULL AND trashed_at<=?1 ORDER BY trashed_at,id LIMIT {TRASH_DUE_BATCH}"
        ))?;
        let due = stmt
            .query_map([cutoff], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(due)
    }
    fn managed_locations(&self, id: &str) -> Result<Vec<(PathBuf, String)>> {
        let mut stmt=self.conn.prepare("SELECT path,sha256 FROM artifact_locations WHERE artifact_id=?1 AND managed=1 AND NOT EXISTS (SELECT 1 FROM artifact_locations l JOIN artifacts a ON a.id=l.artifact_id WHERE l.path=artifact_locations.path AND a.id!=?1 AND a.status!='deleted')")?;
        let locations = stmt
            .query_map([id], |r| {
                Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(locations)
    }
    pub fn deletion_review(&self, id: &str) -> Result<serde_json::Value> {
        let a = self.get(id)?;
        let refs: i64 = self.conn.query_row(
            "SELECT count(*) FROM artifacts WHERE blob_path=?1 AND id!=?2 AND status!='deleted'",
            params![a.blob_path.to_string_lossy(), id],
            |r| r.get(0),
        )?;
        let original = PathBuf::from(&a.source_path);
        let persona = self
            .root
            .join(format!("shared/personas/{}/{}", a.chat_id, a.persona_id));
        let original_refs: i64 = self.conn.query_row(
            "SELECT count(*) FROM artifacts WHERE source_path=?1 AND id!=?2 AND status!='deleted'",
            params![a.source_path, id],
            |r| r.get(0),
        )?;
        let eligible = a.source == "generated"
            && original
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&persona) && p == original)
            && original_refs == 0
            && hash_file(&original).is_ok_and(|h| h == a.sha256);
        let copies = self
            .managed_locations(id)?
            .iter()
            .filter_map(|(p, h)| {
                let canonical = p.canonicalize().ok()?;
                if !(canonical.starts_with(self.root.join("shared/upload"))
                    || canonical.starts_with(self.root.join("uploads")))
                    || !hash_file(p).is_ok_and(|actual| actual == *h)
                {
                    return None;
                }
                p.metadata().ok().map(|m| m.len())
            })
            .sum::<u64>();
        Ok(
            serde_json::json!({"artifact":a,"managed_reclaimable_bytes":copies+if refs==0 && a.blob_path.is_file() {a.size_bytes}else{0},"original_deletable":eligible,"original_reclaimable_bytes":if eligible {a.size_bytes}else{0}}),
        )
    }
    pub fn delete(&mut self, id: &str, original: bool) -> Result<()> {
        // A persisted intent makes retries idempotent after a crash between unlinks.
        let a = self.get(id)?;
        if a.status == "deleted" {
            return Ok(());
        }
        if a.status != "trash" || a.pinned {
            bail!("Only unpinned artifacts in Trash can be deleted")
        }
        let pending: Option<bool> = self
            .conn
            .query_row(
                "SELECT delete_original FROM artifact_deletions WHERE artifact_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let original = pending.unwrap_or(original);
        let review = self.deletion_review(id)?;
        if original && Path::new(&a.source_path).exists() && review["original_deletable"] != true {
            bail!("Original changed, is shared, or is not a verified persona output")
        }
        if pending.is_none() && original && review["original_deletable"] != true {
            bail!("Original cannot be deleted")
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO artifact_deletions VALUES(?1,?2)",
            params![id, original],
        )?;
        let locations = self.managed_locations(id)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: (String, bool) = tx.query_row(
            "SELECT status,pinned FROM artifacts WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if current.0 != "trash" || current.1 {
            bail!("Artifact changed during deletion")
        }
        if original && Path::new(&a.source_path).exists() {
            // Recheck bytes immediately before unlink; never follow a substituted symlink.
            let p = Path::new(&a.source_path);
            if p.canonicalize()? != p || hash_file(p)? != a.sha256 {
                bail!("Original changed during deletion")
            }
            std::fs::remove_file(p)?;
        }
        for (p, hash) in locations {
            if !p.exists() {
                continue;
            }
            if !p
                .canonicalize()?
                .starts_with(self.root.join("shared/upload"))
                || hash_file(&p)? != hash
            {
                continue;
            }
            std::fs::remove_file(p)?;
        }
        let refs: i64 = tx.query_row(
            "SELECT count(*) FROM artifacts WHERE blob_path=?1 AND id!=?2 AND status!='deleted'",
            params![a.blob_path.to_string_lossy(), id],
            |r| r.get(0),
        )?;
        if refs == 0 && a.blob_path.exists() {
            std::fs::remove_file(&a.blob_path)?;
        }
        tx.execute("UPDATE artifacts SET status='deleted' WHERE id=?1", [id])?;
        tx.execute("DELETE FROM artifact_deletions WHERE artifact_id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}

pub async fn publish(
    root: PathBuf,
    cid: i64,
    pid: i64,
    path: PathBuf,
    source: &'static str,
) -> Result<Artifact> {
    tokio::task::spawn_blocking(move || Catalog::open(&root)?.publish(cid, pid, &path, source))
        .await?
}

/// Explicit producer contract. No filename guessing and no recursive directory scan.
pub fn ingest_manifest(
    root: &Path,
    cid: i64,
    pid: i64,
    job: &str,
    session: Option<&str>,
    manifest: &Path,
) -> Result<Vec<Artifact>> {
    let bytes = std::fs::read(manifest)?;
    if bytes.len() > 4 * 1024 * 1024 {
        bail!("Output manifest is too large")
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let entries = value
        .get("artifacts")
        .and_then(|v| v.as_array())
        .context("Manifest must contain artifacts[]")?;
    let mut catalog = Catalog::open(root)?;
    let mut out = Vec::new();
    for entry in entries {
        let path = PathBuf::from(entry["path"].as_str().context("Artifact path missing")?);
        let path = if path.is_absolute() {
            path
        } else {
            manifest
                .parent()
                .context("Manifest parent missing")?
                .join(path)
        };
        let a = catalog.publish_checked(cid, pid, &path, "generated", entry["sha256"].as_str())?;
        catalog.associate(&a.id, Some(job), session, None)?;
        out.push(catalog.get(&a.id)?);
    }
    Ok(out)
}
