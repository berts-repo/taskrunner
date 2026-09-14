import type { OutlineEntry, SessionOutline, TranscriptMessage } from "../domain/tasks.js";

// How an archived transcript is rendered. Three views over the same session,
// chosen by the caller rather than by which surface they arrived through:
//
//   outline   the scannable index: one line per prompt, reply and tool call,
//             each exchange addressed by [N] so it can be drilled into. No
//             message bodies — the whole point is that it is cheap to read.
//   compact   one truncated line per message — every message, one scan.
//   timeline  the audit view: prompts, replies and reasoning in full, tool
//             bodies clipped by line count. Bodies are printed unindented and
//             unmodified so code and diffs stay copy-pasteable.

export type TranscriptView = "outline" | "compact" | "timeline";

export const TRANSCRIPT_VIEWS: TranscriptView[] = ["outline", "compact", "timeline"];

/** Lines of a tool body kept in the timeline; 0 keeps all of them. */
export const DEFAULT_TOOL_LINES = 20;

export function isTranscriptView(value: string): value is TranscriptView {
  return (TRANSCRIPT_VIEWS as string[]).includes(value);
}

/** The views that render messages. The outline never loads a message body, so
 *  it is fed by its own query and cannot share this path. */
export type MessageView = Exclude<TranscriptView, "outline">;

/** Renders a session's messages as body lines, without any header. */
export function renderMessages(
  messages: TranscriptMessage[],
  view: MessageView,
  toolLines: number = DEFAULT_TOOL_LINES,
): string[] {
  return view === "timeline" ? timelineLines(messages, toolLines) : compactLines(messages);
}

/** Width the tool column is padded to; past it the target simply follows. */
const TOOL_COLUMN = 14;
/** One outline line is one line: prose and targets are collapsed to this. */
const OUTLINE_WIDTH = 140;

/**
 * The session as an index rather than a transcript: a counts line, then one
 * block per exchange headed by its `[N]` address. Every tool call gets its own
 * line, in the order it was made — a collapsed or capped list would stop the
 * outline being a complete index of what happened.
 */
export function renderOutline(outline: SessionOutline): string[] {
  const lines = [
    `${outline.message_count} messages · ${outline.prompt_count} prompts · ` +
      `${outline.tool_count} tool calls`,
  ];
  let group: number | undefined;
  // The records before a session's first real prompt are usually all harness
  // furniture, so that heading waits until something is actually filed under it.
  let pending: string | null = null;
  for (const e of outline.entries) {
    if (e.prompt_idx !== group) {
      group = e.prompt_idx;
      pending = null; // the previous group ended without filing anything
      const header = groupHeader(e);
      if (e.prompt_idx === 0) pending = header;
      else lines.push("", header);
    }
    // A user record that does not open a group is one the harness wrote on the
    // user's behalf (see message-facts.ts) — noise in an index.
    if (e.role === "user") continue;
    const line = entryLine(e);
    if (line === null) continue;
    if (pending !== null) {
      lines.push("", pending);
      pending = null;
    }
    lines.push(line);
  }
  return lines;
}

/** `[N] HH:MM  the prompt`, or a standing heading for the records that precede a
 *  session's first real prompt (a worker session may have none at all). */
function groupHeader(e: OutlineEntry): string {
  const at = clock(e.native_ts);
  if (e.prompt_idx === 0) return `[0]${at}  (before the first prompt)`;
  const prompt = e.role === "user" && e.head !== null ? truncate(e.head, OUTLINE_WIDTH) : "";
  return `[${e.prompt_idx}]${at}  ${prompt}`;
}

/** A reply as `→ text`, a call as `name  target`; anything else is left out. */
function entryLine(e: OutlineEntry): string | null {
  if (e.kind === "tool_use") {
    const target = e.tool_target === null ? "" : truncate(e.tool_target, OUTLINE_WIDTH);
    const name = (e.tool_name ?? "tool").padEnd(TOOL_COLUMN);
    return `    ${`${name}${target}`.trimEnd()}${e.is_error === 1 ? "  ✗" : ""}`;
  }
  if (e.role === "assistant" && e.kind === "message" && e.head !== null) {
    const text = truncate(e.head, OUTLINE_WIDTH);
    return text === "" ? null : `    → ${text}`;
  }
  return null; // reasoning, developer preambles, system records
}

/** Time of day from an ISO timestamp; sessions rarely span days and the header
 *  already carries the date. */
function clock(nativeTs: string | null): string {
  const at = nativeTs?.slice(11, 16);
  return at !== undefined && /^\d\d:\d\d$/.test(at) ? ` ${at}` : "";
}

function compactLines(messages: TranscriptMessage[]): string[] {
  return messages.map((m) => {
    const ts = m.native_ts ? `${m.native_ts}  ` : "";
    return `  ${ts}${displayRole(m)}/${m.kind}  ${compactMessageContent(m.content)}`;
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
  if (m.kind === "tool_use") return `${displayRole(m)} · ${m.tool_name ?? "tool"}`;
  if (m.kind === "tool_result") return `tool · result${m.is_error === 1 ? " ✗" : ""}`;
  const role = displayRole(m);
  return m.kind === "message" ? role : `${role} · ${m.kind}`;
}

/** The transcript role says only "assistant". The source records which
 * harness wrote it, so use that durable attribution in every rendered view. */
function displayRole(m: TranscriptMessage): string {
  return m.role === "assistant" ? harnessName(m.source) : m.role;
}

function harnessName(source: string): string {
  const known: Record<string, string> = {
    "claude-code": "Claude",
    claude: "Claude",
    codex: "Codex",
    hermes: "Hermes",
    openclaw: "OpenClaw",
    "open-claw": "OpenClaw",
  };
  const normalized = source.toLowerCase();
  if (known[normalized]) return known[normalized];
  return source
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}`)
    .join(" ") || "Assistant";
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
