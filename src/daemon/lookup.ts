import { ToolError } from "../domain/errors.js";
import {
  findProjectByPath,
  getTaskSnapshot,
  getTurnArtifacts,
  getTurnAudit,
  listTaskSnapshots,
  listTurns,
  type TaskSnapshot,
  type TurnInfo,
} from "../domain/tasks.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { StateIndex } from "../storage/index.js";

// lookup-task semantics (PLAN § MCP tool schemas, history presentation
// rules): compact summaries by default; expansion blocks only for requested
// include fields; history as paired exchanges, never loose audit rows; trace
// replays inputs, observable worker activity, and outputs per in-scope turn.

export type IncludeField = "turns" | "artifacts" | "audit" | "diff" | "trace";

export interface LookupArgs {
  taskId?: string;
  project?: string;
  include?: IncludeField[];
  scope?: { turnId?: string; last?: number };
  limit?: number;
}

export interface LookupDeps {
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

function compactPayload(payload: unknown): string {
  if (payload && typeof payload === "object") {
    const obj = payload as Record<string, unknown>;
    const item = (obj["item"] as Record<string, unknown> | undefined) ?? obj;
    for (const key of ["text", "message", "command", "path"]) {
      if (typeof item[key] === "string") return truncate(item[key] as string, 160);
    }
  }
  if (typeof payload === "string") return truncate(payload, 160);
  return truncate(JSON.stringify(payload) ?? "null", 160);
}

function truncate(text: string, max: number): string {
  const line = text.replaceAll(/\s+/g, " ").trim();
  return line.length <= max ? line : `${line.slice(0, max - 1)}…`;
}
