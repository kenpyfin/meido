//! Integration tests for the Database module.
//!
//! Tests full lifecycle operations across multiple tables,
//! verifying cross-table consistency and complex query patterns.

use finally_a_value_bot::db::{message_origin_interactive, Database, StoredMessage};

fn test_db() -> (Database, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "finally_a_value_bot_integ_{}",
        uuid::Uuid::new_v4()
    ));
    let db = Database::new(dir.to_str().unwrap()).unwrap();
    (db, dir)
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn test_persona(db: &Database, chat_id: i64) -> i64 {
    db.upsert_chat(chat_id, None, "private").unwrap();
    db.get_or_create_default_persona(chat_id).unwrap()
}

/// Full message lifecycle: store → retrieve → verify ordering → cross-chat isolation.
#[test]
fn test_message_full_lifecycle() {
    let (db, dir) = test_db();
    let pid1 = test_persona(&db, 100);
    let pid2 = test_persona(&db, 200);

    // Store messages in two chats
    for i in 0..5 {
        db.store_message(&StoredMessage {
            id: format!("chat1_msg{i}"),
            chat_id: 100,
            persona_id: pid1,
            session_id: None,
            sender_name: "alice".into(),
            content: format!("chat1 message {i}"),
            is_from_bot: false,
            timestamp: format!("2024-01-01T00:00:{:02}Z", i),
            origin: message_origin_interactive(),
        })
        .unwrap();
    }
    for i in 0..3 {
        db.store_message(&StoredMessage {
            id: format!("chat2_msg{i}"),
            chat_id: 200,
            persona_id: pid2,
            session_id: None,
            sender_name: "bob".into(),
            content: format!("chat2 message {i}"),
            is_from_bot: false,
            timestamp: format!("2024-01-01T00:00:{:02}Z", i),
            origin: message_origin_interactive(),
        })
        .unwrap();
    }

    // Verify isolation
    let chat1_msgs = db.get_all_messages(100, pid1).unwrap();
    assert_eq!(chat1_msgs.len(), 5);

    let chat2_msgs = db.get_all_messages(200, pid2).unwrap();
    assert_eq!(chat2_msgs.len(), 3);

    // Verify ordering (oldest first)
    assert_eq!(chat1_msgs[0].content, "chat1 message 0");
    assert_eq!(chat1_msgs[4].content, "chat1 message 4");

    // Verify recent messages with limit
    let recent = db.get_recent_messages(100, pid1, 2, false).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].content, "chat1 message 3"); // oldest of 2 most recent
    assert_eq!(recent[1].content, "chat1 message 4"); // most recent

    // Verify empty chat
    let pid999 = test_persona(&db, 999);
    assert!(db.get_all_messages(999, pid999).unwrap().is_empty());

    cleanup(&dir);
}

/// Session save → load → update → delete → verify lifecycle.
#[test]
fn test_session_lifecycle() {
    let (db, dir) = test_db();
    let pid = test_persona(&db, 100);
    let pid2 = test_persona(&db, 200);

    // No session initially
    assert!(db.load_session(100, pid).unwrap().is_none());

    // Save session
    let json1 = r#"[{"role":"user","content":"hello"}]"#;
    db.save_session(100, pid, json1).unwrap();

    // Load and verify
    let (loaded, ts1) = db.load_session(100, pid).unwrap().unwrap();
    assert_eq!(loaded, json1);
    assert!(!ts1.is_empty());

    // Update session (upsert)
    std::thread::sleep(std::time::Duration::from_millis(10));
    let json2 = r#"[{"role":"user","content":"hello"},{"role":"assistant","content":"hi"}]"#;
    db.save_session(100, pid, json2).unwrap();

    let (loaded2, ts2) = db.load_session(100, pid).unwrap().unwrap();
    assert_eq!(loaded2, json2);
    assert!(ts2 >= ts1); // updated_at should be newer

    // Sessions are per-chat isolated
    assert!(db.load_session(200, pid2).unwrap().is_none());

    // Delete
    assert!(db.delete_session(100, pid).unwrap());
    assert!(db.load_session(100, pid).unwrap().is_none());
    assert!(!db.delete_session(100, pid).unwrap()); // already gone

    cleanup(&dir);
}

/// Scheduled task lifecycle: create → list → pause → resume → cancel → history.
#[test]
fn test_scheduled_task_lifecycle() {
    let (db, dir) = test_db();

    // Create tasks
    let id1 = db
        .create_scheduled_task(
            100,
            "cron task",
            "cron",
            "0 */5 * * * *",
            "2024-01-01T00:05:00Z",
        )
        .unwrap();
    let id2 = db
        .create_scheduled_task(
            100,
            "one-shot",
            "once",
            "2024-06-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
    let _id3 = db
        .create_scheduled_task(
            200,
            "other chat task",
            "cron",
            "0 0 * * * *",
            "2024-01-01T01:00:00Z",
        )
        .unwrap();

    // List by chat - only active/paused
    let tasks_100 = db.get_tasks_for_chat(100).unwrap();
    assert_eq!(tasks_100.len(), 2);
    let tasks_200 = db.get_tasks_for_chat(200).unwrap();
    assert_eq!(tasks_200.len(), 1);

    // Due tasks check — id1 (next_run 00:05) is due, id3 (next_run 01:00) is not yet
    let due = db.get_due_tasks("2024-01-01T00:10:00Z").unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].prompt, "cron task");

    // Pause
    assert!(db.update_task_status(id1, "paused").unwrap());
    let task = db.get_task_by_id(id1).unwrap().unwrap();
    assert_eq!(task.status, "paused");

    // Paused tasks not in due list
    let due = db.get_due_tasks("2024-01-01T00:10:00Z").unwrap();
    assert_eq!(due.len(), 0); // id1 paused, id2/id3 not yet due

    // Resume
    assert!(db.update_task_status(id1, "active").unwrap());

    // Cancel
    assert!(db.update_task_status(id2, "cancelled").unwrap());
    let tasks_100 = db.get_tasks_for_chat(100).unwrap();
    assert_eq!(tasks_100.len(), 1); // cancelled ones filtered out

    // After run - cron task gets new next_run
    db.update_task_after_run(id1, "2024-01-01T00:05:00Z", Some("2024-01-01T00:10:00Z"))
        .unwrap();
    let task = db.get_task_by_id(id1).unwrap().unwrap();
    assert_eq!(task.last_run.as_deref(), Some("2024-01-01T00:05:00Z"));
    assert_eq!(task.next_run, "2024-01-01T00:10:00Z");
    assert_eq!(task.status, "active");

    // One-shot completion
    let id4 = db
        .create_scheduled_task(
            100,
            "once more",
            "once",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
    db.update_task_after_run(id4, "2024-01-01T00:00:00Z", None)
        .unwrap();
    let task = db.get_task_by_id(id4).unwrap().unwrap();
    assert_eq!(task.status, "completed");

    cleanup(&dir);
}

#[test]
fn test_update_task_schedule() {
    let (db, dir) = test_db();
    let id = db
        .create_scheduled_task(100, "x", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
        .unwrap();
    assert!(db
        .update_task_schedule(id, "cron", "0 0 * * * *", "2024-01-02T00:00:00Z")
        .unwrap());
    let t = db.get_task_by_id(id).unwrap().unwrap();
    assert_eq!(t.schedule_type, "cron");
    assert_eq!(t.schedule_value, "0 0 * * * *");
    assert_eq!(t.next_run, "2024-01-02T00:00:00Z");

    cleanup(&dir);
}

#[test]
fn test_scheduled_task_binds_to_active_persona_at_creation() {
    let (db, dir) = test_db();
    db.upsert_chat(100, None, "private").unwrap();
    let default_pid = db.get_or_create_default_persona(100).unwrap();
    let alt_pid = db.create_persona(100, "work", None).unwrap();
    assert!(db.set_active_persona(100, alt_pid).unwrap());

    let task_id = db
        .create_scheduled_task(
            100,
            "persona-bound task",
            "once",
            "2099-12-31T00:00:00Z",
            "2099-12-31T00:00:00Z",
        )
        .unwrap();

    // Switch active persona after scheduling. Task binding should remain unchanged.
    assert!(db.set_active_persona(100, default_pid).unwrap());
    let task = db.get_task_by_id(task_id).unwrap().unwrap();
    assert_eq!(task.persona_id, alt_pid);
    cleanup(&dir);
}

/// Task run logs: create → retrieve → ordering → limit.
#[test]
fn test_task_run_log_lifecycle() {
    let (db, dir) = test_db();

    let task_id = db
        .create_scheduled_task(100, "test", "cron", "0 * * * * *", "2024-01-01T00:00:00Z")
        .unwrap();

    // Log multiple runs
    for i in 0..5 {
        db.log_task_run(
            task_id,
            100,
            &format!("2024-01-01T00:{:02}:00Z", i),
            &format!("2024-01-01T00:{:02}:05Z", i),
            5000,
            i != 2, // run 2 fails
            Some(&format!("Run {i}")),
        )
        .unwrap();
    }

    // Get all logs
    let logs = db.get_task_run_logs(task_id, 100).unwrap();
    assert_eq!(logs.len(), 5);
    assert_eq!(logs[0].result_summary.as_deref(), Some("Run 4")); // most recent first

    // Failed run
    let failed = logs.iter().find(|l| !l.success).unwrap();
    assert_eq!(failed.result_summary.as_deref(), Some("Run 2"));

    // Limit
    let limited = db.get_task_run_logs(task_id, 2).unwrap();
    assert_eq!(limited.len(), 2);

    // No logs for different task
    let empty = db.get_task_run_logs(9999, 10).unwrap();
    assert!(empty.is_empty());

    cleanup(&dir);
}

/// Group catch-up query: messages since last bot response.
#[test]
fn test_catch_up_query_complex() {
    let (db, dir) = test_db();
    let pid = test_persona(&db, 100);

    // Simulate group conversation
    let messages = vec![
        ("m1", "alice", "hi everyone", false, "2024-01-01T00:00:01Z"),
        ("m2", "bob", "hey!", false, "2024-01-01T00:00:02Z"),
        ("m3", "bot", "hello group!", true, "2024-01-01T00:00:03Z"),
        ("m4", "charlie", "what's up?", false, "2024-01-01T00:00:04Z"),
        (
            "m5",
            "alice",
            "working on stuff",
            false,
            "2024-01-01T00:00:05Z",
        ),
        ("m6", "dave", "me too", false, "2024-01-01T00:00:06Z"),
    ];

    for (id, sender, content, is_bot, ts) in &messages {
        db.store_message(&StoredMessage {
            id: id.to_string(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: sender.to_string(),
            content: content.to_string(),
            is_from_bot: *is_bot,
            timestamp: ts.to_string(),
            origin: message_origin_interactive(),
        })
        .unwrap();
    }

    // Should get bot message + everything after it
    let catchup = db
        .get_messages_since_last_bot_response(100, pid, 50, 50, false)
        .unwrap();
    assert!(catchup.len() >= 3); // at least bot msg + 3 after
    assert_eq!(catchup[0].id, "m3"); // starts with bot msg
    assert_eq!(catchup.last().unwrap().id, "m6");

    cleanup(&dir);
}

/// New user messages since timestamp.
#[test]
fn test_new_user_messages_since() {
    let (db, dir) = test_db();
    let pid = test_persona(&db, 100);

    let messages = vec![
        ("m1", "alice", "old", false, "2024-01-01T00:00:01Z"),
        ("m2", "bot", "resp", true, "2024-01-01T00:00:02Z"),
        ("m3", "alice", "new1", false, "2024-01-01T00:00:03Z"),
        ("m4", "bot", "resp2", true, "2024-01-01T00:00:04Z"),
        ("m5", "bob", "new2", false, "2024-01-01T00:00:05Z"),
    ];

    for (id, sender, content, is_bot, ts) in &messages {
        db.store_message(&StoredMessage {
            id: id.to_string(),
            chat_id: 100,
            persona_id: pid,
            session_id: None,
            sender_name: sender.to_string(),
            content: content.to_string(),
            is_from_bot: *is_bot,
            timestamp: ts.to_string(),
            origin: message_origin_interactive(),
        })
        .unwrap();
    }

    // Only non-bot messages after the cutoff
    let new_msgs = db
        .get_new_user_messages_since(100, pid, "2024-01-01T00:00:03Z")
        .unwrap();
    // m3 is at exactly 03Z, but query is >, so only m5 (timestamp > 03Z and not bot)
    assert_eq!(new_msgs.len(), 1);
    assert_eq!(new_msgs[0].content, "new2");

    cleanup(&dir);
}

/// Chat upsert and message storage together.
#[test]
fn test_chat_and_messages_together() {
    let (db, dir) = test_db();

    // Upsert chat first
    db.upsert_chat(100, Some("Test Group"), "group").unwrap();
    let pid = test_persona(&db, 100);

    // Store message
    db.store_message(&StoredMessage {
        id: "msg1".into(),
        chat_id: 100,
        persona_id: pid,
        session_id: None,
        sender_name: "alice".into(),
        content: "hello".into(),
        is_from_bot: false,
        timestamp: "2024-01-01T00:00:00Z".into(),
        origin: message_origin_interactive(),
    })
    .unwrap();

    // Update chat title
    db.upsert_chat(100, Some("Renamed Group"), "group").unwrap();

    // Messages still there
    let msgs = db.get_all_messages(100, pid).unwrap();
    assert_eq!(msgs.len(), 1);

    cleanup(&dir);
}
