import { describe, expect, it } from "vitest";
import { CodexHarness } from "../../src/workers/codex.js";
import type { WorkerEvent } from "../../src/workers/harness.js";
import { tempDir } from "../helpers.js";
import { writeFakeCodex } from "./fake-codex.js";

function collect() {
  const events: WorkerEvent[] = [];
  return { events, onEvent: (e: WorkerEvent) => events.push(e) };
}

describe("CodexHarness", () => {
  it("parses spike-shaped JSONL: thread id, events, response, changed files, usage", async () => {
    const harness = new CodexHarness(writeFakeCodex());
    const workspace = tempDir("codex-ws");
    const { events, onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      prompt: "create hello",
      signal: new AbortController().signal,
      onEvent,
    });

    expect(result.exitCode).toBe(0);
    expect(result.nativeSessionId).toMatch(/^thread-/);
    expect(result.response).toContain("started thread-");
    expect(result.changedFiles).toEqual(["hello.txt"]);
    expect(result.usage).toEqual({ input_tokens: 10, output_tokens: 5 });
    expect(events.map((e) => e.kind)).toEqual([
      "thread.started",
      "turn.started",
      "command_execution",
      "file_change",
      "agent_message",
      "turn.completed",
    ]);
  });

  it("passes the native session id on resume", async () => {
    const harness = new CodexHarness(writeFakeCodex());
    const workspace = tempDir("codex-ws");
    const { onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      prompt: "continue please",
      nativeSessionId: "thread-existing",
      signal: new AbortController().signal,
      onEvent,
    });
    expect(result.nativeSessionId).toBe("thread-existing");
    expect(result.response).toContain("resumed thread-existing");
  });

  it("rejects with stderr detail on nonzero exit", async () => {
    const harness = new CodexHarness(writeFakeCodex());
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: tempDir("codex-ws"),
        prompt: "exit-nonzero",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/code 3.*fake codex blew up/s);
  });

  it("kills the worker on abort", async () => {
    const harness = new CodexHarness(writeFakeCodex());
    const controller = new AbortController();
    const { onEvent } = collect();
    const pending = harness.runTurn({
      workspaceDir: tempDir("codex-ws"),
      prompt: "hang",
      signal: controller.signal,
      onEvent,
    });
    setTimeout(() => controller.abort(), 150);
    await expect(pending).rejects.toThrow(/terminated by abort/);
  });

  it("rejects clearly when the codex binary is missing", async () => {
    const harness = new CodexHarness("/nonexistent/codex-binary");
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: tempDir("codex-ws"),
        prompt: "x",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/failed to start codex worker/);
  });
});
