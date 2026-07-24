import * as fs from "node:fs";
import { dirname } from "node:path";
import { z } from "zod";
import { newId } from "../ids.js";

// Every durable record is one JSONL line appended to the event log first;
// SQLite is a derived index folded from these events.
// All timestamps that reach the index come from event `ts` fields so
// rebuilds are deterministic.

const projectCreated = z.object({
  type: z.literal("project.created"),
  project_id: z.string(),
  root: z.string(),
});

const projectAliasAdded = z.object({
  type: z.literal("project.alias-added"),
  project_id: z.string(),
  path: z.string(),
});

const sessionStarted = z.object({
  type: z.literal("session.started"),
  session_id: z.string(),
  project_id: z.string().optional(),
  client: z.string().optional(),
});

const sessionEnded = z.object({
  type: z.literal("session.ended"),
  session_id: z.string(),
});

const taskCreated = z.object({
  type: z.literal("task.created"),
  task_id: z.string(),
  project_id: z.string(),
  session_id: z.string().optional(),
  worker: z.string(),
  prompt_summary: z.string(),
  // Policy fields; absent on the earliest records (docker/workspace-write
  // equivalents did not exist yet, so readers must not assume defaults).
  tier: z.string().optional(),
  // Legacy (removed host-run flow): never emitted anymore, kept so old logs
  // still parse — readEvents stops at the first unparseable line.
  runtime: z.string().optional(),
  allow_domains: z.array(z.string()).optional(),
});

// Legacy (removed host-run flow): never emitted anymore, kept so old logs
// still parse.
const approvalRequested = z.object({
  type: z.literal("approval.requested"),
  task_id: z.string(),
  tier: z.string(),
  prompt: z.string(),
});

const approvalRecorded = z.object({
  type: z.literal("approval.recorded"),
  approval_id: z.string(),
  task_id: z.string(),
  decision: z.enum(["approved", "denied"]),
  /** agent = relayed in-conversation; human = legacy approve/deny CLI. */
  via: z.enum(["agent", "human"]),
  domains: z.array(z.string()).optional(),
  session_id: z.string().optional(),
});

const turnStarted = z.object({
  type: z.literal("turn.started"),
  turn_id: z.string(),
  task_id: z.string(),
  prompt: z.string(),
});

const turnCompleted = z.object({
  type: z.literal("turn.completed"),
  turn_id: z.string(),
  task_id: z.string(),
  response: z.string(),
  changed_files: z.array(z.string()),
  usage: z.unknown().optional(),
});

const turnFailed = z.object({
  type: z.literal("turn.failed"),
  turn_id: z.string(),
  task_id: z.string(),
  error_code: z.string(),
  error_message: z.string(),
});

const turnCanceled = z.object({
  type: z.literal("turn.canceled"),
  turn_id: z.string(),
  task_id: z.string(),
  reason: z.string().optional(),
});

const workerSessionRecorded = z.object({
  type: z.literal("worker-session.recorded"),
  worker_session_id: z.string(),
  task_id: z.string(),
  worker: z.string(),
  native_session_id: z.string(),
});

const auditRecorded = z.object({
  type: z.literal("audit.recorded"),
  session_id: z.string().optional(),
  task_id: z.string().optional(),
  turn_id: z.string().optional(),
  kind: z.string(),
  payload: z.unknown(),
});

// A single conversation record swept out of a host/native agent transcript
// (Claude Code, Codex, …). `message_id` is deterministic — a hash of
// (source, native_session_id, native_record_id) — so re-sweeping the same
// transcript record always yields the same id and the fold is idempotent.
const messageRecorded = z.object({
  type: z.literal("message.recorded"),
  message_id: z.string(),
  /** Transcript source, e.g. "claude-code" / "codex". Free-form: no reader
   * may assume only the built-in sources exist. */
  source: z.string(),
  native_session_id: z.string(),
  native_record_id: z.string(),
  role: z.string(),
  /** message | tool_use | tool_result | reasoning | system. */
  kind: z.string(),
  /** Plain text, or JSON-encoded structured blocks. */
  content: z.string(),
  /** The record's own timestamp, when the format carries one. */
  native_ts: z.string().optional(),
  project_path: z.string().optional(),
});

const artifactStored = z.object({
  type: z.literal("artifact.stored"),
  artifact_id: z.string(),
  kind: z.string(),
  label: z.string(),
  media_type: z.string(),
  size_bytes: z.number().int().nonnegative(),
  sha256: z.string(),
  locator: z.string(),
});

const artifactLinked = z.object({
  type: z.literal("artifact.linked"),
  artifact_id: z.string(),
  session_id: z.string().optional(),
  task_id: z.string().optional(),
  turn_id: z.string().optional(),
  audit_event_id: z.string().optional(),
});

export const eventBodySchema = z.discriminatedUnion("type", [
  projectCreated,
  projectAliasAdded,
  sessionStarted,
  sessionEnded,
  taskCreated,
  approvalRequested,
  approvalRecorded,
  turnStarted,
  turnCompleted,
  turnFailed,
  turnCanceled,
  workerSessionRecorded,
  messageRecorded,
  auditRecorded,
  artifactStored,
  artifactLinked,
]);

export type EventBody = z.infer<typeof eventBodySchema>;

const envelopeSchema = z.object({ id: z.string(), ts: z.string() });

export type LogEvent = EventBody & { id: string; ts: string };

export function parseEventLine(line: string): LogEvent {
  const raw: unknown = JSON.parse(line);
  const envelope = envelopeSchema.parse(raw);
  const body = eventBodySchema.parse(raw);
  return { ...body, id: envelope.id, ts: envelope.ts };
}

/**
 * Reads all valid events. Stops silently at the first unparseable line: the
 * log is append-only, so anything after a torn line is a torn tail from a
 * crash mid-append.
 */
export function readEvents(path: string): LogEvent[] {
  let content: string;
  try {
    content = fs.readFileSync(path, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw err;
  }
  const events: LogEvent[] = [];
  let offset = 0;
  while (offset < content.length) {
    const newline = content.indexOf("\n", offset);
    if (newline === -1) break; // unterminated tail
    const line = content.slice(offset, newline);
    try {
      events.push(parseEventLine(line));
    } catch {
      break;
    }
    offset = newline + 1;
  }
  return events;
}

/** Truncates any torn tail so new appends land after the last valid record. */
function repairTornTail(path: string): void {
  let buf: Buffer;
  try {
    buf = fs.readFileSync(path);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return;
    throw err;
  }
  const content = buf.toString("utf8");
  let validEnd = 0;
  let offset = 0;
  while (offset < content.length) {
    const newline = content.indexOf("\n", offset);
    if (newline === -1) break;
    try {
      parseEventLine(content.slice(offset, newline));
    } catch {
      break;
    }
    offset = newline + 1;
    validEnd = offset;
  }
  if (validEnd < content.length) {
    const validBytes = Buffer.byteLength(content.slice(0, validEnd), "utf8");
    fs.truncateSync(path, validBytes);
  }
}

export class EventLog {
  private constructor(
    readonly path: string,
    private readonly fd: number,
  ) {}

  static open(path: string): EventLog {
    fs.mkdirSync(dirname(path), { recursive: true });
    repairTornTail(path);
    return new EventLog(path, fs.openSync(path, "a"));
  }

  /**
   * Assigns id/ts, appends one JSONL line, returns the full event.
   *
   * Appends are fsynced by default: lifecycle records are authoritative, so
   * losing one would strand a turn nothing else knows about. Bulk writers of
   * *reconstructible* events (transcript ingest, whose source files are never
   * modified) may pass `sync: false` and call `flush()` once at the end —
   * fsync costs ~28ms per record, which dominates a large backfill. A torn
   * tail from a crash is repaired on next open.
   */
  append(body: EventBody, options: { sync?: boolean } = {}): LogEvent {
    const event: LogEvent = { id: newId("evt"), ts: new Date().toISOString(), ...body };
    fs.writeSync(this.fd, JSON.stringify(event) + "\n");
    if (options.sync !== false) fs.fsyncSync(this.fd);
    return event;
  }

  /** Forces every prior append durable, including unsynced ones. */
  flush(): void {
    fs.fsyncSync(this.fd);
  }

  close(): void {
    fs.closeSync(this.fd);
  }
}
