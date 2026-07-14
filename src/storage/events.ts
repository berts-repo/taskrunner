import * as fs from "node:fs";
import { dirname } from "node:path";
import { z } from "zod";
import { newId } from "../ids.js";

// Every durable record is one JSONL line appended to the event log first;
// SQLite is a derived index folded from these events (PLAN § Storage write
// path). All timestamps that reach the index come from event `ts` fields so
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
  turnStarted,
  turnCompleted,
  turnFailed,
  turnCanceled,
  workerSessionRecorded,
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
 * crash mid-append (PLAN § Storage write path).
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

  /** Assigns id/ts, appends one fsynced JSONL line, returns the full event. */
  append(body: EventBody): LogEvent {
    const event: LogEvent = { id: newId("evt"), ts: new Date().toISOString(), ...body };
    fs.writeSync(this.fd, JSON.stringify(event) + "\n");
    fs.fsyncSync(this.fd);
    return event;
  }

  close(): void {
    fs.closeSync(this.fd);
  }
}
