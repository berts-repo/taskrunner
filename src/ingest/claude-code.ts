import {
  contentToText,
  findJsonlFiles,
  type FileContext,
  type ParsedMessage,
  type TranscriptParser,
} from "./parser.js";

// Parses Claude Code transcripts under ~/.claude/projects/<slug>/*.jsonl
// (including subagents/). Each record is self-describing — it carries its
// own uuid, sessionId, cwd, and timestamp — so no cross-line state is
// needed, but ctx is still refreshed for consistency with formats that do.
//
// A record's message.content is either a plain string (a user turn) or an
// array of blocks (text / thinking / tool_use for assistant turns,
// tool_result for tool returns). One record therefore expands to several
// messages; block index disambiguates their record ids.
//
// The format is unversioned and carries many non-conversation record types
// (queue-operation, attachment, last-prompt, file-history-snapshot,
// ai-title, …); anything that is not user/assistant/system is skipped.

interface ClaudeRecord {
  type?: string;
  uuid?: string;
  sessionId?: string;
  cwd?: string;
  timestamp?: string;
  message?: { role?: string; content?: unknown };
}

const CONVERSATION_TYPES = new Set(["user", "assistant", "system"]);

export const claudeCodeParser: TranscriptParser = {
  format: "claude-code",

  enumerate(dirs: string[]): string[] {
    return findJsonlFiles(dirs);
  },

  parse(line: string, ctx: FileContext): ParsedMessage[] {
    const trimmed = line.trim();
    if (trimmed === "") return [];
    let record: ClaudeRecord;
    try {
      record = JSON.parse(trimmed) as ClaudeRecord;
    } catch {
      return [];
    }
    if (!record.type || !CONVERSATION_TYPES.has(record.type)) return [];

    if (typeof record.sessionId === "string") ctx.sessionId = record.sessionId;
    if (typeof record.cwd === "string") ctx.projectPath = record.cwd;
    const sessionId = ctx.sessionId;
    const recordId = record.uuid;
    if (!sessionId || !recordId) return []; // cannot key a message without both

    const common = {
      nativeSessionId: sessionId,
      ...(record.timestamp ? { nativeTs: record.timestamp } : {}),
      ...(ctx.projectPath ? { projectPath: ctx.projectPath } : {}),
    };

    if (record.type === "system") {
      const text = contentToText(record.message?.content ?? (record as { content?: unknown }).content);
      if (text === "") return [];
      return [{ ...common, nativeRecordId: recordId, role: "system", kind: "system", content: text }];
    }

    const role = record.message?.role ?? record.type;
    const content = record.message?.content;

    if (typeof content === "string") {
      if (content === "") return [];
      return [{ ...common, nativeRecordId: recordId, role, kind: "message", content }];
    }
    if (!Array.isArray(content)) return [];

    const out: ParsedMessage[] = [];
    content.forEach((block, i) => {
      const parsed = parseBlock(block, role);
      if (parsed) out.push({ ...common, nativeRecordId: `${recordId}#${i}`, ...parsed });
    });
    return out;
  },
};

/** Maps one content block to {role, kind, content}, or null to skip. */
function parseBlock(
  block: unknown,
  recordRole: string,
): Pick<ParsedMessage, "role" | "kind" | "content"> | null {
  if (!block || typeof block !== "object") return null;
  const b = block as Record<string, unknown>;
  switch (b["type"]) {
    case "text": {
      const text = typeof b["text"] === "string" ? b["text"] : "";
      return text === "" ? null : { role: recordRole, kind: "message", content: text };
    }
    case "thinking": {
      const text = typeof b["thinking"] === "string" ? b["thinking"] : "";
      return text === "" ? null : { role: recordRole, kind: "reasoning", content: text };
    }
    case "tool_use":
      return {
        role: recordRole,
        kind: "tool_use",
        content: JSON.stringify({ id: b["id"], name: b["name"], input: b["input"] }),
      };
    case "tool_result":
      return {
        role: "tool",
        kind: "tool_result",
        content: JSON.stringify({
          tool_use_id: b["tool_use_id"],
          is_error: b["is_error"] ?? false,
          content: contentToText(b["content"]),
        }),
      };
    default:
      return null;
  }
}
