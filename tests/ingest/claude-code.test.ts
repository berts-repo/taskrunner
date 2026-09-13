import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { claudeCodeParser } from "../../src/ingest/claude-code.js";
import type { FileContext } from "../../src/ingest/parser.js";
import { fixtureLines, fixturePath, parseFixture } from "../helpers.js";

function ctx(): FileContext {
  return { filePath: "/x/session.jsonl", lineIndex: 0 };
}

// Sample lines lifted from a real ~/.claude/projects/**/*.jsonl transcript
// (ids and text trimmed), plus the noise record types that share the file.
// The file is shared with the Rust port's tests, as is expected.json.
const FIXTURE = "claude-code/session.jsonl";
const [USER, ASSISTANT, TOOL_RESULT, ...REST] = fixtureLines(FIXTURE);
const EMPTY_THINKING = REST[4]!;
const NOISE = [REST[0]!, REST[1]!, REST[2]!, REST[3]!, REST[5]!, REST[6]!];

describe("claudeCodeParser", () => {
  it("parses the whole fixture to the messages expected.json records", () => {
    const expected = JSON.parse(readFileSync(fixturePath("claude-code/expected.json"), "utf8"));
    expect(parseFixture(claudeCodeParser, FIXTURE)).toEqual(expected);
  });

  it("parses a plain user message", () => {
    const out = claudeCodeParser.parse(USER!, ctx());
    expect(out).toEqual([
      {
        nativeSessionId: "s1",
        nativeRecordId: "u1",
        role: "user",
        kind: "message",
        content: "hello there",
        nativeTs: "2026-01-01T00:00:00Z",
        projectPath: "/repo",
      },
    ]);
  });

  it("expands assistant content blocks with per-block record ids", () => {
    const out = claudeCodeParser.parse(ASSISTANT!, ctx());
    expect(out.map((m) => [m.nativeRecordId, m.kind])).toEqual([
      ["a1#0", "reasoning"],
      ["a1#1", "message"],
      ["a1#2", "tool_use"],
    ]);
    expect(out[0]!.content).toBe("let me think");
    expect(JSON.parse(out[2]!.content)).toEqual({
      id: "t1",
      name: "Bash",
      input: { command: "ls" },
    });
  });

  it("maps tool_result blocks to a tool role", () => {
    const out = claudeCodeParser.parse(TOOL_RESULT!, ctx());
    expect(out).toHaveLength(1);
    expect(out[0]!.role).toBe("tool");
    expect(out[0]!.kind).toBe("tool_result");
    expect(out[0]!.nativeRecordId).toBe("u2#0");
    expect(JSON.parse(out[0]!.content)).toMatchObject({ tool_use_id: "t1", content: "file.txt" });
  });

  it("skips noise and unknown record types", () => {
    for (const line of NOISE) {
      expect(claudeCodeParser.parse(line, ctx()), line).toEqual([]);
    }
  });

  it("skips empty (redacted) thinking blocks", () => {
    expect(claudeCodeParser.parse(EMPTY_THINKING, ctx())).toEqual([]);
  });

  it("carries session and cwd forward via context", () => {
    const c = ctx();
    claudeCodeParser.parse(USER!, c);
    expect(c.sessionId).toBe("s1");
    expect(c.projectPath).toBe("/repo");
  });
});
