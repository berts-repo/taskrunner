//! How an archived transcript is rendered. Three views over the same session,
//! chosen by the caller rather than by which surface they arrived through:
//!
//!   outline   the scannable index: one line per prompt, reply and tool call,
//!             each exchange addressed by [N] so it can be drilled into. No
//!             message bodies — the whole point is that it is cheap to read.
//!   compact   one truncated line per message — every message, one scan.
//!   timeline  the audit view: prompts, replies and reasoning in full, tool
//!             bodies clipped by line count. Bodies are printed unindented and
//!             unmodified so code and diffs stay copy-pasteable.

use serde_json::Value;

use crate::domain::tasks::{OutlineEntry, SessionOutline, TranscriptMessage};
use crate::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptView {
    Outline,
    Compact,
    Timeline,
}

pub const TRANSCRIPT_VIEWS: [&str; 3] = ["outline", "compact", "timeline"];

impl TranscriptView {
    pub fn parse(value: &str) -> Option<TranscriptView> {
        match value {
            "outline" => Some(TranscriptView::Outline),
            "compact" => Some(TranscriptView::Compact),
            "timeline" => Some(TranscriptView::Timeline),
            _ => None,
        }
    }
}

/// The views that render messages. The outline never loads a message body, so
/// it is fed by its own query and cannot share this path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageView {
    Compact,
    Timeline,
}

/// Lines of a tool body kept in the timeline; 0 keeps all of them.
pub const DEFAULT_TOOL_LINES: usize = 20;

/// Renders a session's messages as body lines, without any header.
pub fn render_messages(
    messages: &[TranscriptMessage],
    view: MessageView,
    tool_lines: usize,
) -> Vec<String> {
    match view {
        MessageView::Timeline => timeline_lines(messages, tool_lines),
        MessageView::Compact => messages.iter().map(compact_line).collect(),
    }
}

/// Width the tool column is padded to; past it the target simply follows.
const TOOL_COLUMN: usize = 14;
/// One outline line is one line: prose and targets are collapsed to this.
const OUTLINE_WIDTH: usize = 140;

/// The session as an index rather than a transcript: a counts line, then one
/// block per exchange headed by its `[N]` address. Every tool call gets its
/// own line, in the order it was made — a collapsed or capped list would stop
/// the outline being a complete index of what happened.
pub fn render_outline(outline: &SessionOutline) -> Vec<String> {
    let mut lines = vec![format!(
        "{} messages · {} prompts · {} tool calls",
        outline.message_count, outline.prompt_count, outline.tool_count
    )];
    let mut group: Option<i64> = None;
    // The records before a session's first real prompt are usually all harness
    // furniture, so that heading waits until something is actually filed under it.
    let mut pending: Option<String> = None;
    for e in &outline.entries {
        if group != Some(e.prompt_idx) {
            group = Some(e.prompt_idx);
            pending = None; // the previous group ended without filing anything
            let header = group_header(e);
            if e.prompt_idx == 0 {
                pending = Some(header);
            } else {
                lines.push(String::new());
                lines.push(header);
            }
        }
        // A user record that does not open a group is one the harness wrote on
        // the user's behalf (see facts.rs) — noise in an index.
        if e.role == "user" {
            continue;
        }
        let Some(line) = entry_line(e) else { continue };
        if let Some(header) = pending.take() {
            lines.push(String::new());
            lines.push(header);
        }
        lines.push(line);
    }
    lines
}

/// `[N] HH:MM  the prompt`, or a standing heading for the records that precede
/// a session's first real prompt (a worker session may have none at all).
fn group_header(e: &OutlineEntry) -> String {
    let at = clock(e.native_ts.as_deref());
    if e.prompt_idx == 0 {
        return format!("[0]{at}  (before the first prompt)");
    }
    let prompt = match &e.head {
        Some(head) if e.role == "user" => truncate(head, OUTLINE_WIDTH),
        _ => String::new(),
    };
    format!("[{}]{at}  {prompt}", e.prompt_idx)
}

/// A reply as `→ text`, a call as `name  target`; anything else is left out.
fn entry_line(e: &OutlineEntry) -> Option<String> {
    if e.kind == "tool_use" {
        let target = e.tool_target.as_deref().map_or(String::new(), |t| truncate(t, OUTLINE_WIDTH));
        let name = js::pad_end(e.tool_name.as_deref().unwrap_or("tool"), TOOL_COLUMN);
        let failed = if e.is_error == Some(1) { "  ✗" } else { "" };
        return Some(format!("    {}{failed}", js::trim_end(&format!("{name}{target}"))));
    }
    if e.role == "assistant" && e.kind == "message" {
        let text = truncate(e.head.as_deref()?, OUTLINE_WIDTH);
        return if text.is_empty() { None } else { Some(format!("    → {text}")) };
    }
    None // reasoning, developer preambles, system records
}

/// Time of day from an ISO timestamp; sessions rarely span days and the header
/// already carries the date.
fn clock(native_ts: Option<&str>) -> String {
    let Some(ts) = native_ts else { return String::new() };
    let at: String = ts.chars().skip(11).take(5).collect();
    let is_hh_mm = at.len() == 5
        && at.chars().enumerate().all(|(i, c)| if i == 2 { c == ':' } else { c.is_ascii_digit() });
    if is_hh_mm { format!(" {at}") } else { String::new() }
}

fn compact_line(m: &TranscriptMessage) -> String {
    let ts = m.native_ts.as_deref().map_or(String::new(), |ts| format!("{ts}  "));
    format!("  {ts}{}/{}  {}", display_role(m), m.kind, compact_message_content(&m.content))
}

/// One header line per message followed by its body. The prompt index is
/// stamped on the first message of each exchange only — that is the address
/// `--prompt N` takes, and repeating it on every line would be noise.
fn timeline_lines(messages: &[TranscriptMessage], tool_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut group: Option<i64> = None;
    for m in messages {
        let opens_exchange = group != Some(m.prompt_idx) && m.prompt_idx > 0;
        group = Some(m.prompt_idx);
        let addr = if opens_exchange { format!("[{}] ", m.prompt_idx) } else { String::new() };
        let ts = m.native_ts.as_deref().map_or(String::new(), |ts| format!("  {ts}"));
        lines.push(String::new());
        lines.push(format!("── {addr}{}{ts}", header_label(m)));
        let body = message_body(m, tool_lines);
        if !body.is_empty() {
            lines.push(body);
        }
    }
    if !lines.is_empty() {
        lines.remove(0); // drop the leading separator
    }
    lines
}

fn header_label(m: &TranscriptMessage) -> String {
    let role = display_role(m);
    match m.kind.as_str() {
        "tool_use" => format!("{role} · {}", m.tool_name.as_deref().unwrap_or("tool")),
        "tool_result" => format!("tool · result{}", if m.is_error == Some(1) { " ✗" } else { "" }),
        "message" => role,
        kind => format!("{role} · {kind}"),
    }
}

fn display_role(m: &TranscriptMessage) -> String {
    role_label(&m.role, &m.source)
}

/// A transcript's role says only "assistant". The ingest source records which
/// harness wrote the reply, and in an archive that mixes harnesses that is the
/// name worth reading.
pub(crate) fn role_label(role: &str, source: &str) -> String {
    if role == "assistant" { harness_name(source) } else { role.to_string() }
}

/// Sources are free-form, so a name is its words capitalised; only the names
/// that rule gets wrong are listed.
fn harness_name(source: &str) -> String {
    match source {
        "claude-code" => "Claude".into(),
        "openclaw" => "OpenClaw".into(),
        _ => {
            let words: Vec<String> = source
                .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
                .filter(|word| !word.is_empty())
                .map(capitalise)
                .collect();
            if words.is_empty() { "Assistant".into() } else { words.join(" ") }
        }
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

/// What the conversation said is never clipped: the user's prompts, the
/// assistant's replies and reasoning. Everything else is harness furniture —
/// tool payloads, and the developer/system preambles a worker session opens
/// with, which run to thousands of lines — so it shares the tool-body budget.
fn message_body(m: &TranscriptMessage, tool_lines: usize) -> String {
    match m.kind.as_str() {
        "tool_use" => clip_lines(&tool_use_body(m), tool_lines),
        "tool_result" => clip_lines(&tool_result_body(&m.content), tool_lines),
        _ => {
            let body = js::trim_end(&m.content);
            if m.role == "user" || m.role == "assistant" {
                body.to_string()
            } else {
                clip_lines(body, tool_lines)
            }
        }
    }
}

/// A call's input, with the target hoisted to the first line. Remaining keys
/// are printed as-is rather than dropped: for Write and Edit the payload *is*
/// the audit record, and `--tool-lines` already bounds it.
fn tool_use_body(m: &TranscriptMessage) -> String {
    let input = js::parse_object_text(&m.content)
        .and_then(|blob| js::first_present(&blob, &["input", "arguments"]).cloned());
    // Codex's exec takes a program, not named arguments: the program is the body.
    if let Some(Value::String(program)) = &input
        && js::parse_object_text(program).is_none()
    {
        return js::trim_end(program).to_string();
    }
    let Some(input) = input.and_then(|value| js::parse_object(&value)) else {
        return m.tool_target.clone().unwrap_or_default();
    };
    let mut lines: Vec<String> = Vec::new();
    if let Some(target) = &m.tool_target {
        lines.push(target.clone());
    }
    for (key, value) in &input {
        let text = render_value(value);
        // Skip the key the target was taken from; its value is already line 1.
        if m.tool_target.as_deref() == Some(js::collapse_whitespace(&text).as_str()) {
            continue;
        }
        lines.push(if text.contains('\n') {
            format!("{key}:\n{text}")
        } else {
            format!("{key}: {text}")
        });
    }
    js::trim_end(&lines.join("\n")).to_string()
}

/// A result's payload text, under whichever key the harness used.
fn tool_result_body(content: &str) -> String {
    let Some(blob) = js::parse_object_text(content) else {
        return js::trim_end(content).to_string();
    };
    // `blob.content ?? blob.output`: a present-but-null output still renders
    // as "null"; only a missing one falls back to the whole blob.
    let payload = match blob.get("content").filter(|v| !v.is_null()) {
        Some(content) => Some(content),
        None => blob.get("output"),
    };
    let text = match payload {
        Some(payload) => render_value(payload),
        None => render_value(&Value::Object(blob.clone())),
    };
    js::trim_end(&text).to_string()
}

/// Strings verbatim; block arrays flattened to their text; anything else JSON.
fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block.get("text") {
                Some(Value::String(text)) => text.clone(),
                _ => block.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn clip_lines(text: &str, max: usize) -> String {
    if max == 0 {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= max {
        return text.to_string();
    }
    let dropped = lines.len() - max;
    let mut kept: Vec<String> = lines[..max].iter().map(|l| l.to_string()).collect();
    kept.push(format!("… {dropped} more lines (tool-lines 0 shows all)"));
    kept.join("\n")
}

/// Message content is plain text or a JSON-encoded block (tool_use/tool_result);
/// compact either to one readable line.
pub fn compact_message_content(content: &str) -> String {
    let trimmed = js::trim_start(content);
    let looks_like_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    // Something that merely looks like JSON falls through to text truncation.
    if looks_like_json && let Ok(payload) = serde_json::from_str::<Value>(content) {
        return compact_payload(&payload);
    }
    truncate(content, 160)
}

pub fn compact_payload(payload: &Value) -> String {
    if let Some(obj) = payload.as_object() {
        // `obj.item ?? obj`: a present non-object item has no text keys at all.
        let item = match obj.get("item").filter(|v| !v.is_null()) {
            Some(item) => item.as_object(),
            None => Some(obj),
        };
        for key in ["text", "message", "command", "path"] {
            if let Some(Value::String(text)) = item.and_then(|item| item.get(key)) {
                return truncate(text, 160);
            }
        }
    }
    if let Value::String(text) = payload {
        return truncate(text, 160);
    }
    truncate(&payload.to_string(), 160)
}

pub fn truncate(text: &str, max: usize) -> String {
    let line = js::collapse_whitespace(text);
    if js::len(&line) <= max { line } else { format!("{}…", js::slice_to(&line, max - 1)) }
}
