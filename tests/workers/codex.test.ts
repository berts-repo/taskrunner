import { Readable } from "node:stream";
import { describe, expect, it } from "vitest";
import { CodexHarness } from "../../src/workers/codex.js";
import type { WorkerEvent } from "../../src/workers/harness.js";
import type { WorkerRunner, WorkerSpawnSpec } from "../../src/workers/runner.js";
import { LocalRunner, tempDir } from "../helpers.js";
import { writeFakeCodex } from "./fake-codex.js";

function collect() {
  const events: WorkerEvent[] = [];
  return { events, onEvent: (e: WorkerEvent) => events.push(e) };
}

/** Local runner whose command override points at the fake codex script. */
function fakeRunner(workspace: string): LocalRunner {
  return new LocalRunner(workspace, writeFakeCodex());
}

describe("CodexHarness", () => {
  it("parses codex exec JSONL: thread id, events, response, changed files, usage", async () => {
    const harness = new CodexHarness();
    const workspace = tempDir("codex-ws");
    const { events, onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      runner: fakeRunner(workspace),
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
    const harness = new CodexHarness();
    const workspace = tempDir("codex-ws");
    const { onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      runner: fakeRunner(workspace),
      prompt: "continue please",
      nativeSessionId: "thread-existing",
      signal: new AbortController().signal,
      onEvent,
    });
    expect(result.nativeSessionId).toBe("thread-existing");
    expect(result.response).toContain("resumed thread-existing");
  });

  it("rejects with stderr detail on nonzero exit", async () => {
    const harness = new CodexHarness();
    const workspace = tempDir("codex-ws");
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: workspace,
        runner: fakeRunner(workspace),
        prompt: "exit-nonzero",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/code 3.*fake codex blew up/s);
  });

  it("kills the worker on abort", async () => {
    const harness = new CodexHarness();
    const workspace = tempDir("codex-ws");
    const controller = new AbortController();
    const { onEvent } = collect();
    const pending = harness.runTurn({
      workspaceDir: workspace,
      runner: fakeRunner(workspace),
      prompt: "hang",
      signal: controller.signal,
      onEvent,
    });
    setTimeout(() => controller.abort(), 150);
    await expect(pending).rejects.toThrow(/terminated by abort/);
  });

  it("builds --oss argv for local-model workers", async () => {
    const captured: WorkerSpawnSpec[] = [];
    const runner: WorkerRunner = {
      kind: "docker",
      workspacePath: "/workspace",
      start: (spec: WorkerSpawnSpec) => {
        captured.push(spec);
        return Promise.resolve({
          stdout: Readable.from([]),
          stderr: Readable.from([]),
          exited: Promise.resolve(0),
          kill() {},
        });
      },
      dispose: () => Promise.resolve(),
    };
    const harness = new CodexHarness({ model: "gpt-oss:20b", provider: "ollama" });
    await harness.runTurn({
      workspaceDir: "/ws",
      runner,
      prompt: "hello",
      signal: new AbortController().signal,
      onEvent: () => {},
    });
    expect(captured[0]!.argv).toEqual([
      "codex",
      "-a",
      "never",
      "-s",
      "danger-full-access",
      "--oss",
      "--local-provider",
      "ollama",
      "-m",
      "gpt-oss:20b",
      "exec",
      "--json",
      "-C",
      "/workspace",
      "hello",
    ]);
    // The model server sits on the host; localhost would be the container.
    expect(captured[0]!.env).toEqual({
      CODEX_OSS_BASE_URL: "http://host.docker.internal:11434/v1",
    });
  });

  it("rejects clearly when the codex binary is missing", async () => {
    const harness = new CodexHarness();
    const workspace = tempDir("codex-ws");
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: workspace,
        runner: new LocalRunner(workspace, "/nonexistent/codex-binary"),
        prompt: "x",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/failed to start codex worker/);
  });
});
