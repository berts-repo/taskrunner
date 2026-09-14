// Phase 3: the search-and-drill loop. The outline is an index of a session that
// never loads a body, search answers questions about tool calls without
// grepping JSON, and every result carries the prompt index it can be read at.

use serde_json::{Value, json};
use taskrunner::domain::tasks::{SearchFilters, get_session_outline};
use taskrunner::storage::events::EventBody;
use taskrunner::storage::index::StateIndex;
use taskrunner::view::lookup::{SessionLookupArgs, ViewArgs, lookup_session, search_transcripts};
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
        format!("2026-07-25T04:{:02}:{:02}.000Z", (n / 60) % 60, n % 60)
    }

    fn msg(&self, role: &str, kind: &str, content: &str, session: &str) -> EventBody {
        Msg {
            source: "claude-code",
            session,
            record_id: format!("r{}", self.seq.tick()),
            role,
            kind,
            content: content.into(),
            native_ts: Some(self.ts()),
            project_path: Some("/repo"),
        }
        .body()
    }

    fn call(&self, id: &str, name: &str, input: Value, session: &str) -> EventBody {
        self.msg(
            "assistant",
            "tool_use",
            &json!({ "id": id, "name": name, "input": input }).to_string(),
            session,
        )
    }

    fn result(&self, id: &str, is_error: Option<bool>, content: &str, session: &str) -> EventBody {
        let mut blob = json!({ "tool_use_id": id, "content": content });
        if let Some(flag) = is_error {
            blob["is_error"] = Value::Bool(flag);
        }
        self.msg("tool", "tool_result", &blob.to_string(), session)
    }

    fn index(&self, bodies: Vec<EventBody>) -> StateIndex {
        index_of(bodies, || self.ts())
    }
}

fn huge() -> String {
    format!("a decision worth finding, followed by {}", "padding ".repeat(400))
        .trim_end()
        .to_string()
}

/// Two exchanges: a clean read, then an edit whose command fails.
fn seeded() -> StateIndex {
    let s = Seed::new();
    let o = "sess-O";
    s.index(vec![
        project_created(),
        s.msg("user", "message", "why is the proxy dropping requests", o),
        s.msg("assistant", "reasoning", "thinking about the proxy", o),
        s.msg("assistant", "message", &huge(), o),
        s.call("c1", "Read", json!({ "file_path": "/repo/src/shim/proxy.ts" }), o),
        s.result("c1", None, "file contents", o),
        // Harness-written: neither a prompt nor an outline entry.
        s.msg("user", "message", "<system-reminder>noise</system-reminder>", o),
        s.msg("user", "message", "fix it then", o),
        s.call(
            "c2",
            "Edit",
            json!({ "file_path": "/repo/src/shim/proxy.ts", "old_string": "a", "new_string": "b" }),
            o,
        ),
        s.result("c2", None, "ok", o),
        s.call("c3", "Bash", json!({ "command": "npm test" }), o),
        s.result("c3", Some(true), "1 failing", o),
        s.msg("assistant", "message", "the timeout was unset", o),
    ])
}

fn session(
    id: &str,
    view: Option<TranscriptView>,
    prompt_idx: Option<i64>,
    last: Option<i64>,
) -> SessionLookupArgs {
    SessionLookupArgs {
        session_id: Some(id.into()),
        last,
        view: ViewArgs { view, prompt_idx, ..Default::default() },
        ..Default::default()
    }
}

fn outline_of(index: &StateIndex, id: &str) -> String {
    lookup_session(index, &session(id, Some(TranscriptView::Outline), None, None)).unwrap()
}

#[test]
fn indexes_prompts_replies_and_calls_without_loading_a_body() {
    let out = outline_of(&seeded(), "sess-O");
    assert!(out.contains("12 messages · 2 prompts · 3 tool calls"));
    assert!(out.contains("[1] 04:00  why is the proxy dropping requests"));
    assert!(out.contains("[2] 04:00  fix it then"));
    assert!(out.contains("Read          /repo/src/shim/proxy.ts"));
    assert!(out.contains("Edit          /repo/src/shim/proxy.ts"));
    // Bodies stay out: prose is truncated, tool inputs and results never appear.
    assert!(!out.contains(&huge()));
    assert!(out.contains("→ a decision worth finding"));
    assert!(!out.contains("old_string"));
    assert!(!out.contains("file contents"));
}

#[test]
fn marks_a_failed_call_from_the_result_it_is_paired_with() {
    let out = outline_of(&seeded(), "sess-O");
    assert!(out.contains("Bash          npm test  ✗"));
    assert!(!out.contains("Edit          /repo/src/shim/proxy.ts  ✗"));
}

#[test]
fn leaves_out_reasoning_and_the_records_the_harness_wrote() {
    let out = outline_of(&seeded(), "sess-O");
    assert!(!out.contains("thinking about the proxy"));
    assert!(!out.contains("noise"));
}

#[test]
fn costs_a_fraction_of_reading_the_session() {
    let index = seeded();
    let outline = outline_of(&index, "sess-O");
    let timeline =
        lookup_session(&index, &session("sess-O", Some(TranscriptView::Timeline), None, None))
            .unwrap();
    assert!(outline.len() < timeline.len() / 4);
}

#[test]
fn degrades_cleanly_with_no_tool_calls_and_with_no_prompt_at_all() {
    let s = Seed::new();
    let bare = s.index(vec![
        project_created(),
        s.msg("user", "message", "just talk to me", "sess-Q"),
        s.msg("assistant", "message", "talking", "sess-Q"),
    ]);
    let out = outline_of(&bare, "sess-Q");
    assert!(out.contains("2 messages · 1 prompts · 0 tool calls"));
    assert!(out.contains("[1]"));

    let headless = s.index(vec![
        project_created(),
        s.msg("assistant", "message", "a session that opens mid-flight", "sess-R"),
    ]);
    let out2 = outline_of(&headless, "sess-R");
    assert!(out2.contains("[0]"));
    assert!(out2.contains("(before the first prompt)"));
}

#[test]
fn reports_an_empty_session_rather_than_a_bare_counts_line() {
    let out =
        lookup_session(&seeded(), &session("sess-O", Some(TranscriptView::Outline), Some(9), None))
            .unwrap();
    assert!(out.contains("(no messages at prompt 9)"));
}

#[test]
fn never_reads_a_message_body_out_of_the_database() {
    let outline = get_session_outline(&seeded(), "claude-code", "sess-O", None).unwrap();
    for e in &outline.entries {
        if e.kind == "tool_use" {
            assert!(e.head.is_none());
        }
        // Tool results carry nothing an index needs; their failure state is on
        // the call, so they are not entries at all.
        assert_ne!(e.kind, "tool_result");
    }
    assert!(
        !outline.entries.iter().any(|e| e.head.as_ref().is_some_and(|h| h.chars().count() > 400))
    );
}

// ---- view resolution ----------------------------------------------------

#[test]
fn defaults_to_the_outline() {
    let index = seeded();
    assert_eq!(
        lookup_session(&index, &session("sess-O", None, None, None)).unwrap(),
        outline_of(&index, "sess-O")
    );
}

#[test]
fn reads_the_exchange_in_full_when_one_is_drilled_into() {
    let index = seeded();
    let out = lookup_session(&index, &session("sess-O", None, Some(1), None)).unwrap();
    assert_eq!(
        out,
        lookup_session(&index, &session("sess-O", Some(TranscriptView::Timeline), Some(1), None))
            .unwrap()
    );
    assert!(out.contains(&huge())); // the reply, whole
}

#[test]
fn keeps_a_bounded_read_compact_as_it_was_before_the_outline_existed() {
    let index = seeded();
    assert_eq!(
        lookup_session(&index, &session("sess-O", None, None, Some(2))).unwrap(),
        lookup_session(&index, &session("sess-O", Some(TranscriptView::Compact), None, Some(2)))
            .unwrap()
    );
}

// ---- structured search --------------------------------------------------

/// The seeded session plus a second one that also touches proxy.ts.
fn corpus() -> StateIndex {
    let s = Seed::new();
    let (o, p) = ("sess-O", "sess-P");
    s.index(vec![
        project_created(),
        s.msg("user", "message", "why is the proxy dropping requests", o),
        s.call("c1", "Read", json!({ "file_path": "/repo/src/shim/proxy.ts" }), o),
        s.result("c1", Some(false), "ok", o),
        s.call("c3", "Bash", json!({ "command": "npm test" }), o),
        s.result("c3", Some(true), "1 failing", o),
        s.msg("user", "message", "now the other one", p),
        // These two results state no outcome — a rejection or an abort.
        s.call("c4", "Edit", json!({ "file_path": "/repo/src/shim/proxy.ts" }), p),
        s.result("c4", None, "ok", p),
        s.call("c5", "Edit", json!({ "file_path": "/repo/README.md" }), p),
        s.result("c5", None, "ok", p),
    ])
}

fn search(
    index: &StateIndex,
    query: Option<&str>,
    filters: SearchFilters,
) -> Result<String, taskrunner::domain::errors::ToolError> {
    search_transcripts(index, query, 20, &filters)
}

fn by(tool: Option<&str>, target: Option<&str>, failed: Option<bool>) -> SearchFilters {
    SearchFilters {
        tool: tool.map(Into::into),
        target: target.map(Into::into),
        failed,
        ..Default::default()
    }
}

#[test]
fn answers_which_sessions_touched_a_file_without_a_text_query() {
    let out = search(&corpus(), None, by(None, Some("shim/proxy.ts"), None)).unwrap();
    assert!(out.contains("transcript matches (2)"));
    assert!(out.contains("sess-O"));
    assert!(out.contains("sess-P"));
    assert!(!out.contains("README.md"));
}

#[test]
fn filters_by_tool_and_combines_tool_with_target() {
    let index = corpus();
    assert!(search(&index, None, by(Some("Edit"), None, None)).unwrap().contains("README.md"));
    let both = search(&index, None, by(Some("Edit"), Some("proxy.ts"), None)).unwrap();
    assert!(both.contains("transcript matches (1)"));
    assert!(!both.contains("README.md"));
}

#[test]
fn finds_a_failed_call_by_the_state_of_its_result() {
    let index = corpus();
    let failed = search(&index, None, by(None, None, Some(true))).unwrap();
    // The call is the hit, not the result — so the tool and target are visible,
    // and one failure is reported once rather than twice for the pair.
    assert!(failed.contains("Bash  npm test  ✗"));
    assert!(failed.contains("transcript matches (1)"));

    let ok = search(&index, None, by(None, None, Some(false))).unwrap();
    assert!(ok.contains("Read  /repo/src/shim/proxy.ts"));
    assert!(ok.contains("transcript matches (1)"));
}

#[test]
fn leaves_a_call_whose_result_states_no_outcome_out_of_both_sides() {
    let index = corpus();
    // The two Edits were neither confirmed nor reported failed: guessing either
    // way would be a false negative in an audit.
    assert!(!search(&index, None, by(None, None, Some(true))).unwrap().contains("Edit"));
    assert!(!search(&index, None, by(None, None, Some(false))).unwrap().contains("Edit"));
    assert!(
        search(&index, None, by(Some("Edit"), None, None))
            .unwrap()
            .contains("transcript matches (2)")
    );
}

#[test]
fn prints_the_prompt_index_a_hit_can_be_read_at() {
    let out = search(&corpus(), None, by(None, Some("proxy.ts"), None)).unwrap();
    assert!(out.contains("prompt 1"));
}

#[test]
fn narrows_a_text_search_by_tool_facts() {
    let index = corpus();
    let kind = SearchFilters { kind: Some("tool_use".into()), ..Default::default() };
    assert!(search(&index, Some("proxy"), kind).unwrap().contains("matches"));
    let scoped = search(&index, Some("proxy"), by(Some("Edit"), None, None)).unwrap();
    assert!(scoped.contains("transcript matches (1)"));
}

#[test]
fn refuses_a_search_with_neither_a_query_nor_a_filter() {
    let err = search(&corpus(), None, SearchFilters::default()).unwrap_err();
    assert!(err.message.contains("provide a query"));
}

#[test]
fn names_what_it_searched_for_when_nothing_matched() {
    let out = search(&corpus(), None, by(Some("Glob"), Some("nope"), None)).unwrap();
    assert!(out.contains("no transcript matches for: tool Glob, target ~ nope"));
}
