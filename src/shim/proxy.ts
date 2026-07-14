import { spawn } from "node:child_process";
import * as fs from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { Agent, fetch as undiciFetch } from "undici";
import type { StatePaths } from "../paths.js";

// Thin stdio shim (PLAN § Process model): bridges MCP JSON-RPC between the
// client's stdio and the daemon's streamable HTTP endpoint on the unix
// socket, starting the daemon on demand. It never interprets messages.

function unixFetch(paths: StatePaths): typeof fetch {
  const agent = new Agent({ connect: { socketPath: paths.socketPath } });
  return ((input: string | URL, init?: RequestInit) =>
    undiciFetch(input as string, {
      ...(init as object),
      dispatcher: agent,
    })) as unknown as typeof fetch;
}

async function daemonIsUp(fetcher: typeof fetch): Promise<boolean> {
  try {
    const res = await fetcher("http://taskrunner/status", {
      signal: AbortSignal.timeout(1000),
    });
    return res.ok;
  } catch {
    return false;
  }
}

/**
 * Ensures a daemon is serving on the socket, spawning `taskrunner up`
 * detached if needed. Losing an auto-start race is fine: the loser's `up`
 * exits on the lock, and both shims connect to the winner.
 */
export async function ensureDaemon(paths: StatePaths): Promise<void> {
  const fetcher = unixFetch(paths);
  if (await daemonIsUp(fetcher)) return;

  fs.mkdirSync(paths.logsDir, { recursive: true });
  const logFd = fs.openSync(join(paths.logsDir, "daemon.log"), "a");
  const cliEntry = process.argv[1];
  if (!cliEntry) throw new Error("cannot determine taskrunner CLI entry point");
  const child = spawn(
    process.execPath,
    [...process.execArgv, cliEntry, "up", "--state-root", paths.root],
    { detached: true, stdio: ["ignore", logFd, logFd] },
  );
  child.unref();
  fs.closeSync(logFd);

  const deadline = Date.now() + 10_000;
  let delay = 50;
  while (Date.now() < deadline) {
    if (await daemonIsUp(fetcher)) return;
    await sleep(delay);
    delay = Math.min(delay * 2, 500);
  }
  throw new Error(`taskrunner daemon did not become ready on ${paths.socketPath}`);
}

export async function runShim(paths: StatePaths): Promise<void> {
  await ensureDaemon(paths);

  const upstream = new StreamableHTTPClientTransport(new URL("http://taskrunner/mcp"), {
    fetch: unixFetch(paths),
  });
  const downstream = new StdioServerTransport();

  let closing = false;
  const shutdown = async (code: number) => {
    if (closing) return;
    closing = true;
    await upstream.terminateSession().catch(() => {});
    await upstream.close().catch(() => {});
    await downstream.close().catch(() => {});
    process.exit(code);
  };

  downstream.onmessage = (message) => {
    upstream.send(message).catch((err) => {
      process.stderr.write(`taskrunner mcp: daemon send failed: ${String(err)}\n`);
      void shutdown(1);
    });
  };
  upstream.onmessage = (message) => {
    downstream.send(message).catch(() => void shutdown(1));
  };
  upstream.onerror = (err) => {
    process.stderr.write(`taskrunner mcp: daemon connection error: ${String(err)}\n`);
  };
  // Client hung up (stdin closed): end the MCP session at the daemon too.
  downstream.onclose = () => void shutdown(0);
  upstream.onclose = () => void shutdown(0);

  await upstream.start();
  await downstream.start();
}
