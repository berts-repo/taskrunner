//! Transcript parsers turn one native agent transcript (Claude Code, Codex, …)
//! into normalized conversation messages. A parser is pure and stateless
//! except for the per-file context it is handed: files are append-only per
//! session, so the sweeper can resume mid-file and replay the persisted
//! context instead of re-reading a session header it has already passed.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::js;

/// One normalized conversation record, ready to become a message.recorded.
/// Serialized in camelCase because the fixture `expected.json` files are
/// written by the TypeScript parsers and read by both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMessage {
    pub native_session_id: String,
    pub native_record_id: String,
    pub role: String,
    /// message | tool_use | tool_result | reasoning | system.
    pub kind: String,
    /// Plain text, or JSON-encoded structured payload.
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

/// Mutable per-file state. A parser reads session/project context from it and
/// writes back whatever a session-header record establishes, so later lines
/// (and resumed sweeps) inherit it. `line_index` is the 0-based index of the
/// line being parsed and is a stable fallback record id for formats whose
/// records carry none.
#[derive(Debug, Clone, Default)]
pub struct FileContext {
    pub file_path: PathBuf,
    pub line_index: usize,
    pub session_id: Option<String>,
    pub project_path: Option<String>,
}

impl FileContext {
    pub fn new(file_path: &Path) -> FileContext {
        FileContext { file_path: file_path.to_path_buf(), ..Default::default() }
    }
}

pub trait TranscriptParser: Send + Sync {
    fn format(&self) -> &'static str;
    /// Transcript files under the source dirs, in a stable sweep order.
    fn enumerate(&self, dirs: &[PathBuf]) -> Vec<PathBuf>;
    /// Parses one transcript line into zero or more messages, mutating `ctx`
    /// with any session/project context the line establishes. Returns nothing
    /// for noise, header-only, and unparseable lines — the format is
    /// unversioned, so unknown shapes are tolerated, never fatal.
    fn parse(&self, line: &str, ctx: &mut FileContext) -> Vec<ParsedMessage>;
}

/// `msg_` + a truncated sha256 of the natural key: stable across re-sweeps.
pub fn message_id(source: &str, native_session_id: &str, native_record_id: &str) -> String {
    let hash = Sha256::digest(format!("{source}\0{native_session_id}\0{native_record_id}"));
    format!("msg_{}", &format!("{hash:x}")[..32])
}

/// All `*.jsonl` files beneath the given dirs (recursive), sorted for a
/// deterministic sweep order. Missing dirs are skipped.
pub fn find_jsonl_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return }; // missing or unreadable: nothing to sweep
        for entry in entries.flatten() {
            // The entry's own type, so a symlink is neither walked nor swept.
            let Ok(file_type) = entry.file_type() else { continue };
            let path = entry.path();
            if file_type.is_dir() {
                walk(&path, out);
            } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".jsonl")
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        walk(dir, &mut out);
    }
    // Whole-path string order, as the TypeScript sweeper sorts: it decides
    // the order events land in the log.
    out.sort_by(|a, b| js::compare(&a.to_string_lossy(), &b.to_string_lossy()));
    out
}

/// Coerces a content value (string, {text}, or array of blocks) to text.
pub fn content_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Value::String(text) => Some(text.clone()),
                Value::Object(map) => map.get("text").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            map.get("text").and_then(Value::as_str).unwrap_or_default().to_string()
        }
        _ => String::new(),
    }
}

/// `JSON.stringify` of an object built from selected keys of `blob`: a key
/// that is absent stays absent, a key that is present but null stays null.
pub fn json_of_keys(blob: &Value, keys: &[&str]) -> String {
    let mut picked = serde_json::Map::new();
    for key in keys {
        if let Some(value) = blob.get(*key) {
            picked.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(picked).to_string()
}
