//! Isolated end-to-end catalog smoke check. Never opens the operator's workspace.
use anyhow::Result;
use finally_a_value_bot::artifacts::{ingest_manifest, Catalog, Filter};
use std::{collections::HashSet, fs};
#[tokio::main]
async fn main() -> Result<()> {
    let uri = "/api/artifacts?chat_id=997894126&persona_id=25&limit=50&q=take%206&kind=video&status=active".parse()?;
    let query =
        axum::extract::Query::<finally_a_value_bot::artifact_http::ListQuery>::try_from_uri(&uri)?;
    assert_eq!(query.chat_id, Some(997894126));
    assert_eq!(query.persona_id, Some(25));
    let filter = query.0.into_filter();
    assert_eq!(filter.limit, Some(50));
    assert_eq!(filter.q.as_deref(), Some("take 6"));
    let bad_uri = "/api/artifacts?limit=invalid".parse()?;
    assert!(
        axum::extract::Query::<finally_a_value_bot::artifact_http::ListQuery>::try_from_uri(
            &bad_uri
        )
        .is_err()
    );
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let dir = root.join("shared/personas/1/2");
    fs::create_dir_all(&dir)?;
    let source = dir.join("take 5 (final).mp4");
    fs::write(&source, b"take-five")?;
    use finally_a_value_bot::final_delivery_media::{
        resolve_workspace_artifact_path, ArtifactResolveKind,
    };
    fs::write(dir.join("same.mp4"), b"persona")?;
    fs::write(root.join("shared/same.mp4"), b"unrelated")?;
    assert_eq!(
        resolve_workspace_artifact_path(
            &root,
            Some(1),
            Some(2),
            "same.mp4",
            ArtifactResolveKind::AnyFile
        ),
        Some(dir.join("same.mp4"))
    );
    fs::create_dir_all(dir.join("older"))?;
    fs::write(dir.join("older/same.mp4"), b"older")?;
    assert!(resolve_workspace_artifact_path(
        &root,
        Some(1),
        Some(2),
        "same.mp4",
        ArtifactResolveKind::AnyFile
    )
    .is_none());
    use finally_a_value_bot::final_delivery_media::materialize_web_delivery_file_links;
    let deliverable = dir.join("cloth02_oneshot_r06.mp4");
    fs::write(&deliverable, b"exact-take-six")?;
    let delivered = materialize_web_delivery_file_links(
        &root,
        None,
        1,
        2,
        &format!("[Take 6]({})", deliverable.display()),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert!(delivered.contains("/api/artifacts/"));
    assert!(!delivered.contains("unavailable"));
    let failed = materialize_web_delivery_file_links(
        &root,
        None,
        1,
        2,
        "[Missing](/nonexistent/output.mp4)",
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert!(failed.contains("Missing — file unavailable"));
    assert!(!failed.contains("#artifact-unavailable"));
    // Publication rejection must also render a non-clickable failure.
    let rejected = materialize_web_delivery_file_links(
        &root,
        None,
        1,
        3,
        &format!("[Other persona]({})", deliverable.display()),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert!(rejected.contains("file unavailable"));
    assert!(!rejected.contains("]("));
    let mut c = Catalog::open(&root)?;
    let a = c.publish(1, 2, &source, "generated")?;
    assert_eq!(c.publish(1, 2, &source, "generated")?.id, a.id);
    assert!(c.publish(1, 3, &source, "generated").is_err());
    assert_eq!(fs::read(&a.blob_path)?, b"take-five");
    c.associate_run(&a.id, "turn-five")?;
    let delivered = c.append_run_outputs(1, 2, "turn-five", "Finished. QC failed.")?;
    assert!(delivered.contains(&a.url));
    assert_eq!(
        c.append_run_outputs(1, 2, "turn-five", &delivered)?,
        delivered
    );
    assert_eq!(
        c.append_run_outputs(1, 2, "unrelated-turn", "Summary")?,
        "Summary"
    );

    fs::write(&source, b"take-six")?;
    let b = c.publish(1, 2, &source, "generated")?;
    assert_ne!(a.id, b.id);
    assert_eq!(fs::read(&a.blob_path)?, b"take-five");
    c.associate(&b.id, Some("job-six"), None, Some("message-six"))?;
    assert_eq!(
        c.list(
            1,
            2,
            &Filter {
                job_id: Some("job-six".into()),
                ..Default::default()
            }
        )?
        .artifacts[0]
            .id,
        b.id
    );
    assert_eq!(c.message_artifacts(1, 2, "message-six")?[0].id, b.id);
    assert!(c.list(1, 3, &Filter::default())?.artifacts.is_empty());
    c.update(&a.id, None, Some(true), None)?;
    assert!(c.update(&a.id, Some("trash"), None, None).is_err());
    c.update(&a.id, None, Some(false), Some(&b.id))?;
    c.update(&a.id, Some("trash"), None, None)?;
    assert_eq!(c.deletion_review(&a.id)?["original_deletable"], false);
    assert!(c.delete(&a.id, true).is_err());
    c.delete(&a.id, false)?;
    c.delete(&a.id, false)?;
    assert_eq!(fs::read(&source)?, b"take-six");
    assert_eq!(c.get(&a.id)?.status, "deleted");
    let manifest = dir.join("artifacts.json");
    fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({"artifacts":[{"path":source,"sha256":b.sha256}]}))?,
    )?;
    assert_eq!(
        ingest_manifest(&root, 1, 2, "collector-job", None, &manifest)?[0].id,
        b.id
    );
    assert_eq!(
        c.list(
            1,
            2,
            &Filter {
                job_id: Some("collector-job".into()),
                ..Default::default()
            }
        )?
        .artifacts[0]
            .id,
        b.id
    );
    fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({"artifacts":[{"path":source,"sha256":"bad"}]}))?,
    )?;
    assert!(ingest_manifest(&root, 1, 2, "wrong-job", None, &manifest).is_err());
    // Two logical outputs with shared bytes; deleting one must retain the other.
    let other = dir.join("duplicate.mp4");
    fs::copy(&source, &other)?;
    let dup = c.publish(1, 2, &other, "generated")?;
    assert_eq!(dup.blob_path, b.blob_path);
    c.update(&b.id, Some("trash"), None, None)?;
    c.delete(&b.id, true)?;
    assert!(!source.exists());
    assert!(dup.blob_path.exists());
    c.update(&dup.id, Some("trash"), None, None)?;
    c.update(&dup.id, Some("active"), None, None)?;
    // Backward URLs remain aliases, including tombstones after deletion.
    c.add_alias("/api/uploads/web/1/2/duplicate.mp4", &dup.id)?;
    c.update(&dup.id, Some("trash"), None, None)?;
    c.delete(&dup.id, false)?;
    assert_eq!(
        c.alias("/api/uploads/web/1/2/duplicate.mp4")?
            .unwrap()
            .status,
        "deleted"
    );
    assert!(other.exists());
    // Exercise real HTTP response generation, including video seeking and bounded text.
    use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
    use finally_a_value_bot::artifact_http::{serve_file, ContentOptions};
    let mut headers = HeaderMap::new();
    headers.insert("range", HeaderValue::from_static("bytes=2-5"));
    let response = serve_file(
        &other,
        "duplicate.mp4",
        &headers,
        &Method::GET,
        &ContentOptions::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert!(response.headers().get("content-security-policy").is_none());
    assert_eq!(response.headers()["content-range"], "bytes 2-5/8");
    assert_eq!(
        &axum::body::to_bytes(response.into_body(), 1024).await?[..],
        b"ke-s"
    );
    headers.insert("range", HeaderValue::from_static("bytes=99-"));
    let response = serve_file(
        &other,
        "duplicate.mp4",
        &headers,
        &Method::GET,
        &ContentOptions::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let response = serve_file(
        &other,
        "duplicate.mp4",
        &HeaderMap::new(),
        &Method::HEAD,
        &ContentOptions::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    assert_eq!(response.headers()["content-length"], "8");
    assert!(axum::body::to_bytes(response.into_body(), 1024)
        .await?
        .is_empty());
    let text = dir.join("large.txt");
    fs::write(&text, vec![b'x'; 1024 * 1024])?;
    let mut preview_headers = HeaderMap::new();
    preview_headers.insert("range", "bytes=0-".parse()?);
    let response = serve_file(
        &text,
        "large.txt",
        &preview_headers,
        &Method::GET,
        &ContentOptions {
            preview: Some(true),
            download: None,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 300000)
            .await?
            .len(),
        262144
    );
    for (filename, body, sandboxed) in [
        ("page.html", &b"<p>preview</p>"[..], true),
        (
            "icon.svg",
            &b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..],
            true,
        ),
        ("still.png", &b"png"[..], false),
        ("clip.webm", &b"webm"[..], false),
    ] {
        let path = dir.join(filename);
        fs::write(&path, body)?;
        let response = serve_file(
            &path,
            filename,
            &HeaderMap::new(),
            &Method::GET,
            &ContentOptions::default(),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
        assert_eq!(
            response.headers().get("content-security-policy").is_some(),
            sandboxed,
            "{filename}"
        );
    }
    let retain = dir.join("retain.mp4");
    fs::write(&retain, b"retain-bytes")?;
    let kept = c.publish(1, 2, &retain, "generated")?;
    c.update(&kept.id, Some("trash"), None, None)?;
    assert!(c.get(&kept.id)?.expires_at.is_some());
    assert!(c
        .trash_due(chrono::Utc::now())?
        .iter()
        .all(|a| a.id != kept.id));
    c.update(&kept.id, Some("active"), None, None)?;
    assert!(c.get(&kept.id)?.trashed_at.is_none());
    c.update(&kept.id, Some("trash"), None, None)?;
    let stale = (chrono::Utc::now() - chrono::Duration::days(31))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let conn = rusqlite::Connection::open(root.join("runtime/artifacts/catalog.sqlite3"))?;
    conn.execute(
        "UPDATE artifacts SET trashed_at=?1 WHERE id=?2",
        rusqlite::params![stale, kept.id],
    )?;
    drop(conn);
    c.update(&kept.id, None, Some(true), None)?;
    assert!(c
        .trash_due(chrono::Utc::now())?
        .iter()
        .all(|a| a.id != kept.id));
    c.update(&kept.id, None, Some(false), None)?;
    let blob = c.get(&kept.id)?.blob_path.clone();
    assert_eq!(c.trash_due(chrono::Utc::now())?[0].id, kept.id);
    c.delete(&kept.id, false)?;
    assert!(retain.exists());
    assert!(!blob.exists());
    assert_eq!(c.get(&kept.id)?.status, "deleted");
    let legacy_dir = tempfile::tempdir()?;
    let legacy_root = legacy_dir.path().canonicalize()?;
    let catalog_dir = legacy_root.join("runtime/artifacts");
    fs::create_dir_all(&catalog_dir)?;
    let legacy = rusqlite::Connection::open(catalog_dir.join("catalog.sqlite3"))?;
    legacy.execute_batch(
        "CREATE TABLE artifacts (
            id TEXT PRIMARY KEY, chat_id INTEGER NOT NULL, persona_id INTEGER NOT NULL,
            name TEXT NOT NULL, kind TEXT NOT NULL, size_bytes INTEGER NOT NULL,
            created_at TEXT NOT NULL, source TEXT NOT NULL, source_path TEXT NOT NULL,
            sha256 TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'active', pinned INTEGER NOT NULL DEFAULT 0,
            job_id TEXT, session_id TEXT, message_id TEXT, superseded_by TEXT, blob_path TEXT NOT NULL);",
    )?;
    legacy.execute(
        "INSERT INTO artifacts(id,chat_id,persona_id,name,kind,size_bytes,created_at,source,source_path,sha256,status,pinned,blob_path) VALUES('legacy-trash',1,2,'old.mp4','video',1,'2020-01-01T00:00:00Z','generated','/missing','abc','trash',0,'/missing')",
        [],
    )?;
    drop(legacy);
    let legacy_catalog = Catalog::open(&legacy_root)?;
    let old = legacy_catalog.get("legacy-trash")?;
    assert!(old.trashed_at.is_some() && old.expires_at.is_some());
    assert!(legacy_catalog.trash_due(chrono::Utc::now())?.is_empty());
    drop(c);
    // Fixture population without 10,000 filesystem copies; exercise indexed cursor queries.
    let mut conn = rusqlite::Connection::open(root.join("runtime/artifacts/catalog.sqlite3"))?;
    let tx = conn.transaction()?;
    for i in 0..10000 {
        tx.execute("INSERT INTO artifacts(id,chat_id,persona_id,name,kind,size_bytes,created_at,source,source_path,sha256,blob_path) VALUES(?1,9,8,?2,'video',1,'2026-10-07T00:00:00Z','fixture','','','')",rusqlite::params![format!("fixture-{i:05}"),format!("video-{i}.mp4")])?;
    }
    tx.commit()?;
    drop(conn);
    let c = Catalog::open(&root)?;
    let started = std::time::Instant::now();
    let mut seen = HashSet::new();
    let mut filter = Filter::default();
    let mut pages = 0;
    loop {
        let page = c.list(9, 8, &filter)?;
        assert!(page.artifacts.len() <= 50);
        for a in page.artifacts {
            assert!(seen.insert(a.id));
        }
        pages += 1;
        if page.next_cursor.is_none() {
            break;
        }
        filter.cursor = page.next_cursor;
    }
    assert_eq!(seen.len(), 10000);
    assert_eq!(pages, 200);
    println!("PASS: immutable delivery, repeat publication, persona isolation, manifests/checksums, provenance, pin/trash/restore/delete, shared blobs, legacy tombstones, restart, and 10,000-row pagination ({} ms)",started.elapsed().as_millis());
    Ok(())
}
