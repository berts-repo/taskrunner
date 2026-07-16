import { spawnSync } from "node:child_process";
import { newId } from "../ids.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";

export function git(
  cwd: string,
  ...args: string[]
): { ok: boolean; stdout: string; stderr: string } {
  const result = spawnSync("git", ["-C", cwd, ...args], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return {
    ok: result.status === 0,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

/** Fallback changed-file detection shared by all workspace providers. */
export function collectGitChanges(workspaceDir: string): { changedFiles: string[] } {
  const status = git(workspaceDir, "status", "--porcelain");
  const changedFiles: string[] = [];
  for (const line of status.stdout.split("\n")) {
    if (line.length < 4) continue;
    const path = line.slice(3);
    const renamed = path.split(" -> ")[1];
    changedFiles.push(renamed ?? path);
  }
  return { changedFiles };
}

/** Captures the turn's cumulative uncommitted diff as a linked artifact. */
export function captureDiffArtifact(
  workspaceDir: string,
  taskId: string,
  turnId: string,
  artifacts: ArtifactStore,
  record: (body: EventBody) => LogEvent,
): void {
  const diff = git(workspaceDir, "diff", "HEAD");
  if (!diff.ok || diff.stdout.trim() === "") return;
  const stored = artifacts.store(diff.stdout);
  const artifactId = newId("art");
  record({
    type: "artifact.stored",
    artifact_id: artifactId,
    kind: "diff",
    label: `workspace diff after turn`,
    media_type: "text/x-diff",
    size_bytes: stored.size_bytes,
    sha256: stored.sha256,
    locator: stored.locator,
  });
  record({
    type: "artifact.linked",
    artifact_id: artifactId,
    task_id: taskId,
    turn_id: turnId,
  });
}
