// Sample lines lifted from a real ~/.claude/projects/**/*.jsonl transcript
// (ids and text trimmed), plus the noise record types that share the file.
// The expected parse is the fixture's expected.json.

use std::path::Path;

use serde_json::{Value, json};
use taskrunner::ingest::claude_code::ClaudeCodeParser;
use taskrunner::ingest::parser::{FileContext, ParsedMessage, TranscriptParser};

use crate::{expected, fixture_lines, parse_fixture};

const FIXTURE: &str = "claude-code/session.jsonl";

fn ctx() -> FileContext {
    FileContext::new(Path::new("/x/session.jsonl"))
}

fn parse(line: &str, ctx: &mut FileContext) -> Vec<ParsedMessage> {
    ClaudeCodeParser.parse(line, ctx)
}

fn lines() -> (String, String, String, Vec<String>) {
    let mut lines = fixture_lines(FIXTURE).into_iter();
    let user = lines.next().unwrap();
    let assistant = lines.next().unwrap();
    let tool_result = lines.next().unwrap();
    (user, assistant, tool_result, lines.collect())
}

#[test]
fn parses_the_whole_fixture_to_the_messages_expected_json_records() {
    assert_eq!(parse_fixture(&ClaudeCodeParser, FIXTURE), expected("claude-code/expected.json"));
}

#[test]
fn parses_a_plain_user_message() {
    let (user, ..) = lines();
    assert_eq!(
        parse(&user, &mut ctx()),
        vec![ParsedMessage {
            native_session_id: "s1".into(),
            native_record_id: "u1".into(),
            role: "user".into(),
            kind: "message".into(),
            content: "hello there".into(),
            native_ts: Some("2026-01-01T00:00:00Z".into()),
            project_path: Some("/repo".into()),
        }]
    );
}

#[test]
fn expands_assistant_content_blocks_with_per_block_record_ids() {
    let (_, assistant, ..) = lines();
    let out = parse(&assistant, &mut ctx());
    let ids: Vec<(&str, &str)> =
        out.iter().map(|m| (m.native_record_id.as_str(), m.kind.as_str())).collect();
    assert_eq!(ids, vec![("a1#0", "reasoning"), ("a1#1", "message"), ("a1#2", "tool_use")]);
    assert_eq!(out[0].content, "let me think");
    let call: Value = serde_json::from_str(&out[2].content).unwrap();
    assert_eq!(call, json!({ "id": "t1", "name": "Bash", "input": { "command": "ls" } }));
}

#[test]
fn maps_tool_result_blocks_to_a_tool_role() {
    let (_, _, tool_result, _) = lines();
    let out = parse(&tool_result, &mut ctx());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].role, "tool");
    assert_eq!(out[0].kind, "tool_result");
    assert_eq!(out[0].native_record_id, "u2#0");
    let blob: Value = serde_json::from_str(&out[0].content).unwrap();
    assert_eq!(blob["tool_use_id"], "t1");
    assert_eq!(blob["content"], "file.txt");
}

#[test]
fn skips_noise_and_unknown_record_types() {
    let (_, _, _, rest) = lines();
    let noise = [&rest[0], &rest[1], &rest[2], &rest[3], &rest[5], &rest[6]];
    for line in noise {
        assert_eq!(parse(line, &mut ctx()), vec![], "{line}");
    }
}

#[test]
fn skips_empty_redacted_thinking_blocks() {
    let (_, _, _, rest) = lines();
    assert_eq!(parse(&rest[4], &mut ctx()), vec![]);
}

#[test]
fn carries_session_and_cwd_forward_via_context() {
    let (user, ..) = lines();
    let mut c = ctx();
    parse(&user, &mut c);
    assert_eq!(c.session_id.as_deref(), Some("s1"));
    assert_eq!(c.project_path.as_deref(), Some("/repo"));
}
