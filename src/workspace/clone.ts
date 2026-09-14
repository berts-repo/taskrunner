import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import { join } from "node:path";
import type { WorkspaceChanges, WorkspaceProvider } from "../daemon/scheduler.js";
import { ToolError } from "../domain/errors.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import { captureDiffArtifact, collectGitChanges, git } from "./git.js";

/**
 * Task-local clone workspaces for Docker workers: a worktree's `.git` file
 * points back into the main repository,
 * so mounting one into a container either breaks git or exposes the host
 * `.git`. A clone is fully self-contained; the container mounts only the
 * clone directory.
 *
 * `--no-hardlinks` matters: a default local clone hardlinks object files,
 * which share inodes with the host repository — a worker writing through one
 * could corrupt host history. Committed work lands host-side via fetch in
 * afterTurn; the worker never touches the real repository.
 */
export class CloneWorkspaces implements WorkspaceProvider {
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
    const cloned = git(".", "clone", "--no-hardlinks", projectRoot, dir);
    if (!cloned.ok) {
      throw new ToolError(
        "internal_error",
        `failed to create task workspace clone: ${cloned.stderr.trim()}`,
      );
    }
    const branch = git(dir, "checkout", "-b", `taskrunner/${taskId}`);
    if (!branch.ok) {
      throw new ToolError(
        "internal_error",
        `failed to create task branch in clone: ${branch.stderr.trim()}`,
      );
    }
    // The origin remote points at the host repository by absolute path; that
    // path is meaningless (and must stay unreachable) inside the container.
    git(dir, "remote", "remove", "origin");
    // The container's non-root worker user has a different uid than the host
    // user who owns this clone (a bind mount keeps host uids, and the base
    // image's own "node" user often already claims uid 1000), so every file
    // and dir here looks owned-by-someone-else to the worker: it can create
    // nothing and edit nothing. World-writable is fine: this is a disposable,
    // single-task clone with no secrets, discarded once the turn lands its
    // changes host-side. `o+rwX` (not `o+rwx`) only adds execute where a file
    // already had one, so plain files don't spuriously become executable.
    const chmodded = spawnSync("chmod", ["-R", "o+rwX", dir], { encoding: "utf8" });
    if (chmodded.status !== 0) {
      throw new ToolError(
        "internal_error",
        `failed to make task workspace writable by the container user: ${chmodded.stderr?.trim()}`,
      );
    }
    return dir;
  }

  collectChanges(workspaceDir: string): WorkspaceChanges {
    return collectGitChanges(workspaceDir);
  }

  /**
   * Diff artifact for uncommitted work, then the host-side landing step:
   * commits the worker made on taskrunner/<task_id> are fetched into the
   * host repository under the same branch name for review.
   */
  afterTurn(taskId: string, turnId: string, workspaceDir: string, projectRoot: string): void {
    captureDiffArtifact(workspaceDir, taskId, turnId, this.artifacts, this.record);

    const tip = git(workspaceDir, "rev-parse", "HEAD").stdout.trim();
    if (!tip) return;
    // Skip when the host already has the tip (no new commits in the clone).
    if (git(projectRoot, "cat-file", "-e", tip).ok) return;
    const branch = `taskrunner/${taskId}`;
    git(projectRoot, "fetch", workspaceDir, `+refs/heads/${branch}:refs/heads/${branch}`);
  }
}
