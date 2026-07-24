import { basename } from "node:path";
import {
  contentToText,
  findJsonlFiles,
  type FileContext,
  type ParsedMessage,
  type TranscriptParser,
} from "./parser.js";

// Parses Codex transcripts under ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl.
// Unlike Claude Code, individual records do not repeat the session id: a
// session_meta header at the top of the file establishes it (with the cwd),
// and the rollout filename (rollout-<ts>-<uuid>.jsonl) carries it too as a
// resume-safe fallback. Records are commonly wrapped as { type, payload,
// timestamp }; older versions flatten the payload, so both shapes are read.
//
// Conversation lives in response_item records (message / function_call /
// function_call_output / reasoning). event_msg records duplicate that stream
// for the live UI and are skipped; turn_context only carries cwd updates.

interface CodexRecord {
  type?: string;
  timestamp?: string;
  payload?: Record<string, unknown>;
  [key: string]: unknown;
}

export const codexParser: TranscriptParser = {
  format: "codex",

  enumerate(dirs: string[]): string[] {
    return findJsonlFiles(dirs).filter((f) => basename(f).startsWith("rollout-"));
  },

  parse(line: string, ctx: FileContext): ParsedMessage[] {
    const trimmed = line.trim();
    if (trimmed === "") return [];
    let record: CodexRecord;
    try {
      record = JSON.parse(trimmed) as CodexRecord;
    } catch {
      return [];
    }
    // payload nests the record body in current Codex; flat records expose it
    // at the top level. Read whichever carries the fields.
    const body = (record.payload ?? record) as Record<string, unknown>;

    if (record.type === "session_meta") {
      const id = body["id"] ?? body["session_id"] ?? body["conversation_id"];
      if (typeof id === "string") ctx.sessionId = id;
      const cwd = body["cwd"] ?? body["cwd_path"];
      if (typeof cwd === "string") ctx.projectPath = cwd;
      ctx.sessionId ??= sessionIdFromFile(ctx.filePath);
      return [];
    }
    if (record.type === "turn_context") {
      const cwd = body["cwd"] ?? body["cwd_path"];
      if (typeof cwd === "string") ctx.projectPath = cwd;
      return [];
    }
    if (record.type === "event_msg") return []; // live-UI duplicate of response_item
    if (record.type !== "response_item") return [];

    ctx.sessionId ??= sessionIdFromFile(ctx.filePath);
    const sessionId = ctx.sessionId;
    if (!sessionId) return [];

    const recordId = stableRecordId(body, ctx.lineIndex);
    const common = {
      nativeSessionId: sessionId,
      nativeRecordId: recordId,
      ...(record.timestamp ? { nativeTs: record.timestamp } : {}),
      ...(ctx.projectPath ? { projectPath: ctx.projectPath } : {}),
    };

    switch (body["type"]) {
      case "message": {
        const text = contentToText(body["content"]);
        if (text === "") return [];
        const role = typeof body["role"] === "string" ? (body["role"] as string) : "assistant";
        return [{ ...common, role, kind: "message", content: text }];
      }
      case "function_call":
        return [
          {
            ...common,
            role: "assistant",
            kind: "tool_use",
            content: JSON.stringify({
              call_id: body["call_id"],
              name: body["name"],
              arguments: body["arguments"],
            }),
          },
        ];
      case "function_call_output":
        return [
          {
            ...common,
            role: "tool",
            kind: "tool_result",
            content: JSON.stringify({
              call_id: body["call_id"],
              output: normalizeOutput(body["output"]),
            }),
          },
        ];
      case "reasoning": {
        const text = contentToText(body["summary"] ?? body["content"]);
        if (text === "") return [];
        return [{ ...common, role: "assistant", kind: "reasoning", content: text }];
      }
      default:
        return [];
    }
  },
};

/** Extracts the session uuid from a rollout-<ts>-<uuid>.jsonl filename. */
function sessionIdFromFile(filePath: string): string | undefined {
  const match = /rollout-.*?-([0-9a-fA-F-]{36})\.jsonl$/.exec(basename(filePath));
  return match?.[1];
}

/**
 * A record's own id if it has one, else a stable per-file line index. Note
 * call_id is deliberately NOT used: a function_call and its
 * function_call_output share a call_id, so keying on it would collapse the
 * two into one message and dedupe the output away.
 */
function stableRecordId(body: Record<string, unknown>, lineIndex: number): string {
  if (typeof body["id"] === "string") return body["id"] as string;
  return `L${lineIndex}`;
}

/** function_call_output.output is sometimes a JSON string wrapping {output}. */
function normalizeOutput(output: unknown): string {
  if (typeof output === "string") return output;
  return contentToText(output) || JSON.stringify(output ?? null);
}
