// Shared worker harness contract (PLAN § Worker Harness): start a turn,
// resume by worker-native session ID, stream structured events, capture the
// final response, exit status, and changed files.

import type { WorkerRunner } from "./runner.js";

export interface WorkerEvent {
  kind: string;
  payload: unknown;
}

export interface TurnRequest {
  /** Host path of the task workspace. */
  workspaceDir: string;
  /** Where and how the worker process runs (host spawn or Docker container). */
  runner: WorkerRunner;
  prompt: string;
  /** Resume this worker-native session when present. */
  nativeSessionId?: string;
  /** Fired by the scheduler on cancellation or timeout. */
  signal: AbortSignal;
  /** Streamed as events arrive; the scheduler appends them to the audit log. */
  onEvent: (event: WorkerEvent) => void;
}

export interface TurnResult {
  response: string;
  nativeSessionId?: string;
  /** Harness-reported changed files; the workspace git fallback fills gaps. */
  changedFiles?: string[];
  usage?: unknown;
  exitCode: number | null;
}

export interface WorkerHarness {
  readonly name: string;
  /** Rejects on worker failure. Must terminate the worker when signal aborts. */
  runTurn(request: TurnRequest): Promise<TurnResult>;
}
