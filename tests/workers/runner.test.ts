import { describe, expect, it } from "vitest";
import { authMountArgs, resourceLimitArgs } from "../../src/workers/runner.js";

describe("authMountArgs", () => {
  it("mounts the volume root when no subpath is given", () => {
    expect(authMountArgs("taskrunner-codex-home", [{ containerPath: "/home/worker/.codex" }])).toEqual([
      "--mount",
      "type=volume,src=taskrunner-codex-home,dst=/home/worker/.codex",
    ]);
  });

  it("mounts only the named subpaths of the volume", () => {
    expect(
      authMountArgs("taskrunner-claude-home", [
        { containerPath: "/home/worker/.claude", subpath: ".claude" },
        { containerPath: "/home/worker/.claude.json", subpath: ".claude.json" },
      ]),
    ).toEqual([
      "--mount",
      "type=volume,src=taskrunner-claude-home,dst=/home/worker/.claude,volume-subpath=.claude",
      "--mount",
      "type=volume,src=taskrunner-claude-home,dst=/home/worker/.claude.json,volume-subpath=.claude.json",
    ]);
  });

  it("marks read-only mounts", () => {
    expect(
      authMountArgs("vol", [{ containerPath: "/x", subpath: "y", readOnly: true }]),
    ).toEqual(["--mount", "type=volume,src=vol,dst=/x,volume-subpath=y,readonly"]);
  });
});

describe("resourceLimitArgs", () => {
  it("emits the configured ceilings as docker flags", () => {
    expect(resourceLimitArgs({ memory: "4g", cpus: 2, pids: 512 })).toEqual([
      "--memory",
      "4g",
      "--cpus",
      "2",
      "--pids-limit",
      "512",
      "--security-opt",
      "no-new-privileges",
    ]);
  });

  it("passes through custom and fractional values", () => {
    expect(resourceLimitArgs({ memory: "512m", cpus: 1.5, pids: 128 })).toEqual([
      "--memory",
      "512m",
      "--cpus",
      "1.5",
      "--pids-limit",
      "128",
      "--security-opt",
      "no-new-privileges",
    ]);
  });

  it("always hardens with no-new-privileges regardless of the limits", () => {
    expect(resourceLimitArgs({ memory: "8g", cpus: 4, pids: 1024 })).toContain("no-new-privileges");
  });
});
