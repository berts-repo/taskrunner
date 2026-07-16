#!/usr/bin/env node
import * as fs from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import { Agent, fetch as undiciFetch } from "undici";
import { AlreadyRunningError, Daemon } from "./daemon/daemon.js";
import { statePaths, type StatePaths } from "./paths.js";
import { runShim } from "./shim/proxy.js";
import { VERSION } from "./version.js";

const USAGE = `Usage: taskrunner <command> [--state-root <dir>]

Commands:
  up                 Start the Taskrunner daemon in the foreground.
  down               Stop the running daemon.
  status             Report daemon status.
  approve <task_id>  Approve a task that waits for a human decision.
  deny <task_id>     Deny a waiting task; it never runs.
  mcp                Run the stdio MCP shim (auto-starts the daemon).
`;

interface Args {
  command: string | undefined;
  taskId: string | undefined;
  paths: StatePaths;
}

function parseArgs(argv: string[]): Args {
  let command: string | undefined;
  let taskId: string | undefined;
  let root = process.env["TASKRUNNER_STATE_ROOT"];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--state-root") {
      root = argv[++i];
      if (!root) throw new Error("--state-root requires a directory argument");
    } else if (command === undefined) {
      command = arg;
    } else if ((command === "approve" || command === "deny") && taskId === undefined) {
      taskId = arg;
    } else {
      throw new Error(`unexpected argument '${arg}'`);
    }
  }
  return { command, taskId, paths: root ? statePaths(root) : statePaths() };
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

async function decide(
  paths: StatePaths,
  decision: "approve" | "deny",
  taskId: string | undefined,
): Promise<number> {
  if (!taskId) {
    process.stderr.write(`taskrunner: ${decision} requires a task id\n\n${USAGE}`);
    return 1;
  }
  const agent = new Agent({ connect: { socketPath: paths.socketPath } });
  try {
    const res = await undiciFetch(`http://taskrunner/${decision}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ task_id: taskId }),
      dispatcher: agent,
      signal: AbortSignal.timeout(10_000),
    });
    const body = (await res.json()) as {
      error?: string;
      message?: string;
      status?: string;
      approval_state?: string;
      turn_id?: string | null;
    };
    if (!res.ok) {
      process.stderr.write(`taskrunner: ${body.error ?? res.status}: ${body.message ?? ""}\n`);
      return 1;
    }
    process.stdout.write(
      `task ${taskId} ${body.approval_state}` +
        (body.turn_id ? `; turn ${body.turn_id} started (status: ${body.status})\n` : "\n"),
    );
    return 0;
  } catch {
    process.stderr.write("taskrunner daemon is not running\n");
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
    case "approve":
    case "deny":
      return decide(args.paths, args.command, args.taskId);
    case "mcp":
      await runShim(args.paths);
      // The shim owns the process from here; it exits via its own shutdown.
      return await new Promise<never>(() => {});
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

process.exitCode = await main(process.argv.slice(2));
