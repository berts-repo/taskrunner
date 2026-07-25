#!/usr/bin/env node
import * as fs from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import { Agent, fetch as undiciFetch } from "undici";
import { AlreadyRunningError, Daemon } from "./daemon/daemon.js";
import { runDoctor } from "./doctor.js";
import { statePaths, type StatePaths } from "./paths.js";
import { runShim } from "./shim/proxy.js";
import { VERSION } from "./version.js";

const USAGE = `Usage: taskrunner <command> [args] [--state-root <dir>]

Commands:
  up        Start the Taskrunner daemon in the foreground.
  down      Stop the running daemon.
  status    Report daemon status.
  doctor    Diagnose Docker, worker images/auth, and ingestion health.
  mcp       Run the stdio MCP shim (auto-starts the daemon).

Query (read the ingested corpus without an MCP session):
  sessions [--project P] [--limit N]
              List ingested transcript sessions, most recent first.
  session <id> [--source S] [--last N] [--prompt N]
               [--compact] [--tool-lines N]
              Print one session's timeline (host or worker session): prompts,
              replies and reasoning in full, tool output capped at 20 lines
              (--tool-lines 0 for all). --prompt N prints one exchange;
              --compact restores one truncated line per message. Pipe to less.
  search "<fts>" [--project P] [--sessions a,b] [--last-sessions N]
                 [--role R] [--kind K] [--since T] [--until T]
                 [--sort rank|recent] [--limit N]
              Full-text search across transcripts.
  task <id> [--include turns,trace,audit,artifacts,diff,transcript]
            [--turn <turnId>] [--last N] [--prompt N]
            [--compact] [--tool-lines N]
              Look up one task; tasks --project P lists a project's tasks.
              --include transcript prints the worker's interior as a timeline,
              with the same rendering flags as session.
`;

interface Args {
  command: string | undefined;
  /** Positional arguments after the command (e.g. session <id>). */
  rest: string[];
  /** `--key value` options after the command. */
  flags: Record<string, string>;
  paths: StatePaths;
}

/** Flags that stand alone; every other `--x` takes the next argv entry. */
const BOOLEAN_FLAGS = new Set(["compact"]);

function parseArgs(argv: string[]): Args {
  let command: string | undefined;
  const rest: string[] = [];
  const flags: Record<string, string> = {};
  let root = process.env["TASKRUNNER_STATE_ROOT"];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i] as string;
    if (arg === "--state-root") {
      root = argv[++i];
      if (!root) throw new Error("--state-root requires a directory argument");
    } else if (arg.startsWith("--") && BOOLEAN_FLAGS.has(arg.slice(2))) {
      flags[arg.slice(2)] = "true";
    } else if (arg.startsWith("--")) {
      const value = argv[++i];
      if (value === undefined) throw new Error(`${arg} requires a value`);
      flags[arg.slice(2)] = value;
    } else if (command === undefined) {
      command = arg;
    } else {
      rest.push(arg);
    }
  }
  return { command, rest, flags, paths: root ? statePaths(root) : statePaths() };
}

/**
 * Fetches a read-only query route from the daemon over the control socket and
 * prints the plain-text body. Mirrors `status`: a down daemon is a soft failure.
 */
async function readQuery(paths: StatePaths, path: string, params: Record<string, string | undefined>): Promise<number> {
  const qs = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== "") qs.set(key, value);
  }
  const agent = new Agent({ connect: { socketPath: paths.socketPath } });
  try {
    const res = await undiciFetch(`http://taskrunner${path}?${qs.toString()}`, {
      dispatcher: agent,
      signal: AbortSignal.timeout(30_000),
    });
    const text = await res.text();
    process.stdout.write(text.endsWith("\n") ? text : text + "\n");
    return res.ok ? 0 : 1;
  } catch {
    process.stdout.write("taskrunner daemon is not running\n");
    return 1;
  } finally {
    await agent.close();
  }
}

/**
 * Transcript rendering params for the query routes. The terminal defaults to
 * the timeline — an audit view is what a person at a shell wants — while the
 * routes themselves keep defaulting to compact for the MCP tools.
 */
function renderFlags(flags: Record<string, string>): Record<string, string | undefined> {
  return {
    view: flags["compact"] ? "compact" : "timeline",
    toolLines: flags["tool-lines"],
    prompt: flags["prompt"],
  };
}

async function up(paths: StatePaths): Promise<number> {
  let daemon: Daemon;
  try {
    daemon = await Daemon.start(paths);
  } catch (err) {
    if (err instanceof AlreadyRunningError) {
      process.stderr.write(`${err.message}\n`);
      return 2;
    }
    throw err;
  }
  process.stdout.write(`taskrunner daemon ${VERSION} listening on ${paths.socketPath}\n`);
  await new Promise<void>((resolve) => {
    const onSignal = () => {
      daemon.stop().finally(resolve);
    };
    process.once("SIGINT", onSignal);
    process.once("SIGTERM", onSignal);
  });
  return 0;
}

async function down(paths: StatePaths): Promise<number> {
  let pid: number;
  try {
    pid = Number(fs.readFileSync(paths.pidFile, "utf8").trim());
  } catch {
    process.stdout.write("taskrunner daemon is not running\n");
    return 0;
  }
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    process.stdout.write("taskrunner daemon is not running (stale pid file)\n");
    return 0;
  }
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch {
      process.stdout.write(`taskrunner daemon stopped (pid ${pid})\n`);
      return 0;
    }
    await sleep(50);
  }
  process.stderr.write(`taskrunner daemon (pid ${pid}) did not stop within 5s\n`);
  return 1;
}

async function status(paths: StatePaths): Promise<number> {
  const agent = new Agent({ connect: { socketPath: paths.socketPath } });
  try {
    const res = await undiciFetch("http://taskrunner/status", {
      dispatcher: agent,
      signal: AbortSignal.timeout(2000),
    });
    const body = (await res.json()) as {
      pid: number;
      version: string;
      state_root: string;
      tasks: Record<string, number>;
      active_mcp_sessions: number;
    };
    process.stdout.write(
      `taskrunner daemon ${body.version} running (pid ${body.pid})\n` +
        `state root: ${body.state_root}\n` +
        `active mcp sessions: ${body.active_mcp_sessions}\n` +
        `tasks: ${
          Object.entries(body.tasks)
            .map(([status, n]) => `${status}=${n}`)
            .join(" ") || "none"
        }\n`,
    );
    return 0;
  } catch {
    process.stdout.write("taskrunner daemon is not running\n");
    return 1;
  } finally {
    await agent.close();
  }
}

async function main(argv: string[]): Promise<number> {
  let args: Args;
  try {
    args = parseArgs(argv);
  } catch (err) {
    process.stderr.write(`taskrunner: ${(err as Error).message}\n\n${USAGE}`);
    return 1;
  }
  switch (args.command) {
    case "up":
      return up(args.paths);
    case "down":
      return down(args.paths);
    case "status":
      return status(args.paths);
    case "doctor":
      return runDoctor(args.paths);
    case "mcp":
      await runShim(args.paths);
      // The shim owns the process from here; it exits via its own shutdown.
      return await new Promise<never>(() => {});
    case "sessions":
      return readQuery(args.paths, "/lookup-session", {
        project: args.flags["project"],
        limit: args.flags["limit"],
      });
    case "session": {
      const id = args.rest[0];
      if (!id) {
        process.stderr.write("taskrunner: session <id> requires a session id\n");
        return 1;
      }
      return readQuery(args.paths, "/lookup-session", {
        sessionId: id,
        source: args.flags["source"],
        last: args.flags["last"],
        ...renderFlags(args.flags),
      });
    }
    case "search": {
      const query = args.rest[0];
      if (!query) {
        process.stderr.write('taskrunner: search "<fts>" requires a query\n');
        return 1;
      }
      return readQuery(args.paths, "/search-transcripts", {
        query,
        project: args.flags["project"],
        sessions: args.flags["sessions"],
        lastSessions: args.flags["last-sessions"],
        role: args.flags["role"],
        kind: args.flags["kind"],
        since: args.flags["since"],
        until: args.flags["until"],
        sort: args.flags["sort"],
        limit: args.flags["limit"],
      });
    }
    case "task": {
      const id = args.rest[0];
      if (!id) {
        process.stderr.write("taskrunner: task <id> requires a task id\n");
        return 1;
      }
      return readQuery(args.paths, "/lookup-task", {
        taskId: id,
        include: args.flags["include"],
        turnId: args.flags["turn"],
        last: args.flags["last"],
        ...renderFlags(args.flags),
      });
    }
    case "tasks":
      return readQuery(args.paths, "/lookup-task", {
        project: args.flags["project"],
        limit: args.flags["limit"],
      });
    case undefined:
    case "help":
    case "--help":
    case "-h":
      process.stdout.write(USAGE);
      return args.command === undefined ? 1 : 0;
    default:
      process.stderr.write(`taskrunner: unknown command '${args.command}'\n\n${USAGE}`);
      return 1;
  }
}

// A timeline is long enough to be read through a pager, and `less`/`head`
// closing the pipe first must end the process quietly, not raise EPIPE.
process.stdout.on("error", (err: NodeJS.ErrnoException) => {
  if (err.code === "EPIPE") process.exit(0);
  throw err;
});

process.exitCode = await main(process.argv.slice(2));
