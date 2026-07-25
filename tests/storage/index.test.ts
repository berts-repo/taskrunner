import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { EventBody, LogEvent } from "../../src/storage/events.js";
import { rebuildIndex, StateIndex } from "../../src/storage/index.js";
import { evt, sampleSequence, tempDir } from "../helpers.js";

const TABLES = [
  "projects",
  "project_aliases",
  "mcp_sessions",
  "tasks",
  "turns",
  "worker_sessions",
  "messages",
  "transcript_sessions",
  "audit_events",
  "artifacts",
  "artifact_links",
];

function dump(index: StateIndex): Record<string, unknown[]> {
  const out: Record<string, unknown[]> = {};
  for (const table of TABLES) {
    out[table] = index.db.prepare(`SELECT * FROM ${table} ORDER BY 1`).all();
  }
  return out;
}

describe("StateIndex", () => {
  it("folds the sample sequence into consistent rows", () => {
    const index = new StateIndex(":memory:");
    for (const event of sampleSequence()) index.apply(event);

    const task = index.db.prepare("SELECT * FROM tasks WHERE id = 'task_a'").get() as any;
    expect(task.status).toBe("completed");
    expect(task.project_id).toBe("proj_a");
    expect(task.session_id).toBe("sess_a");

    const turn = index.db.prepare("SELECT * FROM turns WHERE id = 'turn_a1'").get() as any;
    expect(turn.status).toBe("completed");
    expect(turn.idx).toBe(0);
    expect(turn.response).toBe("created hello.txt");
    expect(JSON.parse(turn.changed_files)).toEqual(["hello.txt"]);
    expect(turn.completed_at).not.toBeNull();

    const session = index.db
      .prepare("SELECT * FROM mcp_sessions WHERE id = 'sess_a'")
      .get() as any;
    expect(session.ended_at).not.toBeNull();

    const wsess = index.db.prepare("SELECT * FROM worker_sessions").get() as any;
    expect(wsess.native_session_id).toBe("019e28f6-9f73-73d0-b601-33505b06d3f5");

    const audit = index.db.prepare("SELECT * FROM audit_events").all();
    expect(audit).toHaveLength(1);
    const links = index.db.prepare("SELECT * FROM artifact_links").all();
    expect(links).toHaveLength(1);
    index.close();
  });

  it("tracks failure and cancellation statuses with turn indexes", () => {
    const index = new StateIndex(":memory:");
    const events = [
      evt({ type: "project.created", project_id: "proj_b", root: "/b" }),
      evt({
        type: "task.created",
        task_id: "task_b",
        project_id: "proj_b",
        worker: "codex",
        prompt_summary: "x",
      }),
      evt({ type: "turn.started", turn_id: "turn_b1", task_id: "task_b", prompt: "one" }),
      evt({
        type: "turn.failed",
        turn_id: "turn_b1",
        task_id: "task_b",
        error_code: "worker_failed",
        error_message: "timeout after 1800s",
      }),
      evt({ type: "turn.started", turn_id: "turn_b2", task_id: "task_b", prompt: "two" }),
      evt({ type: "turn.canceled", turn_id: "turn_b2", task_id: "task_b", reason: "user request" }),
    ];
    for (const event of events) index.apply(event);

    const turns = index.db
      .prepare("SELECT id, idx, status, error_code FROM turns ORDER BY idx")
      .all() as any[];
    expect(turns).toEqual([
      { id: "turn_b1", idx: 0, status: "failed", error_code: "worker_failed" },
      { id: "turn_b2", idx: 1, status: "canceled", error_code: null },
    ]);
    const task = index.db.prepare("SELECT status FROM tasks WHERE id = 'task_b'").get() as any;
    expect(task.status).toBe("canceled");
    index.close();
  });

  it("is idempotent when the same events are applied twice", () => {
    const index = new StateIndex(":memory:");
    const events = sampleSequence();
    for (const event of events) index.apply(event);
    const once = dump(index);
    for (const event of events) index.apply(event);
    expect(dump(index)).toEqual(once);
    index.close();
  });

  it("rebuild equals incremental application", () => {
    const events = sampleSequence();
    const incremental = new StateIndex(":memory:");
    for (const event of events) incremental.apply(event);

    const rebuilt = rebuildIndex(join(tempDir("index"), "index.db"), events);
    expect(dump(rebuilt)).toEqual(dump(incremental));
    incremental.close();
    rebuilt.close();
  });

  it("folds message.recorded rows and dedupes by message id", () => {
    const index = new StateIndex(":memory:");
    const message = (id: string, content: string) =>
      evt({
        type: "message.recorded",
        message_id: id,
        source: "claude-code",
        native_session_id: "s1",
        native_record_id: id,
        role: "user",
        kind: "message",
        content,
        native_ts: "2026-01-01T00:00:00Z",
        project_path: "/repo",
      });
    index.apply(message("msg_a", "hello"));
    index.apply(message("msg_a", "hello again")); // duplicate id: ignored
    index.apply(message("msg_b", "world"));

    const rows = index.db
      .prepare("SELECT id, content, source, native_session_id FROM messages ORDER BY id")
      .all() as any[];
    expect(rows).toEqual([
      { id: "msg_a", content: "hello", source: "claude-code", native_session_id: "s1" },
      { id: "msg_b", content: "world", source: "claude-code", native_session_id: "s1" },
    ]);
    index.close();
  });

  it("numbers prompts per session, skipping the harness-written records", () => {
    const index = new StateIndex(":memory:");
    let n = 0;
    const say = (session: string, role: string, kind: string, content: string) =>
      index.apply(
        evt({
          type: "message.recorded",
          message_id: `m${++n}`,
          source: "claude-code",
          native_session_id: session,
          native_record_id: `m${n}`,
          role,
          kind,
          content,
        }),
      );
    // A session opens with slash-command noise, then the first real prompt.
    say("s1", "user", "message", "<command-name>/clear</command-name>");
    say("s1", "user", "message", "<local-command-caveat>Caveat: …");
    say("s1", "user", "message", "first question");
    say("s1", "assistant", "message", "first answer");
    say("s1", "user", "message", "[Request interrupted by user]");
    say("s1", "user", "message", "second question");
    say("s1", "assistant", "message", "second answer");
    // A second session numbers independently.
    say("s2", "user", "message", "unrelated question");

    const idx = index.db
      .prepare("SELECT id, prompt_idx FROM messages ORDER BY id")
      .all() as any[];
    expect(idx.map((r) => r.prompt_idx)).toEqual([0, 0, 1, 1, 1, 2, 2, 1]);
    const counts = index.db
      .prepare("SELECT native_session_id, prompt_count FROM transcript_sessions ORDER BY 1")
      .all() as any[];
    expect(counts).toEqual([
      { native_session_id: "s1", prompt_count: 2 },
      { native_session_id: "s2", prompt_count: 1 },
    ]);
    index.close();
  });

  it("promotes tool facts so a call joins to its result", () => {
    const index = new StateIndex(":memory:");
    const record = (id: string, role: string, kind: string, content: string) =>
      index.apply(
        evt({
          type: "message.recorded",
          message_id: id,
          source: "claude-code",
          native_session_id: "s1",
          native_record_id: id,
          role,
          kind,
          content,
        }),
      );
    const use = (id: string, name: string, input: unknown) =>
      record("u" + id, "assistant", "tool_use", JSON.stringify({ id: "toolu_" + id, name, input }));
    const result = (id: string, isError: boolean) =>
      record(
        "r" + id,
        "tool",
        "tool_result",
        JSON.stringify({ tool_use_id: "toolu_" + id, is_error: isError, content: "…" }),
      );
    use("1", "Read", { file_path: "/src/shim/proxy.ts" });
    result("1", false);
    use("2", "Bash", { command: "npm test" });
    result("2", true);

    const pairs = index.db
      .prepare(
        `SELECT u.tool_name, u.tool_target, r.is_error
           FROM messages u
           JOIN messages r ON r.tool_use_id = u.tool_use_id AND r.kind = 'tool_result'
          WHERE u.kind = 'tool_use' ORDER BY u.id`,
      )
      .all() as any[];
    expect(pairs).toEqual([
      { tool_name: "Read", tool_target: "/src/shim/proxy.ts", is_error: 0 },
      { tool_name: "Bash", tool_target: "npm test", is_error: 1 },
    ]);
    index.close();
  });

  it("attributes a worker session's messages to the turn running at the time", () => {
    const index = new StateIndex(":memory:");
    // Turn windows are event timestamps, so they have to be pinned explicitly:
    // evt() spaces its events one second apart, far too tight to bucket against.
    const at = (ts: string, body: EventBody): LogEvent => ({ ...evt(body), ts });
    const start = (turnId: string, ts: string) =>
      at(ts, { type: "turn.started", turn_id: turnId, task_id: "task_t", prompt: "p" });
    const finish = (turnId: string, ts: string) =>
      at(ts, {
        type: "turn.completed",
        turn_id: turnId,
        task_id: "task_t",
        response: "r",
        changed_files: [],
      });
    const events = [
      evt({ type: "project.created", project_id: "proj_t", root: "/t" }),
      evt({
        type: "task.created",
        task_id: "task_t",
        project_id: "proj_t",
        worker: "codex",
        prompt_summary: "x",
      }),
      start("turn_1", "2026-03-01T00:00:00Z"),
      finish("turn_1", "2026-03-01T00:10:00Z"),
      start("turn_2", "2026-03-01T00:20:00Z"),
      finish("turn_2", "2026-03-01T00:30:00Z"),
      evt({
        type: "worker-session.recorded",
        worker_session_id: "wsess_t",
        task_id: "task_t",
        worker: "codex",
        native_session_id: "native_t",
        turn_id: "turn_1",
      }),
    ];
    for (const event of events) index.apply(event);

    const swept = (id: string, session: string, nativeTs: string) =>
      index.apply(
        evt({
          type: "message.recorded",
          message_id: id,
          source: "codex",
          native_session_id: session,
          native_record_id: id,
          role: "assistant",
          kind: "message",
          content: "working",
          native_ts: nativeTs,
        }),
      );
    swept("in_1", "native_t", "2026-03-01T00:05:00Z"); // inside turn 1
    swept("in_2", "native_t", "2026-03-01T00:25:00Z"); // inside turn 2
    swept("between", "native_t", "2026-03-01T00:15:00Z"); // between turns
    swept("host", "some-host-session", "2026-03-01T00:05:00Z"); // no task links it

    const rows = index.db
      .prepare("SELECT id, turn_id FROM messages ORDER BY id")
      .all() as any[];
    expect(rows).toEqual([
      { id: "between", turn_id: null },
      { id: "host", turn_id: null },
      { id: "in_1", turn_id: "turn_1" },
      { id: "in_2", turn_id: "turn_2" },
    ]);
    const wsess = index.db.prepare("SELECT turn_id FROM worker_sessions").get() as any;
    expect(wsess.turn_id).toBe("turn_1");
    index.close();
  });

  it("rebuild replaces an existing index file", () => {
    const path = join(tempDir("index"), "index.db");
    const first = rebuildIndex(path, sampleSequence());
    first.close();
    const second = rebuildIndex(path, [
      evt({ type: "project.created", project_id: "proj_only", root: "/only" }),
    ]);
    const projects = second.db.prepare("SELECT id FROM projects").all() as any[];
    expect(projects).toEqual([{ id: "proj_only" }]);
    second.close();
  });
});
