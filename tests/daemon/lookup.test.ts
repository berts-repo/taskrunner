import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { parseConfig } from "../../src/config.js";
import { lookupTask } from "../../src/daemon/lookup.js";
import { Scheduler } from "../../src/daemon/scheduler.js";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import { EventLog } from "../../src/storage/events.js";
import { StateIndex } from "../../src/storage/index.js";
import { CodexHarness } from "../../src/workers/codex.js";
import { HostRunner } from "../../src/workers/runner.js";
import { WorktreeWorkspaces } from "../../src/workspace/worktree.js";
import { tempDir } from "../helpers.js";
import { writeFakeCodex } from "../workers/fake-codex.js";

// One real two-turn task built through the whole stack (fake codex binary +
// worktrees), then lookup assertions against it.

let deps: { index: StateIndex; artifacts: ArtifactStore };
let repo: string;
let taskId: string;
let turnIds: string[];

beforeAll(async () => {
  repo = tempDir("repo");
  const git = (...args: string[]) =>
    execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });
  git("init", "-q");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "T");
  writeFileSync(join(repo, "README.md"), "hi\n");
  git("add", ".");
  git("commit", "-qm", "init");

  const root = tempDir("lookup");
  const log = EventLog.open(join(root, "events.jsonl"));
  const index = new StateIndex(":memory:");
  const record = (body: Parameters<EventLog["append"]>[0]) => {
    const event = log.append(body);
    index.apply(event);
    return event;
  };
  const artifacts = new ArtifactStore(join(root, "artifacts"));
  const worktrees = new WorktreeWorkspaces(join(root, "workspaces"), artifacts, record);
  const fakeCodex = writeFakeCodex();
  const scheduler = new Scheduler({
    config: parseConfig({}),
    index,
    record,
    harnesses: new Map([["codex", new CodexHarness()]]),
    workspaces: () => worktrees,
    makeRunner: (ctx) => new HostRunner(ctx.workspaceDir, fakeCodex),
    artifacts,
  });
  deps = { index, artifacts };

  const first = await scheduler.assignTask({
    project: repo,
    worker: "codex",
    prompt: "create hello",
    wait: true,
  });
  taskId = first.task_id;
  const second = await scheduler.continueTask({
    task_id: taskId,
    prompt: "extend hello",
    wait: true,
  });
  turnIds = [first.turn_id!, second.turn_id!];
});

describe("lookup-task", () => {
  it("returns a compact summary by default, with no expansions", () => {
    const text = lookupTask(deps, { task_id: taskId });
    expect(text).toContain(`task: ${taskId}`);
    expect(text).toContain("status: completed");
    expect(text).toContain("turns: 2");
    expect(text).not.toContain("exchanges:");
    expect(text).not.toContain("trace:");
  });

  it("include turns returns paired exchanges in turn order", () => {
    const text = lookupTask(deps, { task_id: taskId, include: ["turns"] });
    const first = text.indexOf(">> create hello");
    const second = text.indexOf(">> extend hello");
    expect(first).toBeGreaterThan(-1);
    expect(second).toBeGreaterThan(first);
    expect(text).toMatch(/<< started thread-/);
    expect(text).toMatch(/<< resumed thread-/);
  });

  it("include trace replays inputs, activity, and outputs per turn", () => {
    const text = lookupTask(deps, { task_id: taskId, include: ["trace"] });
    expect(text).toContain(`trace: turn 1 (${turnIds[0]}, completed)`);
    expect(text).toContain("inputs:");
    expect(text).toContain("prompt: create hello");
    expect(text).toContain("activity:");
    expect(text).toContain("worker.agent_message");
    expect(text).toContain("worker.file_change");
    expect(text).toContain("outputs:");
    expect(text).toContain("changed files: hello.txt");
    expect(text).toContain("artifact:");
  });

  it("scope turn_id narrows expansions to one turn", () => {
    const text = lookupTask(deps, {
      task_id: taskId,
      include: ["turns"],
      scope: { turn_id: turnIds[1]! },
    });
    expect(text).not.toContain(">> create hello");
    expect(text).toContain(">> extend hello");
  });

  it("scope last N returns only the trailing exchanges", () => {
    const text = lookupTask(deps, {
      task_id: taskId,
      include: ["turns"],
      scope: { last: 1 },
    });
    expect(text).not.toContain(">> create hello");
    expect(text).toContain(">> extend hello");
  });

  it("include artifacts lists handles, not payloads", () => {
    const text = lookupTask(deps, { task_id: taskId, include: ["artifacts"] });
    expect(text).toContain("worker-events");
    expect(text).toMatch(/art_\w+ {2}worker-events/);
    expect(text).toContain("bytes");
  });

  it("include audit lists per-turn audit rows", () => {
    const text = lookupTask(deps, { task_id: taskId, include: ["audit"] });
    expect(text).toContain(`audit: turn 1 (${turnIds[0]}`);
    expect(text).toContain("worker.turn.completed");
  });

  it("project lookup lists tasks most-recent-first without creating records", () => {
    const before = (
      deps.index.db.prepare("SELECT COUNT(*) AS n FROM projects").get() as { n: number }
    ).n;
    const text = lookupTask(deps, { project: repo });
    expect(text).toContain(`project: `);
    expect(text).toContain(taskId);
    expect(text).toContain("completed");
    const after = (
      deps.index.db.prepare("SELECT COUNT(*) AS n FROM projects").get() as { n: number }
    ).n;
    expect(after).toBe(before);
  });

  it("errors match the contract", () => {
    expect(() => lookupTask(deps, {})).toThrowError(/task_id or project/);
    expect(() => lookupTask(deps, { task_id: "task_nope" })).toThrowError(/no task/);
    expect(() => lookupTask(deps, { project: "/nope/nothing" })).toThrowError(
      /no tasks recorded/,
    );
    expect(() =>
      lookupTask(deps, { task_id: taskId, scope: { turn_id: "turn_nope" } }),
    ).toThrowError(/no turn/);
  });
});
