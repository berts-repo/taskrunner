//! The single table that knows which transcript formats have parser code.
//! Adding a new source (e.g. Hermes/OpenClaw) is a config entry naming its
//! `format` plus one parser registered here — never a sweeper change.

use super::claude_code::ClaudeCodeParser;
use super::codex::CodexParser;
use super::parser::TranscriptParser;

pub fn parser_for_format(format: &str) -> Option<&'static dyn TranscriptParser> {
    match format {
        "claude-code" => Some(&ClaudeCodeParser),
        "codex" => Some(&CodexParser),
        _ => None,
    }
}
