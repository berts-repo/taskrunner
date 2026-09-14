import * as fs from "node:fs";
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
  project_root: string;
  worker: string;
  prompt_summary: string;
  status: string;
  tier: string | null;
  allow_domains: string[];
  approval_state: string;
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
        tier: string | null;
        allow_domains: string | null;
        approval_state: string;
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
    project_root: task.project_root,
    worker: task.worker,
    prompt_summary: task.prompt_summary,
    status: task.status,
    tier: task.tier,
    allow_domains: task.allow_domains ? (JSON.parse(task.allow_domains) as string[]) : [],
    approval_state: task.approval_state,
    updated_at: task.updated_at,
    worker_session_id: wsess?.native_session_id ?? null,
    turn_count: n,
    latest_turn: latest ? toTurnInfo(latest) : null,
  };
}

/** Read-only project lookup: never creates records (unlike resolveProject). */
export function findProjectByPath(
  index: StateIndex,
  path: string,
): { project_id: string; root: string } | null {
  const candidates = [path];
  try {
    candidates.push(fs.realpathSync(path));
  } catch {
    // Path may no longer exist; alias lookup can still hit.
  }
  for (const candidate of candidates) {
    const row = index.db
      .prepare(
        `SELECT p.id AS project_id, p.root FROM project_aliases a
         JOIN projects p ON p.id = a.project_id WHERE a.path = ?`,
      )
      .get(candidate) as { project_id: string; root: string } | undefined;
    if (row) return row;
  }
  return null;
}

export function listTaskSnapshots(
  index: StateIndex,
  projectId: string,
  limit: number,
): TaskSnapshot[] {
  const rows = index.db
    .prepare(
      "SELECT id FROM tasks WHERE project_id = ? ORDER BY updated_at DESC, id DESC LIMIT ?",
    )
    .all(projectId, limit) as { id: string }[];
  return rows
    .map((row) => getTaskSnapshot(index, row.id))
    .filter((s): s is TaskSnapshot => s !== null);
}

interface AuditRow {
  ts: string;
  kind: string;
  payload: unknown;
}

export function getTurnAudit(index: StateIndex, turnId: string): AuditRow[] {
  const rows = index.db
    .prepare("SELECT ts, kind, payload FROM audit_events WHERE turn_id = ? ORDER BY ts, id")
    .all(turnId) as { ts: string; kind: string; payload: string }[];
  return rows.map((row) => ({
    ts: row.ts,
    kind: row.kind,
    payload: JSON.parse(row.payload) as unknown,
  }));
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

// One swept transcript record belonging to a task's worker session(s). The
// tool_* and prompt_idx columns are projection-time facts (see message-facts.ts),
// carried here so a renderer never has to re-parse the content blob.
export interface TranscriptMessage {
  /** Ingest source, retained so the reader can name the harness that replied. */
  source: string;
  role: string;
  kind: string;
  content: string;
  native_ts: string | null;
  native_session_id: string;
  prompt_idx: number;
  tool_name: string | null;
  tool_target: string | null;
  is_error: number | null;
}

/** Default cap on messages rendered for one task, so a lookup can't dump a
 * whole delegated turn's interior in a single response. */
export const DEFAULT_TRANSCRIPT_LIMIT = 500;

/** Shared scoping for the two transcript readers. */
export interface MessageQuery {
  /** Keep the newest N. `null` reads the whole session — the timeline view's
   * default, since a capped audit trail is not an audit trail. */
  limit?: number | null;
  /** Restrict to one addressable exchange: a real user prompt and everything
   * that followed it, up to the next one. */
  promptIdx?: number;
}

const MESSAGE_COLUMNS = `source, role, kind, content, native_ts, native_session_id,
                         prompt_idx, tool_name, tool_target, is_error`;

/** `LIMIT -1` is SQLite's "no limit"; the +1 probe detects a truncated read. */
function limitClause(limit: number | null): { bind: number; cap: number | null } {
  return limit === null ? { bind: -1, cap: null } : { bind: limit + 1, cap: limit };
}

function applyCap<T>(rows: T[], cap: number | null): { rows: T[]; capped: boolean } {
  const capped = cap !== null && rows.length > cap;
  if (capped) rows.length = cap as number;
  rows.reverse(); // newest-first for the cap, chronological for display
  return { rows, capped };
}

/**
 * A task's archived transcript: every message swept from the worker session(s)
 * this task ran under, joined via the phase-2 link
 * `worker_sessions.native_session_id → messages.native_session_id`. Returned in
 * chronological order, but capped to the most recent `limit` — a delegated turn
 * can archive thousands of records and the tail is what a caller usually wants.
 * Fetches one past `limit` to report whether older messages were dropped.
 */
export function getTaskMessages(
  index: StateIndex,
  taskId: string,
  opts: MessageQuery = {},
): { messages: TranscriptMessage[]; capped: boolean } {
  const { bind, cap } = limitClause(opts.limit === undefined ? DEFAULT_TRANSCRIPT_LIMIT : opts.limit);
  const promptFilter = opts.promptIdx === undefined ? "" : "AND prompt_idx = ?";
  const rows = index.db
    .prepare(
      `SELECT ${MESSAGE_COLUMNS}
         FROM messages
        WHERE native_session_id IN (
                SELECT DISTINCT native_session_id FROM worker_sessions WHERE task_id = ?
              )
          ${promptFilter}
        ORDER BY native_ts DESC, recorded_at DESC, id DESC
        LIMIT ?`,
    )
    .all(
      ...([taskId, ...(opts.promptIdx === undefined ? [] : [opts.promptIdx]), bind] as never[]),
    ) as unknown as TranscriptMessage[];
  const { rows: messages, capped } = applyCap(rows, cap);
  return { messages, capped };
}

// One ingested transcript session (a distinct source+native_session_id = one
// Claude Code jsonl / Codex rollout), aggregated in `transcript_sessions`.
// `task_id` is set only for a worker session linked to a task; host sessions
// (Claude Code / Codex conversations on this machine) carry none.
export interface SessionInfo {
  id: string;
  source: string;
  native_session_id: string;
  project_path: string | null;
  first_ts: string | null;
  last_ts: string | null;
  first_recorded_at: string;
  last_recorded_at: string;
  message_count: number;
  task_id: string | null;
}

export const DEFAULT_SESSION_LIMIT = 20;

/**
 * Ingested transcript sessions, most recent first. Recency is
 * `COALESCE(last_ts, last_recorded_at)` so a session whose format carries no
 * per-record timestamp still orders by when it was ingested. Optional `project`
 * filters to one project root; `nativeSessionId` narrows to a single session id
 * (used to resolve a bare id to its source). Task linkage is a correlated
 * subquery, not a join, so a session linked to several tasks stays one row.
 */
export function listSessions(
  index: StateIndex,
  opts: { project?: string; nativeSessionId?: string; limit?: number; before?: string } = {},
): SessionInfo[] {
  const clauses: string[] = [];
  const params: unknown[] = [];
  if (opts.project !== undefined) {
    clauses.push("s.project_path = ?");
    params.push(opts.project);
  }
  if (opts.nativeSessionId !== undefined) {
    clauses.push("s.native_session_id = ?");
    params.push(opts.nativeSessionId);
  }
  if (opts.before !== undefined) {
    clauses.push("COALESCE(s.last_ts, s.last_recorded_at) < ?");
    params.push(opts.before);
  }
  const where = clauses.length > 0 ? `WHERE ${clauses.join(" AND ")}` : "";
  params.push(opts.limit ?? DEFAULT_SESSION_LIMIT);
  return index.db
    .prepare(
      `SELECT s.id, s.source, s.native_session_id, s.project_path,
              s.first_ts, s.last_ts, s.first_recorded_at, s.last_recorded_at,
              s.message_count,
              (SELECT ws.task_id FROM worker_sessions ws
                WHERE ws.native_session_id = s.native_session_id
                ORDER BY ws.recorded_at, ws.id LIMIT 1) AS task_id
         FROM transcript_sessions s
         ${where}
        ORDER BY COALESCE(s.last_ts, s.last_recorded_at) DESC, s.id DESC
        LIMIT ?`,
    )
    .all(...(params as never[])) as unknown as SessionInfo[];
}

/**
 * One session's messages in chronological order, keyed directly on
 * source+native_session_id so it works for a host session that no task links.
 * Same cap semantics as {@link getTaskMessages}: fetch the newest `limit`
 * (a long session can hold thousands), plus one to report whether older
 * messages were dropped, then present oldest-first.
 */
export function getSessionMessages(
  index: StateIndex,
  source: string,
  nativeSessionId: string,
  opts: MessageQuery = {},
): { messages: TranscriptMessage[]; capped: boolean } {
  const { bind, cap } = limitClause(opts.limit === undefined ? DEFAULT_TRANSCRIPT_LIMIT : opts.limit);
  const promptFilter = opts.promptIdx === undefined ? "" : "AND prompt_idx = ?";
  const rows = index.db
    .prepare(
      `SELECT ${MESSAGE_COLUMNS}
         FROM messages
        WHERE source = ? AND native_session_id = ?
          ${promptFilter}
        ORDER BY native_ts DESC, recorded_at DESC, id DESC
        LIMIT ?`,
    )
    .all(
      ...([
        source,
        nativeSessionId,
        ...(opts.promptIdx === undefined ? [] : [opts.promptIdx]),
        bind,
      ] as never[]),
    ) as unknown as TranscriptMessage[];
  const { rows: messages, capped } = applyCap(rows, cap);
  return { messages, capped };
}

// One hit from corpus-wide transcript search. `task_id` is present only when the
// matched message came from a worker session linked to a task; host-session
// messages match too and carry none. `prompt_idx` makes a hit drillable: it is
// the address to re-read the exchange it came from.
export interface TranscriptHit {
  task_id: string | null;
  source: string;
  role: string;
  kind: string;
  native_ts: string | null;
  native_session_id: string;
  project_path: string | null;
  prompt_idx: number;
  tool_name: string | null;
  tool_target: string | null;
  is_error: number | null;
  /** The matched text with the hit bracketed; null for a filter-only search,
   * which never touches the full-text index and so has nothing to highlight. */
  snippet: string | null;
}

/** Optional scoping for {@link searchMessages}. */
export interface SearchFilters {
  project?: string;
  /** Restrict to these native session ids. Takes precedence over lastSessions. */
  sessions?: string[];
  /** Restrict to the most-recent N sessions (within `project` when set). */
  lastSessions?: number;
  since?: string;
  until?: string;
  role?: string;
  kind?: string;
  /** Restrict to calls of one tool, e.g. "Edit". Matches tool_use records only. */
  tool?: string;
  /** Substring of the path or command a call acted on; SQL LIKE wildcards apply. */
  target?: string;
  /** true: the call failed. false: it succeeded. Records that state no outcome
   * (rejections, aborts) are excluded either way — see message-facts.ts. */
  failed?: boolean;
  /** "rank" (relevance, default) or "recent" (newest native_ts first). Ignored
   * without a query: a filter-only search has no relevance to rank by. */
  sort?: "rank" | "recent";
}

export const DEFAULT_SEARCH_LIMIT = 20;

/**
 * A call and its result are one thing, so a tool_use record inherits the failure
 * state of the result it is paired with, while a result states its own. Lets
 * `failed` combine with `tool`/`target`, which live on the *call* record.
 */
const ERROR_STATE = `COALESCE(m.is_error,
      (SELECT r.is_error FROM messages r
        WHERE r.tool_use_id = m.tool_use_id AND r.kind = 'tool_result'))`;

/** Columns the fts table carries unindexed, so a text search can filter without
 *  reaching through the join. */
const FTS_COLUMNS = new Set(["source", "role", "kind", "native_ts", "native_session_id"]);

/**
 * Search across ingested transcript messages. With a `query` this is FTS5 over
 * message text; with none it is a structured scan over the facts Phase 1
 * promoted to columns — which is what makes "every Edit under src/shim" a query
 * rather than a grep over JSON blobs. Either way the same filters apply and the
 * same hit shape comes back.
 *
 * Task attribution and every non-content field come from a 1:1 join to
 * `messages` (keyed on the unique message_id), so an fts hit is never
 * multiplied. May throw on malformed FTS5 query syntax — callers map that to a
 * client error.
 */
export function searchMessages(
  index: StateIndex,
  query: string | null,
  limit: number = DEFAULT_SEARCH_LIMIT,
  filters: SearchFilters = {},
): TranscriptHit[] {
  const fts = query !== null && query !== "";
  const col = (name: string): string => (fts && FTS_COLUMNS.has(name) ? `f.${name}` : `m.${name}`);
  const clauses: string[] = [];
  const params: unknown[] = [];
  if (fts) {
    clauses.push("messages_fts MATCH ?");
    params.push(query);
  }

  const sessions =
    filters.sessions ??
    (filters.lastSessions !== undefined
      ? listSessions(index, {
          ...(filters.project !== undefined ? { project: filters.project } : {}),
          limit: filters.lastSessions,
        }).map((s) => s.native_session_id)
      : undefined);
  if (sessions !== undefined) {
    if (sessions.length === 0) return []; // scoped to no session ⇒ no hits
    clauses.push(`${col("native_session_id")} IN (${sessions.map(() => "?").join(", ")})`);
    params.push(...sessions);
  }
  if (filters.project !== undefined) {
    clauses.push("m.project_path = ?");
    params.push(filters.project);
  }
  if (filters.role !== undefined) {
    clauses.push(`${col("role")} = ?`);
    params.push(filters.role);
  }
  if (filters.kind !== undefined) {
    clauses.push(`${col("kind")} = ?`);
    params.push(filters.kind);
  }
  if (filters.since !== undefined) {
    clauses.push(`${col("native_ts")} >= ?`);
    params.push(filters.since);
  }
  if (filters.until !== undefined) {
    clauses.push(`${col("native_ts")} <= ?`);
    params.push(filters.until);
  }
  if (filters.tool !== undefined) {
    clauses.push("m.tool_name = ?");
    params.push(filters.tool);
  }
  if (filters.target !== undefined) {
    clauses.push("m.tool_target LIKE ?");
    params.push(`%${filters.target}%`);
  }
  if (filters.failed !== undefined) {
    clauses.push(`${ERROR_STATE} = ?`);
    params.push(filters.failed ? 1 : 0);
    // Both halves of a pair carry the state, so a filter-only search would
    // report one failure twice. The call is the useful half — it names the tool
    // and the target. A text search is left alone: there the caller asked for
    // whichever record their words appear in.
    if (!fts && filters.kind === undefined) clauses.push("m.kind = 'tool_use'");
  }

  const from = fts
    ? "messages_fts f JOIN messages m ON m.id = f.message_id"
    : "messages m";
  const snippet = fts ? "snippet(messages_fts, 0, '[', ']', '…', 12)" : "NULL";
  const order = fts
    ? filters.sort === "recent"
      ? "f.native_ts DESC, rank"
      : "rank"
    : "m.native_ts DESC, m.id DESC";
  params.push(limit);

  return index.db
    .prepare(
      `SELECT
         (SELECT ws.task_id FROM worker_sessions ws
           WHERE ws.native_session_id = ${col("native_session_id")}
           ORDER BY ws.recorded_at, ws.id LIMIT 1) AS task_id,
         ${col("source")}            AS source,
         ${col("role")}              AS role,
         ${col("kind")}              AS kind,
         ${col("native_ts")}         AS native_ts,
         ${col("native_session_id")} AS native_session_id,
         m.project_path              AS project_path,
         m.prompt_idx                AS prompt_idx,
         m.tool_name                 AS tool_name,
         m.tool_target               AS tool_target,
         ${ERROR_STATE}              AS is_error,
         ${snippet}                  AS snippet
       FROM ${from}
       WHERE ${clauses.join(" AND ")}
       ORDER BY ${order}
       LIMIT ?`,
    )
    .all(...(params as never[])) as unknown as TranscriptHit[];
}

// The scannable index of a session: one entry per prompt, reply and tool call,
// with no message bodies read beyond a leading slice of prose. That is what
// keeps an outline cheap enough to be the default view — a session that costs
// ~28k tokens to read in full outlines in well under a tenth of that.
export interface OutlineEntry {
  prompt_idx: number;
  role: string;
  kind: string;
  native_ts: string | null;
  /** Leading slice of a prose message; null on a tool call, whose body is
   * deliberately never loaded. */
  head: string | null;
  tool_name: string | null;
  tool_target: string | null;
  /** Failure state of the call, resolved from the result it is paired with. */
  is_error: number | null;
}

export interface SessionOutline {
  /** Every archived message, including the tool results the outline omits. */
  message_count: number;
  prompt_count: number;
  tool_count: number;
  entries: OutlineEntry[];
}

/** Prose kept per outline entry. Rendering truncates well below this; the slice
 *  exists so a 40k-character preamble is never pulled out of the database. */
const OUTLINE_HEAD = 400;

/**
 * Tool results are excluded: their failure state is already folded onto the call
 * that produced them, and a result carries nothing else an index needs.
 */
function outlineFor(
  index: StateIndex,
  scope: { sql: string; params: unknown[] },
  opts: { promptIdx?: number } = {},
): SessionOutline {
  const filter = opts.promptIdx === undefined ? "" : "AND m.prompt_idx = ?";
  const params = [...scope.params, ...(opts.promptIdx === undefined ? [] : [opts.promptIdx])];
  const totals = index.db
    .prepare(
      `SELECT COUNT(*) AS message_count,
              COALESCE(MAX(m.prompt_idx), 0) AS prompt_count,
              COALESCE(SUM(m.kind = 'tool_use'), 0) AS tool_count
         FROM messages m
        WHERE ${scope.sql} ${filter}`,
    )
    .get(...(params as never[])) as unknown as Omit<SessionOutline, "entries">;
  const entries = index.db
    .prepare(
      `SELECT m.prompt_idx, m.role, m.kind, m.native_ts, m.tool_name, m.tool_target,
              CASE WHEN m.kind = 'tool_use' THEN NULL
                   ELSE substr(m.content, 1, ${OUTLINE_HEAD}) END AS head,
              ${ERROR_STATE} AS is_error
         FROM messages m
        WHERE ${scope.sql} ${filter}
          AND m.kind != 'tool_result'
        ORDER BY m.native_ts, m.recorded_at, m.id`,
    )
    .all(...(params as never[])) as unknown as OutlineEntry[];
  return { ...totals, entries };
}

export function getSessionOutline(
  index: StateIndex,
  source: string,
  nativeSessionId: string,
  opts: { promptIdx?: number } = {},
): SessionOutline {
  return outlineFor(
    index,
    { sql: "m.source = ? AND m.native_session_id = ?", params: [source, nativeSessionId] },
    opts,
  );
}

export function getTaskOutline(
  index: StateIndex,
  taskId: string,
  opts: { promptIdx?: number } = {},
): SessionOutline {
  return outlineFor(
    index,
    {
      sql: `m.native_session_id IN (
              SELECT DISTINCT native_session_id FROM worker_sessions WHERE task_id = ?
            )`,
      params: [taskId],
    },
    opts,
  );
}
