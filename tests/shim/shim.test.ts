import { readFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { afterEach, describe, expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { statePaths } from "../../src/paths.js";
import { tempDir } from "../helpers.js";

const PROJECT_ROOT = join(import.meta.dirname, "..", "..");
const TSX_CLI = join(PROJECT_ROOT, "node_modules", "tsx", "dist", "cli.mjs");

function shimTransport(root: string): StdioClientTransport {
  return new StdioClientTransport({
    command: process.execPath,
    args: [TSX_CLI, join(PROJECT_ROOT, "src", "cli.ts"), "mcp", "--state-root", root],
    cwd: PROJECT_ROOT,
    stderr: "pipe",
  });
}

let daemonPid: number | undefined;

afterEach(() => {
  if (daemonPid) {
    try {
      process.kill(daemonPid, "SIGTERM");
    } catch {
      // already gone
    }
    daemonPid = undefined;
  }
});

describe("taskrunner mcp shim", () => {
  it("auto-starts one daemon even when two shims race, and both connect", async () => {
    const paths = statePaths(tempDir("shim"));

    const clientA = new Client({ name: "shim-a", version: "0.0.1" });
    const clientB = new Client({ name: "shim-b", version: "0.0.1" });
    await Promise.all([
      clientA.connect(shimTransport(paths.root)),
      clientB.connect(shimTransport(paths.root)),
    ]);
    await Promise.all([clientA.ping(), clientB.ping()]);

    daemonPid = Number(readFileSync(paths.pidFile, "utf8").trim());
    expect(daemonPid).toBeGreaterThan(0);
    // Both shims proxied to the same daemon; pid file is stable.
    await sleep(200);
    expect(Number(readFileSync(paths.pidFile, "utf8").trim())).toBe(daemonPid);

    await clientA.close();
    await clientB.close();

    // The daemon outlives its shims.
    expect(() => process.kill(daemonPid!, 0)).not.toThrow();
  }, 30_000);
});
