import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import type { Config } from "../config.js";
import { ToolError } from "../domain/errors.js";
import type { StatePaths } from "../paths.js";
import { renderCancel, renderOutcome } from "../render.js";
import { lookupSession, lookupTask, searchTranscripts } from "./lookup.js";
import type { SearchFilters } from "../domain/tasks.js";
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
  /** Records session.started once; safe to call on every tool dispatch. */
  ensureSessionStarted: () => void;
  /** Sweeps host transcript dirs on demand (skips the worker-volume copy-out),
   * so a session-recency query reflects the live conversation. Coalesces with
   * any in-flight sweep. */
  sweepHostTranscripts: () => Promise<unknown>;
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

/**
 * Server-level cheatsheet delivered to every client in the MCP handshake.
 * Generated from config so the advertised workers and their egress defaults
 * can never drift from what the daemon actually runs.
 */
function buildInstructions(config: Config): string {
  const workers = Object.entries(config.worker).map(([name, cfg]) => {
    const model = cfg.model ? ` (model: ${cfg.model})` : "";
    const domains = cfg.allowed_domains.length > 0 ? cfg.allowed_domains.join(", ") : "none";
    return `- ${name}${model} — default egress: ${domains}`;
  });
  return [
    "Taskrunner runs delegated coding tasks in isolated Docker containers, one workspace per task.",
    "",
    "Configured workers (the `worker` argument to assign-task):",
    ...workers,
    "",
    "Network access: a worker reaches only its default egress domains above. To grant more, " +
      'pass allowDomains on assign-task; the value "*" means the entire public internet. ' +
      "Loopback, LAN, and other private addresses stay blocked regardless. Any allowDomains " +
      "value requires the user's explicit yes in conversation, relayed via userApproved: true. " +
      "Every connection attempt a worker makes is audit-logged.",
    "",
    "Lifecycle: assign-task starts a task (wait: true blocks for the result); lookup-task " +
      "fetches status, output, and audit records; continue-task sends a follow-up prompt to " +
      "an existing task; cancel-task stops a running turn.",
    "",
    "Transcripts: worker turns and host agent sessions are archived. lookup-session " +
      "lists recent sessions (or one session's full history by id), including host " +
      'sessions no task links; lookup-task with include ["transcript"] returns one ' +
      "task's worker interior; search-transcripts full-text searches the whole ingested " +
      "corpus, optionally scoped to a project, sessions, the last N sessions, or a time window. " +
      'Both lookups return one compacted line per message; pass view "timeline" for the ' +
      "full audit rendering, and prompt N to read a single exchange.",
    "",
    "Worker credentials live in Docker volumes on this host. If a turn fails with a " +
      "login or auth error, the user must re-run the worker login procedure on the host " +
      "(documented in the taskrunner README); it cannot be fixed through these tools.",
  ].join("\n");
}

/**
 * How an archived transcript is rendered, shared by lookup-task and
 * lookup-session. The default stays compact: an agent scanning history should
 * pay for the timeline only when it asks for it.
 */
const VIEW_ARGS = {
  view: z
    .enum(["compact", "timeline"])
    .optional()
    .describe(
      "compact (default): one truncated line per message, for scanning. " +
        "timeline: the audit view — prompts, replies and reasoning in full, " +
        "tool bodies clipped to toolLines.",
    ),
  toolLines: z
    .number()
    .int()
    .min(0)
    .max(1000)
    .optional()
    .describe("timeline only: lines kept of each tool body; 0 keeps all (default 20)"),
  prompt: z
    .number()
    .int()
    .min(0)
    .optional()
    .describe(
      "Return only the exchange at this prompt index — one real user prompt and " +
        "everything that followed it. The timeline marks these as [N].",
    ),
};

/** Maps the wire name `prompt` onto the renderer's `promptIdx`. */
function viewArgs(args: {
  view?: "compact" | "timeline";
  toolLines?: number;
  prompt?: number;
}): { view?: "compact" | "timeline"; toolLines?: number; promptIdx?: number } {
  return {
    ...(args.view !== undefined ? { view: args.view } : {}),
    ...(args.toolLines !== undefined ? { toolLines: args.toolLines } : {}),
    ...(args.prompt !== undefined ? { promptIdx: args.prompt } : {}),
  };
}

export function createMcpServer(ctx: ToolContext): McpServer {
  const server = new McpServer(
    { name: "taskrunner", version: VERSION },
    { capabilities: {}, instructions: buildInstructions(ctx.config) },
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
      ctx.ensureSessionStarted();
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
      allowDomains: z
        .array(z.string())
        .optional()
        .describe(
          "Extra outbound domains the task may reach beyond the worker's API defaults " +
            "(e.g. registry.npmjs.org), or '*' for the full public internet (local and " +
            "private addresses stay blocked). Makes the task 'networked': you must ask " +
            "the user for permission and set userApproved.",
        ),
      userApproved: z
        .boolean()
        .optional()
        .describe(
          "Set true only after the user explicitly said yes to the extra network " +
            "access in this conversation; the approval is recorded as relayed by you.",
        ),
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
          ...(args.allowDomains ? { allowDomains: args.allowDomains } : {}),
          ...(args.userApproved !== undefined ? { userApproved: args.userApproved } : {}),
        }),
      ),
  );

  tool(
    "continue-task",
    "Send a follow-up prompt to an existing task, resuming its worker session. " +
      "Returns immediately unless wait is true. Returns a conflict error while " +
      "a turn is already running.",
    {
      taskId: z.string(),
      prompt: z.string().describe("Follow-up instruction text"),
      wait: z.boolean().optional(),
      metadata: z.record(z.unknown()).optional(),
    },
    async (args) =>
      renderOutcome(
        await ctx.scheduler.continueTask({
          task_id: args.taskId,
          prompt: args.prompt,
          wait: args.wait ?? false,
        }),
      ),
  );

  tool(
    "lookup-task",
    "Look up delegated tasks. Compact summary by default; expand with include " +
      "(turns = paired prompt/response exchanges, trace = end-to-end replay of " +
      "inputs/worker activity/outputs, audit, artifacts, diff, transcript = the " +
      "archived interior of the worker's own session). Scope narrows expansions to " +
      "one turn or the last N exchanges (for transcript, the last N messages). Pass " +
      "project instead of taskId to list a project's tasks.",
    {
      taskId: z.string().optional(),
      project: z
        .string()
        .optional()
        .describe("Absolute project path: list that project's tasks instead"),
      include: z
        .array(z.enum(["turns", "artifacts", "audit", "diff", "trace", "transcript"]))
        .optional(),
      scope: z
        .object({
          turnId: z.string().optional(),
          last: z.number().int().positive().optional().describe("Last N exchanges"),
        })
        .optional(),
      limit: z.number().int().positive().max(50).optional().describe("Max tasks to list"),
      ...VIEW_ARGS,
    },
    async (args) =>
      lookupTask({ index: ctx.index, artifacts: ctx.artifacts }, {
        ...(args as Parameters<typeof lookupTask>[1]),
        ...viewArgs(args),
      }),
  );

  tool(
    "lookup-session",
    "Browse ingested transcript sessions (one conversation = one Claude Code / " +
      "Codex session). With no sessionId: lists sessions most-recent first, " +
      "optionally filtered to a project. With a sessionId: returns that session's " +
      "full history in order — including host sessions no task links (unlike " +
      "lookup-task's transcript). scope.last caps the messages shown. Freshly " +
      "sweeps host transcripts first, so the newest session reflects the live " +
      "conversation up to its last flushed line.",
    {
      sessionId: z
        .string()
        .optional()
        .describe("Native session id to read; omit to list recent sessions"),
      project: z
        .string()
        .optional()
        .describe("Absolute project path filter (applies to the session list)"),
      source: z
        .string()
        .optional()
        .describe("Disambiguate a sessionId shared across sources, e.g. 'claude-code'"),
      limit: z.number().int().positive().max(100).optional().describe("Max sessions to list"),
      scope: z
        .object({ last: z.number().int().positive().optional().describe("Last N messages") })
        .optional(),
      ...VIEW_ARGS,
    },
    async (args) => {
      await ctx.sweepHostTranscripts();
      return lookupSession(ctx.index, { ...args, ...viewArgs(args) });
    },
  );

  tool(
    "search-transcripts",
    "Full-text search across ingested transcripts — the archived interior of " +
      "delegated worker turns and host agent sessions. Returns matching messages " +
      "with a snippet, project, and session, attributed to a task where the message " +
      "came from a linked worker session. Query uses SQLite FTS5 syntax: bare words " +
      'are ANDed, "quoted text" matches a phrase. Optional filters scope the search ' +
      "to a project, specific sessions, the last N sessions, a time window, or a " +
      "role/kind; sort by relevance (default) or recency.",
    {
      query: z.string().describe("FTS5 search expression"),
      project: z.string().optional().describe("Restrict to this absolute project path"),
      sessions: z
        .array(z.string())
        .optional()
        .describe("Restrict to these native session ids"),
      lastSessions: z
        .number()
        .int()
        .positive()
        .optional()
        .describe("Restrict to the most-recent N sessions (within project when set)"),
      since: z.string().optional().describe("Only messages with native_ts >= this ISO timestamp"),
      until: z.string().optional().describe("Only messages with native_ts <= this ISO timestamp"),
      role: z.string().optional().describe("Restrict to one role, e.g. 'user'"),
      kind: z
        .string()
        .optional()
        .describe("Restrict to one kind, e.g. 'message' | 'tool_use' | 'reasoning'"),
      sort: z
        .enum(["rank", "recent"])
        .optional()
        .describe("'rank' relevance (default) or 'recent' newest-first"),
      limit: z
        .number()
        .int()
        .positive()
        .max(50)
        .optional()
        .describe("Max hits to return (default 20)"),
    },
    async (args) => {
      const filters: SearchFilters = {
        ...(args.project !== undefined ? { project: args.project } : {}),
        ...(args.sessions !== undefined ? { sessions: args.sessions } : {}),
        ...(args.lastSessions !== undefined ? { lastSessions: args.lastSessions } : {}),
        ...(args.since !== undefined ? { since: args.since } : {}),
        ...(args.until !== undefined ? { until: args.until } : {}),
        ...(args.role !== undefined ? { role: args.role } : {}),
        ...(args.kind !== undefined ? { kind: args.kind } : {}),
        ...(args.sort !== undefined ? { sort: args.sort } : {}),
      };
      // lastSessions ranks by recency, so refresh host sessions first.
      if (args.lastSessions !== undefined) await ctx.sweepHostTranscripts();
      return searchTranscripts(ctx.index, args.query, args.limit ?? 20, filters);
    },
  );

  tool(
    "cancel-task",
    "Cancel the running turn of a task. The audit trail and task workspace are preserved.",
    {
      taskId: z.string(),
      reason: z.string().optional().describe("Recorded in the audit trail"),
    },
    async (args) =>
      renderCancel(
        await ctx.scheduler.cancelTask({
          task_id: args.taskId,
          ...(args.reason ? { reason: args.reason } : {}),
        }),
      ),
  );

  return server;
}
