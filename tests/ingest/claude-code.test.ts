import { describe, expect, it } from "vitest";
import { claudeCodeParser } from "../../src/ingest/claude-code.js";
import type { FileContext } from "../../src/ingest/parser.js";

function ctx(): FileContext {
  return { filePath: "/x/session.jsonl", lineIndex: 0 };
}

// Sample lines lifted from a real ~/.claude/projects/**/*.jsonl transcript
// (ids and text trimmed), plus the noise record types that share the file.
const USER = JSON.stringify({
  type: "user",
  uuid: "u1",
  sessionId: "s1",
  cwd: "/repo",
  timestamp: "2026-01-01T00:00:00Z",
  message: { role: "user", content: "hello there" },
});
const ASSISTANT = JSON.stringify({
  type: "assistant",
  uuid: "a1",
  sessionId: "s1",
  cwd: "/repo",
  timestamp: "2026-01-01T00:00:01Z",
  message: {
    role: "assistant",
    content: [
      { type: "thinking", thinking: "let me think", signature: "sig" },
      { type: "text", text: "on it" },
      { type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } },
    ],
  },
});
const TOOL_RESULT = JSON.stringify({
  type: "user",
  uuid: "u2",
  sessionId: "s1",
  message: {
    role: "user",
    content: [{ type: "tool_result", tool_use_id: "t1", content: "file.txt", is_error: false }],
  },
});

describe("claudeCodeParser", () => {
  it("parses a plain user message", () => {
    const out = claudeCodeParser.parse(USER, ctx());
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
    const out = claudeCodeParser.parse(ASSISTANT, ctx());
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
    const out = claudeCodeParser.parse(TOOL_RESULT, ctx());
    expect(out).toHaveLength(1);
    expect(out[0]!.role).toBe("tool");
    expect(out[0]!.kind).toBe("tool_result");
    expect(out[0]!.nativeRecordId).toBe("u2#0");
    expect(JSON.parse(out[0]!.content)).toMatchObject({ tool_use_id: "t1", content: "file.txt" });
  });

  it("skips noise and unknown record types", () => {
    for (const line of [
      JSON.stringify({ type: "queue-operation", operation: "enqueue" }),
      JSON.stringify({ type: "file-history-snapshot" }),
      JSON.stringify({ type: "attachment", uuid: "x" }),
      JSON.stringify({ type: "some-future-type", uuid: "y", sessionId: "s1" }),
      "not json at all",
      "",
    ]) {
      expect(claudeCodeParser.parse(line, ctx())).toEqual([]);
    }
  });

  it("skips empty (redacted) thinking blocks", () => {
    const line = JSON.stringify({
      type: "assistant",
      uuid: "a2",
      sessionId: "s1",
      message: { role: "assistant", content: [{ type: "thinking", thinking: "", signature: "s" }] },
    });
    expect(claudeCodeParser.parse(line, ctx())).toEqual([]);
  });

  it("carries session and cwd forward via context", () => {
    const c = ctx();
    claudeCodeParser.parse(USER, c);
    expect(c.sessionId).toBe("s1");
    expect(c.projectPath).toBe("/repo");
  });
});
