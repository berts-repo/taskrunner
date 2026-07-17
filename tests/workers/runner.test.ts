import { describe, expect, it } from "vitest";
import { authMountArgs } from "../../src/workers/runner.js";

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
