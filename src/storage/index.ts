import * as fs from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { LogEvent } from "./events.js";

// Derived, rebuildable index over the event log.
// Delete-and-rebuild is the universal recovery path,
// so the reducer must be deterministic (event timestamps only, no wall clock)
// and idempotent (id-keyed INSERT OR IGNORE, natural-key updates).

export const SCHEMA_VERSION = 3;

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
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id),
  client TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  session_id TEXT REFERENCES sessions(id),
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
  recorded_at TEXT NOT NULL
);
CREATE TABLE audit_events (
  id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id),
  task_id TEXT REFERENCES tasks(id),
  turn_id TEXT REFERENCES turns(id),
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  ts TEXT NOT NULL
);
CREATE INDEX audit_events_task ON audit_events(task_id, ts);
CREATE INDEX audit_events_turn ON audit_events(turn_id, ts);
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
  session_id TEXT REFERENCES sessions(id),
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
          "INSERT OR IGNORE INTO sessions (id, project_id, client, started_at) VALUES (?, ?, ?, ?)",
        ).run(event.session_id, event.project_id ?? null, event.client ?? null, event.ts);
        break;
      case "session.ended":
        db.prepare("UPDATE sessions SET ended_at = ? WHERE id = ?").run(
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
          `INSERT OR IGNORE INTO worker_sessions (id, task_id, worker, native_session_id, recorded_at)
           VALUES (?, ?, ?, ?, ?)`,
        ).run(
          event.worker_session_id,
          event.task_id,
          event.worker,
          event.native_session_id,
          event.ts,
        );
        break;
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
