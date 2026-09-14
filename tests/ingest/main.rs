//! Ingest tests: the parsers, the sweeper, and worker-volume copy-out.

#[path = "../helpers/mod.rs"]
mod helpers;

mod claude_code;
mod codex;
mod sweep;
mod volume;

use std::path::PathBuf;

use taskrunner::ingest::parser::{FileContext, ParsedMessage, TranscriptParser};

/// Absolute path of a file under tests/fixtures.
pub fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(relative)
}

/// The lines of a fixture transcript, in file order (a trailing newline is
/// not a line).
pub fn fixture_lines(relative: &str) -> Vec<String> {
    let text = std::fs::read_to_string(fixture_path(relative)).unwrap();
    text.strip_suffix('\n').unwrap_or(&text).split('\n').map(str::to_string).collect()
}

/// Parses a whole fixture the way the sweeper does — one context per file,
/// line index advancing per line — for comparison with the fixture's
/// expected.json.
pub fn parse_fixture(parser: &dyn TranscriptParser, relative: &str) -> Vec<ParsedMessage> {
    let mut ctx = FileContext::new(&fixture_path(relative));
    let mut out = Vec::new();
    for (line_index, line) in fixture_lines(relative).iter().enumerate() {
        ctx.line_index = line_index;
        out.extend(parser.parse(line, &mut ctx));
    }
    out
}

pub fn expected(relative: &str) -> Vec<ParsedMessage> {
    serde_json::from_str(&std::fs::read_to_string(fixture_path(relative)).unwrap()).unwrap()
}
