import { appendFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { EventLog, readEvents, type EventBody } from "../../src/storage/events.js";
import { StateIndex } from "../../src/storage/index.js";
import { TranscriptSweeper, type IngestSource } from "../../src/ingest/sweep.js";
import { tempDir } from "../helpers.js";

function line(uuid: string, text: string): string {
  return JSON.stringify({
    type: "user",
    uuid,
    sessionId: "s1",
    cwd: "/repo",
    timestamp: "2026-01-01T00:00:00Z",
    message: { role: "user", content: text },
  });
}

interface Harness {
  sweeper: TranscriptSweeper;
  index: StateIndex;
  eventsLog: string;
  transcript: string;
  stateFile: string;
  /** What was observable at each flush, in order. */
  flushes: { loggedMessages: number; offsetsPersisted: boolean }[];
  messageCount(): number;
  loggedMessages(): number;
}

function harness(): Harness {
  const root = tempDir("sweep");
  const sourceDir = join(root, "claude");
  mkdirSync(sourceDir, { recursive: true });
  const transcript = join(sourceDir, "session.jsonl");
  const eventsLog = join(root, "events.jsonl");
  const stateFile = join(root, "ingest-state.json");
  const log = EventLog.open(eventsLog);
  const index = new StateIndex(":memory:");
  // Mirrors the daemon's bulk path: ingest appends unsynced and flushes once.
  const record = (body: EventBody) => {
    const event = log.append(body, { sync: false });
    index.apply(event);
    return event;
  };
  const flushes: { loggedMessages: number; offsetsPersisted: boolean }[] = [];
  const sources: IngestSource[] = [{ format: "claude-code", dirs: [sourceDir] }];
  const sweeper = new TranscriptSweeper({
    sources,
    index,
    record,
    flush: () => {
      log.flush();
      flushes.push({
        loggedMessages: readEvents(eventsLog).filter((e) => e.type === "message.recorded").length,
        // On a first sweep the sidecar does not exist yet, so this reveals
        // whether offsets were persisted before this flush or after it.
        offsetsPersisted: existsSync(stateFile),
      });
    },
    stateFile,
  });
  return {
    sweeper,
    index,
    eventsLog,
    transcript,
    stateFile,
    flushes,
    messageCount: () =>
      (index.db.prepare("SELECT COUNT(*) AS n FROM messages").get() as { n: number }).n,
    loggedMessages: () =>
      readEvents(eventsLog).filter((e) => e.type === "message.recorded").length,
  };
}

describe("TranscriptSweeper", () => {
  it("ingests messages and is idempotent across re-sweeps", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u2", "two") + "\n");

    const first = await h.sweeper.sweep();
    expect(first.recorded).toBe(2);
    expect(h.messageCount()).toBe(2);
    expect(h.loggedMessages()).toBe(2);

    // Second sweep: nothing new in the index AND nothing new appended to the
    // append-forever log.
    const second = await h.sweeper.sweep();
    expect(second.recorded).toBe(0);
    expect(h.messageCount()).toBe(2);
    expect(h.loggedMessages()).toBe(2);
  });

  it("picks up newly appended lines incrementally", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n");
    await h.sweeper.sweep();
    expect(h.messageCount()).toBe(1);

    appendFileSync(h.transcript, line("u2", "two") + "\n");
    const stats = await h.sweeper.sweep();
    expect(stats.recorded).toBe(1);
    expect(h.messageCount()).toBe(2);
  });

  it("leaves a trailing partial line for the next sweep", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n" + '{"type":"user","uuid":"u2","sess');
    await h.sweeper.sweep();
    expect(h.messageCount()).toBe(1); // partial line not yet consumed

    // Completing the line makes it ingestable.
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u2", "two") + "\n");
    await h.sweeper.sweep();
    expect(h.messageCount()).toBe(2);
  });

  it("re-scans harmlessly after the offset sidecar is deleted", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u2", "two") + "\n");
    await h.sweeper.sweep();
    expect(h.loggedMessages()).toBe(2);

    rmSync(join(h.eventsLog, "..", "ingest-state.json"));
    const stats = await h.sweeper.sweep();
    // Full re-scan, but deterministic ids dedupe every record: no new events.
    expect(stats.recorded).toBe(0);
    expect(h.messageCount()).toBe(2);
    expect(h.loggedMessages()).toBe(2);
  });

  it("re-reads from the top when a file becomes shorter (rotation/replacement)", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u2", "two") + "\n");
    await h.sweeper.sweep();

    // Replace with a strictly shorter file: one carried-over and one new
    // record. (Transcripts only ever grow in normal operation; a shorter file
    // means the path was rotated or reused, so we re-scan from the top and let
    // deterministic ids dedupe the carried-over record.)
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u3", "x") + "\n");
    const stats = await h.sweeper.sweep();
    expect(stats.recorded).toBe(1); // only the new u3; u1 dedupes
    expect(h.messageCount()).toBe(3); // u1, u2, u3 all retained
  });

  it("flushes every recorded event before persisting offsets", async () => {
    const h = harness();
    writeFileSync(h.transcript, line("u1", "one") + "\n" + line("u2", "two") + "\n");
    await h.sweeper.sweep();

    // The sidecar must never claim ground the log has not durably kept: a
    // crash that lost those records would skip them forever, because the
    // offset would stop them being re-read and dedupe would never see them.
    expect(h.flushes).toHaveLength(1);
    // Every record already durable at flush time...
    expect(h.flushes[0]?.loggedMessages).toBe(h.loggedMessages());
    // ...and offsets written only afterwards, never before.
    expect(h.flushes[0]?.offsetsPersisted).toBe(false);
    expect(existsSync(h.stateFile)).toBe(true);
  });

  it("never holds the event loop for long while sweeping a large transcript", async () => {
    const h = harness();
    // Enough records that a fully synchronous sweep runs for much longer than
    // one yield budget: it is a single file, so yielding only between files
    // would block the loop from the first record to the last.
    const lines = Array.from({ length: 20_000 }, (_, i) => line(`u${i}`, "x".repeat(400)));
    writeFileSync(h.transcript, lines.join("\n") + "\n");

    // A daemon serves MCP on this loop, and the shim gives it a bounded window
    // to answer, so what matters is the longest single stall, not the total.
    let longestStallMs = 0;
    let last = Date.now();
    const timer = setInterval(() => {
      longestStallMs = Math.max(longestStallMs, Date.now() - last);
      last = Date.now();
    }, 10);
    const started = Date.now();
    const stats = await h.sweeper.sweep();
    clearInterval(timer);

    expect(stats.recorded).toBe(20_000);
    // Meaningful only if the sweep outran a single yield budget several times
    // over; otherwise it could pass without ever yielding.
    expect(Date.now() - started).toBeGreaterThan(500);
    expect(longestStallMs).toBeLessThan(500);
  });

  it("tolerates a source dir that does not exist", async () => {
    const h = harness();
    rmSync(join(h.transcript, ".."), { recursive: true });
    const stats = await h.sweeper.sweep();
    expect(stats.recorded).toBe(0);
    expect(stats.errors).toBe(0);
  });
});
