import { describe, expect, it } from "vitest";
import { lookupSession } from "../../src/daemon/lookup.js";
import { getSessionMessages } from "../../src/domain/tasks.js";
import type { EventBody, LogEvent } from "../../src/storage/events.js";
import { rebuildIndex, StateIndex } from "../../src/storage/index.js";

// Phase 2: the timeline view. The compact view is what every surface returned
// before this phase and must stay byte-identical; the timeline is the audit
// rendering — prose never clipped, tool bodies clipped by line count.

let clock = 0;
function ts(): string {
  clock += 1;
  const s = String(clock).padStart(6, "0");
  return `2026-07-25T00:00:00.${s.slice(-3)}Z`;
}

let seq = 0;
function msg(over: {
  role?: string;
  kind?: string;
  content: string;
  session?: string;
  source?: string;
}): EventBody {
  seq += 1;
  const native_session_id = over.session ?? "sess-T";
  const source = over.source ?? "claude-code";
  return {
    type: "message.recorded",
    message_id: `msg:${source}:${native_session_id}:r${seq}`,
    source,
    native_session_id,
    native_record_id: `r${seq}`,
    role: over.role ?? "assistant",
    kind: over.kind ?? "message",
    content: over.content,
    native_ts: ts(),
    project_path: "/repo",
  } as EventBody;
}

function indexOf(events: EventBody[]): StateIndex {
  let n = 0;
  return rebuildIndex(
    ":memory:",
    events.map((body) => ({ ...body, id: `evt-${n++}`, ts: ts() }) as LogEvent),
  );
}

const LONG_PROSE = `A reply well past the compact view's 160-character budget: ${"x".repeat(200)}`;
const THIRTY_LINES = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");

/** One session: two real prompts, a harness-written user record, tools, an error. */
function seeded(): StateIndex {
  return indexOf([
    { type: "project.created", project_id: "p1", root: "/repo" },
    msg({ role: "user", kind: "message", content: "first question" }),
    msg({ content: LONG_PROSE }),
    msg({
      kind: "tool_use",
      content: JSON.stringify({ id: "tu1", name: "Read", input: { file_path: "/repo/a.ts" } }),
    }),
    msg({
      role: "tool",
      kind: "tool_result",
      content: JSON.stringify({ tool_use_id: "tu1", is_error: false, content: THIRTY_LINES }),
    }),
    // Harness-written: must not advance the prompt counter.
    msg({ role: "user", kind: "message", content: "<system-reminder>ignore me</system-reminder>" }),
    msg({ role: "user", kind: "message", content: "second question" }),
    msg({
      kind: "tool_use",
      content: JSON.stringify({
        id: "tu2",
        name: "Bash",
        input: { command: "ls -la", description: "list the tree" },
      }),
    }),
    msg({
      role: "tool",
      kind: "tool_result",
      content: JSON.stringify({ tool_use_id: "tu2", is_error: true, content: "boom" }),
    }),
  ]);
}

describe("timeline view", () => {
  it("keeps prose whole where compact truncates it", () => {
    const index = seeded();
    const timeline = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(timeline).toContain(LONG_PROSE);

    const compact = lookupSession(index, { sessionId: "sess-T", view: "compact" });
    expect(compact).not.toContain(LONG_PROSE);
    expect(compact).toContain("…"); // truncation marker at 160 chars
  });

  it("still renders compact byte-for-byte when asked for it", () => {
    const index = seeded();
    // Phase 3 moved the default to the outline; scope.last still means compact,
    // so a caller that narrows a read gets the same messages it always did.
    expect(lookupSession(index, { sessionId: "sess-T", scope: { last: 99 } })).toBe(
      lookupSession(index, { sessionId: "sess-T", view: "compact", scope: { last: 99 } }),
    );
  });

  it("renders compact lines in the pre-phase-2 shape", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-T", view: "compact" });
    const line = out.split("\n").find((l) => l.includes("user/message"));
    expect(line).toMatch(/^ {2}2026-07-25T\S+ {2}user\/message {2}first question$/);
  });

  it("labels a tool call by name and shows its target and remaining input", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(out).toContain("── Claude · Read");
    expect(out).toContain("/repo/a.ts");
    expect(out).toContain("── Claude · Bash");
    expect(out).toContain("ls -la");
    expect(out).toContain("description: list the tree");
  });

  it("attributes replies to the harness that wrote each archived session", () => {
    for (const [source, name] of [
      ["claude-code", "Claude"],
      ["codex", "Codex"],
      ["hermes", "Hermes"],
      ["openclaw", "OpenClaw"],
    ]) {
      const index = indexOf([
        { type: "project.created", project_id: "p1", root: "/repo" },
        msg({ source, session: `${source}-session`, content: "a reply" }),
      ]);
      const out = lookupSession(index, { sessionId: `${source}-session`, view: "timeline" });
      expect(out).toContain(`── ${name}`);
      expect(out).not.toContain("── assistant");
    }
  });

  it("marks a failed tool result and not a successful one", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(out).toContain("── tool · result ✗");
    // The other result carries no ✗ between the label and its timestamp.
    expect(out.match(/── tool · result {2}2026/gm)?.length).toBe(1);
  });

  it("caps a tool body by line count, and 0 caps nothing", () => {
    const index = seeded();
    const capped = lookupSession(index, {
      sessionId: "sess-T",
      view: "timeline",
      toolLines: 20,
    });
    expect(capped).toContain("line 20");
    expect(capped).not.toContain("line 21");
    expect(capped).toContain("… 10 more lines");

    const whole = lookupSession(index, { sessionId: "sess-T", view: "timeline", toolLines: 0 });
    expect(whole).toContain("line 30");
    expect(whole).not.toContain("more lines");
  });

  it("addresses exchanges by prompt index, skipping harness-written records", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(out).toContain("── [1] user");
    expect(out).toContain("── [2] user");
    expect(out).not.toContain("[3]");

    const one = lookupSession(index, { sessionId: "sess-T", view: "timeline", promptIdx: 2 });
    expect(one).toContain("second question");
    expect(one).toContain("prompt 2");
    expect(one).not.toContain("first question");
    // The harness-written record sits in exchange 1, not 2.
    expect(one).not.toContain("ignore me");
  });

  it("clips a developer preamble but never the conversation", () => {
    const preamble = Array.from({ length: 40 }, (_, i) => `boilerplate ${i + 1}`).join("\n");
    const prose = Array.from({ length: 40 }, (_, i) => `said ${i + 1}`).join("\n");
    const index = indexOf([
      { type: "project.created", project_id: "p1", root: "/repo" },
      msg({ role: "developer", kind: "message", content: preamble }),
      msg({ role: "user", kind: "message", content: prose }),
      msg({ role: "assistant", kind: "message", content: prose }),
    ]);
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(out).toContain("boilerplate 20");
    expect(out).not.toContain("boilerplate 21");
    expect(out).toContain("said 40"); // both prose messages survive whole
    expect(out.match(/said 40/g)?.length).toBe(2);
  });

  it("reports an out-of-range prompt instead of an empty session", () => {
    const index = seeded();
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline", promptIdx: 99 });
    expect(out).toContain("(no messages at prompt 99)");
  });
});

describe("message limits", () => {
  const long = (): StateIndex => {
    const events: EventBody[] = [{ type: "project.created", project_id: "p1", root: "/repo" }];
    events.push(msg({ role: "user", kind: "message", content: "kick it off" }));
    for (let i = 0; i < 519; i++) events.push(msg({ content: `reply ${i}` }));
    return indexOf(events);
  };

  it("caps compact at 500 messages and leaves the timeline uncapped", () => {
    const index = long();
    const compact = getSessionMessages(index, "claude-code", "sess-T");
    expect(compact.messages).toHaveLength(500);
    expect(compact.capped).toBe(true);

    const timeline = getSessionMessages(index, "claude-code", "sess-T", { limit: null });
    expect(timeline.messages).toHaveLength(520);
    expect(timeline.capped).toBe(false);
  });

  it("honors an explicit last in either view", () => {
    const index = long();
    const { messages, capped } = getSessionMessages(index, "claude-code", "sess-T", { limit: 5 });
    expect(messages).toHaveLength(5);
    expect(capped).toBe(true);
    expect(messages.at(-1)?.content).toBe("reply 518");
  });

  it("reads the whole session in the timeline through lookup-session", () => {
    const index = long();
    const out = lookupSession(index, { sessionId: "sess-T", view: "timeline" });
    expect(out).toContain("reply 0"); // the oldest survives, uncapped
    expect(out).not.toContain("capped at");
  });
});
