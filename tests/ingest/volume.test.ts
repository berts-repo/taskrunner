import { chmodSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { COPY_OUT_LABEL, reapCopyOutContainers } from "../../src/ingest/volume.js";
import { tempDir } from "../helpers.js";

interface FakeDocker {
  /** Path to pass as the `dockerCommand` seam. */
  path: string;
  /** Argv of each invocation, in order. */
  calls(): string[][];
}

/**
 * A stand-in `docker` that appends its argv to a log and replays a scripted
 * stdout for `ps`. Exercises the real spawn/exit-code plumbing, which is where
 * the reap's failure handling lives.
 */
function fakeDocker(psStdout: string, exitCode = 0): FakeDocker {
  const root = tempDir("reap");
  const log = join(root, "calls.jsonl");
  writeFileSync(log, "");
  const path = join(root, "docker");
  writeFileSync(
    path,
    [
      "#!/bin/sh",
      // Record argv one JSON array per line.
      `printf '%s\\n' "$(node -e 'console.log(JSON.stringify(process.argv.slice(1)))' "$@")" >> ${log}`,
      `if [ "$1" = "ps" ]; then printf '%s' '${psStdout}'; fi`,
      `exit ${exitCode}`,
    ].join("\n"),
  );
  chmodSync(path, 0o755);
  return {
    path,
    calls: () =>
      readFileSync(log, "utf8")
        .split("\n")
        .filter((l) => l.trim().length > 0)
        .map((l) => JSON.parse(l) as string[]),
  };
}

describe("reapCopyOutContainers", () => {
  it("removes every container carrying the copy-out label", async () => {
    const docker = fakeDocker("abc123\ndef456\n");
    expect(await reapCopyOutContainers(docker.path)).toBe(2);
    const calls = docker.calls();
    expect(calls[0]).toEqual(["ps", "-aq", "--filter", `label=${COPY_OUT_LABEL}`]);
    expect(calls[1]).toEqual(["rm", "-f", "abc123", "def456"]);
  });

  // The label filter is the whole safety story: without it the reap would be
  // an unfiltered `docker rm -f` against every container on the host.
  it("never issues an rm when nothing carries the label", async () => {
    const docker = fakeDocker("");
    expect(await reapCopyOutContainers(docker.path)).toBe(0);
    expect(docker.calls().map((c) => c[0])).toEqual(["ps"]);
  });

  it("ignores blank lines rather than passing an empty id to rm", async () => {
    const docker = fakeDocker("\n\n");
    expect(await reapCopyOutContainers(docker.path)).toBe(0);
    expect(docker.calls().map((c) => c[0])).toEqual(["ps"]);
  });

  it("rejects when docker fails so the caller can log and carry on", async () => {
    const docker = fakeDocker("", 1);
    await expect(reapCopyOutContainers(docker.path)).rejects.toThrow(/docker ps failed/);
  });

  it("rejects when docker is missing entirely", async () => {
    await expect(reapCopyOutContainers("/nonexistent/docker")).rejects.toThrow(/docker ps failed/);
  });
});
