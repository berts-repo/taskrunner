import { describe, expect, it } from "vitest";
import { lookupSession, searchTranscripts } from "../../src/daemon/lookup.js";
import { getSessionOutline } from "../../src/domain/tasks.js";
import type { EventBody, LogEvent } from "../../src/storage/events.js";
import { rebuildIndex, StateIndex } from "../../src/storage/index.js";

// Phase 3: the search-and-drill loop. The outline is an index of a session that
// never loads a body, search answers questions about tool calls without
// grepping JSON, and every result carries the prompt index it can be read at.

let clock = 0;
function ts(): string {
  clock += 1;
  const mm = String(Math.floor(clock / 60) % 60).padStart(2, "0");
  const ss = String(clock % 60).padStart(2, "0");
  return `2026-07-25T04:${mm}:${ss}.000Z`;
}

let seq = 0;
function msg(over: {
  role?: string;
  kind?: string;
  content: string;
  session?: string;
}): EventBody {
  seq += 1;
  const native_session_id = over.session ?? "sess-O";
  return {
    type: "message.recorded",
    message_id: `msg:claude-code:${native_session_id}:r${seq}`,
    source: "claude-code",
    native_session_id,
    native_record_id: `r${seq}`,
    role: over.role ?? "assistant",
    kind: over.kind ?? "message",
    content: over.content,
    native_ts: ts(),
    project_path: "/repo",
  } as EventBody;
}

function call(id: string, name: string, input: Record<string, unknown>, session?: string): EventBody {
  return msg({
    kind: "tool_use",
    content: JSON.stringify({ id, name, input }),
    ...(session !== undefined ? { session } : {}),
  });
}

function result(id: string, over: { is_error?: boolean; content?: string; session?: string } = {}): EventBody {
  return msg({
    role: "tool",
    kind: "tool_result",
    content: JSON.stringify({
      tool_use_id: id,
      ...(over.is_error !== undefined ? { is_error: over.is_error } : {}),
      content: over.content ?? "ok",
    }),
    ...(over.session !== undefined ? { session: over.session } : {}),
  });
}

function indexOf(events: EventBody[]): StateIndex {
  let n = 0;
  return rebuildIndex(
    ":memory:",
    events.map((body) => ({ ...body, id: `evt-${n++}`, ts: ts() }) as LogEvent),
  );
}

const HUGE = `a decision worth finding, followed by ${"padding ".repeat(400)}`.trimEnd();

/** Two exchanges: a clean read, then an edit whose command fails. */
function seeded(): StateIndex {
  return indexOf([
    { type: "project.created", project_id: "p1", root: "/repo" },
    msg({ role: "user", kind: "message", content: "why is the proxy dropping requests" }),
    msg({ kind: "reasoning", content: "thinking about the proxy" }),
    msg({ content: HUGE }),
    call("c1", "Read", { file_path: "/repo/src/shim/proxy.ts" }),
    result("c1", { content: "file contents" }),
    // Harness-written: neither a prompt nor an outline entry.
    msg({ role: "user", kind: "message", content: "<system-reminder>noise</system-reminder>" }),
    msg({ role: "user", kind: "message", content: "fix it then" }),
    call("c2", "Edit", { file_path: "/repo/src/shim/proxy.ts", old_string: "a", new_string: "b" }),
    result("c2"),
    call("c3", "Bash", { command: "npm test" }),
    result("c3", { is_error: true, content: "1 failing" }),
    msg({ content: "the timeout was unset" }),
  ]);
}

describe("outline", () => {
  it("indexes prompts, replies and calls without loading a body", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-O", view: "outline" });

    expect(out).toContain("12 messages · 2 prompts · 3 tool calls");
    expect(out).toContain("[1] 04:00  why is the proxy dropping requests");
    expect(out).toContain("[2] 04:00  fix it then");
    expect(out).toContain("Read          /repo/src/shim/proxy.ts");
    expect(out).toContain("Edit          /repo/src/shim/proxy.ts");
    // Bodies stay out: prose is truncated, tool inputs and results never appear.
    expect(out).not.toContain(HUGE);
    expect(out).toContain("→ a decision worth finding");
    expect(out).not.toContain("old_string");
    expect(out).not.toContain("file contents");
  });

  it("marks a failed call from the result it is paired with", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-O", view: "outline" });
    expect(out).toContain("Bash          npm test  ✗");
    expect(out).not.toContain("Edit          /repo/src/shim/proxy.ts  ✗");
  });

  it("leaves out reasoning and the records the harness wrote", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-O", view: "outline" });
    expect(out).not.toContain("thinking about the proxy");
    expect(out).not.toContain("noise");
  });

  it("costs a fraction of reading the session", () => {
    const index = seeded();
    const outline = lookupSession(index, { sessionId: "sess-O", view: "outline" });
    const timeline = lookupSession(index, { sessionId: "sess-O", view: "timeline" });
    expect(outline.length).toBeLessThan(timeline.length / 4);
  });

  it("degrades cleanly with no tool calls and with no prompt at all", () => {
    const bare = indexOf([
      { type: "project.created", project_id: "p1", root: "/repo" },
      msg({ role: "user", kind: "message", content: "just talk to me", session: "sess-Q" }),
      msg({ content: "talking", session: "sess-Q" }),
    ]);
    const out = lookupSession(bare, { sessionId: "sess-Q", view: "outline" });
    expect(out).toContain("2 messages · 1 prompts · 0 tool calls");
    expect(out).toContain("[1]");

    const headless = indexOf([
      { type: "project.created", project_id: "p1", root: "/repo" },
      msg({ content: "a session that opens mid-flight", session: "sess-R" }),
    ]);
    const out2 = lookupSession(headless, { sessionId: "sess-R", view: "outline" });
    expect(out2).toContain("[0]");
    expect(out2).toContain("(before the first prompt)");
  });

  it("reports an empty session rather than a bare counts line", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-O", view: "outline", promptIdx: 9 });
    expect(out).toContain("(no messages at prompt 9)");
  });

  it("never reads a message body out of the database", () => {
    const index = seeded();
    const { entries } = getSessionOutline(index, "claude-code", "sess-O");
    for (const e of entries) {
      if (e.kind === "tool_use") expect(e.head).toBeNull();
      // Tool results carry nothing an index needs; their failure state is on
      // the call, so they are not entries at all.
      expect(e.kind).not.toBe("tool_result");
    }
    expect(entries.some((e) => e.head !== null && e.head.length > 400)).toBe(false);
  });
});

describe("view resolution", () => {
  it("defaults to the outline", () => {
    const index = seeded();
    expect(lookupSession(index, { sessionId: "sess-O" })).toBe(
      lookupSession(index, { sessionId: "sess-O", view: "outline" }),
    );
  });

  it("reads the exchange in full when one is drilled into", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-O", promptIdx: 1 });
    expect(out).toBe(lookupSession(index, { sessionId: "sess-O", view: "timeline", promptIdx: 1 }));
    expect(out).toContain(HUGE); // the reply, whole
  });

  it("keeps a bounded read compact, as it was before the outline existed", () => {
    const index = seeded();
    expect(lookupSession(index, { sessionId: "sess-O", scope: { last: 2 } })).toBe(
      lookupSession(index, { sessionId: "sess-O", view: "compact", scope: { last: 2 } }),
    );
  });
});

describe("structured search", () => {
  /** The seeded session plus a second one that also touches proxy.ts. */
  function corpus(): StateIndex {
    return indexOf([
      { type: "project.created", project_id: "p1", root: "/repo" },
      msg({ role: "user", kind: "message", content: "why is the proxy dropping requests" }),
      call("c1", "Read", { file_path: "/repo/src/shim/proxy.ts" }),
      result("c1", { is_error: false }),
      call("c3", "Bash", { command: "npm test" }),
      result("c3", { is_error: true, content: "1 failing" }),
      msg({ role: "user", kind: "message", content: "now the other one", session: "sess-P" }),
      // These two results state no outcome — a rejection or an abort.
      call("c4", "Edit", { file_path: "/repo/src/shim/proxy.ts" }, "sess-P"),
      result("c4", { session: "sess-P" }),
      call("c5", "Edit", { file_path: "/repo/README.md" }, "sess-P"),
      result("c5", { session: "sess-P" }),
    ]);
  }

  it("answers which sessions touched a file, without a text query", () => {
    const out = searchTranscripts(corpus(), null, 20, { target: "shim/proxy.ts" });
    expect(out).toContain("transcript matches (2)");
    expect(out).toContain("sess-O");
    expect(out).toContain("sess-P");
    expect(out).not.toContain("README.md");
  });

  it("filters by tool, and combines tool with target", () => {
    const index = corpus();
    expect(searchTranscripts(index, null, 20, { tool: "Edit" })).toContain("README.md");
    const both = searchTranscripts(index, null, 20, { tool: "Edit", target: "proxy.ts" });
    expect(both).toContain("transcript matches (1)");
    expect(both).not.toContain("README.md");
  });

  it("finds a failed call by the state of its result", () => {
    const index = corpus();
    const failed = searchTranscripts(index, null, 20, { failed: true });
    // The call is the hit, not the result — so the tool and target are visible,
    // and one failure is reported once rather than twice for the pair.
    expect(failed).toContain("Bash  npm test  ✗");
    expect(failed).toContain("transcript matches (1)");

    const ok = searchTranscripts(index, null, 20, { failed: false });
    expect(ok).toContain("Read  /repo/src/shim/proxy.ts");
    expect(ok).toContain("transcript matches (1)");
  });

  it("leaves a call whose result states no outcome out of both sides", () => {
    const index = corpus();
    // The two Edits were neither confirmed nor reported failed: guessing either
    // way would be a false negative in an audit.
    expect(searchTranscripts(index, null, 20, { failed: true })).not.toContain("Edit");
    expect(searchTranscripts(index, null, 20, { failed: false })).not.toContain("Edit");
    expect(searchTranscripts(index, null, 20, { tool: "Edit" })).toContain(
      "transcript matches (2)",
    );
  });

  it("prints the prompt index a hit can be read at", () => {
    const out = searchTranscripts(corpus(), null, 20, { target: "proxy.ts" });
    expect(out).toMatch(/prompt 1/);
  });

  it("narrows a text search by tool facts", () => {
    const index = corpus();
    expect(searchTranscripts(index, "proxy", 20, { kind: "tool_use" })).toContain("matches");
    const scoped = searchTranscripts(index, "proxy", 20, { tool: "Edit" });
    expect(scoped).toContain("transcript matches (1)");
  });

  it("refuses a search with neither a query nor a filter", () => {
    expect(() => searchTranscripts(corpus(), null, 20, {})).toThrowError(/provide a query/);
  });

  it("names what it searched for when nothing matched", () => {
    const out = searchTranscripts(corpus(), null, 20, { tool: "Glob", target: "nope" });
    expect(out).toContain("no transcript matches for: tool Glob, target ~ nope");
  });
});
