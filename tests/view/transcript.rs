// Phase 3: surfacing the ingested transcript corpus. Part A is the lookup-task
// `transcript` include field (one task's worker interior); Part B is the
// corpus-wide `search-transcripts` tool. Both read the same `messages` /
// `messages_fts` tables the ingest sweeper writes.

use serde_json::json;
use taskrunner::domain::tasks::{
    MessageQuery, SearchFilters, SessionListQuery, get_session_messages, list_sessions,
    search_messages,
};
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::EventBody;
use taskrunner::storage::index::StateIndex;
use taskrunner::view::lookup::{
    Include, LookupArgs, LookupDeps, Scope, SessionLookupArgs, ViewArgs, lookup_session,
    lookup_task, search_transcripts,
};
use taskrunner::view::transcript::TranscriptView;

use crate::seed::{Clock, Msg, index_of, project_created};

struct Seed {
    clock: Clock,
}

impl Seed {
    fn new() -> Seed {
        Seed { clock: Clock::new() }
    }

    fn ts(&self) -> String {
        format!("2026-07-24T00:00:{:02}.000Z", self.clock.tick())
    }

    /// A message.recorded body with sensible defaults; deterministic id per record.
    fn message(
        &self,
        source: &str,
        session: &str,
        role: &str,
        kind: &str,
        content: &str,
        project_path: Option<&str>,
    ) -> EventBody {
        let native_ts = self.ts();
        Msg {
            source,
            session,
            record_id: format!("rec-{}", self.clock.now()),
            role,
            kind,
            content: content.into(),
            native_ts: Some(native_ts),
            project_path,
        }
        .body()
    }

    /// The same, with the record id pinned so two bodies can share a message id.
    fn record(
        &self,
        source: &str,
        session: &str,
        record_id: &str,
        content: &str,
        project_path: Option<&str>,
    ) -> EventBody {
        let mut body = self.message(source, session, "assistant", "message", content, project_path);
        if let EventBody::MessageRecorded { message_id, native_record_id, .. } = &mut body {
            *native_record_id = record_id.into();
            *message_id = format!("msg:{source}:{session}:{record_id}");
        }
        body
    }

    fn index(&self, bodies: Vec<EventBody>) -> StateIndex {
        index_of(bodies, || self.ts())
    }
}

fn task_created(task_id: &str, summary: &str) -> EventBody {
    EventBody::TaskCreated {
        task_id: task_id.into(),
        project_id: "p1".into(),
        session_id: None,
        worker: "codex".into(),
        prompt_summary: summary.into(),
        tier: None,
        runtime: None,
        allow_domains: None,
    }
}

fn worker_session(session: &str) -> EventBody {
    EventBody::WorkerSessionRecorded {
        worker_session_id: "ws1".into(),
        task_id: "t1".into(),
        worker: "codex".into(),
        native_session_id: session.into(),
        turn_id: None,
    }
}

struct Stack {
    index: StateIndex,
    artifacts: ArtifactStore,
    _dir: tempfile::TempDir,
}

impl Stack {
    fn deps(&self) -> LookupDeps<'_> {
        LookupDeps { index: &self.index, artifacts: &self.artifacts }
    }
}

fn stack(index: StateIndex) -> Stack {
    let dir = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::new(&dir.path().join("artifacts"));
    Stack { index, artifacts, _dir: dir }
}

/// A fresh index with a linked task and a handful of seeded transcript messages.
fn seeded() -> Stack {
    let s = Seed::new();
    let index = s.index(vec![
        project_created(),
        task_created("t1", "build a widget"),
        EventBody::TurnStarted {
            turn_id: "turn1".into(),
            task_id: "t1".into(),
            prompt: "build a widget".into(),
        },
        worker_session("sess-A"),
        s.message("codex", "sess-A", "user", "message", "please build the widget", Some("/repo")),
        s.message(
            "codex",
            "sess-A",
            "assistant",
            "tool_use",
            &json!({ "id": "tu1", "name": "Bash", "input": { "command": "ls -la" } }).to_string(),
            Some("/repo"),
        ),
        s.message(
            "codex",
            "sess-A",
            "assistant",
            "message",
            "done building the widget",
            Some("/repo"),
        ),
        // A host-session message: archived, matches search, but not linked to a task.
        s.message(
            "claude-code",
            "host-1",
            "user",
            "message",
            "unrelated host chatter about a widget",
            Some("/host"),
        ),
    ]);
    stack(index)
}

fn task_lookup(
    task_id: &str,
    include: Vec<Include>,
    view: Option<TranscriptView>,
    last: Option<i64>,
) -> LookupArgs {
    LookupArgs {
        task_id: Some(task_id.into()),
        include,
        scope: last.map(|last| Scope { last: Some(last), ..Default::default() }),
        view: ViewArgs { view, ..Default::default() },
        ..Default::default()
    }
}

fn search(index: &StateIndex, query: &str, limit: i64, filters: SearchFilters) -> String {
    search_transcripts(index, Some(query), limit, &filters).unwrap()
}

// ---- lookup-task include transcript (Part A) ------------------------------

#[test]
fn renders_the_tasks_worker_transcript_in_order_compacting_a_tool_payload() {
    let st = seeded();
    let out = lookup_task(
        &st.deps(),
        &task_lookup("t1", vec![Include::Transcript], Some(TranscriptView::Compact), None),
    )
    .unwrap();

    assert!(out.contains("transcript:"));
    let user_at = out.find("please build the widget").unwrap();
    let tool_at = out.find("Codex/tool_use").unwrap();
    let done_at = out.find("done building the widget").unwrap();
    // Chronological: user message, then tool_use, then final assistant message.
    assert!(user_at < tool_at);
    assert!(tool_at < done_at);
    // The tool payload is compacted to a single line (no raw multi-line JSON).
    let tool_line = &out[tool_at..out[tool_at..].find('\n').map_or(out.len(), |n| tool_at + n)];
    assert!(tool_line.contains("Bash"));
}

#[test]
fn excludes_host_session_messages_not_linked_to_the_task() {
    let st = seeded();
    let out =
        lookup_task(&st.deps(), &task_lookup("t1", vec![Include::Transcript], None, None)).unwrap();
    assert!(!out.contains("unrelated host chatter"));
}

#[test]
fn outlines_the_worker_interior_when_no_view_is_asked_for() {
    let st = seeded();
    let out =
        lookup_task(&st.deps(), &task_lookup("t1", vec![Include::Transcript], None, None)).unwrap();
    assert!(out.contains("tool calls"));
    assert!(out.contains("[1]"));
    assert!(out.contains("Bash"));
    // An outline names the call but never loads its payload or its result.
    assert!(!out.contains("Codex/tool_use"));
}

#[test]
fn reports_the_empty_state_for_a_task_with_no_transcript() {
    let s = Seed::new();
    let st = stack(s.index(vec![project_created(), task_created("t2", "no session")]));
    let out =
        lookup_task(&st.deps(), &task_lookup("t2", vec![Include::Transcript], None, None)).unwrap();
    assert!(out.contains("(no transcript recorded)"));
}

#[test]
fn honors_scope_last_as_a_cap_keeping_the_most_recent_messages() {
    let st = seeded();
    let out = lookup_task(&st.deps(), &task_lookup("t1", vec![Include::Transcript], None, Some(1)))
        .unwrap();
    assert!(out.contains("done building the widget")); // newest kept
    assert!(!out.contains("please build the widget")); // oldest dropped
    assert!(out.contains("capped at 1 messages"));
}

// ---- search-transcripts (Part B) -----------------------------------------

#[test]
fn matches_across_the_whole_corpus_and_attributes_worker_hits_to_their_task() {
    let out = search(&seeded().index, "widget", 20, SearchFilters::default());
    assert!(out.contains("task t1")); // worker-session hit attributed to the task
    assert!(out.contains("claude-code session host-1")); // host hit, no task
}

#[test]
fn does_not_attribute_a_host_session_hit_to_any_task() {
    let out = search(&seeded().index, "chatter", 20, SearchFilters::default());
    assert!(out.contains("claude-code session host-1"));
    assert!(!out.contains("task t1"));
}

#[test]
fn bounds_results_by_limit() {
    assert!(
        search(&seeded().index, "widget", 1, SearchFilters::default())
            .contains("transcript matches (1)")
    );
}

#[test]
fn reports_no_matches_cleanly() {
    assert!(
        search(&seeded().index, "nonexistentterm", 20, SearchFilters::default())
            .contains("no transcript matches")
    );
}

#[test]
fn maps_malformed_fts5_syntax_to_a_client_error_not_a_crash() {
    let err =
        search_transcripts(&seeded().index, Some("\"unbalanced"), 20, &SearchFilters::default())
            .unwrap_err();
    assert!(err.message.contains("invalid search query"));
}

#[test]
fn indexes_a_re_swept_message_once() {
    // Same deterministic message_id emitted twice, as a re-sweep would.
    let s = Seed::new();
    let dup = s.record("codex", "sess-A", "rec-dup", "uniquephrase appears once", None);
    let index = s.index(vec![
        project_created(),
        task_created("t1", "x"),
        worker_session("sess-A"),
        dup.clone(),
        dup,
    ]);
    assert!(
        search(&index, "uniquephrase", 20, SearchFilters::default())
            .contains("transcript matches (1)")
    );
}

#[test]
fn shows_the_project_on_each_hit_and_filters_by_project() {
    let st = seeded();
    let all = search(&st.index, "widget", 20, SearchFilters::default());
    assert!(all.contains("/repo")); // worker hit's project
    assert!(all.contains("/host")); // host hit's project
    let scoped = search(
        &st.index,
        "widget",
        20,
        SearchFilters { project: Some("/host".into()), ..Default::default() },
    );
    assert!(scoped.contains("host-1"));
    assert!(!scoped.contains("task t1")); // /repo worker hits excluded
}

#[test]
fn scopes_to_specific_sessions_and_to_the_last_n_sessions() {
    let st = seeded();
    // Only the host session id: the worker "widget" hits drop out.
    let by_session = search_messages(
        &st.index,
        Some("widget"),
        20,
        &SearchFilters { sessions: Some(vec!["host-1".into()]), ..Default::default() },
    )
    .unwrap();
    assert!(by_session.iter().all(|h| h.native_session_id == "host-1"));
    // host-1 is the most recent session; last-1 keeps only it.
    let by_last = search_messages(
        &st.index,
        Some("widget"),
        20,
        &SearchFilters { last_sessions: Some(1), ..Default::default() },
    )
    .unwrap();
    assert!(by_last.iter().all(|h| h.native_session_id == "host-1"));
}

#[test]
fn filters_by_role_and_kind() {
    let st = seeded();
    let users = search_messages(
        &st.index,
        Some("widget"),
        20,
        &SearchFilters { role: Some("user".into()), ..Default::default() },
    )
    .unwrap();
    assert!(!users.is_empty());
    assert!(users.iter().all(|h| h.role == "user"));
    let tools = search_messages(
        &st.index,
        Some("ls"),
        20,
        &SearchFilters { kind: Some("tool_use".into()), ..Default::default() },
    )
    .unwrap();
    assert!(tools.iter().all(|h| h.kind == "tool_use"));
}

#[test]
fn returns_no_hits_when_scoped_to_a_session_set_that_has_none() {
    let st = seeded();
    let hits = search_messages(
        &st.index,
        Some("widget"),
        20,
        &SearchFilters { sessions: Some(vec![]), ..Default::default() },
    )
    .unwrap();
    assert!(hits.is_empty());
}

// ---- lookup-session (Part C) ---------------------------------------------

fn session_read(id: &str, source: Option<&str>, last: Option<i64>) -> SessionLookupArgs {
    SessionLookupArgs {
        session_id: Some(id.into()),
        source: source.map(Into::into),
        last,
        ..Default::default()
    }
}

#[test]
fn lists_ingested_sessions_most_recent_first_with_project_and_task_link() {
    let out = lookup_session(&seeded().index, &SessionLookupArgs::default()).unwrap();
    assert!(out.contains("sessions (2)"));
    // host-1 was seeded last (newest), so it lists before sess-A.
    assert!(out.find("host-1").unwrap() < out.find("sess-A").unwrap());
    assert!(out.contains("[task t1]")); // sess-A is linked to a task
    assert!(out.contains("/repo"));
    assert!(out.contains("/host"));
}

#[test]
fn filters_the_list_by_project() {
    let out = lookup_session(
        &seeded().index,
        &SessionLookupArgs { project: Some("/host".into()), ..Default::default() },
    )
    .unwrap();
    assert!(out.contains("host-1"));
    assert!(!out.contains("sess-A"));
}

#[test]
fn reads_a_host_sessions_full_history_in_order() {
    let out = lookup_session(&seeded().index, &session_read("host-1", None, None)).unwrap();
    assert!(out.contains("session host-1 (claude-code, /host)"));
    assert!(out.contains("unrelated host chatter about a widget"));
}

#[test]
fn reads_a_worker_session_by_id_too_chronologically() {
    let out = lookup_session(&seeded().index, &session_read("sess-A", None, None)).unwrap();
    assert!(
        out.find("please build the widget").unwrap()
            < out.find("done building the widget").unwrap()
    );
}

#[test]
fn caps_a_session_read_with_scope_last_keeping_the_newest() {
    let out = lookup_session(&seeded().index, &session_read("sess-A", None, Some(1))).unwrap();
    assert!(out.contains("done building the widget"));
    assert!(!out.contains("please build the widget"));
    assert!(out.contains("capped at 1 messages"));
}

#[test]
fn errors_for_an_unknown_session_id() {
    let err = lookup_session(&seeded().index, &session_read("nope", None, None)).unwrap_err();
    assert!(err.message.contains("no ingested"));
}

#[test]
fn lists_candidates_when_a_bare_id_spans_multiple_sources() {
    let s = Seed::new();
    let index = s.index(vec![
        project_created(),
        s.message("codex", "dup-id", "assistant", "message", "from codex", None),
        s.message("claude-code", "dup-id", "assistant", "message", "from claude", None),
    ]);
    let out = lookup_session(&index, &session_read("dup-id", None, None)).unwrap();
    assert!(out.contains("matches 2 sources"));
    assert!(out.contains("source=codex"));
    assert!(out.contains("source=claude-code"));
    // Disambiguating by source reads it.
    let one = lookup_session(&index, &session_read("dup-id", Some("codex"), None)).unwrap();
    assert!(one.contains("from codex"));
    assert!(!one.contains("from claude"));
}

// ---- transcript_sessions aggregate ---------------------------------------

#[test]
fn counts_a_sessions_messages_and_is_idempotent_across_a_re_sweep() {
    let s = Seed::new();
    let dup = s.record("codex", "sess-A", "rec-dup", "once", None);
    let index = s.index(vec![
        project_created(),
        s.record("codex", "sess-A", "r1", "a", None),
        dup.clone(),
        dup, // re-swept identical message: must not double-count
    ]);
    let sessions = list_sessions(
        &index,
        &SessionListQuery { native_session_id: Some("sess-A".into()), ..Default::default() },
    )
    .unwrap();
    assert_eq!(sessions[0].message_count, 2);
}

#[test]
fn is_reconstructed_identically_by_a_rebuild_from_the_log() {
    let s = Seed::new();
    let bodies = || {
        vec![
            project_created(),
            s.record("codex", "sess-A", "r1", "a", Some("/repo")),
            s.record("codex", "sess-A", "r2", "b", Some("/repo")),
            s.record("claude-code", "host-1", "h1", "c", Some("/host")),
        ]
    };
    let events = bodies();
    let a = index_of(events.clone(), || "2026-07-24T00:01:00.000Z".into());
    let b = index_of(events, || "2026-07-24T00:01:00.000Z".into());
    let norm = |idx: &StateIndex| {
        serde_json::to_string(&list_sessions(idx, &SessionListQuery::default()).unwrap()).unwrap()
    };
    assert_eq!(norm(&a), norm(&b));
}

#[test]
fn get_session_messages_reads_only_the_requested_session() {
    let page =
        get_session_messages(&seeded().index, "claude-code", "host-1", MessageQuery::default())
            .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(page.messages[0].content.contains("host chatter"));
}
