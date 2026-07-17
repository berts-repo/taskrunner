import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { Agent, fetch as undiciFetch } from "undici";
import { Daemon } from "../../src/daemon/daemon.js";
import { statePaths } from "../../src/paths.js";
import { readEvents } from "../../src/storage/events.js";
import { FakeHarness, ProjectRootWorkspaces, tempDir } from "../helpers.js";

function unixFetch(socketPath: string): typeof fetch {
  const agent = new Agent({ connect: { socketPath } });
  return ((input: string | URL, init?: RequestInit) =>
    undiciFetch(input as string, { ...(init as object), dispatcher: agent })) as unknown as typeof fetch;
}

function toolText(result: any): string {
  return (result.content as { type: string; text: string }[])
    .map((c) => c.text)
    .join("\n");
}

describe("MCP tool surface", () => {
  let daemon: Daemon;
  let client: Client;
  let project: string;
  let eventsLog: string;

  beforeEach(async () => {
    const paths = statePaths(tempDir("tools"));
    eventsLog = paths.eventsLog;
    daemon = await Daemon.start(paths, {
      harnesses: new Map([["fake", new FakeHarness()]]),
      workspaces: new ProjectRootWorkspaces(),
    });
    client = new Client({ name: "tools-test", version: "0.0.1" });
    await client.connect(
      new StreamableHTTPClientTransport(new URL("http://taskrunner/mcp"), {
        fetch: unixFetch(paths.socketPath),
      }),
    );
    project = tempDir("proj");
  });

  afterEach(async () => {
    await client.close();
    await daemon.stop();
  });

  it("lists the four core tools", async () => {
    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name).sort()).toEqual([
      "assign-task",
      "cancel-task",
      "continue-task",
      "lookup-task",
    ]);
  });

  it("assign-task with wait, then lookup-task shows the exchange", async () => {
    const assign = await client.callTool({
      name: "assign-task",
      arguments: { project, worker: "fake", prompt: "make it so", wait: true },
    });
    const assignText = toolText(assign);
    expect(assign.isError).toBeFalsy();
    expect(assignText).toContain("status: completed");
    const taskId = /task: (task_\w+)/.exec(assignText)![1]!;

    const lookup = await client.callTool({
      name: "lookup-task",
      arguments: { taskId: taskId, include: ["turns"] },
    });
    const lookupText = toolText(lookup);
    expect(lookupText).toContain(">> make it so");
    expect(lookupText).toContain("<< echo: make it so");
  });

  it("continue-task on a running turn reports conflict; cancel-task stops it", async () => {
    const assign = await client.callTool({
      name: "assign-task",
      arguments: { project, worker: "fake", prompt: "sleep:10000" },
    });
    const taskId = /task: (task_\w+)/.exec(toolText(assign))![1]!;

    const conflict = await client.callTool({
      name: "continue-task",
      arguments: { taskId: taskId, prompt: "more" },
    });
    expect(conflict.isError).toBe(true);
    expect(toolText(conflict)).toContain("error conflict:");

    const cancel = await client.callTool({
      name: "cancel-task",
      arguments: { taskId: taskId, reason: "test cleanup" },
    });
    expect(toolText(cancel)).toContain("status: canceled");
  });

  it("maps unknown workers to not_configured and audits tool calls", async () => {
    const result = await client.callTool({
      name: "assign-task",
      arguments: { project, worker: "gemini", prompt: "x" },
    });
    expect(result.isError).toBe(true);
    expect(toolText(result)).toContain("error not_configured:");

    const auditKinds = readEvents(eventsLog)
      .filter((e) => e.type === "audit.recorded")
      .map((e) => (e as any).kind);
    expect(auditKinds).toContain("tool.assign-task");
  });
});
