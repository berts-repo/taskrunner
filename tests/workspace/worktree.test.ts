import { execFileSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import { StateIndex } from "../../src/storage/index.js";
import { WorktreeWorkspaces } from "../../src/workspace/worktree.js";
import { evt, tempDir } from "../helpers.js";
import type { EventBody } from "../../src/storage/events.js";

function initRepo(): string {
  const repo = tempDir("repo");
  const git = (...args: string[]) =>
    execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });
  git("init", "-q");
  git("config", "user.email", "test@example.com");
  git("config", "user.name", "Test");
  writeFileSync(join(repo, "README.md"), "hi\n");
  git("add", ".");
  git("commit", "-qm", "init");
  return repo;
}

function makeProvider() {
  const index = new StateIndex(":memory:");
  const record = (body: EventBody) => {
    const event = evt(body);
    index.apply(event);
    return event;
  };
  const provider = new WorktreeWorkspaces(
    tempDir("workspaces"),
    new ArtifactStore(tempDir("artifacts")),
    record,
  );
  return { provider, index };
}

describe("WorktreeWorkspaces", () => {
  it("creates a worktree per task on a taskrunner/<task_id> branch and reuses it", async () => {
    const repo = initRepo();
    const { provider } = makeProvider();

    const dir = await provider.ensureWorkspace("task_w1", repo);
    expect(existsSync(join(dir, "README.md"))).toBe(true);
    const branch = execFileSync("git", ["-C", dir, "branch", "--show-current"], {
      encoding: "utf8",
    }).trim();
    expect(branch).toBe("taskrunner/task_w1");

    expect(await provider.ensureWorkspace("task_w1", repo)).toBe(dir);
    // A second task gets its own isolated worktree.
    const dir2 = await provider.ensureWorkspace("task_w2", repo);
    expect(dir2).not.toBe(dir);
  });

  it("detects modified and untracked files as changes", async () => {
    const repo = initRepo();
    const { provider } = makeProvider();
    const dir = await provider.ensureWorkspace("task_w1", repo);
    writeFileSync(join(dir, "README.md"), "changed\n");
    writeFileSync(join(dir, "new.txt"), "brand new\n");

    const { changedFiles } = provider.collectChanges(dir);
    expect(changedFiles.sort()).toEqual(["README.md", "new.txt"]);
  });

  it("captures a diff artifact after a turn with tracked changes", async () => {
    const repo = initRepo();
    const { provider, index } = makeProvider();
    // Satisfy FK integrity: the linked task and turn must exist.
    index.apply(evt({ type: "project.created", project_id: "proj_w", root: repo }));
    index.apply(
      evt({
        type: "task.created",
        task_id: "task_w1",
        project_id: "proj_w",
        worker: "codex",
        prompt_summary: "x",
      }),
    );
    index.apply(evt({ type: "turn.started", turn_id: "turn_w1", task_id: "task_w1", prompt: "x" }));
    const dir = await provider.ensureWorkspace("task_w1", repo);
    writeFileSync(join(dir, "README.md"), "changed\n");

    provider.afterTurn("task_w1", "turn_w1", dir);

    const artifact = index.db
      .prepare(
        `SELECT a.kind, a.media_type, l.turn_id FROM artifacts a
         JOIN artifact_links l ON l.artifact_id = a.id`,
      )
      .get() as any;
    expect(artifact).toEqual({ kind: "diff", media_type: "text/x-diff", turn_id: "turn_w1" });
  });

  it("rejects non-git projects", async () => {
    const { provider } = makeProvider();
    await expect(provider.ensureWorkspace("task_w1", tempDir("plain"))).rejects.toMatchObject({
      code: "invalid_request",
    });
  });
});
