use rusqlite::Connection;
use serde_json::Value;
use taskrunner::storage::events::{EventBody, LogEvent};
use taskrunner::storage::index::{IN_MEMORY, StateIndex, rebuild_index};

use crate::helpers::{evt, message, sample_sequence};

const TABLES: [&str; 11] = [
    "projects",
    "project_aliases",
    "mcp_sessions",
    "tasks",
    "turns",
    "worker_sessions",
    "messages",
    "transcript_sessions",
    "audit_events",
    "artifacts",
    "artifact_links",
];

/// Every table as JSON rows ordered by first column, for whole-index equality.
fn dump(index: &StateIndex) -> Vec<Vec<Vec<Value>>> {
    TABLES
        .iter()
        .map(|table| rows(&index.db, &format!("SELECT * FROM {table} ORDER BY 1")))
        .collect()
}

fn rows(db: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = db.prepare(sql).unwrap();
    let width = stmt.column_count();
    stmt.query_map([], |row| {
        (0..width)
            .map(|i| {
                Ok(match row.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => Value::from(n),
                    rusqlite::types::ValueRef::Real(f) => Value::from(f),
                    rusqlite::types::ValueRef::Text(t) => Value::from(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(b) => Value::from(b.to_vec()),
                })
            })
            .collect()
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn one<T: rusqlite::types::FromSql>(db: &Connection, sql: &str) -> T {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn apply_all(index: &StateIndex, events: &[LogEvent]) {
    for event in events {
        index.apply(event).unwrap();
    }
}

#[test]
fn folds_the_sample_sequence_into_consistent_rows() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    apply_all(&index, &sample_sequence());
    let db = &index.db;

    let (status, project_id, session_id): (String, String, String) = db
        .query_row(
            "SELECT status, project_id, session_id FROM tasks WHERE id = 'task_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (status.as_str(), project_id.as_str(), session_id.as_str()),
        ("completed", "proj_a", "sess_a")
    );

    let (status, idx, response, changed, completed_at): (String, i64, String, String, Option<String>) = db
        .query_row(
            "SELECT status, idx, response, changed_files, completed_at FROM turns WHERE id = 'turn_a1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(idx, 0);
    assert_eq!(response, "created hello.txt");
    assert_eq!(serde_json::from_str::<Vec<String>>(&changed).unwrap(), vec!["hello.txt"]);
    assert!(completed_at.is_some());

    let ended_at: Option<String> = one(db, "SELECT ended_at FROM mcp_sessions WHERE id = 'sess_a'");
    assert!(ended_at.is_some());
    let native: String = one(db, "SELECT native_session_id FROM worker_sessions");
    assert_eq!(native, "019e28f6-9f73-73d0-b601-33505b06d3f5");
    assert_eq!(one::<i64>(db, "SELECT COUNT(*) FROM audit_events"), 1);
    assert_eq!(one::<i64>(db, "SELECT COUNT(*) FROM artifact_links"), 1);
}

#[test]
fn tracks_failure_and_cancellation_statuses_with_turn_indexes() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    let events = vec![
        evt(EventBody::ProjectCreated { project_id: "proj_b".into(), root: "/b".into() }),
        evt(EventBody::TaskCreated {
            task_id: "task_b".into(),
            project_id: "proj_b".into(),
            session_id: None,
            worker: "codex".into(),
            prompt_summary: "x".into(),
            tier: None,
            runtime: None,
            allow_domains: None,
        }),
        evt(EventBody::TurnStarted {
            turn_id: "turn_b1".into(),
            task_id: "task_b".into(),
            prompt: "one".into(),
        }),
        evt(EventBody::TurnFailed {
            turn_id: "turn_b1".into(),
            task_id: "task_b".into(),
            error_code: "worker_failed".into(),
            error_message: "timeout after 1800s".into(),
        }),
        evt(EventBody::TurnStarted {
            turn_id: "turn_b2".into(),
            task_id: "task_b".into(),
            prompt: "two".into(),
        }),
        evt(EventBody::TurnCanceled {
            turn_id: "turn_b2".into(),
            task_id: "task_b".into(),
            reason: Some("user request".into()),
        }),
    ];
    apply_all(&index, &events);

    let turns = rows(&index.db, "SELECT id, idx, status, error_code FROM turns ORDER BY idx");
    assert_eq!(
        turns,
        vec![
            vec![
                Value::from("turn_b1"),
                Value::from(0),
                Value::from("failed"),
                Value::from("worker_failed")
            ],
            vec![Value::from("turn_b2"), Value::from(1), Value::from("canceled"), Value::Null],
        ]
    );
    assert_eq!(
        one::<String>(&index.db, "SELECT status FROM tasks WHERE id = 'task_b'"),
        "canceled"
    );
}

#[test]
fn is_idempotent_when_the_same_events_are_applied_twice() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    let events = sample_sequence();
    apply_all(&index, &events);
    let once = dump(&index);
    apply_all(&index, &events);
    assert_eq!(dump(&index), once);
}

#[test]
fn rebuild_equals_incremental_application() {
    let events = sample_sequence();
    let incremental = StateIndex::open(IN_MEMORY).unwrap();
    apply_all(&incremental, &events);

    let dir = tempfile::tempdir().unwrap();
    let rebuilt = rebuild_index(dir.path().join("index.db").to_str().unwrap(), &events).unwrap();
    assert_eq!(dump(&rebuilt), dump(&incremental));
}

#[test]
fn folds_message_recorded_rows_and_dedupes_by_message_id() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    let recorded = |id: &str, content: &str| {
        let mut body = message(id, "claude-code", "s1", "user", "message", content);
        if let EventBody::MessageRecorded { native_ts, project_path, .. } = &mut body {
            *native_ts = Some("2026-01-01T00:00:00Z".into());
            *project_path = Some("/repo".into());
        }
        evt(body)
    };
    index.apply(&recorded("msg_a", "hello")).unwrap();
    index.apply(&recorded("msg_a", "hello again")).unwrap(); // duplicate id: ignored
    index.apply(&recorded("msg_b", "world")).unwrap();

    let got =
        rows(&index.db, "SELECT id, content, source, native_session_id FROM messages ORDER BY id");
    assert_eq!(
        got,
        vec![
            vec![
                Value::from("msg_a"),
                Value::from("hello"),
                Value::from("claude-code"),
                Value::from("s1")
            ],
            vec![
                Value::from("msg_b"),
                Value::from("world"),
                Value::from("claude-code"),
                Value::from("s1")
            ],
        ]
    );
}

#[test]
fn numbers_prompts_per_session_skipping_the_harness_written_records() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    let mut n = 0;
    let mut say = |session: &str, role: &str, kind: &str, content: &str| {
        n += 1;
        index
            .apply(&evt(message(&format!("m{n}"), "claude-code", session, role, kind, content)))
            .unwrap();
    };
    // A session opens with slash-command noise, then the first real prompt.
    say("s1", "user", "message", "<command-name>/clear</command-name>");
    say("s1", "user", "message", "<local-command-caveat>Caveat: …");
    say("s1", "user", "message", "first question");
    say("s1", "assistant", "message", "first answer");
    say("s1", "user", "message", "[Request interrupted by user]");
    say("s1", "user", "message", "second question");
    say("s1", "assistant", "message", "second answer");
    // A second session numbers independently.
    say("s2", "user", "message", "unrelated question");

    let idx: Vec<i64> = rows(&index.db, "SELECT prompt_idx FROM messages ORDER BY id")
        .into_iter()
        .map(|row| row[0].as_i64().unwrap())
        .collect();
    assert_eq!(idx, vec![0, 0, 1, 1, 1, 2, 2, 1]);
    let counts = rows(
        &index.db,
        "SELECT native_session_id, prompt_count FROM transcript_sessions ORDER BY 1",
    );
    assert_eq!(
        counts,
        vec![vec![Value::from("s1"), Value::from(2)], vec![Value::from("s2"), Value::from(1)]]
    );
}

#[test]
fn promotes_tool_facts_so_a_call_joins_to_its_result() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    let record = |id: &str, role: &str, kind: &str, content: String| {
        index.apply(&evt(message(id, "claude-code", "s1", role, kind, &content))).unwrap();
    };
    let call = |id: &str, name: &str, input: Value| {
        let content =
            serde_json::json!({ "id": format!("toolu_{id}"), "name": name, "input": input });
        record(&format!("u{id}"), "assistant", "tool_use", content.to_string());
    };
    let result = |id: &str, is_error: bool| {
        let content = serde_json::json!({ "tool_use_id": format!("toolu_{id}"), "is_error": is_error, "content": "…" });
        record(&format!("r{id}"), "tool", "tool_result", content.to_string());
    };
    call("1", "Read", serde_json::json!({ "file_path": "/src/shim/proxy.ts" }));
    result("1", false);
    call("2", "Bash", serde_json::json!({ "command": "npm test" }));
    result("2", true);

    let pairs = rows(
        &index.db,
        "SELECT u.tool_name, u.tool_target, r.is_error
           FROM messages u
           JOIN messages r ON r.tool_use_id = u.tool_use_id AND r.kind = 'tool_result'
          WHERE u.kind = 'tool_use' ORDER BY u.id",
    );
    assert_eq!(
        pairs,
        vec![
            vec![Value::from("Read"), Value::from("/src/shim/proxy.ts"), Value::from(0)],
            vec![Value::from("Bash"), Value::from("npm test"), Value::from(1)],
        ]
    );
}

#[test]
fn attributes_a_worker_sessions_messages_to_the_turn_running_at_the_time() {
    let index = StateIndex::open(IN_MEMORY).unwrap();
    // Turn windows are event timestamps, so they have to be pinned explicitly:
    // evt() spaces its events one second apart, far too tight to bucket against.
    let at = |ts: &str, body: EventBody| LogEvent { ts: ts.into(), ..evt(body) };
    let start = |turn_id: &str, ts: &str| {
        at(
            ts,
            EventBody::TurnStarted {
                turn_id: turn_id.into(),
                task_id: "task_t".into(),
                prompt: "p".into(),
            },
        )
    };
    let finish = |turn_id: &str, ts: &str| {
        at(
            ts,
            EventBody::TurnCompleted {
                turn_id: turn_id.into(),
                task_id: "task_t".into(),
                response: "r".into(),
                changed_files: vec![],
                usage: None,
            },
        )
    };
    let events = vec![
        evt(EventBody::ProjectCreated { project_id: "proj_t".into(), root: "/t".into() }),
        evt(EventBody::TaskCreated {
            task_id: "task_t".into(),
            project_id: "proj_t".into(),
            session_id: None,
            worker: "codex".into(),
            prompt_summary: "x".into(),
            tier: None,
            runtime: None,
            allow_domains: None,
        }),
        start("turn_1", "2026-03-01T00:00:00Z"),
        finish("turn_1", "2026-03-01T00:10:00Z"),
        start("turn_2", "2026-03-01T00:20:00Z"),
        finish("turn_2", "2026-03-01T00:30:00Z"),
        evt(EventBody::WorkerSessionRecorded {
            worker_session_id: "wsess_t".into(),
            task_id: "task_t".into(),
            worker: "codex".into(),
            native_session_id: "native_t".into(),
            turn_id: Some("turn_1".into()),
        }),
    ];
    apply_all(&index, &events);

    let swept = |id: &str, session: &str, native_ts: &str| {
        let mut body = message(id, "codex", session, "assistant", "message", "working");
        if let EventBody::MessageRecorded { native_ts: ts, .. } = &mut body {
            *ts = Some(native_ts.into());
        }
        index.apply(&evt(body)).unwrap();
    };
    swept("in_1", "native_t", "2026-03-01T00:05:00Z"); // inside turn 1
    swept("in_2", "native_t", "2026-03-01T00:25:00Z"); // inside turn 2
    swept("between", "native_t", "2026-03-01T00:15:00Z"); // between turns
    swept("host", "some-host-session", "2026-03-01T00:05:00Z"); // no task links it

    let got = rows(&index.db, "SELECT id, turn_id FROM messages ORDER BY id");
    assert_eq!(
        got,
        vec![
            vec![Value::from("between"), Value::Null],
            vec![Value::from("host"), Value::Null],
            vec![Value::from("in_1"), Value::from("turn_1")],
            vec![Value::from("in_2"), Value::from("turn_2")],
        ]
    );
    assert_eq!(one::<String>(&index.db, "SELECT turn_id FROM worker_sessions"), "turn_1");
}

#[test]
fn rebuild_replaces_an_existing_index_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.db");
    let path = path.to_str().unwrap();
    drop(rebuild_index(path, &sample_sequence()).unwrap());
    let second = rebuild_index(
        path,
        &[evt(EventBody::ProjectCreated { project_id: "proj_only".into(), root: "/only".into() })],
    )
    .unwrap();
    let projects = rows(&second.db, "SELECT id FROM projects");
    assert_eq!(projects, vec![vec![Value::from("proj_only")]]);
}
