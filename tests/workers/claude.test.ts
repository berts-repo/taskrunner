import { realpathSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ClaudeHarness } from "../../src/workers/claude.js";
import type { WorkerEvent } from "../../src/workers/harness.js";
import { HostRunner } from "../../src/workers/runner.js";
import { tempDir } from "../helpers.js";
import { writeFakeClaude } from "./fake-claude.js";

// realpath matters: the fake claude reports file_path from its resolved cwd
// (/private/var/... on macOS), and prefix stripping compares string paths.
function workspaceDir(): string {
  return realpathSync(tempDir("claude-ws"));
}

function collect() {
  const events: WorkerEvent[] = [];
  return { events, onEvent: (e: WorkerEvent) => events.push(e) };
}

function fakeRunner(workspace: string): HostRunner {
  return new HostRunner(workspace, writeFakeClaude());
}

describe("ClaudeHarness", () => {
  it("parses stream-json: session id, events, response, edited files, usage", async () => {
    const harness = new ClaudeHarness();
    const workspace = workspaceDir();
    const { events, onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      runner: fakeRunner(workspace),
      prompt: "create hello",
      signal: new AbortController().signal,
      onEvent,
    });

    expect(result.exitCode).toBe(0);
    expect(result.nativeSessionId).toMatch(/^sess-/);
    expect(result.response).toContain("started sess-");
    // Workspace-relative: the absolute file_path prefix is stripped.
    expect(result.changedFiles).toEqual(["hello.txt"]);
    expect(result.usage).toEqual({ input_tokens: 7, output_tokens: 3 });
    // Event kinds quote Claude's own line types verbatim.
    expect(events.map((e) => e.kind)).toEqual(["system", "assistant", "user", "result"]);
  });

  it("passes the native session id on resume", async () => {
    const harness = new ClaudeHarness();
    const workspace = workspaceDir();
    const { onEvent } = collect();

    const result = await harness.runTurn({
      workspaceDir: workspace,
      runner: fakeRunner(workspace),
      prompt: "continue please",
      nativeSessionId: "sess-existing",
      signal: new AbortController().signal,
      onEvent,
    });
    expect(result.nativeSessionId).toBe("sess-existing");
    expect(result.response).toContain("resumed sess-existing");
  });

  it("rejects when claude reports an error result despite exit 0", async () => {
    const harness = new ClaudeHarness();
    const workspace = workspaceDir();
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: workspace,
        runner: fakeRunner(workspace),
        prompt: "result-error",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/fake claude task failed/);
  });

  it("rejects with stderr detail on nonzero exit", async () => {
    const harness = new ClaudeHarness();
    const workspace = workspaceDir();
    const { onEvent } = collect();
    await expect(
      harness.runTurn({
        workspaceDir: workspace,
        runner: fakeRunner(workspace),
        prompt: "exit-nonzero",
        signal: new AbortController().signal,
        onEvent,
      }),
    ).rejects.toThrow(/code 3.*fake claude blew up/s);
  });

  it("kills the worker on abort", async () => {
    const harness = new ClaudeHarness();
    const workspace = workspaceDir();
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
});
