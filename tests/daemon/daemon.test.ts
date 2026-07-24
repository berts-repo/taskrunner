import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { afterEach, describe, expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { Agent, fetch as undiciFetch } from "undici";
import { AlreadyRunningError, Daemon, type DaemonOptions } from "../../src/daemon/daemon.js";
import { statePaths } from "../../src/paths.js";
import { EventLog, readEvents } from "../../src/storage/events.js";
import { initGitRepo, LocalRunner, tempDir } from "../helpers.js";
import { writeFakeCodex } from "../workers/fake-codex.js";

function unixFetch(socketPath: string): typeof fetch {
  const agent = new Agent({ connect: { socketPath } });
  return ((input: string | URL, init?: RequestInit) =>
    undiciFetch(input as string, { ...(init as object), dispatcher: agent })) as unknown as typeof fetch;
}

const daemons: Daemon[] = [];

async function startDaemon(root: string, options: DaemonOptions = {}): Promise<Daemon> {
  // Default off: tests must never sweep the developer's real host transcripts.
  const daemon = await Daemon.start(statePaths(root), { ingestSources: [], ...options });
  daemons.push(daemon);
  return daemon;
}

afterEach(async () => {
  while (daemons.length > 0) await daemons.pop()!.stop();
});

describe("Daemon", () => {
  it("serves /status on the unix socket and cleans up on stop", async () => {
    const paths = statePaths(tempDir("daemon"));
    const daemon = await Daemon.start(paths, { ingestSources: [] });
    daemons.push(daemon);

    const res = await unixFetch(paths.socketPath)("http://taskrunner/status");
    expect(res.status).toBe(200);
    const body = (await res.json()) as any;
    expect(body.pid).toBe(process.pid);
    expect(body.state_root).toBe(paths.root);

    await daemons.pop()!.stop();
    expect(existsSync(paths.socketPath)).toBe(false);
    expect(existsSync(paths.pidFile)).toBe(false);
    expect(existsSync(paths.lockFile)).toBe(false);
  });

  it("refuses a second daemon on the same state root", async () => {
    const root = tempDir("daemon");
    await startDaemon(root);
    await expect(
      Daemon.start(statePaths(root), { ingestSources: [] }),
    ).rejects.toBeInstanceOf(AlreadyRunningError);
  });

  it("allows a restart after a clean stop", async () => {
    const root = tempDir("daemon");
    const first = await Daemon.start(statePaths(root), { ingestSources: [] });
    await first.stop();
    const second = await startDaemon(root);
    expect(second).toBeDefined();
  });

  it("fails turns left running by a crash, keeping their audit trail", async () => {
    const paths = statePaths(tempDir("daemon"));
    const log = EventLog.open(paths.eventsLog);
    log.append({ type: "project.created", project_id: "proj_a", root: "/repo" });
    log.append({
      type: "task.created",
      task_id: "task_a",
      project_id: "proj_a",
      worker: "codex",
      prompt_summary: "x",
    });
    log.append({ type: "turn.started", turn_id: "turn_a1", task_id: "task_a", prompt: "go" });
    log.append({
      type: "audit.recorded",
      task_id: "task_a",
      turn_id: "turn_a1",
      kind: "worker.command_execution",
      payload: { command: "sleep 999" },
    });
    log.close();

    const daemon = await startDaemon(paths.root);
    const turn = daemon.index.db
      .prepare("SELECT status, error_code FROM turns WHERE id = 'turn_a1'")
      .get() as any;
    expect(turn).toEqual({ status: "failed", error_code: "worker_failed" });

    const events = readEvents(paths.eventsLog);
    const last = events[events.length - 1]!;
    expect(last.type).toBe("turn.failed");
    // The pre-crash audit trail is untouched.
    expect(events.filter((e) => e.type === "audit.recorded")).toHaveLength(1);
  });

  it("runs a worker that exists only in config, end to end", async () => {
    // The pluggability acceptance test: no injected harnesses, no code that
    // knows this worker's name — just a [worker.<name>] config section. Only
    // the runner factory is stubbed so the fake codex binary runs sans Docker.
    const root = tempDir("daemon");
    mkdirSync(root, { recursive: true });
    writeFileSync(join(root, "config.toml"), `[worker.configling]\nharness = "codex"\n`);

    const repo = initGitRepo();

    const fakeCodex = writeFakeCodex();
    const daemon = await startDaemon(root, {
      makeRunner: (ctx) => new LocalRunner(ctx.workspaceDir, fakeCodex),
    });
    const assigned = await daemon.scheduler.assignTask({
      project: repo,
      worker: "configling",
      prompt: "create hello",
    });
    expect(assigned.tier).toBe("workspace-write");
    let status = "";
    for (let i = 0; i < 200; i++) {
      status = daemon.scheduler.outcome(assigned.task_id).status;
      if (status !== "running" && status !== "created") break;
      await sleep(25);
    }
    expect(status).toBe("completed");
    const final = daemon.scheduler.outcome(assigned.task_id);
    expect(final.summary).toContain("create hello");
    expect(final.changed_files).toEqual(["hello.txt"]);
  });

  it("records session.started/ended for MCP sessions", async () => {
    const paths = statePaths(tempDir("daemon"));
    await startDaemon(paths.root);

    const transport = new StreamableHTTPClientTransport(new URL("http://taskrunner/mcp"), {
      fetch: unixFetch(paths.socketPath),
    });
    const client = new Client({ name: "daemon-test-client", version: "0.0.1" });
    await client.connect(transport);
    await client.ping();

    let events = readEvents(paths.eventsLog);
    const started = events.find((e) => e.type === "session.started") as any;
    expect(started).toBeDefined();
    expect(started.client).toBe("daemon-test-client");

    await transport.terminateSession();
    await client.close();

    events = readEvents(paths.eventsLog);
    const ended = events.find((e) => e.type === "session.ended") as any;
    expect(ended).toBeDefined();
    expect(ended.session_id).toBe(started.session_id);
  });
});
