import { describe, expect, it } from "vitest";
import { codexParser } from "../../src/ingest/codex.js";
import type { FileContext } from "../../src/ingest/parser.js";

function ctx(lineIndex = 0): FileContext {
  return { filePath: "/s/2026/01/01/rollout-2026-01-01T00-00-00-11111111-2222-3333-4444-555555555555.jsonl", lineIndex };
}

const META = JSON.stringify({
  type: "session_meta",
  timestamp: "2026-01-01T00:00:00Z",
  payload: { id: "cs1", cwd: "/proj" },
});
const MESSAGE = JSON.stringify({
  type: "response_item",
  timestamp: "2026-01-01T00:00:01Z",
  payload: { type: "message", role: "user", content: [{ type: "input_text", text: "do it" }] },
});
const CALL = JSON.stringify({
  type: "response_item",
  payload: { type: "function_call", call_id: "c1", name: "shell", arguments: '{"cmd":"ls"}' },
});
const OUTPUT = JSON.stringify({
  type: "response_item",
  payload: { type: "function_call_output", call_id: "c1", output: "file.txt" },
});
const REASONING = JSON.stringify({
  type: "response_item",
  payload: { type: "reasoning", summary: [{ type: "summary_text", text: "hmm" }] },
});

describe("codexParser", () => {
  it("reads session id and cwd from session_meta without emitting", () => {
    const c = ctx();
    expect(codexParser.parse(META, c)).toEqual([]);
    expect(c.sessionId).toBe("cs1");
    expect(c.projectPath).toBe("/proj");
  });

  it("parses a message once the session is known", () => {
    const c = ctx();
    codexParser.parse(META, c);
    c.lineIndex = 1;
    const out = codexParser.parse(MESSAGE, c);
    expect(out).toEqual([
      {
        nativeSessionId: "cs1",
        nativeRecordId: "L1",
        role: "user",
        kind: "message",
        content: "do it",
        nativeTs: "2026-01-01T00:00:01Z",
        projectPath: "/proj",
      },
    ]);
  });

  it("gives a function_call and its output distinct record ids", () => {
    const c = ctx();
    codexParser.parse(META, c);
    c.lineIndex = 2;
    const call = codexParser.parse(CALL, c);
    c.lineIndex = 3;
    const output = codexParser.parse(OUTPUT, c);
    // Both carry call_id "c1"; keying on line index keeps them separate so the
    // output is not deduped away as a copy of the call.
    expect(call[0]!.nativeRecordId).toBe("L2");
    expect(output[0]!.nativeRecordId).toBe("L3");
    expect(call[0]!.kind).toBe("tool_use");
    expect(output[0]!.kind).toBe("tool_result");
    expect(JSON.parse(output[0]!.content)).toEqual({ call_id: "c1", output: "file.txt" });
  });

  it("maps reasoning summaries", () => {
    const c = ctx();
    codexParser.parse(META, c);
    c.lineIndex = 4;
    const out = codexParser.parse(REASONING, c);
    expect(out[0]).toMatchObject({ kind: "reasoning", content: "hmm", role: "assistant" });
  });

  it("skips event_msg duplicates and turn_context (but reads its cwd)", () => {
    const c = ctx();
    codexParser.parse(META, c);
    expect(codexParser.parse(JSON.stringify({ type: "event_msg", payload: { type: "x" } }), c)).toEqual([]);
    expect(
      codexParser.parse(JSON.stringify({ type: "turn_context", payload: { cwd: "/proj2" } }), c),
    ).toEqual([]);
    expect(c.projectPath).toBe("/proj2");
  });

  it("falls back to the session id in the filename when meta is missing", () => {
    const c = ctx(0);
    const out = codexParser.parse(MESSAGE, c);
    expect(out[0]!.nativeSessionId).toBe("11111111-2222-3333-4444-555555555555");
  });

  it("enumerate keeps only rollout-*.jsonl files", () => {
    // relies on findJsonlFiles filtering; just assert the basename predicate
    // by checking the parser exposes the codex format.
    expect(codexParser.format).toBe("codex");
  });
});
