use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;
use taskrunner::ingest::sweep::{Archive, IngestSource, SweeperDeps, TranscriptSweeper};
use taskrunner::storage::events::{EventBody, EventLog, read_events};
use taskrunner::storage::index::{IN_MEMORY, StateIndex};

fn line(uuid: &str, text: &str) -> String {
    json!({
        "type": "user",
        "uuid": uuid,
        "sessionId": "s1",
        "cwd": "/repo",
        "timestamp": "2026-01-01T00:00:00Z",
        "message": { "role": "user", "content": text },
    })
    .to_string()
}

fn write(path: &Path, lines: &[String]) {
    fs::write(path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

/// What was observable at each flush, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Flush {
    logged_messages: usize,
    offsets_persisted: bool,
}

/// The daemon's bulk path in miniature: ingest appends unsynced and flushes
/// once. Shared with the sweeper through an Arc so the test can inspect it.
struct Stack {
    log: Mutex<EventLog>,
    index: Mutex<StateIndex>,
    events_log: PathBuf,
    state_file: PathBuf,
    flushes: Mutex<Vec<Flush>>,
    /// Set by the copy-out test to make the next copy fail.
    fail_next_copy: Mutex<bool>,
    copy_calls: Mutex<Vec<(String, String, String, PathBuf)>>,
}

impl Stack {
    fn message_count(&self) -> i64 {
        self.index
            .lock()
            .unwrap()
            .db
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap()
    }

    fn logged_messages(&self) -> usize {
        read_events(&self.events_log)
            .unwrap()
            .iter()
            .filter(|e| matches!(e.body, EventBody::MessageRecorded { .. }))
            .count()
    }
}

impl Archive for Stack {
    fn has_message(&self, message_id: &str) -> anyhow::Result<bool> {
        let n: i64 = self.index.lock().unwrap().db.query_row(
            "SELECT COUNT(*) FROM messages WHERE id = ?",
            [message_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn record_unsynced(&self, body: EventBody) -> anyhow::Result<()> {
        let event = self.log.lock().unwrap().append_unsynced(body)?;
        self.index.lock().unwrap().apply(&event)?;
        Ok(())
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.log.lock().unwrap().flush()?;
        self.flushes.lock().unwrap().push(Flush {
            logged_messages: self.logged_messages(),
            // On a first sweep the sidecar does not exist yet, so this reveals
            // whether offsets were persisted before this flush or after it.
            offsets_persisted: self.state_file.exists(),
        });
        Ok(())
    }
}

struct Harness {
    stack: Arc<Stack>,
    sweeper: TranscriptSweeper,
    transcript: PathBuf,
    logs: Arc<Mutex<Vec<String>>>,
    _root: tempfile::TempDir,
}

fn stack(root: &Path) -> Arc<Stack> {
    Arc::new(Stack {
        log: Mutex::new(EventLog::open(&root.join("events.jsonl")).unwrap()),
        index: Mutex::new(StateIndex::open(IN_MEMORY).unwrap()),
        events_log: root.join("events.jsonl"),
        state_file: root.join("ingest-state.json"),
        flushes: Mutex::new(Vec::new()),
        fail_next_copy: Mutex::new(false),
        copy_calls: Mutex::new(Vec::new()),
    })
}

/// An `Archive` handle the sweeper can own while the test keeps its own.
struct Shared(Arc<Stack>);

impl Archive for Shared {
    fn has_message(&self, id: &str) -> anyhow::Result<bool> {
        self.0.has_message(id)
    }
    fn record_unsynced(&self, body: EventBody) -> anyhow::Result<()> {
        self.0.record_unsynced(body)
    }
    fn flush(&self) -> anyhow::Result<()> {
        self.0.flush()
    }
}

fn harness() -> Harness {
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("claude");
    fs::create_dir_all(&source_dir).unwrap();
    let stack = stack(root.path());
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let sweeper = TranscriptSweeper::new(SweeperDeps {
        sources: vec![IngestSource {
            format: "claude-code".into(),
            dirs: vec![source_dir.to_string_lossy().into_owned()],
            ..Default::default()
        }],
        archive: Box::new(Shared(stack.clone())),
        state_file: stack.state_file.clone(),
        staging_dir: None,
        copy_volume: None,
        // Capture diagnostics so expected-error tests don't spam the test output.
        on_log: Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string()))),
    });
    Harness { stack, sweeper, transcript: source_dir.join("session.jsonl"), logs, _root: root }
}

#[test]
fn ingests_messages_and_is_idempotent_across_re_sweeps() {
    let h = harness();
    write(&h.transcript, &[line("u1", "one"), line("u2", "two")]);

    let first = h.sweeper.sweep(false);
    assert_eq!(first.recorded, 2);
    assert_eq!(h.stack.message_count(), 2);
    assert_eq!(h.stack.logged_messages(), 2);

    // Second sweep: nothing new in the index AND nothing new appended to the
    // append-forever log.
    let second = h.sweeper.sweep(false);
    assert_eq!(second.recorded, 0);
    assert_eq!(h.stack.message_count(), 2);
    assert_eq!(h.stack.logged_messages(), 2);
}

#[test]
fn picks_up_newly_appended_lines_incrementally() {
    let h = harness();
    write(&h.transcript, &[line("u1", "one")]);
    h.sweeper.sweep(false);
    assert_eq!(h.stack.message_count(), 1);

    append(&h.transcript, &format!("{}\n", line("u2", "two")));
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.recorded, 1);
    assert_eq!(h.stack.message_count(), 2);
}

#[test]
fn leaves_a_trailing_partial_line_for_the_next_sweep() {
    let h = harness();
    fs::write(
        &h.transcript,
        format!("{}\n{{\"type\":\"user\",\"uuid\":\"u2\",\"sess", line("u1", "one")),
    )
    .unwrap();
    h.sweeper.sweep(false);
    assert_eq!(h.stack.message_count(), 1); // partial line not yet consumed

    // Completing the line makes it ingestable.
    write(&h.transcript, &[line("u1", "one"), line("u2", "two")]);
    h.sweeper.sweep(false);
    assert_eq!(h.stack.message_count(), 2);
}

#[test]
fn re_scans_harmlessly_after_the_offset_sidecar_is_deleted() {
    let h = harness();
    write(&h.transcript, &[line("u1", "one"), line("u2", "two")]);
    h.sweeper.sweep(false);
    assert_eq!(h.stack.logged_messages(), 2);

    fs::remove_file(&h.stack.state_file).unwrap();
    let stats = h.sweeper.sweep(false);
    // Full re-scan, but deterministic ids dedupe every record: no new events.
    assert_eq!(stats.recorded, 0);
    assert_eq!(h.stack.message_count(), 2);
    assert_eq!(h.stack.logged_messages(), 2);
}

#[test]
fn re_reads_from_the_top_when_a_file_becomes_shorter() {
    let h = harness();
    write(&h.transcript, &[line("u1", "one"), line("u2", "two")]);
    h.sweeper.sweep(false);

    // Replace with a strictly shorter file: one carried-over and one new
    // record. (Transcripts only ever grow in normal operation; a shorter file
    // means the path was rotated or reused, so we re-scan from the top and let
    // deterministic ids dedupe the carried-over record.)
    write(&h.transcript, &[line("u1", "one"), line("u3", "x")]);
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.recorded, 1); // only the new u3; u1 dedupes
    assert_eq!(h.stack.message_count(), 3); // u1, u2, u3 all retained
}

#[test]
fn flushes_every_recorded_event_before_persisting_offsets() {
    let h = harness();
    write(&h.transcript, &[line("u1", "one"), line("u2", "two")]);
    h.sweeper.sweep(false);

    // The sidecar must never claim ground the log has not durably kept: a
    // crash that lost those records would skip them forever, because the
    // offset would stop them being re-read and dedupe would never see them.
    let flushes = h.stack.flushes.lock().unwrap().clone();
    assert_eq!(flushes.len(), 1);
    // Every record already durable at flush time...
    assert_eq!(flushes[0].logged_messages, h.stack.logged_messages());
    // ...and offsets written only afterwards, never before.
    assert!(!flushes[0].offsets_persisted);
    assert!(h.stack.state_file.exists());
}

#[test]
fn sweeps_a_large_transcript_completely() {
    // The sweep runs on a blocking thread and the socket is served on others,
    // so a big file can't stall the daemon; what's left to check is that it is
    // swept whole.
    let h = harness();
    let lines: Vec<String> = (0..6_000).map(|i| line(&format!("u{i}"), &"x".repeat(400))).collect();
    write(&h.transcript, &lines);
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.recorded, 6_000);
    assert_eq!(h.stack.message_count(), 6_000);
}

#[test]
fn tolerates_a_source_dir_that_does_not_exist() {
    let h = harness();
    fs::remove_dir_all(h.transcript.parent().unwrap()).unwrap();
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.recorded, 0);
    assert_eq!(stats.errors, 0);
    assert!(h.logs.lock().unwrap().is_empty());
}

#[test]
fn logs_the_records_a_parser_does_not_recognise_once_per_new_line() {
    // A harness that renames a record type must not vanish from the archive
    // silently: that is how Codex's tool calls went missing.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("codex");
    fs::create_dir_all(&dir).unwrap();
    let stack = stack(root.path());
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let sweeper = TranscriptSweeper::new(SweeperDeps {
        sources: vec![IngestSource {
            format: "codex".into(),
            dirs: vec![dir.to_string_lossy().into_owned()],
            ..Default::default()
        }],
        archive: Box::new(Shared(stack.clone())),
        state_file: stack.state_file.clone(),
        staging_dir: None,
        copy_volume: None,
        on_log: Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string()))),
    });
    let meta = json!({ "type": "session_meta", "payload": { "id": "cs1" } }).to_string();
    let future = json!({ "type": "response_item", "payload": { "type": "future_call" } });
    let usage = json!({ "type": "token_usage_record", "payload": {} }).to_string();
    write(
        &dir.join("rollout-2026-01-01T00-00-00-11111111-2222-3333-4444-555555555555.jsonl"),
        &[meta, future.to_string(), future.to_string(), usage],
    );

    sweeper.sweep(false);
    let logged = logs.lock().unwrap().clone();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].contains("response_item/future_call ×2"), "{logged:?}");
    assert!(!logged[0].contains("token_usage_record"), "{logged:?}");

    // Lines already read are not parsed again, so they are not reported again.
    sweeper.sweep(false);
    assert_eq!(logs.lock().unwrap().len(), 1);
}

// ---- volume sources -------------------------------------------------------

struct VolumeHarness {
    stack: Arc<Stack>,
    sweeper: TranscriptSweeper,
    /// The transcript file inside the simulated volume backing store.
    volume_file: PathBuf,
    logs: Arc<Mutex<Vec<String>>>,
    _root: tempfile::TempDir,
}

/// Simulates a volume source: a fake `copy_volume` mirrors a backing dir (the
/// "volume") into the staging dest, exercising the real
/// copy-out → staging → parse → dedupe path without Docker.
fn volume_harness(sources: Option<Vec<IngestSource>>) -> VolumeHarness {
    let root = tempfile::tempdir().unwrap();
    let backing = root.path().join("volume-backing"); // stands in for the Docker volume
    let subdir = ".claude/projects";
    let transcript_dir = backing.join(subdir);
    fs::create_dir_all(&transcript_dir).unwrap();
    let stack = stack(root.path());
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let calls = stack.clone();
    let copy_volume =
        move |volume: &str, sub: &str, image: &str, dest: &Path| -> anyhow::Result<()> {
            calls.copy_calls.lock().unwrap().push((
                volume.into(),
                sub.into(),
                image.into(),
                dest.to_path_buf(),
            ));
            if std::mem::take(&mut *calls.fail_next_copy.lock().unwrap()) {
                anyhow::bail!("docker not available");
            }
            let src = backing.join(sub);
            if !src.exists() {
                return Ok(()); // nothing logged yet: like a missing subdir
            }
            fs::create_dir_all(dest)?;
            for entry in fs::read_dir(&src)? {
                let entry = entry?;
                fs::copy(entry.path(), dest.join(entry.file_name()))?;
            }
            Ok(())
        };
    let sweeper = TranscriptSweeper::new(SweeperDeps {
        sources: sources.unwrap_or_else(|| {
            vec![IngestSource {
                format: "claude-code".into(),
                volume: Some("taskrunner-claude-home".into()),
                subdir: Some(subdir.into()),
                image: Some("img".into()),
                ..Default::default()
            }]
        }),
        archive: Box::new(Shared(stack.clone())),
        state_file: root.path().join("ingest-state.json"),
        staging_dir: Some(root.path().join("ingest-staging")),
        copy_volume: Some(Box::new(copy_volume)),
        on_log: Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string()))),
    });
    VolumeHarness {
        stack,
        sweeper,
        volume_file: transcript_dir.join("session.jsonl"),
        logs,
        _root: root,
    }
}

#[test]
fn copies_a_volume_subtree_out_and_ingests_it_idempotently() {
    let h = volume_harness(None);
    write(&h.volume_file, &[line("u1", "one"), line("u2", "two")]);

    let first = h.sweeper.sweep(false);
    assert_eq!(first.recorded, 2);
    assert_eq!(h.stack.message_count(), 2);
    let calls = h.stack.copy_calls.lock().unwrap().clone();
    let (volume, sub, image, dest) = &calls[0];
    assert_eq!(
        (volume.as_str(), sub.as_str(), image.as_str()),
        ("taskrunner-claude-home", ".claude/projects", "img")
    );
    assert!(dest.to_string_lossy().contains("taskrunner-claude-home"));

    let second = h.sweeper.sweep(false);
    assert_eq!(second.recorded, 0); // re-copied, but every record dedupes
    assert_eq!(h.stack.message_count(), 2);
}

#[test]
fn picks_up_records_appended_to_the_volume_between_sweeps() {
    let h = volume_harness(None);
    write(&h.volume_file, &[line("u1", "one")]);
    h.sweeper.sweep(false);
    assert_eq!(h.stack.message_count(), 1);

    append(&h.volume_file, &format!("{}\n", line("u2", "two")));
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.recorded, 1);
    assert_eq!(h.stack.message_count(), 2);
}

#[test]
fn counts_an_error_and_skips_the_source_when_copy_out_fails() {
    let h = volume_harness(None);
    write(&h.volume_file, &[line("u1", "one")]);
    *h.stack.fail_next_copy.lock().unwrap() = true;
    let stats = h.sweeper.sweep(false);
    assert_eq!(stats.errors, 1);
    assert_eq!(stats.recorded, 0);
    assert_eq!(h.stack.message_count(), 0);
    assert!(h.logs.lock().unwrap().iter().any(|l| l.contains("docker not available")));

    // Recovers on the next sweep once copy-out works again.
    let ok = h.sweeper.sweep(false);
    assert_eq!(ok.recorded, 1);
}

// A worker auth volume holds that worker's credentials at its root, and
// `docker cp <id>:/v//.` copies the whole volume. Refusing an incomplete
// volume source is what keeps secrets out of the staging dir.
#[test]
fn refuses_a_volume_source_with_no_subdir_or_image_instead_of_copying_out() {
    let incomplete = [
        (
            "subdir",
            IngestSource {
                format: "claude-code".into(),
                volume: Some("vol".into()),
                image: Some("img".into()),
                ..Default::default()
            },
        ),
        (
            "image",
            IngestSource {
                format: "claude-code".into(),
                volume: Some("vol".into()),
                subdir: Some(".claude/projects".into()),
                ..Default::default()
            },
        ),
    ];
    for (what, source) in incomplete {
        let h = volume_harness(Some(vec![source]));
        write(&h.volume_file, &[line("u1", "one")]);
        let stats = h.sweeper.sweep(false);
        assert_eq!(stats.errors, 1, "{what}");
        assert!(h.stack.copy_calls.lock().unwrap().is_empty(), "{what}");
        assert_eq!(h.stack.message_count(), 0, "{what}");
        assert!(
            h.logs.lock().unwrap().iter().any(|l| l.contains(&format!("has no {what}"))),
            "{what}"
        );
    }
}
