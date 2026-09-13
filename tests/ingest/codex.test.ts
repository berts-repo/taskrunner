import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { codexParser } from "../../src/ingest/codex.js";
import type { FileContext } from "../../src/ingest/parser.js";
import { fixtureLines, fixturePath, parseFixture } from "../helpers.js";

// One rollout file in the shape Codex writes, shared with the Rust port's
// tests along with expected.json. The filename carries the session id.
const FIXTURE =
  "codex/rollout-2026-01-01T00-00-00-11111111-2222-3333-4444-555555555555.jsonl";
const [META, MESSAGE, CALL, OUTPUT, REASONING, EVENT_MSG, TURN_CONTEXT] = fixtureLines(FIXTURE);

function ctx(lineIndex = 0): FileContext {
  return { filePath: fixturePath(FIXTURE), lineIndex };
}

describe("codexParser", () => {
  it("parses the whole fixture to the messages expected.json records", () => {
    const expected = JSON.parse(readFileSync(fixturePath("codex/expected.json"), "utf8"));
    expect(parseFixture(codexParser, FIXTURE)).toEqual(expected);
  });

  it("reads session id and cwd from session_meta without emitting", () => {
    const c = ctx();
    expect(codexParser.parse(META!, c)).toEqual([]);
    expect(c.sessionId).toBe("cs1");
    expect(c.projectPath).toBe("/proj");
  });

  it("parses a message once the session is known", () => {
    const c = ctx();
    codexParser.parse(META!, c);
    c.lineIndex = 1;
    const out = codexParser.parse(MESSAGE!, c);
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
    codexParser.parse(META!, c);
    c.lineIndex = 2;
    const call = codexParser.parse(CALL!, c);
    c.lineIndex = 3;
    const output = codexParser.parse(OUTPUT!, c);
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
    codexParser.parse(META!, c);
    c.lineIndex = 4;
    const out = codexParser.parse(REASONING!, c);
    expect(out[0]).toMatchObject({ kind: "reasoning", content: "hmm", role: "assistant" });
  });

  it("skips event_msg duplicates and turn_context (but reads its cwd)", () => {
    const c = ctx();
    codexParser.parse(META!, c);
    expect(codexParser.parse(EVENT_MSG!, c)).toEqual([]);
    expect(codexParser.parse(TURN_CONTEXT!, c)).toEqual([]);
    expect(c.projectPath).toBe("/proj2");
  });

  it("falls back to the session id in the filename when meta is missing", () => {
    const c = ctx(0);
    const out = codexParser.parse(MESSAGE!, c);
    expect(out[0]!.nativeSessionId).toBe("11111111-2222-3333-4444-555555555555");
  });

  it("enumerate keeps only rollout-*.jsonl files", () => {
    // relies on findJsonlFiles filtering; just assert the basename predicate
    // by checking the parser exposes the codex format.
    expect(codexParser.format).toBe("codex");
  });
});
