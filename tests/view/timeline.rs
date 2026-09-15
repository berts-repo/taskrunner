// Phase 2: the timeline view. The compact view is what every surface returned
// before this phase and must stay byte-identical; the timeline is the audit
// rendering — prose never clipped, tool bodies clipped by line count.

use serde_json::json;
use taskrunner::domain::tasks::{MessageLimit, MessageQuery, get_session_messages};
use taskrunner::storage::events::EventBody;
use taskrunner::storage::index::StateIndex;
use taskrunner::view::lookup::{SessionLookupArgs, ViewArgs, lookup_session};
use taskrunner::view::transcript::TranscriptView;

use crate::seed::{Clock, Msg, index_of, project_created};

struct Seed {
    clock: Clock,
    seq: Clock,
}

impl Seed {
    fn new() -> Seed {
        Seed { clock: Clock::new(), seq: Clock::new() }
    }

    fn ts(&self) -> String {
        let n = self.clock.tick();
        format!("2026-07-25T00:00:00.{:03}Z", n % 1000)
    }

    fn msg(&self, role: &str, kind: &str, content: &str) -> EventBody {
        Msg {
            source: "claude-code",
            session: "sess-T",
            record_id: format!("r{}", self.seq.tick()),
            role,
            kind,
            content: content.into(),
            native_ts: Some(self.ts()),
            project_path: Some("/repo"),
        }
        .body()
    }

    fn index(&self, bodies: Vec<EventBody>) -> StateIndex {
        index_of(bodies, || self.ts())
    }
}

fn long_prose() -> String {
    format!("A reply well past the compact view's 160-character budget: {}", "x".repeat(200))
}

fn numbered(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix} {i}")).collect::<Vec<_>>().join("\n")
}

/// One session: two real prompts, a harness-written user record, tools, an error.
fn seeded() -> StateIndex {
    let s = Seed::new();
    s.index(vec![
        project_created(),
        s.msg("user", "message", "first question"),
        s.msg("assistant", "message", &long_prose()),
        s.msg("assistant", "tool_use", &json!({ "id": "tu1", "name": "Read", "input": { "file_path": "/repo/a.ts" } }).to_string()),
        s.msg(
            "tool",
            "tool_result",
            &json!({ "tool_use_id": "tu1", "is_error": false, "content": numbered("line", 30) }).to_string(),
        ),
        // Harness-written: must not advance the prompt counter.
        s.msg("user", "message", "<system-reminder>ignore me</system-reminder>"),
        s.msg("user", "message", "second question"),
        s.msg(
            "assistant",
            "tool_use",
            &json!({ "id": "tu2", "name": "Bash", "input": { "command": "ls -la", "description": "list the tree" } }).to_string(),
        ),
        s.msg("tool", "tool_result", &json!({ "tool_use_id": "tu2", "is_error": true, "content": "boom" }).to_string()),
    ])
}

fn read(
    index: &StateIndex,
    view: Option<TranscriptView>,
    last: Option<i64>,
    prompt_idx: Option<i64>,
    tool_lines: Option<usize>,
) -> String {
    lookup_session(
        index,
        &SessionLookupArgs {
            session_id: Some("sess-T".into()),
            last,
            view: ViewArgs { view, tool_lines, prompt_idx },
            ..Default::default()
        },
    )
    .unwrap()
}

fn timeline(index: &StateIndex) -> String {
    read(index, Some(TranscriptView::Timeline), None, None, None)
}

#[test]
fn keeps_prose_whole_where_compact_truncates_it() {
    let index = seeded();
    assert!(timeline(&index).contains(&long_prose()));
    let compact = read(&index, Some(TranscriptView::Compact), None, None, None);
    assert!(!compact.contains(&long_prose()));
    assert!(compact.contains('…')); // truncation marker at 160 chars
}

#[test]
fn still_renders_compact_byte_for_byte_when_asked_for_it() {
    let index = seeded();
    // Phase 3 moved the default to the outline; scope.last still means compact,
    // so a caller that narrows a read gets the same messages it always did.
    assert_eq!(
        read(&index, None, Some(99), None, None),
        read(&index, Some(TranscriptView::Compact), Some(99), None, None)
    );
}

#[test]
fn renders_compact_lines_in_the_pre_phase_2_shape() {
    let out = read(&seeded(), Some(TranscriptView::Compact), None, None, None);
    let line = out.lines().find(|l| l.contains("user/message")).unwrap();
    let (prefix, rest) = line.split_once("  2026-07-25T").unwrap();
    assert_eq!(prefix, "");
    let (ts, tail) = rest.split_once("  ").unwrap();
    assert!(!ts.contains(' '));
    assert_eq!(tail, "user/message  first question");
}

#[test]
fn shows_the_program_a_codex_exec_call_ran() {
    // exec takes a JavaScript program as its input, not named arguments; the
    // program is the record of what the call did.
    let s = Seed::new();
    let program = "const r = await tools.exec_command({cmd:\"ls -la\"});\ntext(r);";
    let index = s.index(vec![
        project_created(),
        s.msg("user", "message", "list the tree"),
        s.msg(
            "assistant",
            "tool_use",
            &json!({ "call_id": "call_1", "name": "exec", "input": program }).to_string(),
        ),
    ]);
    let out = timeline(&index);
    assert!(out.contains("· exec"), "{out}");
    for line in program.lines() {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
}

#[test]
fn labels_a_tool_call_by_name_and_shows_its_target_and_remaining_input() {
    let out = timeline(&seeded());
    assert!(out.contains("── Claude · Read"));
    assert!(out.contains("/repo/a.ts"));
    assert!(out.contains("── Claude · Bash"));
    assert!(out.contains("ls -la"));
    assert!(out.contains("description: list the tree"));
}

#[test]
fn attributes_replies_to_the_harness_that_wrote_each_archived_session() {
    for (source, name) in [
        ("claude-code", "Claude"),
        ("codex", "Codex"),
        ("hermes", "Hermes"),
        ("openclaw", "OpenClaw"),
    ] {
        let s = Seed::new();
        let reply = Msg {
            source,
            session: "sess-T",
            record_id: "r1".into(),
            role: "assistant",
            kind: "message",
            content: "a reply".into(),
            native_ts: Some(s.ts()),
            project_path: Some("/repo"),
        }
        .body();
        let out = timeline(&s.index(vec![project_created(), reply]));
        assert!(out.contains(&format!("── {name}")), "{source}: {out}");
        assert!(!out.contains("── assistant"), "{source}: {out}");
    }
}

#[test]
fn marks_a_failed_tool_result_and_not_a_successful_one() {
    let out = timeline(&seeded());
    assert!(out.contains("── tool · result ✗"));
    // The other result carries no ✗ between the label and its timestamp.
    assert_eq!(out.matches("── tool · result  2026").count(), 1);
}

#[test]
fn caps_a_tool_body_by_line_count_and_0_caps_nothing() {
    let index = seeded();
    let capped = read(&index, Some(TranscriptView::Timeline), None, None, Some(20));
    assert!(capped.contains("line 20"));
    assert!(!capped.contains("line 21"));
    assert!(capped.contains("… 10 more lines"));

    let whole = read(&index, Some(TranscriptView::Timeline), None, None, Some(0));
    assert!(whole.contains("line 30"));
    assert!(!whole.contains("more lines"));
}

#[test]
fn addresses_exchanges_by_prompt_index_skipping_harness_written_records() {
    let index = seeded();
    let out = timeline(&index);
    assert!(out.contains("── [1] user"));
    assert!(out.contains("── [2] user"));
    assert!(!out.contains("[3]"));

    let one = read(&index, Some(TranscriptView::Timeline), None, Some(2), None);
    assert!(one.contains("second question"));
    assert!(one.contains("prompt 2"));
    assert!(!one.contains("first question"));
    // The harness-written record sits in exchange 1, not 2.
    assert!(!one.contains("ignore me"));
}

#[test]
fn clips_a_developer_preamble_but_never_the_conversation() {
    let s = Seed::new();
    let preamble = numbered("boilerplate", 40);
    let prose = numbered("said", 40);
    let index = s.index(vec![
        project_created(),
        s.msg("developer", "message", &preamble),
        s.msg("user", "message", &prose),
        s.msg("assistant", "message", &prose),
    ]);
    let out = timeline(&index);
    assert!(out.contains("boilerplate 20"));
    assert!(!out.contains("boilerplate 21"));
    assert!(out.contains("said 40")); // both prose messages survive whole
    assert_eq!(out.matches("said 40").count(), 2);
}

#[test]
fn reports_an_out_of_range_prompt_instead_of_an_empty_session() {
    let out = read(&seeded(), Some(TranscriptView::Timeline), None, Some(99), None);
    assert!(out.contains("(no messages at prompt 99)"));
}

// ---- message limits -----------------------------------------------------

fn long() -> StateIndex {
    let s = Seed::new();
    let mut bodies = vec![project_created(), s.msg("user", "message", "kick it off")];
    for i in 0..519 {
        bodies.push(s.msg("assistant", "message", &format!("reply {i}")));
    }
    s.index(bodies)
}

fn query(limit: MessageLimit) -> MessageQuery {
    MessageQuery { limit, prompt_idx: None }
}

#[test]
fn caps_compact_at_500_messages_and_leaves_the_timeline_uncapped() {
    let index = long();
    let compact =
        get_session_messages(&index, "claude-code", "sess-T", query(MessageLimit::Default))
            .unwrap();
    assert_eq!(compact.messages.len(), 500);
    assert!(compact.capped);

    let timeline =
        get_session_messages(&index, "claude-code", "sess-T", query(MessageLimit::All)).unwrap();
    assert_eq!(timeline.messages.len(), 520);
    assert!(!timeline.capped);
}

#[test]
fn honors_an_explicit_last_in_either_view() {
    let page =
        get_session_messages(&long(), "claude-code", "sess-T", query(MessageLimit::Newest(5)))
            .unwrap();
    assert_eq!(page.messages.len(), 5);
    assert!(page.capped);
    assert_eq!(page.messages.last().unwrap().content, "reply 518");
}

#[test]
fn reads_the_whole_session_in_the_timeline_through_lookup_session() {
    let out = timeline(&long());
    assert!(out.contains("reply 0")); // the oldest survives, uncapped
    assert!(!out.contains("capped at"));
}
