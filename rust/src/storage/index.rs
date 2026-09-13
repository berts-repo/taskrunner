//! Derived, rebuildable index over the event log. Delete-and-rebuild is the
//! universal recovery path, so the reducer must be deterministic (event
//! timestamps only, no wall clock) and idempotent (id-keyed INSERT OR IGNORE,
//! natural-key updates).

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, ToSql, params};

use super::events::{EventBody, LogEvent};
use super::facts::message_facts;

const SCHEMA_VERSION: i64 = 7;

const SCHEMA: &str = r#"
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  root TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE project_aliases (
  path TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id)
);
-- MCP client connections to the daemon, NOT conversations. A conversation is a
-- row in transcript_sessions; the two are unrelated and were only ever confused
-- because this table used to be called "sessions".
CREATE TABLE mcp_sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id),
  client TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  session_id TEXT REFERENCES mcp_sessions(id),
  worker TEXT NOT NULL,
  prompt_summary TEXT NOT NULL,
  status TEXT NOT NULL,
  tier TEXT,
  allow_domains TEXT,
  approval_state TEXT NOT NULL DEFAULT 'none',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE approvals (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  decision TEXT NOT NULL,
  via TEXT NOT NULL,
  domains TEXT,
  session_id TEXT,
  ts TEXT NOT NULL
);
CREATE TABLE turns (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  idx INTEGER NOT NULL,
  prompt TEXT NOT NULL,
  response TEXT,
  status TEXT NOT NULL,
  error_code TEXT,
  error_message TEXT,
  changed_files TEXT,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (task_id, idx)
);
CREATE TABLE worker_sessions (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  worker TEXT NOT NULL,
  native_session_id TEXT NOT NULL,
  turn_id TEXT REFERENCES turns(id), -- null on records written before it was emitted
  recorded_at TEXT NOT NULL
);
CREATE INDEX worker_sessions_native ON worker_sessions(native_session_id);
CREATE TABLE audit_events (
  id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES mcp_sessions(id),
  task_id TEXT REFERENCES tasks(id),
  turn_id TEXT REFERENCES turns(id),
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  ts TEXT NOT NULL
);
CREATE INDEX audit_events_task ON audit_events(task_id, ts);
CREATE INDEX audit_events_turn ON audit_events(turn_id, ts);
-- The trailing columns are facts parsed out of "content" by the message.recorded
-- fold (see facts.rs). They exist so the archive can be filtered, joined and
-- counted instead of only grepped: a tool call's identity currently lives
-- inside a JSON blob, which full-text search can hit but SQL cannot use.
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  native_session_id TEXT NOT NULL,
  native_record_id TEXT NOT NULL,
  role TEXT NOT NULL,
  kind TEXT NOT NULL,
  content TEXT NOT NULL,
  native_ts TEXT,
  project_path TEXT,
  recorded_at TEXT NOT NULL,
  tool_use_id TEXT,      -- pairs a tool_use with its tool_result
  tool_name TEXT,        -- tool_use only
  tool_target TEXT,      -- tool_use only: the path or command it acted on
  is_error INTEGER,      -- tool_result only; null when the record does not say
  prompt_idx INTEGER NOT NULL DEFAULT 0, -- see transcript_sessions.prompt_count
  turn_id TEXT REFERENCES turns(id)      -- worker sessions only; null elsewhere
);
CREATE INDEX messages_session ON messages(source, native_session_id);
CREATE INDEX messages_tool_use ON messages(tool_use_id);
CREATE INDEX messages_prompt ON messages(source, native_session_id, prompt_idx);
CREATE INDEX messages_tool ON messages(tool_name, tool_target);
CREATE INDEX messages_turn ON messages(turn_id);
CREATE VIRTUAL TABLE messages_fts USING fts5(
  content,
  message_id UNINDEXED,
  source UNINDEXED,
  native_session_id UNINDEXED,
  role UNINDEXED,
  kind UNINDEXED,
  native_ts UNINDEXED
);
-- Aggregate over messages, one row per distinct (source, native_session_id) =
-- one ingested transcript session (a Claude Code jsonl / Codex rollout). Kept in
-- lockstep with messages by the message.recorded fold so that listing sessions
-- by recency is O(sessions), not a full messages scan. Derived like everything
-- else here: a delete-and-rebuild replays the log and reconstructs it exactly.
CREATE TABLE transcript_sessions (
  id TEXT PRIMARY KEY,                       -- source || '/' || native_session_id
  source TEXT NOT NULL,
  native_session_id TEXT NOT NULL,
  project_path TEXT,
  first_ts TEXT,                             -- min native_ts seen (may be null)
  last_ts TEXT,                              -- max native_ts seen (may be null)
  first_recorded_at TEXT NOT NULL,
  last_recorded_at TEXT NOT NULL,
  message_count INTEGER NOT NULL DEFAULT 0,
  -- Real prompts seen so far; the running value of the counter that stamps
  -- messages.prompt_idx. Held here rather than in memory so incremental folds
  -- across daemon restarts continue the same numbering a rebuild produces.
  prompt_count INTEGER NOT NULL DEFAULT 0,
  UNIQUE (source, native_session_id)
);
CREATE INDEX transcript_sessions_recency ON transcript_sessions(last_ts, last_recorded_at);
CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  label TEXT NOT NULL,
  media_type TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  locator TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE artifact_links (
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  session_id TEXT REFERENCES mcp_sessions(id),
  task_id TEXT REFERENCES tasks(id),
  turn_id TEXT REFERENCES turns(id),
  audit_event_id TEXT REFERENCES audit_events(id)
);
"#;

pub const IN_MEMORY: &str = ":memory:";

pub struct StateIndex {
    pub db: Connection,
}

impl StateIndex {
    /// Opens (creating if needed) the index at `path`, or a private in-memory
    /// one for [`IN_MEMORY`]. A file at another schema version is refused:
    /// the caller deletes and rebuilds from the log.
    pub fn open(path: &str) -> anyhow::Result<StateIndex> {
        if path != IN_MEMORY
            && let Some(dir) = Path::new(path).parent()
        {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version == 0 {
            db.execute_batch(SCHEMA)?;
            db.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        } else if version != SCHEMA_VERSION {
            bail!("index schema version {version} != {SCHEMA_VERSION}; delete and rebuild");
        }
        Ok(StateIndex { db })
    }

    pub fn apply(&self, event: &LogEvent) -> rusqlite::Result<()> {
        let ts = &event.ts;
        match &event.body {
            EventBody::ProjectCreated { project_id, root } => {
                self.exec(
                    "INSERT OR IGNORE INTO projects (id, root, created_at) VALUES (?, ?, ?)",
                    params![project_id, root, ts],
                )?;
                self.exec(
                    "INSERT OR IGNORE INTO project_aliases (path, project_id) VALUES (?, ?)",
                    params![root, project_id],
                )?;
            }
            EventBody::ProjectAliasAdded { project_id, path } => {
                self.exec(
                    "INSERT OR IGNORE INTO project_aliases (path, project_id) VALUES (?, ?)",
                    params![path, project_id],
                )?;
            }
            EventBody::SessionStarted { session_id, project_id, client } => {
                self.exec(
                    "INSERT OR IGNORE INTO mcp_sessions (id, project_id, client, started_at) VALUES (?, ?, ?, ?)",
                    params![session_id, project_id, client, ts],
                )?;
            }
            EventBody::SessionEnded { session_id } => {
                self.exec(
                    "UPDATE mcp_sessions SET ended_at = ? WHERE id = ?",
                    params![ts, session_id],
                )?;
            }
            EventBody::TaskCreated {
                task_id,
                project_id,
                session_id,
                worker,
                prompt_summary,
                tier,
                allow_domains,
                ..
            } => {
                self.exec(
                    "INSERT OR IGNORE INTO tasks
                       (id, project_id, session_id, worker, prompt_summary, status,
                        tier, allow_domains, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, 'created', ?, ?, ?, ?)",
                    params![
                        task_id,
                        project_id,
                        session_id,
                        worker,
                        prompt_summary,
                        tier,
                        allow_domains.as_ref().map(json_text),
                        ts,
                        ts
                    ],
                )?;
            }
            // Legacy human-approval flow: parsed but not folded; its tasks
            // simply stay at their recorded approval_state.
            EventBody::ApprovalRequested { .. } => {}
            EventBody::ApprovalRecorded {
                approval_id,
                task_id,
                decision,
                via,
                domains,
                session_id,
            } => {
                self.exec(
                    "INSERT OR IGNORE INTO approvals (id, task_id, decision, via, domains, session_id, ts)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        approval_id, task_id, decision.as_str(), via.as_str(),
                        domains.as_ref().map(json_text), session_id, ts
                    ],
                )?;
                self.exec(
                    "UPDATE tasks SET approval_state = ?, updated_at = ? WHERE id = ?",
                    params![decision.as_str(), ts, task_id],
                )?;
            }
            EventBody::TurnStarted { turn_id, task_id, prompt } => {
                let idx: i64 = self.query(
                    "SELECT COUNT(*) FROM turns WHERE task_id = ?",
                    &[task_id],
                    |row| row.get(0),
                )?;
                self.exec(
                    "INSERT OR IGNORE INTO turns (id, task_id, idx, prompt, status, started_at)
                     VALUES (?, ?, ?, ?, 'running', ?)",
                    params![turn_id, task_id, idx, prompt, ts],
                )?;
                self.set_task_status(task_id, "running", ts)?;
            }
            EventBody::TurnCompleted { turn_id, task_id, response, changed_files, .. } => {
                self.exec(
                    "UPDATE turns SET response = ?, changed_files = ?, status = 'completed', completed_at = ?
                     WHERE id = ?",
                    params![response, json_text(changed_files), ts, turn_id],
                )?;
                self.set_task_status(task_id, "completed", ts)?;
            }
            EventBody::TurnFailed { turn_id, task_id, error_code, error_message } => {
                self.exec(
                    "UPDATE turns SET status = 'failed', error_code = ?, error_message = ?, completed_at = ?
                     WHERE id = ?",
                    params![error_code, error_message, ts, turn_id],
                )?;
                self.set_task_status(task_id, "failed", ts)?;
            }
            EventBody::TurnCanceled { turn_id, task_id, reason } => {
                self.exec(
                    "UPDATE turns SET status = 'canceled', error_message = ?, completed_at = ?
                     WHERE id = ?",
                    params![reason, ts, turn_id],
                )?;
                self.set_task_status(task_id, "canceled", ts)?;
            }
            EventBody::WorkerSessionRecorded {
                worker_session_id,
                task_id,
                worker,
                native_session_id,
                turn_id,
            } => {
                self.exec(
                    "INSERT OR IGNORE INTO worker_sessions
                       (id, task_id, worker, native_session_id, turn_id, recorded_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![worker_session_id, task_id, worker, native_session_id, turn_id, ts],
                )?;
            }
            EventBody::MessageRecorded { .. } => self.apply_message(event)?,
            EventBody::AuditRecorded { session_id, task_id, turn_id, kind, payload } => {
                self.exec(
                    "INSERT OR IGNORE INTO audit_events (id, session_id, task_id, turn_id, kind, payload, ts)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![event.id, session_id, task_id, turn_id, kind, json_text(payload), ts],
                )?;
            }
            EventBody::ArtifactStored {
                artifact_id,
                kind,
                label,
                media_type,
                size_bytes,
                sha256,
                locator,
            } => {
                self.exec(
                    "INSERT OR IGNORE INTO artifacts
                       (id, kind, label, media_type, size_bytes, sha256, locator, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        artifact_id,
                        kind,
                        label,
                        media_type,
                        *size_bytes as i64,
                        sha256,
                        locator,
                        ts
                    ],
                )?;
            }
            EventBody::ArtifactLinked {
                artifact_id,
                session_id,
                task_id,
                turn_id,
                audit_event_id,
            } => {
                self.exec(
                    "INSERT INTO artifact_links (artifact_id, session_id, task_id, turn_id, audit_event_id)
                     SELECT ?, ?, ?, ?, ?
                     WHERE NOT EXISTS (
                       SELECT 1 FROM artifact_links
                       WHERE artifact_id = ?
                         AND session_id IS ?
                         AND task_id IS ?
                         AND turn_id IS ?
                         AND audit_event_id IS ?
                     )",
                    params![
                        artifact_id, session_id, task_id, turn_id, audit_event_id,
                        artifact_id, session_id, task_id, turn_id, audit_event_id
                    ],
                )?;
            }
        }
        Ok(())
    }

    fn apply_message(&self, event: &LogEvent) -> rusqlite::Result<()> {
        let EventBody::MessageRecorded {
            message_id,
            source,
            native_session_id,
            native_record_id,
            role,
            kind,
            content,
            native_ts,
            project_path,
        } = &event.body
        else {
            return Ok(());
        };
        let session_key = format!("{source}/{native_session_id}");
        let facts = message_facts(role, kind, content);

        // prompt_idx addresses an exchange within a conversation: it is the
        // number of real prompts seen up to and including this message, so
        // everything before the first prompt (a session's harness preamble)
        // stays at 0. Read before the insert, committed only if the insert
        // actually added a row, so a re-swept duplicate never advances it.
        let prior: Option<i64> = self
            .query(
                "SELECT prompt_count FROM transcript_sessions WHERE id = ?",
                &[&session_key],
                |row| row.get(0),
            )
            .optional()?;
        let prompt_idx = prior.unwrap_or(0) + i64::from(facts.is_prompt);

        // message_id is a deterministic hash, so re-sweeping the same record
        // re-emits the event; INSERT OR IGNORE keeps `messages` idempotent.
        // FTS5 has no such guard, so only index when a row was actually added —
        // otherwise a rebuild would double-index every re-swept message.
        let inserted = self.exec(
            "INSERT OR IGNORE INTO messages
               (id, source, native_session_id, native_record_id, role, kind,
                content, native_ts, project_path, recorded_at,
                tool_use_id, tool_name, tool_target, is_error, prompt_idx, turn_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                message_id,
                source,
                native_session_id,
                native_record_id,
                role,
                kind,
                content,
                native_ts,
                project_path,
                event.ts,
                facts.tool_use_id,
                facts.tool_name,
                facts.tool_target,
                facts.is_error,
                prompt_idx,
                self.turn_for(native_session_id, native_ts.as_deref())?
            ],
        )?;
        if inserted == 0 {
            return Ok(());
        }
        self.exec(
            "INSERT INTO messages_fts
               (content, message_id, source, native_session_id, role, kind, native_ts)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![content, message_id, source, native_session_id, role, kind, native_ts],
        )?;
        // Keep the session aggregate in step. Same guard, so a re-swept (deduped)
        // message never double-counts. MIN/MAX are wrapped in COALESCE because
        // SQLite's scalar min()/max() return NULL if any argument is NULL, and
        // native_ts is optional.
        self.exec(
            "INSERT INTO transcript_sessions
               (id, source, native_session_id, project_path, first_ts, last_ts,
                first_recorded_at, last_recorded_at, message_count, prompt_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)
             ON CONFLICT(id) DO UPDATE SET
               message_count = message_count + 1,
               prompt_count = excluded.prompt_count,
               project_path = COALESCE(excluded.project_path, transcript_sessions.project_path),
               first_ts = MIN(
                 COALESCE(transcript_sessions.first_ts, excluded.first_ts),
                 COALESCE(excluded.first_ts, transcript_sessions.first_ts)),
               last_ts = MAX(
                 COALESCE(transcript_sessions.last_ts, excluded.last_ts),
                 COALESCE(excluded.last_ts, transcript_sessions.last_ts)),
               last_recorded_at = MAX(transcript_sessions.last_recorded_at, excluded.last_recorded_at)",
            params![
                session_key, source, native_session_id, project_path, native_ts, native_ts,
                event.ts, event.ts, prompt_idx
            ],
        )?;
        Ok(())
    }

    /// The turn a worker's transcript record belongs to, or None for a host
    /// session (the overwhelming majority — nothing links those to a task).
    ///
    /// Attribution is by time bucket rather than by any id in the record,
    /// because the worker CLIs write their transcripts knowing nothing about
    /// turns. It is sound because a task's container runs exactly one turn at
    /// a time, and it is deterministic on rebuild because both bounds come
    /// from event timestamps. A record swept before its turn was recorded
    /// stays None — best effort, and a later rebuild picks it up.
    fn turn_for(
        &self,
        native_session_id: &str,
        native_ts: Option<&str>,
    ) -> rusqlite::Result<Option<String>> {
        let Some(native_ts) = native_ts else { return Ok(None) };
        self.query(
            "SELECT t.id FROM worker_sessions ws
               JOIN turns t ON t.task_id = ws.task_id
              WHERE ws.native_session_id = ?
                AND t.started_at <= ?
                AND (t.completed_at IS NULL OR t.completed_at >= ?)
              ORDER BY t.started_at LIMIT 1",
            params![native_session_id, native_ts, native_ts],
            |row| row.get(0),
        )
        .optional()
    }

    fn set_task_status(&self, task_id: &str, status: &str, ts: &str) -> rusqlite::Result<()> {
        self.exec(
            "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
            params![status, ts, task_id],
        )?;
        Ok(())
    }

    // Statements are prepared once and reused: a backfill folds thousands of
    // events, and parsing the SQL every time dominated the fold.

    fn exec(&self, sql: &str, params: &[&dyn ToSql]) -> rusqlite::Result<usize> {
        self.db.prepare_cached(sql)?.execute(params)
    }

    fn query<T>(
        &self,
        sql: &str,
        params: &[&dyn ToSql],
        read: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        self.db.prepare_cached(sql)?.query_row(params, read)
    }
}

/// Compact JSON, the same text `JSON.stringify` produces for these values.
fn json_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("log values always serialize")
}

/// Deletes any existing index and folds the given events into a fresh one.
pub fn rebuild_index<'a>(
    path: &str,
    events: impl IntoIterator<Item = &'a LogEvent>,
) -> anyhow::Result<StateIndex> {
    if path != IN_MEMORY {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{path}{suffix}"));
        }
    }
    let index = StateIndex::open(path)?;
    index.db.execute_batch("BEGIN")?;
    let folded = events.into_iter().try_for_each(|event| index.apply(event));
    match folded {
        Ok(()) => index.db.execute_batch("COMMIT")?,
        Err(err) => {
            index.db.execute_batch("ROLLBACK")?;
            return Err(err.into());
        }
    }
    Ok(index)
}
