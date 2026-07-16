import type { TurnOutcome } from "./daemon/scheduler.js";
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
  if (outcome.tier) lines.push(`tier: ${outcome.tier}`);
  if (outcome.approval_state !== "none") lines.push(`approval: ${outcome.approval_state}`);
  if (outcome.summary) lines.push("", outcome.summary);
  if (outcome.changed_files.length > 0) {
    lines.push("", "changed files:", ...outcome.changed_files.map((f) => `  ${f}`));
  }
  if (outcome.artifacts.length > 0) {
    lines.push("", "artifacts:", ...outcome.artifacts.map(artifactLine));
  }
  if (outcome.error) lines.push("", `error ${outcome.error.code}: ${outcome.error.message}`);
  if (outcome.approval_state === "pending") {
    lines.push(
      "",
      "This task waits for a human decision. Tell the user to run:",
      `  taskrunner approve ${outcome.task_id}`,
      `or: taskrunner deny ${outcome.task_id}`,
    );
  } else if (outcome.status === "running") {
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

