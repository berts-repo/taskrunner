import { execFileSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { CloneWorkspaces } from "../../src/workspace/clone.js";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import type { EventBody, LogEvent } from "../../src/storage/events.js";
import { tempDir } from "../helpers.js";

function initRepo(): string {
  const repo = tempDir("repo");
  const git = (...args: string[]) =>
    execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });
  git("init", "-q");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "T");
  writeFileSync(join(repo, "README.md"), "hi\n");
  git("add", ".");
  git("commit", "-qm", "init");
  return repo;
}

function gitIn(dir: string, ...args: string[]): string {
  return execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" });
}

function makeProvider() {
  const root = tempDir("clone-ws");
  const recorded: EventBody[] = [];
  const record = (body: EventBody): LogEvent => {
    recorded.push(body);
    return { ...body, id: "evt_x", ts: new Date().toISOString() } as LogEvent;
  };
  const provider = new CloneWorkspaces(
    join(root, "workspaces"),
    new ArtifactStore(join(root, "artifacts")),
    record,
  );
  return { provider, recorded };
}

describe("CloneWorkspaces", () => {
  it("creates a self-contained clone on a task branch with no remotes", async () => {
    const repo = initRepo();
    const { provider } = makeProvider();
    const dir = await provider.ensureWorkspace("task_c1", repo);

    // A real .git directory (self-contained), not a worktree pointer file.
    expect(existsSync(join(dir, ".git", "HEAD"))).toBe(true);
    expect(gitIn(dir, "branch", "--show-current").trim()).toBe("taskrunner/task_c1");
    expect(gitIn(dir, "remote").trim()).toBe("");
    // Idempotent: a second call reuses the same workspace.
    expect(await provider.ensureWorkspace("task_c1", repo)).toBe(dir);
  });

  it("rejects projects that are not git repositories", async () => {
    const { provider } = makeProvider();
    await expect(provider.ensureWorkspace("task_c2", tempDir("plain"))).rejects.toMatchObject({
      code: "invalid_request",
    });
  });

  it("collects uncommitted changes and captures a diff artifact", async () => {
    const repo = initRepo();
    const { provider, recorded } = makeProvider();
    const dir = await provider.ensureWorkspace("task_c3", repo);

    writeFileSync(join(dir, "README.md"), "changed\n");
    expect(provider.collectChanges(dir).changedFiles).toEqual(["README.md"]);

    provider.afterTurn("task_c3", "turn_c3", dir, repo);
    const kinds = recorded.map((e) => e.type);
    expect(kinds).toContain("artifact.stored");
    expect(kinds).toContain("artifact.linked");
    const stored = recorded.find((e) => e.type === "artifact.stored") as any;
    expect(stored.kind).toBe("diff");
  });

  it("lands committed work on the host repo under the task branch", async () => {
    const repo = initRepo();
    const { provider } = makeProvider();
    const dir = await provider.ensureWorkspace("task_c4", repo);

    writeFileSync(join(dir, "new.txt"), "worker output\n");
    gitIn(dir, "add", ".");
    gitIn(dir, "commit", "-qm", "worker commit");
    const tip = gitIn(dir, "rev-parse", "HEAD").trim();

    provider.afterTurn("task_c4", "turn_c4", dir, repo);
    expect(gitIn(repo, "rev-parse", "taskrunner/task_c4").trim()).toBe(tip);
    // The host branch is a real local branch; the working tree is untouched.
    expect(existsSync(join(repo, "new.txt"))).toBe(false);

    // Re-running afterTurn without new commits is a no-op.
    provider.afterTurn("task_c4", "turn_c4b", dir, repo);
    expect(gitIn(repo, "rev-parse", "taskrunner/task_c4").trim()).toBe(tip);
  });
});
