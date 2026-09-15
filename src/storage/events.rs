//! Every durable record is one JSONL line appended to the event log first;
//! SQLite is a derived index folded from these events. All timestamps that
//! reach the index come from event `ts` fields so rebuilds are deterministic.
//! Each line is also linked to every line before it and anchored; see `chain`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::chain::{self, Anchor};
use crate::ids::{IdPrefix, new_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approved,
    Denied,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Approved => "approved",
            Decision::Denied => "denied",
        }
    }
}

/// agent = relayed in-conversation; human = legacy approve/deny CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Agent,
    Human,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Agent => "agent",
            Via::Human => "human",
        }
    }
}

/// One event body per line of the log, discriminated by `type`. Optional
/// fields are absent on older records; readers must not assume defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventBody {
    #[serde(rename = "project.created")]
    ProjectCreated { project_id: String, root: String },

    #[serde(rename = "project.alias-added")]
    ProjectAliasAdded { project_id: String, path: String },

    #[serde(rename = "session.started")]
    SessionStarted {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        /// The harness the connection was registered as (`taskrunner mcp
        /// --host`). The local shim says so itself: it labels, never authorizes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },

    #[serde(rename = "session.ended")]
    SessionEnded { session_id: String },

    #[serde(rename = "task.created")]
    TaskCreated {
        task_id: String,
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        worker: String,
        prompt_summary: String,
        /// Policy field; absent on the earliest records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<String>,
        /// Legacy (removed host-run flow): never emitted anymore, kept so old
        /// logs still parse — a line that does not parse stops the daemon.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_domains: Option<Vec<String>>,
    },

    /// Legacy (removed host-run flow): never emitted anymore, kept so old logs
    /// still parse.
    #[serde(rename = "approval.requested")]
    ApprovalRequested { task_id: String, tier: String, prompt: String },

    #[serde(rename = "approval.recorded")]
    ApprovalRecorded {
        approval_id: String,
        task_id: String,
        decision: Decision,
        via: Via,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domains: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    #[serde(rename = "turn.started")]
    TurnStarted { turn_id: String, task_id: String, prompt: String },

    #[serde(rename = "turn.completed")]
    TurnCompleted {
        turn_id: String,
        task_id: String,
        response: String,
        changed_files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Value>,
    },

    #[serde(rename = "turn.failed")]
    TurnFailed { turn_id: String, task_id: String, error_code: String, error_message: String },

    #[serde(rename = "turn.canceled")]
    TurnCanceled {
        turn_id: String,
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    #[serde(rename = "worker-session.recorded")]
    WorkerSessionRecorded {
        worker_session_id: String,
        task_id: String,
        worker: String,
        native_session_id: String,
        /// The turn that opened this worker session. Absent on records written
        /// before it was emitted, so readers must not require it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },

    #[serde(rename = "audit.recorded")]
    AuditRecorded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        kind: String,
        #[serde(default)]
        payload: Value,
    },

    /// A single conversation record swept out of a host/native agent transcript
    /// (Claude Code, Codex, …). `message_id` is deterministic — a hash of
    /// (source, native_session_id, native_record_id) — so re-sweeping the same
    /// transcript record always yields the same id and the fold is idempotent.
    #[serde(rename = "message.recorded")]
    MessageRecorded {
        message_id: String,
        /// Transcript source, e.g. "claude-code" / "codex". Free-form: no reader
        /// may assume only the built-in sources exist.
        source: String,
        native_session_id: String,
        native_record_id: String,
        role: String,
        /// message | tool_use | tool_result | reasoning | system.
        kind: String,
        /// Plain text, or JSON-encoded structured blocks.
        content: String,
        /// The record's own timestamp, when the format carries one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_ts: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_path: Option<String>,
    },

    #[serde(rename = "artifact.stored")]
    ArtifactStored {
        artifact_id: String,
        kind: String,
        label: String,
        media_type: String,
        size_bytes: u64,
        sha256: String,
        locator: String,
    },

    #[serde(rename = "artifact.linked")]
    ArtifactLinked {
        artifact_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit_event_id: Option<String>,
    },
}

/// A body with the envelope the log stamps on it. Serialized as one flat
/// object: `id`, `ts`, then `type` and the body's fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub id: String,
    pub ts: String,
    #[serde(flatten)]
    pub body: EventBody,
}

pub fn parse_event_line(line: &str) -> serde_json::Result<LogEvent> {
    serde_json::from_str(line)
}

/// Reads every event. An unterminated last line is a write a crash cut short
/// and is left out; a complete line that is not an event is damage, and an
/// error.
pub fn read_events(path: &Path) -> io::Result<Vec<LogEvent>> {
    let content = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(parse_log(path, &content)?.0)
}

/// The log's events, and the byte offset where its complete lines end.
fn parse_log(path: &Path, content: &str) -> io::Result<(Vec<LogEvent>, usize)> {
    let mut events = Vec::new();
    let mut offset = 0;
    while let Some(newline) = content[offset..].find('\n') {
        let line = &content[offset..offset + newline];
        let event = parse_event_line(line).map_err(|err| damaged(path, events.len() + 1, err))?;
        events.push(event);
        offset += newline + 1;
    }
    Ok((events, offset))
}

/// Repairing a damaged line would mean discarding it and every line after
/// it — the loss an audit log exists to prevent — so the choice is left to
/// the person who owns the log.
fn damaged(path: &Path, line: usize, err: serde_json::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{}: line {line} is not a valid event ({err}). Taskrunner will not start on a \
             damaged log: repairing it would discard that line and every line after it. See \
             docs/log-integrity.md, \"If the daemon refuses to start\".",
            path.display()
        ),
    )
}

/// Removes an unterminated last line, a write a crash cut short, so new
/// appends start on a line of their own. Fails on a damaged line instead.
fn repair_torn_tail(path: &Path) -> io::Result<()> {
    let content = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let (_, complete) = parse_log(path, &content)?;
    if complete < content.len() {
        File::options().write(true).open(path)?.set_len(complete as u64)?;
    }
    Ok(())
}

/// Wall-clock time in the exact shape JavaScript's `toISOString()` produces.
/// Timestamps are compared as text in SQL, so the format is load-bearing.
pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub struct EventLog {
    pub path: PathBuf,
    file: File,
    /// The fingerprint of every line so far: the next line's `prev`.
    head: String,
    events: u64,
    last_id: String,
    last_ts: String,
    /// The last event an anchor covers.
    anchored: u64,
}

impl EventLog {
    pub fn open(path: &Path) -> io::Result<EventLog> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        repair_torn_tail(path)?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let walk = chain::walk(path, |_, _| {})?;
        let anchors = chain::read_anchors(&chain::anchors_path(path))?;
        let mut log = EventLog {
            path: path.to_path_buf(),
            file,
            head: walk.head,
            events: walk.events,
            last_id: walk.last_id,
            last_ts: walk.last_ts,
            anchored: anchors.anchors.last().map_or(0, |anchor| anchor.event),
        };
        // The first time a log written before chaining is opened, this anchors
        // all of that history.
        log.anchor();
        Ok(log)
    }

    /// Assigns id/ts, appends one JSONL line, fsyncs, returns the full event.
    ///
    /// Lifecycle records are authoritative, so losing one would strand a turn
    /// nothing else knows about — hence the fsync. A torn tail from a crash is
    /// repaired on next open.
    pub fn append(&mut self, body: EventBody) -> io::Result<LogEvent> {
        let event = self.append_unsynced(body)?;
        self.file.sync_all()?;
        self.anchor_if_due();
        Ok(event)
    }

    /// Appends without fsync. For bulk writers of *reconstructible* events
    /// (transcript ingest, whose source files are never modified): fsync
    /// costs ~28 ms per record, which dominates a large backfill. Call
    /// `flush()` once at the end.
    pub fn append_unsynced(&mut self, body: EventBody) -> io::Result<LogEvent> {
        let event = LogEvent { id: new_id(IdPrefix::Event), ts: now_iso(), body };
        let line = self.linked_line(&event);
        self.file.write_all(format!("{line}\n").as_bytes())?;
        self.head = chain::fingerprint(&self.head, line.as_bytes());
        self.events += 1;
        self.last_id.clone_from(&event.id);
        self.last_ts.clone_from(&event.ts);
        Ok(event)
    }

    /// Forces every prior append durable, including unsynced ones.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()?;
        self.anchor_if_due();
        Ok(())
    }

    /// Appends the log's current fingerprint to the anchors file, unless the
    /// latest anchor already covers the last event. Call it only once every
    /// append is durable. A failed write is reported, never fatal: the events
    /// are already safe in the log, and the next anchor covers them too.
    pub fn anchor(&mut self) {
        if self.events == 0 || self.events == self.anchored {
            return;
        }
        let anchor = Anchor {
            event: self.events,
            id: self.last_id.clone(),
            ts: self.last_ts.clone(),
            fingerprint: self.head.clone(),
        };
        match chain::write_anchor(&chain::anchors_path(&self.path), &anchor) {
            Ok(()) => self.anchored = self.events,
            Err(err) => {
                eprintln!("taskrunner: could not write an anchor for event {}: {err}", self.events)
            }
        }
    }

    fn anchor_if_due(&mut self) {
        if self.events.saturating_sub(self.anchored) >= chain::ANCHOR_EVERY {
            self.anchor();
        }
    }

    /// The event as one JSON line, linked to every line before it. `prev` is
    /// the line's place in the file rather than part of the event, so
    /// `LogEvent` leaves it out and the index never sees it.
    fn linked_line(&self, event: &LogEvent) -> String {
        let mut line = serde_json::to_value(event).expect("event bodies always serialize");
        if !self.head.is_empty() {
            line.as_object_mut()
                .expect("an event is a JSON object")
                .insert("prev".into(), self.head.clone().into());
        }
        line.to_string()
    }
}
