//! Parses Codex transcripts under ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl.
//! Unlike Claude Code, individual records do not repeat the session id: a
//! session_meta header at the top of the file establishes it (with the cwd),
//! and the rollout filename (rollout-<ts>-<uuid>.jsonl) carries it too as a
//! resume-safe fallback. Records are commonly wrapped as { type, payload,
//! timestamp }; older versions flatten the payload, so both shapes are read.
//!
//! Conversation lives in response_item records (message / function_call /
//! function_call_output / reasoning). event_msg records duplicate that stream
//! for the live UI and are skipped; turn_context only carries cwd updates.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::parser::{
    FileContext, ParsedMessage, TranscriptParser, content_to_text, find_jsonl_files, json_of_keys,
};
use crate::js;

pub struct CodexParser;

impl TranscriptParser for CodexParser {
    fn format(&self) -> &'static str {
        "codex"
    }

    fn enumerate(&self, dirs: &[PathBuf]) -> Vec<PathBuf> {
        find_jsonl_files(dirs)
            .into_iter()
            .filter(|f| {
                f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-"))
            })
            .collect()
    }

    fn parse(&self, line: &str, ctx: &mut FileContext) -> Vec<ParsedMessage> {
        let trimmed = js::trim(line);
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else { return vec![] };
        // payload nests the record body in current Codex; flat records expose
        // it at the top level. Read whichever carries the fields.
        let body = match record.get("payload") {
            Some(payload) if !payload.is_null() => payload,
            _ => &record,
        };
        let string_at = |value: &Value, keys: &[&str]| {
            keys.iter()
                .find_map(|key| value.get(*key))
                .filter(|v| !v.is_null())
                .and_then(Value::as_str)
                .map(str::to_string)
        };

        match record.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(id) = string_at(body, &["id", "session_id", "conversation_id"]) {
                    ctx.session_id = Some(id);
                }
                if let Some(cwd) = string_at(body, &["cwd", "cwd_path"]) {
                    ctx.project_path = Some(cwd);
                }
                if ctx.session_id.is_none() {
                    ctx.session_id = session_id_from_file(&ctx.file_path);
                }
                return vec![];
            }
            Some("turn_context") => {
                if let Some(cwd) = string_at(body, &["cwd", "cwd_path"]) {
                    ctx.project_path = Some(cwd);
                }
                return vec![];
            }
            Some("response_item") => {}
            _ => return vec![], // event_msg is a live-UI duplicate of response_item
        }

        if ctx.session_id.is_none() {
            ctx.session_id = session_id_from_file(&ctx.file_path);
        }
        let Some(session_id) = ctx.session_id.clone() else { return vec![] };
        let message = |role: &str, kind: &str, content: String| {
            vec![ParsedMessage {
                native_session_id: session_id.clone(),
                native_record_id: stable_record_id(body, ctx.line_index),
                role: role.to_string(),
                kind: kind.to_string(),
                content,
                native_ts: record
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string),
                project_path: ctx.project_path.clone().filter(|p| !p.is_empty()),
            }]
        };

        match body.get("type").and_then(Value::as_str) {
            Some("message") => {
                let text = body.get("content").map(content_to_text).unwrap_or_default();
                if text.is_empty() {
                    return vec![];
                }
                let role = body.get("role").and_then(Value::as_str).unwrap_or("assistant");
                message(role, "message", text)
            }
            Some("function_call") => message(
                "assistant",
                "tool_use",
                json_of_keys(body, &["call_id", "name", "arguments"]),
            ),
            Some("function_call_output") => {
                let mut result = serde_json::Map::new();
                if let Some(call_id) = body.get("call_id") {
                    result.insert("call_id".into(), call_id.clone());
                }
                result.insert("output".into(), Value::String(normalize_output(body.get("output"))));
                message("tool", "tool_result", Value::Object(result).to_string())
            }
            Some("reasoning") => {
                // `summary ?? content`.
                let source =
                    body.get("summary").filter(|s| !s.is_null()).or_else(|| body.get("content"));
                let text = source.map(content_to_text).unwrap_or_default();
                if text.is_empty() {
                    return vec![];
                }
                message("assistant", "reasoning", text)
            }
            _ => vec![],
        }
    }
}

/// Extracts the session uuid from a rollout-<ts>-<uuid>.jsonl filename.
fn session_id_from_file(file_path: &Path) -> Option<String> {
    let name = file_path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".jsonl")?;
    let split = stem.len().checked_sub(36)?;
    let (prefix, uuid) = stem.split_at_checked(split)?;
    let uuid_shaped = uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if uuid_shaped
        && prefix.starts_with("rollout-")
        && prefix.ends_with('-')
        && prefix.len() > "rollout-".len()
    {
        Some(uuid.to_string())
    } else {
        None
    }
}

/// A record's own id if it has one, else a stable per-file line index. Note
/// call_id is deliberately NOT used: a function_call and its
/// function_call_output share a call_id, so keying on it would collapse the
/// two into one message and dedupe the output away.
fn stable_record_id(body: &Value, line_index: usize) -> String {
    match body.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => format!("L{line_index}"),
    }
}

/// function_call_output.output is sometimes a JSON string wrapping {output}.
fn normalize_output(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(other) => {
            let text = content_to_text(other);
            if text.is_empty() { other.to_string() } else { text }
        }
        None => "null".to_string(),
    }
}
