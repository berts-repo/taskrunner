import { setTimeout as sleep } from "node:timers/promises";
import type { TurnRequest, TurnResult, WorkerHarness } from "./harness.js";

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
      exitCode: 0,
    };
  }
}
