//! Every durable record is one JSONL line appended to the event log first;
//! SQLite is a derived index folded from these events. All timestamps that
//! reach the index come from event `ts` fields so rebuilds are deterministic.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        /// logs still parse — `read_events` stops at the first unparseable line.
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

/// Reads all valid events. Stops silently at the first unparseable line: the
/// log is append-only, so anything after a torn line is a torn tail from a
/// crash mid-append.
pub fn read_events(path: &Path) -> io::Result<Vec<LogEvent>> {
    let content = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(valid_lines(&content).map(|(_, event)| event).collect())
}

/// Every parseable complete line from the top of the log, paired with the byte
/// offset just past it, ending at the first line that is torn or corrupt.
fn valid_lines(content: &str) -> impl Iterator<Item = (usize, LogEvent)> + '_ {
    let mut offset = 0;
    std::iter::from_fn(move || {
        let newline = content[offset..].find('\n')?; // no newline: unterminated tail
        let line = &content[offset..offset + newline];
        let event = parse_event_line(line).ok()?;
        offset += newline + 1;
        Some((offset, event))
    })
}

/// Truncates any torn tail so new appends land after the last valid record.
fn repair_torn_tail(path: &Path) -> io::Result<()> {
    let content = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let valid_end = valid_lines(&content).last().map_or(0, |(end, _)| end);
    if valid_end < content.len() {
        File::options().write(true).open(path)?.set_len(valid_end as u64)?;
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
}

impl EventLog {
    pub fn open(path: &Path) -> io::Result<EventLog> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        repair_torn_tail(path)?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(EventLog { path: path.to_path_buf(), file })
    }

    /// Assigns id/ts, appends one JSONL line, fsyncs, returns the full event.
    ///
    /// Lifecycle records are authoritative, so losing one would strand a turn
    /// nothing else knows about — hence the fsync. A torn tail from a crash is
    /// repaired on next open.
    pub fn append(&mut self, body: EventBody) -> io::Result<LogEvent> {
        let event = self.append_unsynced(body)?;
        self.file.sync_all()?;
        Ok(event)
    }

    /// Appends without fsync. For bulk writers of *reconstructible* events
    /// (transcript ingest, whose source files are never modified): fsync
    /// costs ~28 ms per record, which dominates a large backfill. Call
    /// `flush()` once at the end.
    pub fn append_unsynced(&mut self, body: EventBody) -> io::Result<LogEvent> {
        let event = LogEvent { id: new_id(IdPrefix::Event), ts: now_iso(), body };
        let mut line = serde_json::to_string(&event).expect("event bodies always serialize");
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        Ok(event)
    }

    /// Forces every prior append durable, including unsynced ones.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}
