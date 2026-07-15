import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { parseConfig } from "../../src/config.js";
import { Scheduler } from "../../src/daemon/scheduler.js";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import { EventLog } from "../../src/storage/events.js";
import { StateIndex } from "../../src/storage/index.js";
import { CodexHarness } from "../../src/workers/codex.js";
import { WorktreeWorkspaces } from "../../src/workspace/worktree.js";
import { tempDir } from "../helpers.js";
import { writeFakeCodex } from "./fake-codex.js";

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

function makeStack(codexCommand: string) {
  const root = tempDir("stack");
  const log = EventLog.open(join(root, "events.jsonl"));
  const index = new StateIndex(":memory:");
  const record = (body: Parameters<EventLog["append"]>[0]) => {
    const event = log.append(body);
    index.apply(event);
    return event;
  };
  const artifacts = new ArtifactStore(join(root, "artifacts"));
  const workspacesDir = join(root, "workspaces");
  const scheduler = new Scheduler({
    config: parseConfig({}),
    index,
    record,
    harnesses: new Map([["codex", new CodexHarness(codexCommand)]]),
    workspaces: new WorktreeWorkspaces(workspacesDir, artifacts, record),
    artifacts,
  });
  return { scheduler, index, workspacesDir };
}

describe("scheduler + worktree + codex harness", () => {
  it("delegates, edits in the task workspace, resumes the same thread", async () => {
    const repo = initRepo();
    const { scheduler, index, workspacesDir } = makeStack(writeFakeCodex());

    const first = await scheduler.assignTask({
      project: repo,
      worker: "codex",
      prompt: "create hello",
      wait: true,
    });
    expect(first.status).toBe("completed");
    expect(first.changed_files).toEqual(["hello.txt"]);
    expect(first.worker_session_id).toMatch(/^thread-/);

    // The edit landed in the task workspace, not the project.
    const workspace = join(workspacesDir, first.task_id);
    expect(readFileSync(join(workspace, "hello.txt"), "utf8")).toBe("line one\n");
    expect(existsSync(join(repo, "hello.txt"))).toBe(false);

    // Raw worker events were captured as a linked artifact.
    expect(first.artifacts.map((a) => a.kind)).toContain("worker-events");

    const second = await scheduler.continueTask({
      task_id: first.task_id,
      prompt: "add another line",
      wait: true,
    });
    expect(second.status).toBe("completed");
    expect(second.summary).toContain(`resumed ${first.worker_session_id}`);
    expect(readFileSync(join(workspace, "hello.txt"), "utf8")).toBe("line one\nline two\n");

    // Same native session: no second worker_sessions row.
    const { n } = index.db
      .prepare("SELECT COUNT(*) AS n FROM worker_sessions")
      .get() as { n: number };
    expect(n).toBe(1);
  });
});

describe.runIf(process.env["TASKRUNNER_LIVE_CODEX"] === "1")("live codex", () => {
  it("runs a real delegated turn end to end", async () => {
    const repo = initRepo();
    const { scheduler, workspacesDir } = makeStack("codex");
    const outcome = await scheduler.assignTask({
      project: repo,
      worker: "codex",
      prompt: "Create a file named live.txt containing exactly the line: hello from live codex",
      wait: true,
    });
    expect(outcome.status).toBe("completed");
    expect(outcome.worker_session_id).toBeTruthy();
    const workspace = join(workspacesDir, outcome.task_id);
    expect(readFileSync(join(workspace, "live.txt"), "utf8")).toContain("hello from live codex");
  }, 300_000);
});
