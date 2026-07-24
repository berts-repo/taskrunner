import { createHash } from "node:crypto";
import * as fs from "node:fs";
import { join } from "node:path";

// Transcript parsers turn one native agent transcript (Claude Code, Codex, …)
// into normalized conversation messages. A parser is pure and stateless
// except for the per-file context it is handed: files are append-only per
// session, so the sweeper can resume mid-file and replay the persisted
// context instead of re-reading a session header it has already passed.

/** One normalized conversation record, ready to become a message.recorded. */
export interface ParsedMessage {
  nativeSessionId: string;
  nativeRecordId: string;
  role: string;
  /** message | tool_use | tool_result | reasoning | system. */
  kind: string;
  /** Plain text, or JSON-encoded structured payload. */
  content: string;
  nativeTs?: string;
  projectPath?: string;
}

/**
 * Mutable per-file state. A parser reads session/project context from it and
 * writes back whatever a session-header record establishes, so later lines
 * (and resumed sweeps) inherit it. `lineIndex` is the 0-based index of the
 * line being parsed and is a stable fallback record id for formats whose
 * records carry none.
 */
export interface FileContext {
  filePath: string;
  lineIndex: number;
  sessionId?: string;
  projectPath?: string;
}

export interface TranscriptParser {
  readonly format: string;
  /** Transcript files under the source dirs, in a stable sweep order. */
  enumerate(dirs: string[]): string[];
  /**
   * Parses one transcript line into zero or more messages, mutating `ctx`
   * with any session/project context the line establishes. Returns [] for
   * noise, header-only, and unparseable lines — the format is unversioned,
   * so unknown shapes are tolerated, never fatal.
   */
  parse(line: string, ctx: FileContext): ParsedMessage[];
}

/** `msg_` + a truncated sha256 of the natural key: stable across re-sweeps. */
export function messageId(
  source: string,
  nativeSessionId: string,
  nativeRecordId: string,
): string {
  const hash = createHash("sha256")
    .update(`${source}\0${nativeSessionId}\0${nativeRecordId}`)
    .digest("hex");
  return `msg_${hash.slice(0, 32)}`;
}

/** All `*.jsonl` files beneath the given dirs (recursive), sorted for a
 * deterministic sweep order. Missing dirs are skipped. */
export function findJsonlFiles(dirs: string[]): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return; // missing or unreadable dir: nothing to sweep
    }
    for (const entry of entries) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && entry.name.endsWith(".jsonl")) out.push(full);
    }
  };
  for (const dir of dirs) walk(dir);
  return out.sort();
}

/** Coerces a content value (string, {text}, or array of blocks) to text. */
export function contentToText(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    return value
      .map((block) => {
        if (typeof block === "string") return block;
        if (block && typeof block === "object") {
          const b = block as Record<string, unknown>;
          if (typeof b["text"] === "string") return b["text"] as string;
        }
        return "";
      })
      .filter((s) => s !== "")
      .join("\n");
  }
  if (value && typeof value === "object") {
    const v = value as Record<string, unknown>;
    if (typeof v["text"] === "string") return v["text"] as string;
  }
  return "";
}
