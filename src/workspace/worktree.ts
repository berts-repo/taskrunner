import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import { join } from "node:path";
import type { WorkspaceChanges, WorkspaceProvider } from "../daemon/scheduler.js";
import { ToolError } from "../domain/errors.js";
import { newId } from "../ids.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";

function git(cwd: string, ...args: string[]): { ok: boolean; stdout: string; stderr: string } {
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

/**
 * Host-run task workspaces (PLAN § Workspace And Isolation): one git worktree
 * per task at <state root>/workspaces/<task_id> on branch taskrunner/<task_id>,
 * reused across turns, persisting after completion. Task-local clones for
 * Docker workers are a later phase.
 */
export class WorktreeWorkspaces implements WorkspaceProvider {
  constructor(
    private readonly workspacesDir: string,
    private readonly artifacts: ArtifactStore,
    private readonly record: (body: EventBody) => LogEvent,
  ) {}

  async ensureWorkspace(taskId: string, projectRoot: string): Promise<string> {
    const dir = join(this.workspacesDir, taskId);
    if (fs.existsSync(join(dir, ".git"))) return dir;

    if (!git(projectRoot, "rev-parse", "--git-dir").ok) {
      throw new ToolError(
        "invalid_request",
        `project is not a git repository, so no task workspace can be created: ${projectRoot}`,
      );
    }
    fs.mkdirSync(this.workspacesDir, { recursive: true });
    const branch = `taskrunner/${taskId}`;
    const result = git(projectRoot, "worktree", "add", dir, "-b", branch);
    if (!result.ok) {
      throw new ToolError(
        "internal_error",
        `failed to create task workspace worktree: ${result.stderr.trim()}`,
      );
    }
    return dir;
  }

  /** Fallback changed-file detection when the worker reports none. */
  collectChanges(workspaceDir: string): WorkspaceChanges {
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

  /** Captures the turn's cumulative diff as a linked artifact. */
  afterTurn(taskId: string, turnId: string, workspaceDir: string): void {
    const diff = git(workspaceDir, "diff", "HEAD");
    if (!diff.ok || diff.stdout.trim() === "") return;
    const stored = this.artifacts.store(diff.stdout);
    const artifactId = newId("art");
    this.record({
      type: "artifact.stored",
      artifact_id: artifactId,
      kind: "diff",
      label: `workspace diff after turn`,
      media_type: "text/x-diff",
      size_bytes: stored.size_bytes,
      sha256: stored.sha256,
      locator: stored.locator,
    });
    this.record({
      type: "artifact.linked",
      artifact_id: artifactId,
      task_id: taskId,
      turn_id: turnId,
    });
  }
}
