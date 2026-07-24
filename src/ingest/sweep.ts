import * as fs from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { EventBody, LogEvent } from "../storage/events.js";
import type { StateIndex } from "../storage/index.js";
import { messageId, type FileContext, type TranscriptParser } from "./parser.js";
import { parserForFormat } from "./registry.js";

// Periodic transcript sweeper. It reads new lines out of each configured
// source's transcript files (resuming from a persisted byte offset), parses
// them, and appends a message.recorded for every record it has not seen.
//
// Idempotency is enforced BEFORE the append: message ids are deterministic,
// so a record already in the `messages` table is skipped. The byte offsets
// are only a performance cache in a sidecar JSON file — deleting it forces a
// full re-scan that dedupe makes harmless, and the event log stays the sole
// source of truth for the index.

/** One transcript source: a parser format and the dirs to scan for it. */
export interface IngestSource {
  format: string;
  dirs: string[];
}

/** Per-file resume state persisted in the sidecar. */
interface FileState {
  /** Byte offset just past the last fully-consumed line. */
  offset: number;
  /** Number of complete lines already consumed (stable codex record ids). */
  lineIndex: number;
  sessionId?: string;
  projectPath?: string;
}

export interface SweepStats {
  filesScanned: number;
  recorded: number;
  errors: number;
}

interface SweeperDeps {
  sources: IngestSource[];
  index: StateIndex;
  record: (body: EventBody) => LogEvent;
  /**
   * Forces recorded events durable. Called before offsets are persisted, so
   * the sidecar can never claim a record the log has not durably kept.
   */
  flush: () => void;
  /** Sidecar path, e.g. <state root>/ingest-state.json. */
  stateFile: string;
  /** Diagnostics sink; defaults to stderr. */
  onLog?: (message: string) => void;
}

/** Expands a leading ~ to the user's home directory. */
export function expandHome(dir: string): string {
  if (dir === "~") return homedir();
  if (dir.startsWith("~/")) return join(homedir(), dir.slice(2));
  return dir;
}

const NEWLINE = 0x0a;

/**
 * Longest the sweep may hold the event loop before handing it back. Records
 * vary hugely in cost between formats (a Codex rollout line can carry a whole
 * tool payload), so this is a time budget rather than a line count.
 */
const YIELD_INTERVAL_MS = 50;

/** Hands the event loop back so queued I/O runs before the sweep continues. */
function yieldToLoop(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

export class TranscriptSweeper {
  private inFlight: Promise<SweepStats> | null = null;
  private readonly hasMessage;

  constructor(private readonly deps: SweeperDeps) {
    this.hasMessage = deps.index.db.prepare("SELECT 1 FROM messages WHERE id = ? LIMIT 1");
  }

  /** Runs one sweep, coalescing with any already in flight. */
  sweep(): Promise<SweepStats> {
    if (this.inFlight) return this.inFlight;
    this.inFlight = Promise.resolve()
      .then(() => this.runSweep())
      .finally(() => {
        this.inFlight = null;
      });
    return this.inFlight;
  }

  /** Resolves once any in-flight sweep has settled (for daemon shutdown). */
  async settle(): Promise<void> {
    await this.inFlight?.catch(() => {});
  }

  private async runSweep(): Promise<SweepStats> {
    const state = this.loadState();
    const stats: SweepStats = { filesScanned: 0, recorded: 0, errors: 0 };
    for (const source of this.deps.sources) {
      const parser = parserForFormat(source.format);
      if (!parser) {
        this.log(`ingest: no parser for format '${source.format}', skipping`);
        continue;
      }
      const dirs = source.dirs.map(expandHome);
      for (const file of parser.enumerate(dirs)) {
        try {
          stats.recorded += await this.sweepFile(parser, source.format, file, state);
          stats.filesScanned += 1;
        } catch (err) {
          stats.errors += 1;
          this.log(`ingest: error sweeping ${file}: ${(err as Error).message}`);
        }
        await yieldToLoop(); // never hold the loop across files
      }
    }
    // Offsets may only ever advance over durably-logged records: a lost log
    // tail combined with an advanced offset would skip those messages
    // forever, since dedupe would never see them again.
    this.deps.flush();
    this.saveState(state);
    return stats;
  }

  /** Reads new complete lines from one file and records unseen messages. */
  private async sweepFile(
    parser: TranscriptParser,
    format: string,
    file: string,
    state: Record<string, FileState>,
  ): Promise<number> {
    const buf = fs.readFileSync(file);
    let st = state[file] ?? { offset: 0, lineIndex: 0 };
    // Shorter than where we left off means the file was truncated or
    // rewritten; re-read from the top. Dedupe keeps that harmless.
    if (buf.length < st.offset) st = { offset: 0, lineIndex: 0 };

    const ctx: FileContext = {
      filePath: file,
      lineIndex: st.lineIndex,
      ...(st.sessionId ? { sessionId: st.sessionId } : {}),
      ...(st.projectPath ? { projectPath: st.projectPath } : {}),
    };

    let pos = st.offset;
    let lineIndex = st.lineIndex;
    let recorded = 0;
    let lastYield = Date.now();
    for (;;) {
      const nl = buf.indexOf(NEWLINE, pos);
      if (nl === -1) break; // trailing partial line: leave for the next sweep
      const line = buf.toString("utf8", pos, nl);
      ctx.lineIndex = lineIndex;
      for (const msg of parser.parse(line, ctx)) {
        const id = messageId(format, msg.nativeSessionId, msg.nativeRecordId);
        if (this.hasMessage.get(id)) continue;
        this.deps.record({
          type: "message.recorded",
          message_id: id,
          source: format,
          native_session_id: msg.nativeSessionId,
          native_record_id: msg.nativeRecordId,
          role: msg.role,
          kind: msg.kind,
          content: msg.content,
          ...(msg.nativeTs ? { native_ts: msg.nativeTs } : {}),
          ...(msg.projectPath ? { project_path: msg.projectPath } : {}),
        });
        recorded += 1;
      }
      pos = nl + 1;
      lineIndex += 1;
      // A first backfill runs for minutes; yielding on a time budget keeps it
      // from starving the MCP server. Interrupting mid-file is safe: offsets
      // are only persisted once the file completes, and dedupe makes the
      // re-read harmless.
      if (Date.now() - lastYield >= YIELD_INTERVAL_MS) {
        await yieldToLoop();
        lastYield = Date.now();
      }
    }

    state[file] = {
      offset: pos,
      lineIndex,
      ...(ctx.sessionId ? { sessionId: ctx.sessionId } : {}),
      ...(ctx.projectPath ? { projectPath: ctx.projectPath } : {}),
    };
    return recorded;
  }

  private loadState(): Record<string, FileState> {
    try {
      return JSON.parse(fs.readFileSync(this.deps.stateFile, "utf8")) as Record<string, FileState>;
    } catch {
      // Missing or unparseable: start fresh. Full re-scan is dedupe-safe.
      return {};
    }
  }

  private saveState(state: Record<string, FileState>): void {
    const tmp = `${this.deps.stateFile}.tmp`;
    fs.mkdirSync(dirname(this.deps.stateFile), { recursive: true });
    fs.writeFileSync(tmp, JSON.stringify(state));
    fs.renameSync(tmp, this.deps.stateFile);
  }

  private log(message: string): void {
    (this.deps.onLog ?? ((m: string) => process.stderr.write(m + "\n")))(message);
  }
}
