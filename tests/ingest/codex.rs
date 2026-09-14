// One rollout file in the shape Codex writes, with its expected parse in
// expected.json. The filename carries the session id.

use serde_json::{Value, json};
use taskrunner::ingest::codex::CodexParser;
use taskrunner::ingest::parser::{FileContext, ParsedMessage, TranscriptParser};

use crate::{expected, fixture_lines, fixture_path, parse_fixture};

const FIXTURE: &str =
    "codex/rollout-2026-01-01T00-00-00-11111111-2222-3333-4444-555555555555.jsonl";

fn ctx(line_index: usize) -> FileContext {
    FileContext { line_index, ..FileContext::new(&fixture_path(FIXTURE)) }
}

fn parse(line: &str, ctx: &mut FileContext) -> Vec<ParsedMessage> {
    CodexParser.parse(line, ctx)
}

struct Lines {
    meta: String,
    message: String,
    call: String,
    output: String,
    reasoning: String,
    event_msg: String,
    turn_context: String,
}

fn lines() -> Lines {
    let mut it = fixture_lines(FIXTURE).into_iter();
    let mut next = || it.next().unwrap();
    Lines {
        meta: next(),
        message: next(),
        call: next(),
        output: next(),
        reasoning: next(),
        event_msg: next(),
        turn_context: next(),
    }
}

#[test]
fn parses_the_whole_fixture_to_the_messages_expected_json_records() {
    assert_eq!(parse_fixture(&CodexParser, FIXTURE), expected("codex/expected.json"));
}

#[test]
fn reads_session_id_and_cwd_from_session_meta_without_emitting() {
    let mut c = ctx(0);
    assert_eq!(parse(&lines().meta, &mut c), vec![]);
    assert_eq!(c.session_id.as_deref(), Some("cs1"));
    assert_eq!(c.project_path.as_deref(), Some("/proj"));
}

#[test]
fn parses_a_message_once_the_session_is_known() {
    let l = lines();
    let mut c = ctx(0);
    parse(&l.meta, &mut c);
    c.line_index = 1;
    assert_eq!(
        parse(&l.message, &mut c),
        vec![ParsedMessage {
            native_session_id: "cs1".into(),
            native_record_id: "L1".into(),
            role: "user".into(),
            kind: "message".into(),
            content: "do it".into(),
            native_ts: Some("2026-01-01T00:00:01Z".into()),
            project_path: Some("/proj".into()),
        }]
    );
}

#[test]
fn gives_a_function_call_and_its_output_distinct_record_ids() {
    let l = lines();
    let mut c = ctx(0);
    parse(&l.meta, &mut c);
    c.line_index = 2;
    let call = parse(&l.call, &mut c);
    c.line_index = 3;
    let output = parse(&l.output, &mut c);
    // Both carry call_id "c1"; keying on line index keeps them separate so the
    // output is not deduped away as a copy of the call.
    assert_eq!(call[0].native_record_id, "L2");
    assert_eq!(output[0].native_record_id, "L3");
    assert_eq!(call[0].kind, "tool_use");
    assert_eq!(output[0].kind, "tool_result");
    let blob: Value = serde_json::from_str(&output[0].content).unwrap();
    assert_eq!(blob, json!({ "call_id": "c1", "output": "file.txt" }));
}

#[test]
fn maps_reasoning_summaries() {
    let l = lines();
    let mut c = ctx(0);
    parse(&l.meta, &mut c);
    c.line_index = 4;
    let out = parse(&l.reasoning, &mut c);
    assert_eq!(
        (out[0].kind.as_str(), out[0].content.as_str(), out[0].role.as_str()),
        ("reasoning", "hmm", "assistant")
    );
}

#[test]
fn skips_event_msg_duplicates_and_turn_context_but_reads_its_cwd() {
    let l = lines();
    let mut c = ctx(0);
    parse(&l.meta, &mut c);
    assert_eq!(parse(&l.event_msg, &mut c), vec![]);
    assert_eq!(parse(&l.turn_context, &mut c), vec![]);
    assert_eq!(c.project_path.as_deref(), Some("/proj2"));
}

#[test]
fn falls_back_to_the_session_id_in_the_filename_when_meta_is_missing() {
    let out = parse(&lines().message, &mut ctx(0));
    assert_eq!(out[0].native_session_id, "11111111-2222-3333-4444-555555555555");
}

#[test]
fn enumerate_keeps_only_rollout_jsonl_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("rollout-a.jsonl"), "").unwrap();
    std::fs::write(dir.path().join("notes.jsonl"), "").unwrap();
    let names: Vec<String> = CodexParser
        .enumerate(&[dir.path().to_path_buf()])
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["rollout-a.jsonl"]);
    assert_eq!(CodexParser.format(), "codex");
}
