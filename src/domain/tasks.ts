import type { StateIndex } from "../storage/index.js";

// Read-side helpers over the derived index. Turn statuses are
// running | completed | failed | canceled; a task mirrors its latest turn.

export interface ArtifactHandle {
  artifact_id: string;
  kind: string;
  label: string;
  media_type: string;
  size_bytes: number;
  sha256: string;
  locator: string;
}

export interface TurnInfo {
  turn_id: string;
  idx: number;
  prompt: string;
  response: string | null;
  status: string;
  error_code: string | null;
  error_message: string | null;
  changed_files: string[];
  started_at: string;
  completed_at: string | null;
}

export interface TaskSnapshot {
  task_id: string;
  project_id: string;
  project_root: string;
  session_id: string | null;
  worker: string;
  prompt_summary: string;
  status: string;
  created_at: string;
  updated_at: string;
  worker_session_id: string | null;
  turn_count: number;
  latest_turn: TurnInfo | null;
}

interface TurnRow {
  id: string;
  idx: number;
  prompt: string;
  response: string | null;
  status: string;
  error_code: string | null;
  error_message: string | null;
  changed_files: string | null;
  started_at: string;
  completed_at: string | null;
}

function toTurnInfo(row: TurnRow): TurnInfo {
  return {
    turn_id: row.id,
    idx: row.idx,
    prompt: row.prompt,
    response: row.response,
    status: row.status,
    error_code: row.error_code,
    error_message: row.error_message,
    changed_files: row.changed_files ? (JSON.parse(row.changed_files) as string[]) : [],
    started_at: row.started_at,
    completed_at: row.completed_at,
  };
}

export function listTurns(index: StateIndex, taskId: string): TurnInfo[] {
  const rows = index.db
    .prepare("SELECT * FROM turns WHERE task_id = ? ORDER BY idx")
    .all(taskId) as unknown as TurnRow[];
  return rows.map(toTurnInfo);
}

export function getTaskSnapshot(index: StateIndex, taskId: string): TaskSnapshot | null {
  const task = index.db
    .prepare(
      `SELECT t.*, p.root AS project_root FROM tasks t JOIN projects p ON p.id = t.project_id
       WHERE t.id = ?`,
    )
    .get(taskId) as
    | {
        id: string;
        project_id: string;
        project_root: string;
        session_id: string | null;
        worker: string;
        prompt_summary: string;
        status: string;
        created_at: string;
        updated_at: string;
      }
    | undefined;
  if (!task) return null;

  const latest = index.db
    .prepare("SELECT * FROM turns WHERE task_id = ? ORDER BY idx DESC LIMIT 1")
    .get(taskId) as TurnRow | undefined;
  const { n } = index.db
    .prepare("SELECT COUNT(*) AS n FROM turns WHERE task_id = ?")
    .get(taskId) as { n: number };
  const wsess = index.db
    .prepare(
      "SELECT native_session_id FROM worker_sessions WHERE task_id = ? ORDER BY recorded_at DESC, id DESC LIMIT 1",
    )
    .get(taskId) as { native_session_id: string } | undefined;

  return {
    task_id: task.id,
    project_id: task.project_id,
    project_root: task.project_root,
    session_id: task.session_id,
    worker: task.worker,
    prompt_summary: task.prompt_summary,
    status: task.status,
    created_at: task.created_at,
    updated_at: task.updated_at,
    worker_session_id: wsess?.native_session_id ?? null,
    turn_count: n,
    latest_turn: latest ? toTurnInfo(latest) : null,
  };
}

export function getTurnArtifacts(index: StateIndex, turnId: string): ArtifactHandle[] {
  return index.db
    .prepare(
      `SELECT a.id AS artifact_id, a.kind, a.label, a.media_type, a.size_bytes, a.sha256, a.locator
       FROM artifact_links l JOIN artifacts a ON a.id = l.artifact_id
       WHERE l.turn_id = ? ORDER BY a.created_at, a.id`,
    )
    .all(turnId) as unknown as ArtifactHandle[];
}
