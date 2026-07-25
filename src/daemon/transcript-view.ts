import type { TranscriptMessage } from "../domain/tasks.js";

// How an archived transcript is rendered. Two views over the same messages,
// chosen by the caller rather than by which surface they arrived through:
//
//   compact   one truncated line per message — a scan, and what the MCP tools
//             have always returned.
//   timeline  the audit view: prompts, replies and reasoning in full, tool
//             bodies clipped by line count. Bodies are printed unindented and
//             unmodified so code and diffs stay copy-pasteable.

export type TranscriptView = "compact" | "timeline";

export const TRANSCRIPT_VIEWS: TranscriptView[] = ["compact", "timeline"];

/** Lines of a tool body kept in the timeline; 0 keeps all of them. */
export const DEFAULT_TOOL_LINES = 20;

export function isTranscriptView(value: string): value is TranscriptView {
  return (TRANSCRIPT_VIEWS as string[]).includes(value);
}

/** Renders a session's messages as body lines, without any header. */
export function renderMessages(
  messages: TranscriptMessage[],
  view: TranscriptView,
  toolLines: number = DEFAULT_TOOL_LINES,
): string[] {
  return view === "timeline" ? timelineLines(messages, toolLines) : compactLines(messages);
}

function compactLines(messages: TranscriptMessage[]): string[] {
  return messages.map((m) => {
    const ts = m.native_ts ? `${m.native_ts}  ` : "";
    return `  ${ts}${m.role}/${m.kind}  ${compactMessageContent(m.content)}`;
  });
}

/**
 * One header line per message followed by its body. The prompt index is stamped
 * on the first message of each exchange only — that is the address `--prompt N`
 * takes, and repeating it on every line would be noise.
 */
function timelineLines(messages: TranscriptMessage[], toolLines: number): string[] {
  const lines: string[] = [];
  let group: number | undefined;
  for (const m of messages) {
    const opensExchange = m.prompt_idx !== group && m.prompt_idx > 0;
    group = m.prompt_idx;
    const addr = opensExchange ? `[${m.prompt_idx}] ` : "";
    const ts = m.native_ts ? `  ${m.native_ts}` : "";
    lines.push("", `── ${addr}${headerLabel(m)}${ts}`);
    const body = messageBody(m, toolLines);
    if (body !== "") lines.push(body);
  }
  return lines.slice(1); // drop the leading separator
}

function headerLabel(m: TranscriptMessage): string {
  if (m.kind === "tool_use") return `${m.role} · ${m.tool_name ?? "tool"}`;
  if (m.kind === "tool_result") return `tool · result${m.is_error === 1 ? " ✗" : ""}`;
  return m.kind === "message" ? m.role : `${m.role} · ${m.kind}`;
}

/**
 * What the conversation said is never clipped: the user's prompts, the
 * assistant's replies and reasoning. Everything else is harness furniture —
 * tool payloads, and the developer/system preambles a worker session opens with,
 * which run to thousands of lines — so it shares the tool-body budget.
 */
function messageBody(m: TranscriptMessage, toolLines: number): string {
  if (m.kind === "tool_use") return clipLines(toolUseBody(m), toolLines);
  if (m.kind === "tool_result") return clipLines(toolResultBody(m.content), toolLines);
  const body = m.content.trimEnd();
  return m.role === "user" || m.role === "assistant" ? body : clipLines(body, toolLines);
}

/**
 * A call's input, with the target hoisted to the first line. Remaining keys are
 * printed as-is rather than dropped: for Write and Edit the payload *is* the
 * audit record, and `--tool-lines` already bounds it.
 */
function toolUseBody(m: TranscriptMessage): string {
  const blob = parseObject(m.content);
  const input = blob ? parseObject(blob["input"] ?? blob["arguments"]) : null;
  if (!input) return m.tool_target ?? "";
  const lines: string[] = [];
  if (m.tool_target !== null) lines.push(m.tool_target);
  for (const [key, value] of Object.entries(input)) {
    const text = renderValue(value);
    // Skip the key the target was taken from; its value is already line 1.
    if (m.tool_target !== null && collapse(text) === m.tool_target) continue;
    lines.push(text.includes("\n") ? `${key}:\n${text}` : `${key}: ${text}`);
  }
  return lines.join("\n").trimEnd();
}

/** A result's payload text, under whichever key the harness used. */
function toolResultBody(content: string): string {
  const blob = parseObject(content);
  if (!blob) return content.trimEnd();
  const payload = blob["content"] ?? blob["output"];
  if (payload === undefined) return renderValue(blob).trimEnd();
  return renderValue(payload).trimEnd();
}

/** Strings verbatim; block arrays flattened to their text; anything else JSON. */
function renderValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    const texts = value.map((v) =>
      v && typeof v === "object" && typeof (v as Record<string, unknown>)["text"] === "string"
        ? ((v as Record<string, unknown>)["text"] as string)
        : JSON.stringify(v),
    );
    return texts.join("\n");
  }
  return JSON.stringify(value, null, 2) ?? String(value);
}

function clipLines(text: string, max: number): string {
  if (max <= 0) return text;
  const lines = text.split("\n");
  if (lines.length <= max) return text;
  const dropped = lines.length - max;
  return [...lines.slice(0, max), `… ${dropped} more lines (tool-lines 0 shows all)`].join("\n");
}

/** Message content is plain text or a JSON-encoded block (tool_use/tool_result);
 *  compact either to one readable line. */
export function compactMessageContent(content: string): string {
  const trimmed = content.trimStart();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      return compactPayload(JSON.parse(content));
    } catch {
      // Not actually JSON — fall through to plain-text truncation.
    }
  }
  return truncate(content, 160);
}

export function compactPayload(payload: unknown): string {
  if (payload && typeof payload === "object") {
    const obj = payload as Record<string, unknown>;
    const item = (obj["item"] as Record<string, unknown> | undefined) ?? obj;
    for (const key of ["text", "message", "command", "path"]) {
      if (typeof item[key] === "string") return truncate(item[key] as string, 160);
    }
  }
  if (typeof payload === "string") return truncate(payload, 160);
  return truncate(JSON.stringify(payload) ?? "null", 160);
}

export function truncate(text: string, max: number): string {
  const line = collapse(text);
  return line.length <= max ? line : `${line.slice(0, max - 1)}…`;
}

function collapse(text: string): string {
  return text.replaceAll(/\s+/g, " ").trim();
}

/** JSON objects arrive both parsed and as nested strings (codex `arguments`). */
function parseObject(value: unknown): Record<string, unknown> | null {
  if (typeof value === "string") {
    try {
      return parseObject(JSON.parse(value));
    } catch {
      return null;
    }
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return null;
}
