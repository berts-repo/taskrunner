import { newId } from "../ids.js";
import { workerConfig, type Config } from "../config.js";
import { ToolError } from "../domain/errors.js";
import { resolveTier } from "../domain/policy.js";
import { resolveProject } from "../domain/projects.js";
import {
  getTaskSnapshot,
  getTurnArtifacts,
  type ArtifactHandle,
  type TaskSnapshot,
} from "../domain/tasks.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import type { StateIndex } from "../storage/index.js";
import type { WorkerHarness } from "../workers/harness.js";
import type { WorkerRunner } from "../workers/runner.js";

/** Result contract shared by assign-task and continue-task (PLAN § MCP tool schemas). */
export interface TurnOutcome {
  task_id: string;
  turn_id: string | null;
  status: string;
  worker: string;
  worker_session_id: string | null;
  tier: string | null;
  /** none | pending | approved | denied */
  approval_state: string;
  summary: string | null;
  changed_files: string[];
  artifacts: ArtifactHandle[];
  error: { code: string; message: string } | null;
}

export interface WorkspaceChanges {
  changedFiles: string[];
}

/** Provides the per-task isolated workspace (a task-local clone). */
export interface WorkspaceProvider {
  ensureWorkspace(taskId: string, projectRoot: string): Promise<string>;
  /** Optional git-based fallback when the harness reports nothing. */
  collectChanges?(workspaceDir: string): WorkspaceChanges;
  /** Optional post-turn hook: diff artifacts and clone-branch landing. */
  afterTurn?(taskId: string, turnId: string, workspaceDir: string, projectRoot: string): void;
}

export class ProjectRootWorkspaces implements WorkspaceProvider {
  ensureWorkspace(_taskId: string, projectRoot: string): Promise<string> {
    return Promise.resolve(projectRoot);
  }
}

/** Everything the runner factory needs to place one turn's worker process. */
export interface RunnerContext {
  worker: string;
  workspaceDir: string;
  taskId: string;
  turnId: string;
  /** Egress allowlist: worker API defaults plus approved task additions. */
  allowedDomains: string[];
}

export interface SchedulerDeps {
  config: Config;
  index: StateIndex;
  record: (body: EventBody) => LogEvent;
  harnesses: Map<string, WorkerHarness>;
  workspaces: WorkspaceProvider;
  makeRunner: (ctx: RunnerContext) => WorkerRunner;
  artifacts: ArtifactStore;
}

interface RunningTurn {
  taskId: string;
  turnId: string;
  controller: AbortController;
  done: Promise<void>;
  cancelReason?: "cancel" | "timeout";
  cancelNote?: string;
}

/**
 * Turn lifecycle owner (PLAN § Async delegation contract, § Risk tiers and
 * default policy): assign/continue return immediately with a running status
 * unless `wait`; one running turn per task; every turn has a timeout;
 * networked tasks need a relayed user approval before they run.
 */
export class Scheduler {
  private readonly running = new Map<string, RunningTurn>();

  constructor(private readonly deps: SchedulerDeps) {}

  async assignTask(args: {
    project: string;
    worker: string;
    prompt: string;
    sessionId?: string;
    wait?: boolean;
    allowDomains?: string[];
    userApproved?: boolean;
  }): Promise<TurnOutcome> {
    const harness = this.harnessFor(args.worker);
    if (!args.prompt.trim()) throw new ToolError("invalid_request", "prompt must not be empty");
    const allowDomains = args.allowDomains ?? [];
    const tier = resolveTier(allowDomains);

    if (tier === "networked" && !args.userApproved) {
      throw new ToolError(
        "approval_required",
        `this task needs outbound network access to: ${allowDomains.join(", ")}. ` +
          "Ask the user for permission first, then retry with userApproved: true.",
      );
    }

    const project = resolveProject(this.deps.index, this.deps.record, args.project);
    const taskId = newId("task");
    this.deps.record({
      type: "task.created",
      task_id: taskId,
      project_id: project.project_id,
      ...(args.sessionId ? { session_id: args.sessionId } : {}),
      worker: args.worker,
      prompt_summary: summarize(args.prompt),
      tier,
      ...(allowDomains.length > 0 ? { allow_domains: allowDomains } : {}),
    });

    if (tier === "networked") {
      // The calling agent relayed the user's yes; the record says so.
      this.deps.record({
        type: "approval.recorded",
        approval_id: newId("appr"),
        task_id: taskId,
        decision: "approved",
        via: "agent",
        domains: allowDomains,
        ...(args.sessionId ? { session_id: args.sessionId } : {}),
      });
    }

    return this.startTurn(taskId, project.root, harness, args.prompt, args.wait ?? false);
  }

  async continueTask(args: {
    task_id: string;
    prompt: string;
    wait?: boolean;
  }): Promise<TurnOutcome> {
    const snapshot = this.snapshotFor(args.task_id);
    if (this.running.has(args.task_id)) {
      throw new ToolError(
        "conflict",
        `task ${args.task_id} already has a running turn; cancel it or wait for it to finish`,
      );
    }
    if (!args.prompt.trim()) throw new ToolError("invalid_request", "prompt must not be empty");
    // Legacy state: tasks denied under the removed human-approval flow stay denied.
    if (snapshot.approval_state === "denied") {
      throw new ToolError(
        "policy_denied",
        `task ${snapshot.task_id} was denied by the user and cannot run`,
      );
    }
    const harness = this.harnessFor(snapshot.worker);
    return this.startTurn(
      args.task_id,
      snapshot.project_root,
      harness,
      args.prompt,
      args.wait ?? false,
    );
  }

  async cancelTask(args: { task_id: string; reason?: string }): Promise<{
    task_id: string;
    turn_id: string | null;
    status: string;
  }> {
    const snapshot = getTaskSnapshot(this.deps.index, args.task_id);
    if (!snapshot) throw new ToolError("not_found", `no task ${args.task_id}`);
    const entry = this.running.get(args.task_id);
    if (!entry) return { task_id: args.task_id, turn_id: null, status: snapshot.status };

    entry.cancelReason ??= "cancel";
    if (args.reason) entry.cancelNote = args.reason;
    entry.controller.abort();
    await entry.done;
    const after = getTaskSnapshot(this.deps.index, args.task_id);
    return {
      task_id: args.task_id,
      turn_id: entry.turnId,
      status: after?.latest_turn?.status ?? "canceled",
    };
  }

  hasRunningTurn(taskId: string): boolean {
    return this.running.has(taskId);
  }

  /** Aborts all running turns and waits for their terminal records. */
  async shutdown(): Promise<void> {
    const entries = [...this.running.values()];
    for (const entry of entries) {
      entry.cancelReason ??= "cancel";
      entry.cancelNote ??= "daemon stopping";
      entry.controller.abort();
    }
    await Promise.all(entries.map((entry) => entry.done));
  }

  private snapshotFor(taskId: string): TaskSnapshot {
    const snapshot = getTaskSnapshot(this.deps.index, taskId);
    if (!snapshot) throw new ToolError("not_found", `no task ${taskId}`);
    return snapshot;
  }

  private harnessFor(worker: string): WorkerHarness {
    const harness = this.deps.harnesses.get(worker);
    if (!harness) {
      throw new ToolError(
        "not_configured",
        `worker '${worker}' is not configured on this workstation`,
      );
    }
    return harness;
  }

  private async startTurn(
    taskId: string,
    projectRoot: string,
    harness: WorkerHarness,
    prompt: string,
    wait: boolean,
  ): Promise<TurnOutcome> {
    const turnId = newId("turn");
    this.deps.record({ type: "turn.started", turn_id: turnId, task_id: taskId, prompt });

    const entry: RunningTurn = {
      taskId,
      turnId,
      controller: new AbortController(),
      done: Promise.resolve(),
    };
    entry.done = this.runTurn(entry, projectRoot, harness, prompt);
    this.running.set(taskId, entry);

    if (wait) await entry.done;
    return this.outcome(taskId, turnId);
  }

  private async runTurn(
    entry: RunningTurn,
    projectRoot: string,
    harness: WorkerHarness,
    prompt: string,
  ): Promise<void> {
    const { record, config } = this.deps;
    const { taskId, turnId } = entry;
    const timeoutSeconds = config.task.turn_timeout_seconds;
    const rawEventLines: string[] = [];
    const timer = setTimeout(() => {
      entry.cancelReason ??= "timeout";
      entry.controller.abort();
    }, timeoutSeconds * 1000);

    let runner: WorkerRunner | undefined;
    try {
      const snapshot = this.snapshotFor(taskId);
      const workspaces = this.deps.workspaces;
      const workspaceDir = await workspaces.ensureWorkspace(taskId, projectRoot);
      const previousNativeId = snapshot.worker_session_id ?? undefined;

      const workerCfg = workerConfig(config, snapshot.worker);
      const approvedDomains =
        snapshot.approval_state === "approved" ? snapshot.allow_domains : [];
      runner = this.deps.makeRunner({
        worker: snapshot.worker,
        workspaceDir,
        taskId,
        turnId,
        allowedDomains: [...workerCfg.allowed_domains, ...approvedDomains],
      });

      const result = await harness.runTurn({
        workspaceDir,
        runner,
        prompt,
        ...(previousNativeId ? { nativeSessionId: previousNativeId } : {}),
        signal: entry.controller.signal,
        // Streamed into the audit log as they arrive, not buffered
        // (PLAN § Storage write path).
        onEvent: (event) => {
          rawEventLines.push(JSON.stringify(event));
          record({
            type: "audit.recorded",
            task_id: taskId,
            turn_id: turnId,
            kind: `worker.${event.kind}`,
            payload: event.payload,
          });
        },
      });

      if (result.nativeSessionId && result.nativeSessionId !== previousNativeId) {
        record({
          type: "worker-session.recorded",
          worker_session_id: newId("wsess"),
          task_id: taskId,
          worker: harness.name,
          native_session_id: result.nativeSessionId,
        });
      }

      let changedFiles = result.changedFiles ?? [];
      if (changedFiles.length === 0 && workspaces.collectChanges) {
        changedFiles = workspaces.collectChanges(workspaceDir).changedFiles;
      }
      workspaces.afterTurn?.(taskId, turnId, workspaceDir, projectRoot);

      record({
        type: "turn.completed",
        turn_id: turnId,
        task_id: taskId,
        response: result.response,
        changed_files: changedFiles,
        ...(result.usage !== undefined ? { usage: result.usage } : {}),
      });
    } catch (err) {
      if (entry.cancelReason === "cancel") {
        record({
          type: "turn.canceled",
          turn_id: turnId,
          task_id: taskId,
          ...(entry.cancelNote ? { reason: entry.cancelNote } : {}),
        });
      } else if (entry.cancelReason === "timeout") {
        record({
          type: "turn.failed",
          turn_id: turnId,
          task_id: taskId,
          error_code: "worker_failed",
          error_message: `turn timed out after ${timeoutSeconds}s`,
        });
      } else {
        record({
          type: "turn.failed",
          turn_id: turnId,
          task_id: taskId,
          error_code: err instanceof ToolError ? err.code : "worker_failed",
          error_message: err instanceof Error ? err.message : String(err),
        });
      }
    } finally {
      clearTimeout(timer);
      await runner?.dispose().catch(() => {});
      // Bulky raw event streams are end-of-turn copy-out; the per-event audit
      // records above are already durable (PLAN § Storage write path).
      if (rawEventLines.length > 0) {
        const stored = this.deps.artifacts.store(rawEventLines.join("\n") + "\n");
        const artifactId = newId("art");
        record({
          type: "artifact.stored",
          artifact_id: artifactId,
          kind: "worker-events",
          label: "raw worker events",
          media_type: "application/jsonl",
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
      this.running.delete(taskId);
    }
  }

  outcome(taskId: string, turnId?: string): TurnOutcome {
    const snapshot = getTaskSnapshot(this.deps.index, taskId);
    if (!snapshot) throw new ToolError("internal_error", `task ${taskId} vanished`);
    const turn =
      turnId && snapshot.latest_turn?.turn_id !== turnId
        ? null
        : snapshot.latest_turn;
    return {
      task_id: taskId,
      turn_id: turn?.turn_id ?? turnId ?? null,
      status: turn?.status ?? snapshot.status,
      worker: snapshot.worker,
      worker_session_id: snapshot.worker_session_id,
      tier: snapshot.tier,
      approval_state: snapshot.approval_state,
      summary: turn?.response ?? null,
      changed_files: turn?.changed_files ?? [],
      artifacts: turn ? getTurnArtifacts(this.deps.index, turn.turn_id) : [],
      error:
        turn?.error_code && turn.status === "failed"
          ? { code: turn.error_code, message: turn.error_message ?? "" }
          : null,
    };
  }
}

function summarize(prompt: string): string {
  const line = prompt.replaceAll(/\s+/g, " ").trim();
  return line.length <= 100 ? line : `${line.slice(0, 97)}...`;
}
