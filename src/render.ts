import type { TurnOutcome } from "./daemon/scheduler.js";
import type { TaskSnapshot, TurnInfo } from "./domain/tasks.js";
import type { ArtifactHandle } from "./domain/tasks.js";

// Tool responses are compact readable text with handles, not raw JSON blobs
// (PLAN § MCP Tool Surface).

function artifactLine(a: ArtifactHandle): string {
  return `  ${a.artifact_id}  ${a.kind}  ${a.label} (${a.media_type}, ${a.size_bytes} bytes)`;
}

export function renderOutcome(outcome: TurnOutcome): string {
  const lines = [
    `task: ${outcome.task_id}`,
    `turn: ${outcome.turn_id ?? "-"}`,
    `status: ${outcome.status}`,
    `worker: ${outcome.worker}${outcome.worker_session_id ? ` (native session ${outcome.worker_session_id})` : ""}`,
  ];
  if (outcome.summary) lines.push("", outcome.summary);
  if (outcome.changed_files.length > 0) {
    lines.push("", "changed files:", ...outcome.changed_files.map((f) => `  ${f}`));
  }
  if (outcome.artifacts.length > 0) {
    lines.push("", "artifacts:", ...outcome.artifacts.map(artifactLine));
  }
  if (outcome.error) lines.push("", `error ${outcome.error.code}: ${outcome.error.message}`);
  if (outcome.status === "running") {
    lines.push("", "Turn is running. Use lookup-task with this task id to retrieve the result.");
  }
  return lines.join("\n");
}

export function renderCancel(result: {
  task_id: string;
  turn_id: string | null;
  status: string;
}): string {
  const lines = [`task: ${result.task_id}`, `status: ${result.status}`];
  if (result.turn_id) lines.splice(1, 0, `turn: ${result.turn_id}`);
  else lines.push("no turn was running");
  return lines.join("\n");
}

export function renderTaskDetail(snapshot: TaskSnapshot, turns: TurnInfo[]): string {
  const lines = [
    `task: ${snapshot.task_id}`,
    `project: ${snapshot.project_root}`,
    `worker: ${snapshot.worker}${
      snapshot.worker_session_id ? ` (native session ${snapshot.worker_session_id})` : ""
    }`,
    `status: ${snapshot.status}`,
    `about: ${snapshot.prompt_summary}`,
    `turns: ${snapshot.turn_count}`,
  ];
  for (const turn of turns) {
    lines.push("", `--- turn ${turn.idx + 1} (${turn.turn_id}, ${turn.status})`);
    lines.push(`>> ${turn.prompt}`);
    if (turn.response) lines.push(`<< ${turn.response}`);
    if (turn.status === "failed") {
      lines.push(`error ${turn.error_code}: ${turn.error_message ?? ""}`);
    }
    if (turn.status === "canceled" && turn.error_message) {
      lines.push(`canceled: ${turn.error_message}`);
    }
    if (turn.changed_files.length > 0) {
      lines.push("changed files:", ...turn.changed_files.map((f) => `  ${f}`));
    }
  }
  return lines.join("\n");
}
