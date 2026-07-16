import { describe, expect, it } from "vitest";
import { parseConfig, workerConfig } from "../../src/config.js";
import { buildHarnesses } from "../../src/daemon/daemon.js";
import { ClaudeHarness } from "../../src/workers/claude.js";
import { CodexHarness } from "../../src/workers/codex.js";

// Workers are pluggable: a worker is a config entry, and the harness map is
// built from config alone.

describe("buildHarnesses", () => {
  it("always provides the built-in codex and claude workers", () => {
    const harnesses = buildHarnesses(parseConfig({}));
    expect(harnesses.get("codex")).toBeInstanceOf(CodexHarness);
    expect(harnesses.get("claude")).toBeInstanceOf(ClaudeHarness);
    expect(harnesses.size).toBe(2);
  });

  it("brings a custom worker to life from config alone", () => {
    const config = parseConfig({
      worker: {
        qwen: {
          harness: "codex",
          model: "qwen2.5-coder:32b",
          provider: "ollama",
          allowed_domains: ["host.docker.internal:11434"],
        },
      },
    });
    const harnesses = buildHarnesses(config);
    expect(harnesses.get("qwen")).toBeInstanceOf(CodexHarness);
    // The catchall keeps custom worker sections intact.
    const cfg = workerConfig(config, "qwen");
    expect(cfg.model).toBe("qwen2.5-coder:32b");
    expect(cfg.provider).toBe("ollama");
    expect(cfg.allowed_domains).toEqual(["host.docker.internal:11434"]);
    expect(cfg.auth_volume).toBeUndefined();
  });

  it("skips workers whose harness kind does not exist", () => {
    const config = parseConfig({ worker: { mystery: { model: "x" } } });
    const harnesses = buildHarnesses(config);
    expect(harnesses.has("mystery")).toBe(false);
  });

  it("rejects unknown harness kinds at config parse time", () => {
    expect(() =>
      parseConfig({ worker: { q: { harness: "not-a-harness" } } }),
    ).toThrow();
  });
});
