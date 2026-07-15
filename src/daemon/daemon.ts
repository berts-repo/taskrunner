import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as http from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { loadConfig, type Config } from "../config.js";
import { newId } from "../ids.js";
import type { StatePaths } from "../paths.js";
import { ArtifactStore } from "../storage/artifacts.js";
import { EventLog, readEvents, type EventBody, type LogEvent } from "../storage/events.js";
import { rebuildIndex, type StateIndex } from "../storage/index.js";
import { VERSION } from "../version.js";
import { CodexHarness } from "../workers/codex.js";
import type { WorkerHarness } from "../workers/harness.js";
import { WorktreeWorkspaces } from "../workspace/worktree.js";
import { createMcpServer, type ToolContext } from "./mcp-server.js";
import { Scheduler, type WorkspaceProvider } from "./scheduler.js";

export interface DaemonOptions {
  /** Configured worker harnesses; tests may inject a fake. */
  harnesses?: Map<string, WorkerHarness>;
  workspaces?: WorkspaceProvider;
}

export class AlreadyRunningError extends Error {
  constructor(readonly pid: number) {
    super(`taskrunner daemon already running (pid ${pid})`);
  }
}

interface McpSession {
  transport: StreamableHTTPServerTransport;
  taskrunnerSessionId: string;
  started: boolean;
  ended: boolean;
}

/**
 * One daemon per state root owns the event log, index, and worker lifecycle
 * (PLAN § Process model). Single-writer is enforced by construction: all
 * durable writes go through `record`, and the lock file keeps a second
 * daemon out.
 */
export class Daemon {
  private readonly sessions = new Map<string, McpSession>();
  private stopped = false;

  private constructor(
    readonly paths: StatePaths,
    readonly config: Config,
    private readonly log: EventLog,
    readonly index: StateIndex,
    readonly artifacts: ArtifactStore,
    private readonly server: http.Server,
    readonly scheduler: Scheduler,
  ) {}

  static async start(paths: StatePaths, options: DaemonOptions = {}): Promise<Daemon> {
    // sun_path is ~104 bytes on macOS; fail with a clear message instead of
    // a bare EINVAL from listen().
    if (Buffer.byteLength(paths.socketPath) > 100) {
      throw new Error(
        `state root produces a unix socket path longer than the OS limit: ${paths.socketPath}`,
      );
    }
    fs.mkdirSync(paths.runtimeDir, { recursive: true });
    fs.mkdirSync(paths.logsDir, { recursive: true });
    acquireLock(paths);

    try {
      const log = EventLog.open(paths.eventsLog);
      const index = rebuildIndex(paths.indexDb, readEvents(paths.eventsLog));
      const config = loadConfig(paths.configFile);
      const artifacts = new ArtifactStore(paths.artifactsDir);
      const server = http.createServer();
      let record!: (body: EventBody) => LogEvent;
      const scheduler = new Scheduler({
        config,
        index,
        record: (body) => record(body),
        harnesses:
          options.harnesses ??
          new Map<string, WorkerHarness>([
            ["codex", new CodexHarness(config.worker.codex.command)],
          ]),
        workspaces:
          options.workspaces ??
          new WorktreeWorkspaces(paths.workspacesDir, artifacts, (body) => record(body)),
        artifacts,
      });
      const daemon = new Daemon(paths, config, log, index, artifacts, server, scheduler);
      record = (body) => daemon.record(body);

      daemon.recoverCrashedTurns();
      server.on("request", (req, res) => {
        daemon.handleRequest(req, res).catch(() => {
          if (!res.headersSent) res.writeHead(500);
          res.end();
        });
      });

      fs.rmSync(paths.socketPath, { force: true });
      await new Promise<void>((resolve, reject) => {
        server.once("error", reject);
        server.listen(paths.socketPath, () => resolve());
      });
      return daemon;
    } catch (err) {
      releaseLock(paths);
      throw err;
    }
  }

  /** The single durable write path: append to the log, fold into the index. */
  record(body: EventBody): LogEvent {
    const event = this.log.append(body);
    this.index.apply(event);
    return event;
  }

  /** Turns left `running` by a crash are failed with their audit retained. */
  private recoverCrashedTurns(): void {
    const orphans = this.index.db
      .prepare("SELECT id, task_id FROM turns WHERE status = 'running'")
      .all() as { id: string; task_id: string }[];
    for (const turn of orphans) {
      this.record({
        type: "turn.failed",
        turn_id: turn.id,
        task_id: turn.task_id,
        error_code: "worker_failed",
        error_message: "daemon restarted while the turn was running",
      });
    }
  }

  private async handleRequest(req: http.IncomingMessage, res: http.ServerResponse): Promise<void> {
    const url = new URL(req.url ?? "/", "http://taskrunner");
    if (url.pathname === "/status" && req.method === "GET") {
      this.handleStatus(res);
      return;
    }
    if (url.pathname === "/mcp") {
      await this.handleMcp(req, res);
      return;
    }
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not_found" }));
  }

  private handleStatus(res: http.ServerResponse): void {
    const taskCounts: Record<string, number> = {};
    const rows = this.index.db
      .prepare("SELECT status, COUNT(*) AS n FROM tasks GROUP BY status")
      .all() as { status: string; n: number }[];
    for (const row of rows) taskCounts[row.status] = row.n;
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        pid: process.pid,
        version: VERSION,
        state_root: this.paths.root,
        tasks: taskCounts,
        active_mcp_sessions: this.sessions.size,
      }),
    );
  }

  private async handleMcp(req: http.IncomingMessage, res: http.ServerResponse): Promise<void> {
    const sessionId = req.headers["mcp-session-id"];
    if (typeof sessionId === "string") {
      const session = this.sessions.get(sessionId);
      if (!session) {
        res.writeHead(404, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: "unknown mcp session" }));
        return;
      }
      await session.transport.handleRequest(req, res);
      return;
    }
    if (req.method !== "POST") {
      res.writeHead(405);
      res.end();
      return;
    }
    // New MCP session: each connecting shim/client gets its own server
    // instance and a durable Taskrunner session record.
    const session: McpSession = {
      transport: new StreamableHTTPServerTransport({
        sessionIdGenerator: () => randomUUID(),
        onsessioninitialized: (sid) => {
          this.sessions.set(sid, session);
        },
      }),
      taskrunnerSessionId: newId("sess"),
      started: false,
      ended: false,
    };
    const mcpServer = createMcpServer(this.toolContext(session.taskrunnerSessionId));
    mcpServer.server.oninitialized = () => {
      if (session.started) return;
      session.started = true;
      const client = mcpServer.server.getClientVersion();
      this.record({
        type: "session.started",
        session_id: session.taskrunnerSessionId,
        ...(client?.name ? { client: client.name } : {}),
      });
    };
    session.transport.onclose = () => {
      if (session.transport.sessionId) this.sessions.delete(session.transport.sessionId);
      if (session.started && !session.ended) {
        session.ended = true;
        this.record({ type: "session.ended", session_id: session.taskrunnerSessionId });
      }
    };
    await mcpServer.connect(session.transport);
    await session.transport.handleRequest(req, res);
  }

  private toolContext(sessionId: string): ToolContext {
    return {
      paths: this.paths,
      config: this.config,
      index: this.index,
      artifacts: this.artifacts,
      scheduler: this.scheduler,
      record: (body) => this.record(body),
      sessionId,
    };
  }

  async stop(): Promise<void> {
    if (this.stopped) return;
    this.stopped = true;
    // Cancel running turns first so their terminal events land in the log
    // before it closes.
    await this.scheduler.shutdown();
    for (const session of this.sessions.values()) {
      await session.transport.close().catch(() => {});
    }
    await new Promise<void>((resolve) => {
      this.server.close(() => resolve());
      this.server.closeAllConnections();
    });
    this.index.close();
    this.log.close();
    fs.rmSync(this.paths.socketPath, { force: true });
    releaseLock(this.paths);
  }
}

function acquireLock(paths: StatePaths): void {
  for (let attempt = 0; attempt < 2; attempt++) {
    // link() publishes the lock atomically WITH its pid content, so a racing
    // daemon can never observe an empty lock file and misjudge it stale.
    const tmp = `${paths.lockFile}.${process.pid}.tmp`;
    fs.writeFileSync(tmp, String(process.pid));
    try {
      fs.linkSync(tmp, paths.lockFile);
      fs.rmSync(tmp);
      fs.writeFileSync(paths.pidFile, String(process.pid));
      return;
    } catch (err) {
      fs.rmSync(tmp, { force: true });
      if ((err as NodeJS.ErrnoException).code !== "EEXIST") throw err;
      let holder = NaN;
      try {
        holder = Number(fs.readFileSync(paths.lockFile, "utf8").trim());
      } catch {
        // Lock vanished between link() and read; retry.
        continue;
      }
      if (holder && isProcessAlive(holder)) throw new AlreadyRunningError(holder);
      // Stale lock from a crashed daemon; clear it and retry once.
      fs.rmSync(paths.lockFile, { force: true });
      fs.rmSync(paths.pidFile, { force: true });
    }
  }
  throw new Error("could not acquire daemon lock");
}

/** Removes lock/pid files only if this process still owns them. */
function releaseLock(paths: StatePaths): void {
  for (const file of [paths.lockFile, paths.pidFile]) {
    try {
      if (Number(fs.readFileSync(file, "utf8").trim()) === process.pid) fs.rmSync(file);
    } catch {
      // Missing or unreadable: nothing of ours to release.
    }
  }
}

function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}
