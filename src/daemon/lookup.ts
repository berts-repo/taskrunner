import { ToolError } from "../domain/errors.js";
import {
  findProjectByPath,
  getSessionMessages,
  getSessionOutline,
  getTaskMessages,
  getTaskOutline,
  getTaskSnapshot,
  getTurnArtifacts,
  getTurnAudit,
  listSessions,
  listTaskSnapshots,
  listTurns,
  searchMessages,
  type SearchFilters,
  type SessionInfo,
  type SessionOutline,
  type TaskSnapshot,
  type TranscriptHit,
  type TranscriptMessage,
  type TurnInfo,
} from "../domain/tasks.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { StateIndex } from "../storage/index.js";
import {
  compactPayload,
  DEFAULT_TOOL_LINES,
  renderMessages,
  renderOutline,
  truncate,
  type MessageView,
  type TranscriptView,
} from "./transcript-view.js";

// lookup-task semantics: compact summaries by default; expansion blocks only for requested
// include fields; history as paired exchanges, never loose audit rows; trace
// replays inputs, observable worker activity, and outputs per in-scope turn;
// transcript surfaces the archived interior of the worker's own session(s).

type IncludeField = "turns" | "artifacts" | "audit" | "diff" | "trace" | "transcript";

interface LookupArgs {
  taskId?: string;
  project?: string;
  include?: IncludeField[];
  scope?: { turnId?: string; last?: number };
  limit?: number;
  /** Rendering of the `transcript` include; see {@link resolveView}. */
  view?: TranscriptView;
  toolLines?: number;
  promptIdx?: number;
}

/**
 * Which view an unset `view` means. The outline is the default because reading a
 * whole session should be a decision, not an accident — but the two ways a
 * caller can already narrow a read say plainly what they want, and outlining
 * them instead would be a regression:
 *
 *   prompt N   drilling into one exchange means reading it → timeline
 *   last N     a bounded read of the newest messages → compact, as before
 */
function resolveView(args: {
  view?: TranscriptView;
  promptIdx?: number;
  last?: number;
}): TranscriptView {
  if (args.view !== undefined) return args.view;
  if (args.promptIdx !== undefined) return "timeline";
  return args.last !== undefined ? "compact" : "outline";
}

/**
 * How many messages a transcript read returns. The timeline is an audit view, so
 * it is uncapped unless the caller narrows it; compact keeps its 500 default.
 * An explicit `last` always wins.
 */
function messageQuery(
  view: MessageView,
  last: number | undefined,
  promptIdx: number | undefined,
): { limit?: number | null; promptIdx?: number } {
  return {
    limit: last ?? (view === "timeline" ? null : undefined),
    ...(promptIdx !== undefined ? { promptIdx } : {}),
  };
}

/** Shared tail of the two outline renderings: the index itself, or a plain
 *  statement that there is nothing to index. */
function outlineSection(outline: SessionOutline, promptIdx: number | undefined): string[] {
  if (outline.message_count === 0) {
    return [
      promptIdx === undefined
        ? "  (no transcript recorded)"
        : `  (no messages at prompt ${promptIdx})`,
    ];
  }
  return renderOutline(outline);
}

interface LookupDeps {
  index: StateIndex;
  artifacts: ArtifactStore;
}

const DIFF_INLINE_LIMIT = 50_000;

export function lookupTask(deps: LookupDeps, args: LookupArgs): string {
  if (args.taskId) return lookupSingleTask(deps, args.taskId, args);
  if (args.project) return lookupProjectTasks(deps, args.project, args.limit ?? 10);
  throw new ToolError("invalid_request", "provide taskId or project");
}

function lookupSingleTask(deps: LookupDeps, taskId: string, args: LookupArgs): string {
  const { index } = deps;
  const snapshot = getTaskSnapshot(index, taskId);
  if (!snapshot) throw new ToolError("not_found", `no task ${taskId}`);
  const include = new Set<IncludeField>(args.include ?? []);

  const turns = applyScope(listTurns(index, taskId), args.scope, taskId);
  const sections: string[] = [renderSummary(snapshot)];
  if (include.has("turns")) sections.push(renderExchanges(turns));
  if (include.has("trace")) sections.push(...turns.map((turn) => renderTrace(deps, turn)));
  if (include.has("audit")) sections.push(...turns.map((turn) => renderAudit(index, turn)));
  if (include.has("artifacts")) sections.push(renderArtifacts(index, turns));
  if (include.has("diff")) sections.push(renderDiffs(deps, turns));
  if (include.has("transcript")) sections.push(renderTranscript(index, taskId, args));
  return sections.join("\n\n");
}

function lookupProjectTasks(deps: LookupDeps, projectPath: string, limit: number): string {
  const project = findProjectByPath(deps.index, projectPath);
  if (!project) {
    throw new ToolError("not_found", `no tasks recorded for project ${projectPath}`);
  }
  const snapshots = listTaskSnapshots(deps.index, project.project_id, limit);
  const lines = [`project: ${project.root}`, `tasks: ${snapshots.length}`];
  for (const s of snapshots) {
    lines.push(
      `  ${s.task_id}  ${s.status.padEnd(9)}  ${s.worker.padEnd(6)}  ` +
        `turns=${s.turn_count}  ${s.updated_at}  ${s.prompt_summary}`,
    );
  }
  return lines.join("\n");
}

function applyScope(
  turns: TurnInfo[],
  scope: LookupArgs["scope"],
  taskId: string,
): TurnInfo[] {
  if (!scope) return turns;
  if (scope.turnId) {
    const hit = turns.filter((t) => t.turn_id === scope.turnId);
    if (hit.length === 0) {
      throw new ToolError("not_found", `no turn ${scope.turnId} in task ${taskId}`);
    }
    return hit;
  }
  if (scope.last !== undefined) return turns.slice(-scope.last);
  return turns;
}

function renderSummary(s: TaskSnapshot): string {
  return [
    `task: ${s.task_id}`,
    `project: ${s.project_root}`,
    `worker: ${s.worker}${s.worker_session_id ? ` (native session ${s.worker_session_id})` : ""}`,
    `status: ${s.status}`,
    `about: ${s.prompt_summary}`,
    `turns: ${s.turn_count}`,
    `updated: ${s.updated_at}`,
  ].join("\n");
}

/** Ordered prompt/response exchange pairs, never loose audit rows. */
function renderExchanges(turns: TurnInfo[]): string {
  const lines = ["exchanges:"];
  for (const turn of turns) {
    lines.push("", `--- turn ${turn.idx + 1} (${turn.turn_id}, ${turn.status})`);
    lines.push(`>> ${turn.prompt}`);
    if (turn.response !== null) lines.push(`<< ${turn.response}`);
    else if (turn.status === "running") lines.push("<< (turn still running)");
    if (turn.status === "failed") {
      lines.push(`error ${turn.error_code}: ${turn.error_message ?? ""}`);
    }
    if (turn.status === "canceled") {
      lines.push(`canceled${turn.error_message ? `: ${turn.error_message}` : ""}`);
    }
  }
  return lines.join("\n");
}

/** End-to-end replay of one turn: inputs, worker activity, outputs. */
function renderTrace(deps: LookupDeps, turn: TurnInfo): string {
  const lines = [`trace: turn ${turn.idx + 1} (${turn.turn_id}, ${turn.status})`];
  lines.push("inputs:", `  prompt: ${turn.prompt}`, `  started: ${turn.started_at}`);
  lines.push("activity:");
  const audit = getTurnAudit(deps.index, turn.turn_id);
  if (audit.length === 0) lines.push("  (none captured)");
  for (const row of audit) {
    lines.push(`  ${row.ts}  ${row.kind}  ${compactPayload(row.payload)}`);
  }
  lines.push("outputs:", `  status: ${turn.status}`);
  if (turn.response !== null) lines.push(`  response: ${turn.response}`);
  if (turn.error_code) lines.push(`  error ${turn.error_code}: ${turn.error_message ?? ""}`);
  if (turn.changed_files.length > 0) {
    lines.push(`  changed files: ${turn.changed_files.join(", ")}`);
  }
  const artifacts = getTurnArtifacts(deps.index, turn.turn_id);
  for (const a of artifacts) {
    lines.push(`  artifact: ${a.artifact_id}  ${a.kind}  (${a.media_type}, ${a.size_bytes} bytes)`);
  }
  if (turn.completed_at) lines.push(`  completed: ${turn.completed_at}`);
  return lines.join("\n");
}

function renderAudit(index: StateIndex, turn: TurnInfo): string {
  const rows = getTurnAudit(index, turn.turn_id);
  const lines = [`audit: turn ${turn.idx + 1} (${turn.turn_id}, ${rows.length} events)`];
  for (const row of rows) {
    lines.push(`  ${row.ts}  ${row.kind}  ${compactPayload(row.payload)}`);
  }
  return lines.join("\n");
}

function renderArtifacts(index: StateIndex, turns: TurnInfo[]): string {
  const lines = ["artifacts:"];
  let any = false;
  for (const turn of turns) {
    for (const a of getTurnArtifacts(index, turn.turn_id)) {
      any = true;
      lines.push(
        `  ${a.artifact_id}  ${a.kind}  ${a.label}  ` +
          `(${a.media_type}, ${a.size_bytes} bytes, sha256 ${a.sha256.slice(0, 12)}…, ` +
          `turn ${turn.idx + 1})`,
      );
    }
  }
  if (!any) lines.push("  (none)");
  return lines.join("\n");
}

function renderDiffs(deps: LookupDeps, turns: TurnInfo[]): string {
  const lines = ["diffs:"];
  let any = false;
  for (const turn of turns) {
    for (const a of getTurnArtifacts(deps.index, turn.turn_id)) {
      if (a.kind !== "diff") continue;
      any = true;
      lines.push(`--- turn ${turn.idx + 1} (${a.artifact_id})`);
      let text = deps.artifacts.read(a.locator).toString("utf8");
      if (text.length > DIFF_INLINE_LIMIT) {
        text = `${text.slice(0, DIFF_INLINE_LIMIT)}\n… truncated; full diff in artifact ${a.artifact_id}`;
      }
      lines.push(text.trimEnd());
    }
  }
  if (!any) lines.push("  (no diff artifacts in scope)");
  return lines.join("\n");
}

/** The worker's archived transcript for a task: every swept message from the
 *  session(s) it ran under, in the requested view. Task-level — `scope.turnId`
 *  does not sub-select it; `scope.last` caps the number of messages shown, and
 *  `promptIdx` narrows to one exchange. */
function renderTranscript(index: StateIndex, taskId: string, args: LookupArgs): string {
  const view = resolveView({
    ...(args.view !== undefined ? { view: args.view } : {}),
    ...(args.promptIdx !== undefined ? { promptIdx: args.promptIdx } : {}),
    ...(args.scope?.last !== undefined ? { last: args.scope.last } : {}),
  });
  if (view === "outline") {
    const outline = getTaskOutline(
      index,
      taskId,
      args.promptIdx === undefined ? {} : { promptIdx: args.promptIdx },
    );
    return ["transcript:", ...outlineSection(outline, args.promptIdx)].join("\n");
  }
  const { messages, capped } = getTaskMessages(
    index,
    taskId,
    messageQuery(view, args.scope?.last, args.promptIdx),
  );
  const lines = ["transcript:"];
  if (messages.length === 0) {
    lines.push(
      args.promptIdx === undefined
        ? "  (no transcript recorded)"
        : `  (no messages at prompt ${args.promptIdx})`,
    );
    return lines.join("\n");
  }
  lines.push(...renderMessages(messages, view, args.toolLines ?? DEFAULT_TOOL_LINES));
  if (capped) lines.push(`  … capped at ${messages.length} messages (raise scope.last for more)`);
  return lines.join("\n");
}

/**
 * Corpus-wide transcript search rendered for the search-transcripts tool. The
 * query is optional — the structured filters stand on their own — but a search
 * with neither is a session listing, which lookup-session already does better.
 * Every hit prints its session and prompt index, so a result is an address to
 * drill into and not just a sighting.
 */
export function searchTranscripts(
  index: StateIndex,
  query: string | null,
  limit: number,
  filters: SearchFilters = {},
): string {
  const structured =
    filters.tool !== undefined || filters.target !== undefined || filters.failed !== undefined;
  if ((query === null || query === "") && !structured) {
    throw new ToolError("invalid_request", "provide a query, or a tool / target / failed filter");
  }
  let hits;
  try {
    hits = searchMessages(index, query, limit, filters);
  } catch (err) {
    throw new ToolError(
      "invalid_request",
      `invalid search query: ${err instanceof Error ? err.message : String(err)}`,
    );
  }
  const what = describeSearch(query, filters);
  if (hits.length === 0) return `no transcript matches for: ${what}`;
  const lines = [`transcript matches (${hits.length}) for ${what}:`];
  for (const h of hits) {
    const where = h.task_id ? `task ${h.task_id}` : `${h.source} session ${h.native_session_id}`;
    const proj = h.project_path ? ` · ${h.project_path}` : "";
    const ts = h.native_ts ? ` · ${h.native_ts}` : "";
    lines.push(`  ${where}${proj} · ${h.role}/${h.kind} · prompt ${h.prompt_idx}${ts}`);
    lines.push(`    ${hitBody(h)}`);
  }
  return lines.join("\n");
}

/** The snippet where a text search highlighted one; otherwise the call itself,
 *  which is the whole content of a structured hit. */
function hitBody(h: TranscriptHit): string {
  if (h.snippet !== null) return truncate(h.snippet, 200);
  // Truncated per part: collapsing the pair together would eat the gap that
  // separates the tool from what it acted on.
  const call = [h.tool_name, h.tool_target]
    .filter((v): v is string => v !== null)
    .map((v) => truncate(v, 200))
    .join("  ");
  return `${call === "" ? h.kind : call}${h.is_error === 1 ? "  ✗" : ""}`;
}

/** Echoes back what was actually searched for, so a filter-only search does not
 *  report "no matches for: null". */
function describeSearch(query: string | null, filters: SearchFilters): string {
  const parts: string[] = [];
  if (query !== null && query !== "") parts.push(query);
  if (filters.tool !== undefined) parts.push(`tool ${filters.tool}`);
  if (filters.target !== undefined) parts.push(`target ~ ${filters.target}`);
  if (filters.failed !== undefined) parts.push(filters.failed ? "failed" : "succeeded");
  return parts.join(", ");
}

interface SessionLookupArgs {
  sessionId?: string;
  project?: string;
  source?: string;
  limit?: number;
  scope?: { last?: number };
  /** Rendering of a single session's history; see {@link resolveView}. */
  view?: TranscriptView;
  toolLines?: number;
  promptIdx?: number;
}

/**
 * lookup-session: no sessionId lists ingested transcript sessions most-recent
 * first (optionally filtered to a project); a sessionId returns that session's
 * full history in order. Works for host sessions that no task links, unlike
 * lookup-task's transcript include. A bare id matching several sources is not
 * guessed — the candidates are listed for the caller to disambiguate.
 */
export function lookupSession(index: StateIndex, args: SessionLookupArgs): string {
  if (!args.sessionId) {
    const sessions = listSessions(index, {
      ...(args.project !== undefined ? { project: args.project } : {}),
      ...(args.limit !== undefined ? { limit: args.limit } : {}),
    });
    return renderSessionList(sessions);
  }

  const matches = listSessions(index, { nativeSessionId: args.sessionId, limit: 50 });
  const candidates = args.source ? matches.filter((m) => m.source === args.source) : matches;
  if (candidates.length === 0) {
    const where = args.source ? `${args.source} session` : "session";
    throw new ToolError("not_found", `no ingested ${where} ${args.sessionId}`);
  }
  if (candidates.length > 1) {
    const lines = [
      `session id ${args.sessionId} matches ${candidates.length} sources; re-run with source=<one of>:`,
      ...candidates.map((m) => `  source=${m.source}  (${m.message_count} messages)`),
    ];
    return lines.join("\n");
  }
  const info = candidates[0]!;
  const view = resolveView({
    ...(args.view !== undefined ? { view: args.view } : {}),
    ...(args.promptIdx !== undefined ? { promptIdx: args.promptIdx } : {}),
    ...(args.scope?.last !== undefined ? { last: args.scope.last } : {}),
  });
  if (view === "outline") {
    const outline = getSessionOutline(
      index,
      info.source,
      info.native_session_id,
      args.promptIdx === undefined ? {} : { promptIdx: args.promptIdx },
    );
    return [sessionHeader(info, args.promptIdx), ...outlineSection(outline, args.promptIdx)].join(
      "\n",
    );
  }
  const { messages, capped } = getSessionMessages(
    index,
    info.source,
    info.native_session_id,
    messageQuery(view, args.scope?.last, args.promptIdx),
  );
  return renderSessionHistory(info, messages, capped, args, view);
}

function renderSessionList(sessions: SessionInfo[]): string {
  if (sessions.length === 0) return "sessions: (none ingested)";
  const lines = [`sessions (${sessions.length}):`];
  for (const s of sessions) {
    const when = s.last_ts ?? s.last_recorded_at;
    const proj = s.project_path ?? "(no project)";
    const task = s.task_id ? `  [task ${s.task_id}]` : "";
    lines.push(
      `  ${s.native_session_id}  ${s.source.padEnd(11)}  ${when}  ` +
        `msgs=${s.message_count}  ${proj}${task}`,
    );
  }
  return lines.join("\n");
}

function sessionHeader(info: SessionInfo, promptIdx: number | undefined): string {
  const proj = info.project_path ? `, ${info.project_path}` : "";
  const at = promptIdx === undefined ? "" : ` · prompt ${promptIdx}`;
  return `session ${info.native_session_id} (${info.source}${proj})${at}:`;
}

function renderSessionHistory(
  info: SessionInfo,
  messages: TranscriptMessage[],
  capped: boolean,
  args: SessionLookupArgs,
  view: MessageView,
): string {
  const lines = [sessionHeader(info, args.promptIdx)];
  if (messages.length === 0) {
    lines.push(
      args.promptIdx === undefined
        ? "  (no messages recorded)"
        : `  (no messages at prompt ${args.promptIdx})`,
    );
    return lines.join("\n");
  }
  lines.push(...renderMessages(messages, view, args.toolLines ?? DEFAULT_TOOL_LINES));
  if (capped) lines.push(`  … capped at ${messages.length} messages (raise scope.last for more)`);
  return lines.join("\n");
}

