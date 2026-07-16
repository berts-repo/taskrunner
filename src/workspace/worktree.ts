import * as fs from "node:fs";
import { join } from "node:path";
import type { WorkspaceChanges, WorkspaceProvider } from "../daemon/scheduler.js";
import { ToolError } from "../domain/errors.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import { captureDiffArtifact, collectGitChanges, git } from "./git.js";

/**
 * Host-run task workspaces (PLAN § Workspace And Isolation): one git worktree
 * per task at <state root>/workspaces/<task_id> on branch taskrunner/<task_id>,
 * reused across turns, persisting after completion. Docker workers use
 * task-local clones instead (see clone.ts).
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
    return collectGitChanges(workspaceDir);
  }

  /** Captures the turn's cumulative diff as a linked artifact. */
  afterTurn(taskId: string, turnId: string, workspaceDir: string): void {
    captureDiffArtifact(workspaceDir, taskId, turnId, this.artifacts, this.record);
  }
}
