use std::fs;

use taskrunner::storage::events::{EventBody, EventLog, read_events};

fn project_created() -> EventBody {
    EventBody::ProjectCreated { project_id: "proj_a".into(), root: "/repo".into() }
}

#[test]
fn round_trips_appended_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut log = EventLog::open(&path).unwrap();
    let a = log.append(project_created()).unwrap();
    let b = log
        .append(EventBody::TurnStarted {
            turn_id: "turn_1".into(),
            task_id: "task_1".into(),
            prompt: "do the thing".into(),
        })
        .unwrap();
    drop(log);

    assert!(a.id.starts_with("evt_"));
    assert!(chrono::DateTime::parse_from_rfc3339(&a.ts).is_ok());
    assert_eq!(read_events(&path).unwrap(), vec![a, b]);
}

#[test]
fn reader_stops_at_a_torn_tail_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut log = EventLog::open(&path).unwrap();
    let a = log.append(project_created()).unwrap();
    drop(log);
    append(&path, r#"{"id":"evt_torn","ts":"2026-01-01T00:00:00Z","type":"turn.sta"#);

    assert_eq!(read_events(&path).unwrap(), vec![a]);
}

#[test]
fn open_truncates_a_torn_tail_so_new_appends_stay_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut first = EventLog::open(&path).unwrap();
    let a = first.append(project_created()).unwrap();
    drop(first);
    append(&path, r#"{"id":"evt_torn","ts":"2026-01-01T00:0"#);

    let mut reopened = EventLog::open(&path).unwrap();
    let b = reopened
        .append(EventBody::SessionStarted {
            session_id: "sess_a".into(),
            project_id: None,
            client: None,
            host: None,
        })
        .unwrap();
    drop(reopened);

    assert_eq!(read_events(&path).unwrap(), vec![a, b]);
    assert!(!fs::read_to_string(&path).unwrap().contains("evt_torn"));
}

#[test]
fn open_drops_everything_after_an_interior_corrupt_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut first = EventLog::open(&path).unwrap();
    let a = first.append(project_created()).unwrap();
    drop(first);
    append(&path, "not json at all\n");
    append(
        &path,
        "{\"id\":\"evt_after\",\"ts\":\"2026-01-01T00:00:00Z\",\"type\":\"session.ended\",\"session_id\":\"sess_a\"}\n",
    );

    drop(EventLog::open(&path).unwrap());
    assert_eq!(read_events(&path).unwrap(), vec![a]);
}

#[test]
fn returns_no_events_for_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_events(&dir.path().join("missing.jsonl")).unwrap(), vec![]);
}

fn append(path: &std::path::Path, text: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}
