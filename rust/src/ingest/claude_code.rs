//! Parses Claude Code transcripts under ~/.claude/projects/<slug>/*.jsonl
//! (including subagents/). Each record is self-describing — it carries its
//! own uuid, sessionId, cwd, and timestamp — so no cross-line state is
//! needed, but ctx is still refreshed for consistency with formats that do.
//!
//! A record's message.content is either a plain string (a user turn) or an
//! array of blocks (text / thinking / tool_use for assistant turns,
//! tool_result for tool returns). One record therefore expands to several
//! messages; block index disambiguates their record ids.
//!
//! The format is unversioned and carries many non-conversation record types
//! (queue-operation, attachment, last-prompt, file-history-snapshot,
//! ai-title, …); anything that is not user/assistant/system is skipped.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::parser::{
    FileContext, ParsedMessage, TranscriptParser, content_to_text, find_jsonl_files, json_of_keys,
};
use crate::js;

const CONVERSATION_TYPES: [&str; 3] = ["user", "assistant", "system"];

pub struct ClaudeCodeParser;

impl TranscriptParser for ClaudeCodeParser {
    fn format(&self) -> &'static str {
        "claude-code"
    }

    fn enumerate(&self, dirs: &[PathBuf]) -> Vec<PathBuf> {
        find_jsonl_files(dirs)
    }

    fn parse(&self, line: &str, ctx: &mut FileContext) -> Vec<ParsedMessage> {
        let trimmed = js::trim(line);
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else { return vec![] };
        let Some(record_type) = record.get("type").and_then(Value::as_str) else { return vec![] };
        if !CONVERSATION_TYPES.contains(&record_type) {
            return vec![];
        }

        if let Some(session_id) = record.get("sessionId").and_then(Value::as_str) {
            ctx.session_id = Some(session_id.to_string());
        }
        if let Some(cwd) = record.get("cwd").and_then(Value::as_str) {
            ctx.project_path = Some(cwd.to_string());
        }
        let (Some(session_id), Some(record_id)) =
            (ctx.session_id.clone(), record.get("uuid").and_then(Value::as_str))
        else {
            return vec![]; // cannot key a message without both
        };
        let message = |record_id: String, role: &str, kind: &str, content: String| ParsedMessage {
            native_session_id: session_id.clone(),
            native_record_id: record_id,
            role: role.to_string(),
            kind: kind.to_string(),
            content,
            native_ts: record
                .get("timestamp")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
            project_path: ctx.project_path.clone().filter(|p| !p.is_empty()),
        };

        let message_field = record.get("message");
        if record_type == "system" {
            // `message.content ?? content`: older system records carry it at the top level.
            let content = message_field
                .and_then(|m| m.get("content"))
                .filter(|c| !c.is_null())
                .or_else(|| record.get("content"));
            let text = content.map(content_to_text).unwrap_or_default();
            if text.is_empty() {
                return vec![];
            }
            return vec![message(record_id.to_string(), "system", "system", text)];
        }

        let role = message_field
            .and_then(|m| m.get("role"))
            .and_then(Value::as_str)
            .unwrap_or(record_type);
        match message_field.and_then(|m| m.get("content")) {
            Some(Value::String(content)) => {
                if content.is_empty() {
                    vec![]
                } else {
                    vec![message(record_id.to_string(), role, "message", content.clone())]
                }
            }
            Some(Value::Array(blocks)) => blocks
                .iter()
                .enumerate()
                .filter_map(|(i, block)| {
                    let (block_role, kind, content) = parse_block(block, role)?;
                    Some(message(format!("{record_id}#{i}"), &block_role, kind, content))
                })
                .collect(),
            _ => vec![],
        }
    }
}

/// Maps one content block to (role, kind, content), or None to skip.
fn parse_block(block: &Value, record_role: &str) -> Option<(String, &'static str, String)> {
    let text_or_skip = |key: &str, kind: &'static str| {
        let text = block.get(key).and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() { None } else { Some((record_role.to_string(), kind, text.to_string())) }
    };
    match block.get("type").and_then(Value::as_str)? {
        "text" => text_or_skip("text", "message"),
        "thinking" => text_or_skip("thinking", "reasoning"),
        "tool_use" => Some((
            record_role.to_string(),
            "tool_use",
            json_of_keys(block, &["id", "name", "input"]),
        )),
        "tool_result" => {
            let mut result = serde_json::Map::new();
            if let Some(id) = block.get("tool_use_id") {
                result.insert("tool_use_id".into(), id.clone());
            }
            let is_error =
                block.get("is_error").filter(|v| !v.is_null()).cloned().unwrap_or(json!(false));
            result.insert("is_error".into(), is_error);
            let content = block.get("content").map(content_to_text).unwrap_or_default();
            result.insert("content".into(), Value::String(content));
            Some(("tool".to_string(), "tool_result", Value::Object(result).to_string()))
        }
        _ => None,
    }
}
