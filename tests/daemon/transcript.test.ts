import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { lookupSession, lookupTask, searchTranscripts } from "../../src/daemon/lookup.js";
import { getSessionMessages, listSessions, searchMessages } from "../../src/domain/tasks.js";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import type { EventBody, LogEvent } from "../../src/storage/events.js";
import { rebuildIndex, StateIndex } from "../../src/storage/index.js";
import { tempDir } from "../helpers.js";

// Phase 3: surfacing the ingested transcript corpus. Part A is the lookup-task
// `transcript` include field (one task's worker interior); Part B is the
// corpus-wide `search-transcripts` tool. Both read the same `messages` /
// `messages_fts` tables the ingest sweeper writes.

let clock = 0;
function ts(): string {
  clock += 1;
  return `2026-07-24T00:00:${String(clock).padStart(2, "0")}.000Z`;
}

/** A message.recorded body with sensible defaults; deterministic id per record. */
function message(over: Partial<Extract<EventBody, { type: "message.recorded" }>>): EventBody {
  const source = over.source ?? "codex";
  const native_session_id = over.native_session_id ?? "sess-A";
  const native_record_id = over.native_record_id ?? `rec-${clock}`;
  return {
    type: "message.recorded",
    message_id: over.message_id ?? `msg:${source}:${native_session_id}:${native_record_id}`,
    source,
    native_session_id,
    native_record_id,
    role: over.role ?? "assistant",
    kind: over.kind ?? "message",
    content: over.content ?? "",
    native_ts: over.native_ts,
    ...(over.project_path ? { project_path: over.project_path } : {}),
  } as EventBody;
}

/** A fresh index with a linked task and a handful of seeded transcript messages. */
function seededIndex(): { index: StateIndex; deps: { index: StateIndex; artifacts: ArtifactStore } } {
  const events: EventBody[] = [
    { type: "project.created", project_id: "p1", root: "/repo" },
    { type: "task.created", task_id: "t1", project_id: "p1", worker: "codex", prompt_summary: "build a widget" },
    { type: "turn.started", turn_id: "turn1", task_id: "t1", prompt: "build a widget" },
    { type: "worker-session.recorded", worker_session_id: "ws1", task_id: "t1", worker: "codex", native_session_id: "sess-A" },
    message({ role: "user", kind: "message", content: "please build the widget", native_ts: ts(), project_path: "/repo" }),
    message({
      role: "assistant",
      kind: "tool_use",
      content: JSON.stringify({ id: "tu1", name: "Bash", input: { command: "ls -la" } }),
      native_ts: ts(),
      project_path: "/repo",
    }),
    message({ role: "assistant", kind: "message", content: "done building the widget", native_ts: ts(), project_path: "/repo" }),
    // A host-session message: archived, matches search, but not linked to a task.
    message({
      source: "claude-code",
      native_session_id: "host-1",
      role: "user",
      kind: "message",
      content: "unrelated host chatter about a widget",
      native_ts: ts(),
      project_path: "/host",
    }),
  ];
  const index = rebuildIndex(":memory:", events.map(toLogEvent));
  const artifacts = new ArtifactStore(join(tempDir("transcript"), "artifacts"));
  return { index, deps: { index, artifacts } };
}

let evtSeq = 0;
function toLogEvent(body: EventBody): LogEvent {
  return { ...body, id: `evt-${evtSeq++}`, ts: ts() } as LogEvent;
}

describe("lookup-task include transcript (Part A)", () => {
  it("renders the task's worker transcript in order, compacting a tool payload", () => {
    const { deps } = seededIndex();
    const out = lookupTask(deps, { taskId: "t1", include: ["transcript"], view: "compact" });

    expect(out).toContain("transcript:");
    const userAt = out.indexOf("please build the widget");
    const toolAt = out.indexOf("assistant/tool_use");
    const doneAt = out.indexOf("done building the widget");
    expect(userAt).toBeGreaterThan(-1);
    expect(toolAt).toBeGreaterThan(-1);
    expect(doneAt).toBeGreaterThan(-1);
    // Chronological: user message, then tool_use, then final assistant message.
    expect(userAt).toBeLessThan(toolAt);
    expect(toolAt).toBeLessThan(doneAt);
    // The tool payload is compacted to a single line (no raw multi-line JSON).
    const toolLine = out.slice(toolAt, out.indexOf("\n", toolAt));
    expect(toolLine).not.toContain("\n");
    expect(toolLine).toContain("Bash");
  });

  it("excludes host-session messages not linked to the task", () => {
    const { deps } = seededIndex();
    const out = lookupTask(deps, { taskId: "t1", include: ["transcript"] });
    expect(out).not.toContain("unrelated host chatter");
  });

  it("outlines the worker interior when no view is asked for", () => {
    const { deps } = seededIndex();
    const out = lookupTask(deps, { taskId: "t1", include: ["transcript"] });
    expect(out).toContain("tool calls");
    expect(out).toContain("[1]");
    expect(out).toContain("Bash");
    // An outline names the call but never loads its payload or its result.
    expect(out).not.toContain("assistant/tool_use");
  });

  it("reports the empty state for a task with no transcript", () => {
    const events: EventBody[] = [
      { type: "project.created", project_id: "p1", root: "/repo" },
      { type: "task.created", task_id: "t2", project_id: "p1", worker: "codex", prompt_summary: "no session" },
    ];
    const index = rebuildIndex(":memory:", events.map(toLogEvent));
    const deps = { index, artifacts: new ArtifactStore(join(tempDir("transcript"), "artifacts")) };
    const out = lookupTask(deps, { taskId: "t2", include: ["transcript"] });
    expect(out).toContain("(no transcript recorded)");
  });

  it("honors scope.last as a cap, keeping the most recent messages", () => {
    const { deps } = seededIndex();
    const out = lookupTask(deps, { taskId: "t1", include: ["transcript"], scope: { last: 1 } });
    expect(out).toContain("done building the widget"); // newest kept
    expect(out).not.toContain("please build the widget"); // oldest dropped
    expect(out).toContain("capped at 1 messages");
  });
});

describe("search-transcripts (Part B)", () => {
  it("matches across the whole corpus and attributes worker hits to their task", () => {
    const { index } = seededIndex();
    const out = searchTranscripts(index, "widget", 20);
    expect(out).toContain("task t1"); // worker-session hit attributed to the task
    expect(out).toContain("claude-code session host-1"); // host hit, no task
  });

  it("does not attribute a host-session hit to any task", () => {
    const { index } = seededIndex();
    const out = searchTranscripts(index, "chatter", 20);
    expect(out).toContain("claude-code session host-1");
    expect(out).not.toContain("task t1");
  });

  it("bounds results by limit", () => {
    const { index } = seededIndex();
    const one = searchTranscripts(index, "widget", 1);
    expect(one).toContain("transcript matches (1)");
  });

  it("reports no matches cleanly", () => {
    const { index } = seededIndex();
    expect(searchTranscripts(index, "nonexistentterm", 20)).toContain("no transcript matches");
  });

  it("maps malformed FTS5 syntax to a client error, not a crash", () => {
    const { index } = seededIndex();
    expect(() => searchTranscripts(index, '"unbalanced', 20)).toThrowError(/invalid search query/);
  });

  it("indexes a re-swept message once (no double-count on rebuild)", () => {
    // Same deterministic message_id emitted twice, as a re-sweep would.
    const dup = message({
      native_session_id: "sess-A",
      native_record_id: "rec-dup",
      role: "assistant",
      kind: "message",
      content: "uniquephrase appears once",
      native_ts: ts(),
    });
    const events: EventBody[] = [
      { type: "project.created", project_id: "p1", root: "/repo" },
      { type: "task.created", task_id: "t1", project_id: "p1", worker: "codex", prompt_summary: "x" },
      { type: "worker-session.recorded", worker_session_id: "ws1", task_id: "t1", worker: "codex", native_session_id: "sess-A" },
      dup,
      dup, // duplicate event, identical message_id
    ];
    const index = rebuildIndex(":memory:", events.map(toLogEvent));
    const out = searchTranscripts(index, "uniquephrase", 20);
    expect(out).toContain("transcript matches (1)");
  });

  it("shows the project on each hit and filters by project", () => {
    const { index } = seededIndex();
    const all = searchTranscripts(index, "widget", 20);
    expect(all).toContain("/repo"); // worker hit's project
    expect(all).toContain("/host"); // host hit's project
    const scoped = searchTranscripts(index, "widget", 20, { project: "/host" });
    expect(scoped).toContain("host-1");
    expect(scoped).not.toContain("task t1"); // /repo worker hits excluded
  });

  it("scopes to specific sessions and to the last N sessions", () => {
    const { index } = seededIndex();
    // Only the host session id: the worker "widget" hits drop out.
    const bySession = searchMessages(index, "widget", 20, { sessions: ["host-1"] });
    expect(bySession.every((h) => h.native_session_id === "host-1")).toBe(true);
    // host-1 is the most recent session; last-1 keeps only it.
    const byLast = searchMessages(index, "widget", 20, { lastSessions: 1 });
    expect(byLast.every((h) => h.native_session_id === "host-1")).toBe(true);
  });

  it("filters by role and kind", () => {
    const { index } = seededIndex();
    const users = searchMessages(index, "widget", 20, { role: "user" });
    expect(users.length).toBeGreaterThan(0);
    expect(users.every((h) => h.role === "user")).toBe(true);
    const tools = searchMessages(index, "ls", 20, { kind: "tool_use" });
    expect(tools.every((h) => h.kind === "tool_use")).toBe(true);
  });

  it("returns no hits when scoped to a session set that has none", () => {
    const { index } = seededIndex();
    expect(searchMessages(index, "widget", 20, { sessions: [] })).toEqual([]);
  });
});

describe("lookup-session (Part C)", () => {
  it("lists ingested sessions most-recent first, with project and task link", () => {
    const { index } = seededIndex();
    const out = lookupSession(index, {});
    expect(out).toContain("sessions (2)");
    // host-1 was seeded last (newest), so it lists before sess-A.
    expect(out.indexOf("host-1")).toBeLessThan(out.indexOf("sess-A"));
    expect(out).toContain("[task t1]"); // sess-A is linked to a task
    expect(out).toContain("/repo");
    expect(out).toContain("/host");
  });

  it("filters the list by project", () => {
    const { index } = seededIndex();
    const out = lookupSession(index, { project: "/host" });
    expect(out).toContain("host-1");
    expect(out).not.toContain("sess-A");
  });

  it("reads a host session's full history in order (no task needed)", () => {
    const { index } = seededIndex();
    const out = lookupSession(index, { sessionId: "host-1" });
    expect(out).toContain("session host-1 (claude-code, /host)");
    expect(out).toContain("unrelated host chatter about a widget");
  });

  it("reads a worker session by id too, chronologically", () => {
    const { index } = seededIndex();
    const out = lookupSession(index, { sessionId: "sess-A" });
    const userAt = out.indexOf("please build the widget");
    const doneAt = out.indexOf("done building the widget");
    expect(userAt).toBeGreaterThan(-1);
    expect(userAt).toBeLessThan(doneAt);
  });

  it("caps a session read with scope.last, keeping the newest", () => {
    const { index } = seededIndex();
    const out = lookupSession(index, { sessionId: "sess-A", scope: { last: 1 } });
    expect(out).toContain("done building the widget");
    expect(out).not.toContain("please build the widget");
    expect(out).toContain("capped at 1 messages");
  });

  it("errors for an unknown session id", () => {
    const { index } = seededIndex();
    expect(() => lookupSession(index, { sessionId: "nope" })).toThrowError(/no ingested/);
  });

  it("lists candidates when a bare id spans multiple sources", () => {
    const events: EventBody[] = [
      { type: "project.created", project_id: "p1", root: "/repo" },
      message({ source: "codex", native_session_id: "dup-id", content: "from codex", native_ts: ts() }),
      message({ source: "claude-code", native_session_id: "dup-id", content: "from claude", native_ts: ts() }),
    ];
    const index = rebuildIndex(":memory:", events.map(toLogEvent));
    const out = lookupSession(index, { sessionId: "dup-id" });
    expect(out).toContain("matches 2 sources");
    expect(out).toContain("source=codex");
    expect(out).toContain("source=claude-code");
    // Disambiguating by source reads it.
    const one = lookupSession(index, { sessionId: "dup-id", source: "codex" });
    expect(one).toContain("from codex");
    expect(one).not.toContain("from claude");
  });
});

describe("transcript_sessions aggregate", () => {
  it("counts a session's messages and is idempotent across a re-sweep", () => {
    const dup = message({
      native_session_id: "sess-A",
      native_record_id: "rec-dup",
      content: "once",
      native_ts: ts(),
    });
    const events: EventBody[] = [
      { type: "project.created", project_id: "p1", root: "/repo" },
      message({ native_session_id: "sess-A", native_record_id: "r1", content: "a", native_ts: ts() }),
      dup,
      dup, // re-swept identical message: must not double-count
    ];
    const index = rebuildIndex(":memory:", events.map(toLogEvent));
    const [session] = listSessions(index, { nativeSessionId: "sess-A" });
    expect(session?.message_count).toBe(2);
  });

  it("is reconstructed identically by a rebuild from the log", () => {
    const events: EventBody[] = [
      { type: "project.created", project_id: "p1", root: "/repo" },
      message({ native_session_id: "sess-A", native_record_id: "r1", content: "a", native_ts: ts(), project_path: "/repo" }),
      message({ native_session_id: "sess-A", native_record_id: "r2", content: "b", native_ts: ts(), project_path: "/repo" }),
      message({ source: "claude-code", native_session_id: "host-1", native_record_id: "h1", content: "c", native_ts: ts(), project_path: "/host" }),
    ].map(toLogEvent);
    const a = rebuildIndex(":memory:", events);
    const b = rebuildIndex(":memory:", events);
    const norm = (idx: StateIndex) => JSON.stringify(listSessions(idx, {}));
    expect(norm(a)).toBe(norm(b));
  });

  it("getSessionMessages reads only the requested session", () => {
    const { index } = seededIndex();
    const { messages } = getSessionMessages(index, "claude-code", "host-1");
    expect(messages).toHaveLength(1);
    expect(messages[0]?.content).toContain("host chatter");
  });
});
