import * as fs from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { LogEvent } from "./events.js";
import { messageFacts } from "./message-facts.js";

// Derived, rebuildable index over the event log.
// Delete-and-rebuild is the universal recovery path,
// so the reducer must be deterministic (event timestamps only, no wall clock)
// and idempotent (id-keyed INSERT OR IGNORE, natural-key updates).

const SCHEMA_VERSION = 7;

const SCHEMA = `
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
-- fold (see message-facts.ts). They exist so the archive can be filtered, joined
-- and counted instead of only grepped: a tool call's identity currently lives
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
`;

export class StateIndex {
  readonly db: DatabaseSync;

  constructor(path: string) {
    if (path !== ":memory:") fs.mkdirSync(dirname(path), { recursive: true });
    this.db = new DatabaseSync(path);
    this.db.exec("PRAGMA journal_mode = WAL");
    this.db.exec("PRAGMA foreign_keys = ON");
    const row = this.db.prepare("PRAGMA user_version").get() as {
      user_version: number;
    };
    if (row.user_version === 0) {
      this.db.exec(SCHEMA);
      this.db.exec(`PRAGMA user_version = ${SCHEMA_VERSION}`);
    } else if (row.user_version !== SCHEMA_VERSION) {
      this.db.close();
      throw new Error(
        `index schema version ${row.user_version} != ${SCHEMA_VERSION}; delete and rebuild`,
      );
    }
  }

  apply(event: LogEvent): void {
    const db = this.db;
    switch (event.type) {
      case "project.created":
        db.prepare(
          "INSERT OR IGNORE INTO projects (id, root, created_at) VALUES (?, ?, ?)",
        ).run(event.project_id, event.root, event.ts);
        db.prepare(
          "INSERT OR IGNORE INTO project_aliases (path, project_id) VALUES (?, ?)",
        ).run(event.root, event.project_id);
        break;
      case "project.alias-added":
        db.prepare(
          "INSERT OR IGNORE INTO project_aliases (path, project_id) VALUES (?, ?)",
        ).run(event.path, event.project_id);
        break;
      case "session.started":
        db.prepare(
          "INSERT OR IGNORE INTO mcp_sessions (id, project_id, client, started_at) VALUES (?, ?, ?, ?)",
        ).run(event.session_id, event.project_id ?? null, event.client ?? null, event.ts);
        break;
      case "session.ended":
        db.prepare("UPDATE mcp_sessions SET ended_at = ? WHERE id = ?").run(
          event.ts,
          event.session_id,
        );
        break;
      case "task.created":
        db.prepare(
          `INSERT OR IGNORE INTO tasks
             (id, project_id, session_id, worker, prompt_summary, status,
              tier, allow_domains, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, 'created', ?, ?, ?, ?)`,
        ).run(
          event.task_id,
          event.project_id,
          event.session_id ?? null,
          event.worker,
          event.prompt_summary,
          event.tier ?? null,
          event.allow_domains ? JSON.stringify(event.allow_domains) : null,
          event.ts,
          event.ts,
        );
        break;
      // "approval.requested" (legacy human-approval flow) is parsed but not
      // folded; its tasks simply stay at their recorded approval_state.
      case "approval.recorded":
        db.prepare(
          `INSERT OR IGNORE INTO approvals (id, task_id, decision, via, domains, session_id, ts)
           VALUES (?, ?, ?, ?, ?, ?, ?)`,
        ).run(
          event.approval_id,
          event.task_id,
          event.decision,
          event.via,
          event.domains ? JSON.stringify(event.domains) : null,
          event.session_id ?? null,
          event.ts,
        );
        db.prepare("UPDATE tasks SET approval_state = ?, updated_at = ? WHERE id = ?").run(
          event.decision,
          event.ts,
          event.task_id,
        );
        break;
      case "turn.started": {
        const { n } = db
          .prepare("SELECT COUNT(*) AS n FROM turns WHERE task_id = ?")
          .get(event.task_id) as { n: number };
        db.prepare(
          `INSERT OR IGNORE INTO turns (id, task_id, idx, prompt, status, started_at)
           VALUES (?, ?, ?, ?, 'running', ?)`,
        ).run(event.turn_id, event.task_id, n, event.prompt, event.ts);
        this.setTaskStatus(event.task_id, "running", event.ts);
        break;
      }
      case "turn.completed":
        db.prepare(
          `UPDATE turns SET response = ?, changed_files = ?, status = 'completed', completed_at = ?
           WHERE id = ?`,
        ).run(
          event.response,
          JSON.stringify(event.changed_files),
          event.ts,
          event.turn_id,
        );
        this.setTaskStatus(event.task_id, "completed", event.ts);
        break;
      case "turn.failed":
        db.prepare(
          `UPDATE turns SET status = 'failed', error_code = ?, error_message = ?, completed_at = ?
           WHERE id = ?`,
        ).run(event.error_code, event.error_message, event.ts, event.turn_id);
        this.setTaskStatus(event.task_id, "failed", event.ts);
        break;
      case "turn.canceled":
        db.prepare(
          `UPDATE turns SET status = 'canceled', error_message = ?, completed_at = ?
           WHERE id = ?`,
        ).run(event.reason ?? null, event.ts, event.turn_id);
        this.setTaskStatus(event.task_id, "canceled", event.ts);
        break;
      case "worker-session.recorded":
        db.prepare(
          `INSERT OR IGNORE INTO worker_sessions
             (id, task_id, worker, native_session_id, turn_id, recorded_at)
           VALUES (?, ?, ?, ?, ?, ?)`,
        ).run(
          event.worker_session_id,
          event.task_id,
          event.worker,
          event.native_session_id,
          event.turn_id ?? null,
          event.ts,
        );
        break;
      case "message.recorded": {
        const sessionKey = `${event.source}/${event.native_session_id}`;
        const facts = messageFacts(event.role, event.kind, event.content);
        // prompt_idx addresses an exchange within a conversation: it is the
        // number of real prompts seen up to and including this message, so
        // everything before the first prompt (a session's harness preamble)
        // stays at 0. Read before the insert, committed only if the insert
        // actually added a row, so a re-swept duplicate never advances it.
        const prior = db
          .prepare("SELECT prompt_count FROM transcript_sessions WHERE id = ?")
          .get(sessionKey) as { prompt_count: number } | undefined;
        const promptIdx = (prior?.prompt_count ?? 0) + (facts.is_prompt ? 1 : 0);

        // message_id is a deterministic hash, so re-sweeping the same record
        // re-emits the event; INSERT OR IGNORE keeps `messages` idempotent.
        // FTS5 has no such guard, so only index when a row was actually added —
        // otherwise a rebuild would double-index every re-swept message.
        const res = db
          .prepare(
            `INSERT OR IGNORE INTO messages
               (id, source, native_session_id, native_record_id, role, kind,
                content, native_ts, project_path, recorded_at,
                tool_use_id, tool_name, tool_target, is_error, prompt_idx, turn_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            event.message_id,
            event.source,
            event.native_session_id,
            event.native_record_id,
            event.role,
            event.kind,
            event.content,
            event.native_ts ?? null,
            event.project_path ?? null,
            event.ts,
            facts.tool_use_id,
            facts.tool_name,
            facts.tool_target,
            facts.is_error,
            promptIdx,
            this.turnFor(event.native_session_id, event.native_ts),
          );
        if (Number(res.changes) > 0) {
          db.prepare(
            `INSERT INTO messages_fts
               (content, message_id, source, native_session_id, role, kind, native_ts)
             VALUES (?, ?, ?, ?, ?, ?, ?)`,
          ).run(
            event.content,
            event.message_id,
            event.source,
            event.native_session_id,
            event.role,
            event.kind,
            event.native_ts ?? null,
          );
          // Keep the session aggregate in step. Guarded by res.changes so a
          // re-swept (deduped) message never double-counts. MIN/MAX are wrapped
          // in COALESCE because SQLite's scalar min()/max() return NULL if any
          // argument is NULL, and native_ts is optional.
          db.prepare(
            `INSERT INTO transcript_sessions
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
               last_recorded_at = MAX(transcript_sessions.last_recorded_at, excluded.last_recorded_at)`,
          ).run(
            sessionKey,
            event.source,
            event.native_session_id,
            event.project_path ?? null,
            event.native_ts ?? null,
            event.native_ts ?? null,
            event.ts,
            event.ts,
            promptIdx,
          );
        }
        break;
      }
      case "audit.recorded":
        db.prepare(
          `INSERT OR IGNORE INTO audit_events (id, session_id, task_id, turn_id, kind, payload, ts)
           VALUES (?, ?, ?, ?, ?, ?, ?)`,
        ).run(
          event.id,
          event.session_id ?? null,
          event.task_id ?? null,
          event.turn_id ?? null,
          event.kind,
          JSON.stringify(event.payload ?? null),
          event.ts,
        );
        break;
      case "artifact.stored":
        db.prepare(
          `INSERT OR IGNORE INTO artifacts
             (id, kind, label, media_type, size_bytes, sha256, locator, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
        ).run(
          event.artifact_id,
          event.kind,
          event.label,
          event.media_type,
          event.size_bytes,
          event.sha256,
          event.locator,
          event.ts,
        );
        break;
      case "artifact.linked":
        db.prepare(
          `INSERT INTO artifact_links (artifact_id, session_id, task_id, turn_id, audit_event_id)
           SELECT ?, ?, ?, ?, ?
           WHERE NOT EXISTS (
             SELECT 1 FROM artifact_links
             WHERE artifact_id = ?
               AND session_id IS ?
               AND task_id IS ?
               AND turn_id IS ?
               AND audit_event_id IS ?
           )`,
        ).run(
          event.artifact_id,
          event.session_id ?? null,
          event.task_id ?? null,
          event.turn_id ?? null,
          event.audit_event_id ?? null,
          event.artifact_id,
          event.session_id ?? null,
          event.task_id ?? null,
          event.turn_id ?? null,
          event.audit_event_id ?? null,
        );
        break;
    }
  }

  /**
   * The turn a worker's transcript record belongs to, or null for a host
   * session (the overwhelming majority — nothing links those to a task).
   *
   * Attribution is by time bucket rather than by any id in the record, because
   * the worker CLIs write their transcripts knowing nothing about turns. It is
   * sound because a task's container runs exactly one turn at a time, and it is
   * deterministic on rebuild because both bounds come from event timestamps.
   * A record swept before its turn was recorded stays null — best effort, and
   * a later rebuild picks it up.
   */
  private turnFor(nativeSessionId: string, nativeTs: string | undefined): string | null {
    if (nativeTs === undefined) return null;
    const row = this.db
      .prepare(
        `SELECT t.id FROM worker_sessions ws
           JOIN turns t ON t.task_id = ws.task_id
          WHERE ws.native_session_id = ?
            AND t.started_at <= ?
            AND (t.completed_at IS NULL OR t.completed_at >= ?)
          ORDER BY t.started_at LIMIT 1`,
      )
      .get(nativeSessionId, nativeTs, nativeTs) as { id: string } | undefined;
    return row?.id ?? null;
  }

  private setTaskStatus(taskId: string, status: string, ts: string): void {
    this.db
      .prepare("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?")
      .run(status, ts, taskId);
  }

  close(): void {
    this.db.close();
  }
}

/** Deletes any existing index and folds the given events into a fresh one. */
export function rebuildIndex(path: string, events: Iterable<LogEvent>): StateIndex {
  if (path !== ":memory:") {
    for (const suffix of ["", "-wal", "-shm"]) {
      fs.rmSync(path + suffix, { force: true });
    }
  }
  const index = new StateIndex(path);
  index.db.exec("BEGIN");
  try {
    for (const event of events) index.apply(event);
    index.db.exec("COMMIT");
  } catch (err) {
    index.db.exec("ROLLBACK");
    index.close();
    throw err;
  }
  return index;
}
