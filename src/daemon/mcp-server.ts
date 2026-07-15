import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import type { Config } from "../config.js";
import { ToolError } from "../domain/errors.js";
import { getTaskSnapshot, listTurns } from "../domain/tasks.js";
import type { StatePaths } from "../paths.js";
import { renderCancel, renderOutcome, renderTaskDetail } from "../render.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import type { StateIndex } from "../storage/index.js";
import { VERSION } from "../version.js";
import type { Scheduler } from "./scheduler.js";

/** Everything a tool handler needs from the daemon. */
export interface ToolContext {
  paths: StatePaths;
  config: Config;
  index: StateIndex;
  artifacts: ArtifactStore;
  scheduler: Scheduler;
  /** Appends to the event log and folds into the index — the only write path. */
  record: (body: EventBody) => LogEvent;
  /** Durable Taskrunner session for the connected client. */
  sessionId: string;
}

type ToolResult = CallToolResult;

function textResult(text: string): ToolResult {
  return { content: [{ type: "text", text }] };
}

function errorResult(err: unknown): ToolResult {
  const code = err instanceof ToolError ? err.code : "internal_error";
  const message = err instanceof Error ? err.message : String(err);
  return { content: [{ type: "text", text: `error ${code}: ${message}` }], isError: true };
}

export function createMcpServer(ctx: ToolContext): McpServer {
  const server = new McpServer(
    { name: "taskrunner", version: VERSION },
    { capabilities: {} },
  );

  /** Registers a tool with call auditing and uniform error mapping. */
  function tool<Shape extends z.ZodRawShape>(
    name: string,
    description: string,
    shape: Shape,
    handler: (args: z.objectOutputType<Shape, z.ZodTypeAny>) => Promise<string>,
  ): void {
    // Cast: ToolCallback<Shape> is a conditional type over an unresolved
    // generic here, which TS refuses to unify with a concrete function.
    const callback = async (args: z.objectOutputType<Shape, z.ZodTypeAny>): Promise<ToolResult> => {
      ctx.record({
        type: "audit.recorded",
        session_id: ctx.sessionId,
        kind: `tool.${name}`,
        payload: args,
      });
      try {
        return textResult(await handler(args));
      } catch (err) {
        return errorResult(err);
      }
    };
    server.registerTool(name, { description, inputSchema: shape }, callback as never);
  }

  tool(
    "assign-task",
    "Delegate a new task to a configured worker in an isolated task workspace. " +
      "Returns immediately with a running status unless wait is true; " +
      "retrieve results with lookup-task.",
    {
      project: z.string().describe("Absolute path of the project directory"),
      worker: z.string().describe("Configured worker capability, e.g. 'codex'"),
      prompt: z.string().describe("Delegated instruction text for the first turn"),
      wait: z
        .boolean()
        .optional()
        .describe("Block until the turn completes instead of returning immediately"),
      metadata: z
        .record(z.unknown())
        .optional()
        .describe("Optional caller identity and correlation data"),
    },
    async (args) =>
      renderOutcome(
        await ctx.scheduler.assignTask({
          project: args.project,
          worker: args.worker,
          prompt: args.prompt,
          sessionId: ctx.sessionId,
          wait: args.wait ?? false,
        }),
      ),
  );

  tool(
    "continue-task",
    "Send a follow-up prompt to an existing task, resuming its worker session. " +
      "Returns immediately unless wait is true. Returns a conflict error while " +
      "a turn is already running.",
    {
      task_id: z.string(),
      prompt: z.string().describe("Follow-up instruction text"),
      wait: z.boolean().optional(),
      metadata: z.record(z.unknown()).optional(),
    },
    async (args) =>
      renderOutcome(
        await ctx.scheduler.continueTask({
          task_id: args.task_id,
          prompt: args.prompt,
          wait: args.wait ?? false,
        }),
      ),
  );

  tool(
    "lookup-task",
    "Look up a delegated task: compact summary and its prompt/response exchanges.",
    {
      task_id: z.string(),
    },
    async (args) => {
      const snapshot = getTaskSnapshot(ctx.index, args.task_id);
      if (!snapshot) throw new ToolError("not_found", `no task ${args.task_id}`);
      return renderTaskDetail(snapshot, listTurns(ctx.index, args.task_id));
    },
  );

  tool(
    "cancel-task",
    "Cancel the running turn of a task. The audit trail and task workspace are preserved.",
    {
      task_id: z.string(),
      reason: z.string().optional().describe("Recorded in the audit trail"),
    },
    async (args) =>
      renderCancel(
        await ctx.scheduler.cancelTask({
          task_id: args.task_id,
          ...(args.reason ? { reason: args.reason } : {}),
        }),
      ),
  );

  return server;
}
