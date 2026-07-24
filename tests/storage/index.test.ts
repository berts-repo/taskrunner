import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { rebuildIndex, StateIndex } from "../../src/storage/index.js";
import { evt, sampleSequence, tempDir } from "../helpers.js";

const TABLES = [
  "projects",
  "project_aliases",
  "sessions",
  "tasks",
  "turns",
  "worker_sessions",
  "messages",
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

    const session = index.db.prepare("SELECT * FROM sessions WHERE id = 'sess_a'").get() as any;
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
