import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { EventLog, readEvents } from "../../src/storage/events.js";
import { tempDir } from "../helpers.js";

describe("EventLog", () => {
  it("round-trips appended events", () => {
    const path = join(tempDir("events"), "events.jsonl");
    const log = EventLog.open(path);
    const a = log.append({ type: "project.created", project_id: "proj_a", root: "/repo" });
    const b = log.append({
      type: "turn.started",
      turn_id: "turn_1",
      task_id: "task_1",
      prompt: "do the thing",
    });
    log.close();

    expect(a.id).toMatch(/^evt_/);
    expect(Date.parse(a.ts)).not.toBeNaN();
    const events = readEvents(path);
    expect(events).toEqual([a, b]);
  });

  it("reader stops at a torn tail without throwing", () => {
    const path = join(tempDir("events"), "events.jsonl");
    const log = EventLog.open(path);
    const a = log.append({ type: "project.created", project_id: "proj_a", root: "/repo" });
    log.close();
    appendFileSync(path, '{"id":"evt_torn","ts":"2026-01-01T00:00:00Z","type":"turn.sta');

    expect(readEvents(path)).toEqual([a]);
  });

  it("open() truncates a torn tail so new appends stay valid", () => {
    const path = join(tempDir("events"), "events.jsonl");
    const first = EventLog.open(path);
    const a = first.append({ type: "project.created", project_id: "proj_a", root: "/repo" });
    first.close();
    appendFileSync(path, '{"id":"evt_torn","ts":"2026-01-01T00:0');

    const reopened = EventLog.open(path);
    const b = reopened.append({ type: "session.started", session_id: "sess_a" });
    reopened.close();

    expect(readEvents(path)).toEqual([a, b]);
    expect(readFileSync(path, "utf8")).not.toContain("evt_torn");
  });

  it("open() drops everything after an interior corrupt line", () => {
    const path = join(tempDir("events"), "events.jsonl");
    const first = EventLog.open(path);
    const a = first.append({ type: "project.created", project_id: "proj_a", root: "/repo" });
    first.close();
    appendFileSync(path, "not json at all\n");
    appendFileSync(
      path,
      '{"id":"evt_after","ts":"2026-01-01T00:00:00Z","type":"session.ended","session_id":"sess_a"}\n',
    );

    const reopened = EventLog.open(path);
    reopened.close();
    expect(readEvents(path)).toEqual([a]);
  });

  it("returns no events for a missing file", () => {
    expect(readEvents(join(tempDir("events"), "missing.jsonl"))).toEqual([]);
  });
});
