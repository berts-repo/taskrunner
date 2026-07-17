import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Readable } from "node:stream";
import { setTimeout as sleep } from "node:timers/promises";
import type { WorkspaceProvider } from "../src/daemon/scheduler.js";
import type { EventBody, LogEvent } from "../src/storage/events.js";
import type { TurnRequest, TurnResult, WorkerHarness } from "../src/workers/harness.js";
import type { RunningWorker, WorkerRunner, WorkerSpawnSpec } from "../src/workers/runner.js";

export function tempDir(prefix: string): string {
  return mkdtempSync(join(tmpdir(), `${prefix}-`));
}

/** Throwaway git repo with one committed README, for clone-workspace tests. */
export function initGitRepo(): string {
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

/** Spawns the worker argv directly in the workspace: the no-Docker test runner. */
export class LocalRunner implements WorkerRunner {
  readonly kind = "host";

  constructor(
    readonly workspacePath: string,
    /** Overrides argv[0], e.g. a fake codex binary path. */
    private readonly command?: string,
  ) {}

  start(spec: WorkerSpawnSpec): Promise<RunningWorker> {
    const [logical, ...rest] = spec.argv;
    const child = spawn(this.command ?? (logical as string), rest, {
      cwd: this.workspacePath,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, ...spec.env },
    });
    return Promise.resolve({
      stdout: child.stdout as Readable,
      stderr: child.stderr as Readable,
      exited: new Promise((resolve, reject) => {
        child.on("error", reject);
        child.on("close", (code) => resolve(code));
      }),
      kill: () => {
        child.kill("SIGTERM");
        const hard = setTimeout(() => child.kill("SIGKILL"), 2000);
        hard.unref();
      },
    });
  }

  dispose(): Promise<void> {
    return Promise.resolve();
  }
}

/** Runs workers directly in the project root: no clone, no isolation. */
export class ProjectRootWorkspaces implements WorkspaceProvider {
  ensureWorkspace(_taskId: string, projectRoot: string): Promise<string> {
    return Promise.resolve(projectRoot);
  }
}

/**
 * Scripted in-process harness for tests. Behavior is driven by directives in
 * the prompt: `sleep:<ms>` delays (abortably), `fail` rejects. Native session
 * IDs are `fake-<n>` and each resumed turn increments a per-session counter
 * so continuation is observable.
 */
export class FakeHarness implements WorkerHarness {
  readonly name = "fake";
  private nextSession = 1;
  private readonly turnCounts = new Map<string, number>();

  async runTurn(request: TurnRequest): Promise<TurnResult> {
    request.onEvent({ kind: "agent_message", payload: { text: "fake worker starting" } });

    const sleepMatch = /sleep:(\d+)/.exec(request.prompt);
    if (sleepMatch) {
      await sleep(Number(sleepMatch[1]), undefined, { signal: request.signal });
    }
    request.signal.throwIfAborted();
    if (request.prompt.includes("fail")) {
      throw new Error("fake worker failure");
    }

    const sessionId = request.nativeSessionId ?? `fake-${this.nextSession++}`;
    const turn = (this.turnCounts.get(sessionId) ?? 0) + 1;
    this.turnCounts.set(sessionId, turn);
    request.onEvent({ kind: "command_execution", payload: { command: "true" } });

    return {
      response: `echo: ${request.prompt} (session ${sessionId}, turn ${turn})`,
      nativeSessionId: sessionId,
      changedFiles: [],
    };
  }
}

let seq = 0;

/** Builds a LogEvent with deterministic id/ts without going through a log file. */
export function evt(body: EventBody): LogEvent {
  seq += 1;
  return {
    id: `evt_${String(seq).padStart(8, "0")}`,
    ts: new Date(Date.UTC(2026, 0, 1, 0, 0, seq)).toISOString(),
    ...body,
  };
}

/** A realistic event sequence: one project, session, task, and completed turn. */
export function sampleSequence(): LogEvent[] {
  return [
    evt({ type: "project.created", project_id: "proj_a", root: "/repo" }),
    evt({ type: "project.alias-added", project_id: "proj_a", path: "/repo-symlink" }),
    evt({ type: "session.started", session_id: "sess_a", project_id: "proj_a", client: "claude-code" }),
    evt({
      type: "task.created",
      task_id: "task_a",
      project_id: "proj_a",
      session_id: "sess_a",
      worker: "codex",
      prompt_summary: "add greeting file",
    }),
    evt({ type: "turn.started", turn_id: "turn_a1", task_id: "task_a", prompt: "create hello.txt" }),
    evt({
      type: "audit.recorded",
      task_id: "task_a",
      turn_id: "turn_a1",
      kind: "worker.command_execution",
      payload: { command: "touch hello.txt" },
    }),
    evt({
      type: "worker-session.recorded",
      worker_session_id: "wsess_a",
      task_id: "task_a",
      worker: "codex",
      native_session_id: "019e28f6-9f73-73d0-b601-33505b06d3f5",
    }),
    evt({
      type: "artifact.stored",
      artifact_id: "art_a",
      kind: "worker-events",
      label: "raw codex events",
      media_type: "application/jsonl",
      size_bytes: 123,
      sha256: "ab".repeat(32),
      locator: "ab/" + "ab".repeat(32),
    }),
    evt({ type: "artifact.linked", artifact_id: "art_a", task_id: "task_a", turn_id: "turn_a1" }),
    evt({
      type: "turn.completed",
      turn_id: "turn_a1",
      task_id: "task_a",
      response: "created hello.txt",
      changed_files: ["hello.txt"],
    }),
    evt({ type: "session.ended", session_id: "sess_a" }),
  ];
}
