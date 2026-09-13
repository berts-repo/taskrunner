//! Shared test scaffolding, mirroring the TypeScript tests/helpers.ts.

use std::cell::Cell;

use taskrunner::storage::events::{EventBody, LogEvent};

thread_local! {
    static SEQ: Cell<u32> = const { Cell::new(0) };
}

/// Builds a LogEvent with deterministic id/ts without going through a log file.
pub fn evt(body: EventBody) -> LogEvent {
    let seq = SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let ts = chrono::DateTime::from_timestamp(1_767_225_600 + i64::from(seq), 0)
        .expect("fixed epoch")
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    LogEvent { id: format!("evt_{seq:08}"), ts, body }
}

/// A realistic event sequence: one project, session, task, and completed turn.
pub fn sample_sequence() -> Vec<LogEvent> {
    vec![
        evt(EventBody::ProjectCreated { project_id: "proj_a".into(), root: "/repo".into() }),
        evt(EventBody::ProjectAliasAdded {
            project_id: "proj_a".into(),
            path: "/repo-symlink".into(),
        }),
        evt(EventBody::SessionStarted {
            session_id: "sess_a".into(),
            project_id: Some("proj_a".into()),
            client: Some("claude-code".into()),
        }),
        evt(EventBody::TaskCreated {
            task_id: "task_a".into(),
            project_id: "proj_a".into(),
            session_id: Some("sess_a".into()),
            worker: "codex".into(),
            prompt_summary: "add greeting file".into(),
            tier: None,
            runtime: None,
            allow_domains: None,
        }),
        evt(EventBody::TurnStarted {
            turn_id: "turn_a1".into(),
            task_id: "task_a".into(),
            prompt: "create hello.txt".into(),
        }),
        evt(EventBody::AuditRecorded {
            session_id: None,
            task_id: Some("task_a".into()),
            turn_id: Some("turn_a1".into()),
            kind: "worker.command_execution".into(),
            payload: serde_json::json!({ "command": "touch hello.txt" }),
        }),
        evt(EventBody::WorkerSessionRecorded {
            worker_session_id: "wsess_a".into(),
            task_id: "task_a".into(),
            worker: "codex".into(),
            native_session_id: "019e28f6-9f73-73d0-b601-33505b06d3f5".into(),
            turn_id: None,
        }),
        evt(EventBody::ArtifactStored {
            artifact_id: "art_a".into(),
            kind: "worker-events".into(),
            label: "raw codex events".into(),
            media_type: "application/jsonl".into(),
            size_bytes: 123,
            sha256: "ab".repeat(32),
            locator: format!("ab/{}", "ab".repeat(32)),
        }),
        evt(EventBody::ArtifactLinked {
            artifact_id: "art_a".into(),
            session_id: None,
            task_id: Some("task_a".into()),
            turn_id: Some("turn_a1".into()),
            audit_event_id: None,
        }),
        evt(EventBody::TurnCompleted {
            turn_id: "turn_a1".into(),
            task_id: "task_a".into(),
            response: "created hello.txt".into(),
            changed_files: vec!["hello.txt".into()],
            usage: None,
        }),
        evt(EventBody::SessionEnded { session_id: "sess_a".into() }),
    ]
}

/// A message.recorded body with only the fields a test cares about set.
pub fn message(
    message_id: &str,
    source: &str,
    session: &str,
    role: &str,
    kind: &str,
    content: &str,
) -> EventBody {
    EventBody::MessageRecorded {
        message_id: message_id.into(),
        source: source.into(),
        native_session_id: session.into(),
        native_record_id: message_id.into(),
        role: role.into(),
        kind: kind.into(),
        content: content.into(),
        native_ts: None,
        project_path: None,
    }
}
