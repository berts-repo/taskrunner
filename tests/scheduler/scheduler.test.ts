import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { describe, expect, it } from "vitest";
import { parseConfig } from "../../src/config.js";
import { Scheduler } from "../../src/daemon/scheduler.js";
import { ToolError } from "../../src/domain/errors.js";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import { EventLog, readEvents } from "../../src/storage/events.js";
import { StateIndex } from "../../src/storage/index.js";
import { FakeHarness, LocalRunner, ProjectRootWorkspaces, tempDir } from "../helpers.js";

function makeScheduler(configOverrides: object = {}) {
  const root = tempDir("sched");
  const logPath = join(root, "events.jsonl");
  const log = EventLog.open(logPath);
  const index = new StateIndex(":memory:");
  const record = (body: Parameters<EventLog["append"]>[0]) => {
    const event = log.append(body);
    index.apply(event);
    return event;
  };
  const workspaces = new ProjectRootWorkspaces();
  const scheduler = new Scheduler({
    config: parseConfig(configOverrides),
    index,
    record,
    harnesses: new Map([["fake", new FakeHarness()]]),
    workspaces,
    // FakeHarness never spawns a process, so a local runner suffices.
    makeRunner: (ctx) => new LocalRunner(ctx.workspaceDir),
    artifacts: new ArtifactStore(join(root, "artifacts")),
  });
  const project = tempDir("proj");
  return { scheduler, index, logPath, project };
}

async function waitForTerminal(scheduler: Scheduler, taskId: string): Promise<string> {
  for (let i = 0; i < 200; i++) {
    const status = scheduler.outcome(taskId).status;
    if (status !== "running") return status;
    await sleep(25);
  }
  throw new Error("turn never reached a terminal status");
}

describe("Scheduler async contract", () => {
  it("assign-task returns immediately with running, then completes", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "sleep:200 do something",
    });
    expect(outcome.status).toBe("running");
    expect(outcome.task_id).toMatch(/^task_/);
    expect(outcome.turn_id).toMatch(/^turn_/);

    expect(await waitForTerminal(scheduler, outcome.task_id)).toBe("completed");
    const final = scheduler.outcome(outcome.task_id);
    expect(final.summary).toContain("echo:");
    expect(final.worker_session_id).toMatch(/^fake-/);
    expect(final.error).toBeNull();
  });

  it("wait:true blocks until the turn is terminal", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "quick",
      wait: true,
    });
    expect(outcome.status).toBe("completed");
    expect(outcome.summary).toContain("turn 1");
  });

  it("continue-task resumes the same worker-native session", async () => {
    const { scheduler, project } = makeScheduler();
    const first = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "start",
      wait: true,
    });
    const second = await scheduler.continueTask({
      task_id: first.task_id,
      prompt: "again",
      wait: true,
    });
    expect(second.task_id).toBe(first.task_id);
    expect(second.turn_id).not.toBe(first.turn_id);
    expect(second.worker_session_id).toBe(first.worker_session_id);
    expect(second.summary).toContain("turn 2");
  });

  it("continue-task during a running turn returns conflict", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "sleep:2000",
    });
    await expect(
      scheduler.continueTask({ task_id: outcome.task_id, prompt: "more" }),
    ).rejects.toMatchObject({ code: "conflict" });
    await scheduler.cancelTask({ task_id: outcome.task_id });
  });

  it("times out a turn and records worker_failed with audit retained", async () => {
    const { scheduler, project, logPath } = makeScheduler({
      task: { turn_timeout_seconds: 1 },
    });
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "sleep:10000",
      wait: true,
    });
    expect(outcome.status).toBe("failed");
    expect(outcome.error).toMatchObject({ code: "worker_failed" });
    expect(outcome.error?.message).toContain("timed out after 1s");
    // The pre-timeout streamed audit events survive.
    const kinds = readEvents(logPath)
      .filter((e) => e.type === "audit.recorded")
      .map((e) => (e as any).kind);
    expect(kinds).toContain("worker.agent_message");
  });

  it("cancel-task cancels the running turn and preserves the audit trail", async () => {
    const { scheduler, project, logPath } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "sleep:10000",
    });
    const result = await scheduler.cancelTask({
      task_id: outcome.task_id,
      reason: "changed my mind",
    });
    expect(result).toEqual({
      task_id: outcome.task_id,
      turn_id: outcome.turn_id,
      status: "canceled",
    });
    const events = readEvents(logPath);
    const canceled = events.find((e) => e.type === "turn.canceled") as any;
    expect(canceled.reason).toBe("changed my mind");
    expect(events.some((e) => e.type === "audit.recorded")).toBe(true);
  });

  it("cancel-task on an idle task reports the current status without a turn", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "quick",
      wait: true,
    });
    const result = await scheduler.cancelTask({ task_id: outcome.task_id });
    expect(result).toEqual({
      task_id: outcome.task_id,
      turn_id: null,
      status: "completed",
    });
  });

  it("records worker crashes as failed turns", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "please fail",
      wait: true,
    });
    expect(outcome.status).toBe("failed");
    expect(outcome.error?.message).toContain("fake worker failure");
  });

  it("rejects unknown workers, tasks, and empty prompts", async () => {
    const { scheduler, project } = makeScheduler();
    await expect(
      scheduler.assignTask({ project, worker: "nope", prompt: "x" }),
    ).rejects.toMatchObject({ code: "not_configured" });
    await expect(
      scheduler.continueTask({ task_id: "task_missing", prompt: "x" }),
    ).rejects.toMatchObject({ code: "not_found" });
    await expect(scheduler.cancelTask({ task_id: "task_missing" })).rejects.toMatchObject({
      code: "not_found",
    });
    await expect(
      scheduler.assignTask({ project, worker: "fake", prompt: "   " }),
    ).rejects.toMatchObject({ code: "invalid_request" });
    await expect(
      scheduler.assignTask({ project: "relative/path", worker: "fake", prompt: "x" }),
    ).rejects.toBeInstanceOf(ToolError);
    await expect(
      scheduler.assignTask({ project: join(project, "missing"), worker: "fake", prompt: "x" }),
    ).rejects.toMatchObject({ code: "invalid_request" });
  });

  it("runs different tasks concurrently", async () => {
    const { scheduler, project } = makeScheduler();
    const a = await scheduler.assignTask({ project, worker: "fake", prompt: "sleep:10000 a" });
    const b = await scheduler.assignTask({ project, worker: "fake", prompt: "sleep:10000 b" });
    // Two different tasks hold running turns at the same time (within one
    // task this is impossible: continue-task returns conflict).
    expect(scheduler.hasRunningTurn(a.task_id)).toBe(true);
    expect(scheduler.hasRunningTurn(b.task_id)).toBe(true);
    const canceledA = await scheduler.cancelTask({ task_id: a.task_id });
    const canceledB = await scheduler.cancelTask({ task_id: b.task_id });
    expect(canceledA.status).toBe("canceled");
    expect(canceledB.status).toBe("canceled");
  });

  it("reuses one project record across tasks", async () => {
    const { scheduler, index, project } = makeScheduler();
    await scheduler.assignTask({ project, worker: "fake", prompt: "a", wait: true });
    await scheduler.assignTask({ project, worker: "fake", prompt: "b", wait: true });
    const { n } = index.db.prepare("SELECT COUNT(*) AS n FROM projects").get() as { n: number };
    expect(n).toBe(1);
  });
});

describe("Risk tiers and approvals", () => {
  it("records workspace-write tier on plain tasks", async () => {
    const { scheduler, project } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "quick",
      wait: true,
    });
    expect(outcome.tier).toBe("workspace-write");
    expect(outcome.approval_state).toBe("none");
  });

  it("networked without user_approved is rejected with approval_required", async () => {
    const { scheduler, project } = makeScheduler();
    await expect(
      scheduler.assignTask({
        project,
        worker: "fake",
        prompt: "install deps",
        allowDomains: ["registry.npmjs.org"],
      }),
    ).rejects.toMatchObject({ code: "approval_required" });
  });

  it("networked with user_approved runs and records an agent-relayed approval", async () => {
    const { scheduler, project, logPath } = makeScheduler();
    const outcome = await scheduler.assignTask({
      project,
      worker: "fake",
      prompt: "install deps",
      allowDomains: ["registry.npmjs.org"],
      userApproved: true,
      wait: true,
    });
    expect(outcome.tier).toBe("networked");
    expect(outcome.approval_state).toBe("approved");
    expect(outcome.status).toBe("completed");
    const approval = readEvents(logPath).find((e) => e.type === "approval.recorded") as any;
    expect(approval.via).toBe("agent");
    expect(approval.domains).toEqual(["registry.npmjs.org"]);
  });
});
