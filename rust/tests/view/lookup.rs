// One two-turn task, then lookup assertions against it. The TypeScript test
// builds the task through the scheduler and the fake codex; that stack lands
// with step 6, so here the events are the ones the scheduler wrote for the
// same flow (copied from the parity corpus), applied straight to the index.

use serde_json::json;
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::EventBody;
use taskrunner::storage::index::{IN_MEMORY, StateIndex, rebuild_index};
use taskrunner::view::lookup::{Include, LookupArgs, LookupDeps, Scope, lookup_task};

use crate::helpers::evt;

const TASK: &str = "task_a";
const TURNS: [&str; 2] = ["turn_a1", "turn_a2"];
const THREAD: &str = "thread-335001";

struct Stack {
    index: StateIndex,
    artifacts: ArtifactStore,
    repo: tempfile::TempDir,
}

impl Stack {
    fn deps(&self) -> LookupDeps<'_> {
        LookupDeps { index: &self.index, artifacts: &self.artifacts }
    }

    fn repo(&self) -> String {
        self.repo.path().to_string_lossy().into_owned()
    }
}

fn audit(turn_id: &str, kind: &str, payload: serde_json::Value) -> EventBody {
    EventBody::AuditRecorded {
        session_id: None,
        task_id: Some(TASK.into()),
        turn_id: Some(turn_id.into()),
        kind: kind.into(),
        payload,
    }
}

/// What one fake-codex turn leaves in the log.
fn turn(turn_id: &str, prompt: &str, response: &str, resumed: bool) -> Vec<EventBody> {
    let artifact_id = format!("art_{turn_id}");
    let mut events = vec![
        EventBody::TurnStarted {
            turn_id: turn_id.into(),
            task_id: TASK.into(),
            prompt: prompt.into(),
        },
        audit(
            turn_id,
            "worker.thread.started",
            json!({ "type": "thread.started", "thread_id": THREAD }),
        ),
        audit(turn_id, "worker.turn.started", json!({ "type": "turn.started" })),
        audit(
            turn_id,
            "worker.command_execution",
            json!({ "type": "item.completed", "item": { "item_type": "command_execution", "command": "append hello.txt" } }),
        ),
        audit(
            turn_id,
            "worker.file_change",
            json!({ "type": "item.completed", "item": { "item_type": "file_change", "changes": [{ "path": "hello.txt" }] } }),
        ),
        audit(
            turn_id,
            "worker.agent_message",
            json!({ "type": "item.completed", "item": { "item_type": "agent_message", "text": response } }),
        ),
        audit(
            turn_id,
            "worker.turn.completed",
            json!({ "type": "turn.completed", "usage": { "input_tokens": 10, "output_tokens": 5 } }),
        ),
    ];
    if !resumed {
        events.push(EventBody::WorkerSessionRecorded {
            worker_session_id: "wsess_a".into(),
            task_id: TASK.into(),
            worker: "codex".into(),
            native_session_id: THREAD.into(),
            turn_id: Some(turn_id.into()),
        });
    }
    events.extend([
        EventBody::TurnCompleted {
            turn_id: turn_id.into(),
            task_id: TASK.into(),
            response: response.into(),
            changed_files: vec!["hello.txt".into()],
            usage: Some(json!({ "input_tokens": 10, "output_tokens": 5 })),
        },
        EventBody::ArtifactStored {
            artifact_id: artifact_id.clone(),
            kind: "worker-events".into(),
            label: "raw worker events".into(),
            media_type: "application/jsonl".into(),
            size_bytes: 665,
            sha256: "a001709ac8a5780e9db239a6ee8fba462fcafd8ddb9e984c11d2edfb59df81d7".into(),
            locator: "a0/a001709ac8a5780e9db239a6ee8fba462fcafd8ddb9e984c11d2edfb59df81d7".into(),
        },
        EventBody::ArtifactLinked {
            artifact_id,
            session_id: None,
            task_id: Some(TASK.into()),
            turn_id: Some(turn_id.into()),
            audit_event_id: None,
        },
    ]);
    events
}

fn stack() -> Stack {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().to_string_lossy().into_owned();
    let mut bodies = vec![
        EventBody::ProjectCreated { project_id: "proj_a".into(), root },
        EventBody::TaskCreated {
            task_id: TASK.into(),
            project_id: "proj_a".into(),
            session_id: None,
            worker: "codex".into(),
            prompt_summary: "create hello".into(),
            tier: Some("workspace-write".into()),
            runtime: None,
            allow_domains: None,
        },
    ];
    bodies.extend(turn(
        TURNS[0],
        "create hello",
        &format!("started {THREAD} for: create hello"),
        false,
    ));
    bodies.extend(turn(
        TURNS[1],
        "extend hello",
        &format!("resumed {THREAD} for: extend hello"),
        true,
    ));
    let events: Vec<_> = bodies.into_iter().map(evt).collect();
    let index = rebuild_index(IN_MEMORY, &events).unwrap();
    let artifacts = ArtifactStore::new(&repo.path().join("artifacts"));
    Stack { index, artifacts, repo }
}

fn lookup(st: &Stack, args: LookupArgs) -> String {
    lookup_task(&st.deps(), &args).unwrap()
}

fn by_task(include: Vec<Include>, scope: Option<Scope>) -> LookupArgs {
    LookupArgs { task_id: Some(TASK.into()), include, scope, ..Default::default() }
}

#[test]
fn returns_a_compact_summary_by_default_with_no_expansions() {
    let text = lookup(&stack(), by_task(vec![], None));
    assert!(text.contains(&format!("task: {TASK}")));
    assert!(text.contains("status: completed"));
    assert!(text.contains("turns: 2"));
    assert!(!text.contains("exchanges:"));
    assert!(!text.contains("trace:"));
}

#[test]
fn include_turns_returns_paired_exchanges_in_turn_order() {
    let text = lookup(&stack(), by_task(vec![Include::Turns], None));
    let first = text.find(">> create hello").unwrap();
    let second = text.find(">> extend hello").unwrap();
    assert!(second > first);
    assert!(text.contains("<< started thread-"));
    assert!(text.contains("<< resumed thread-"));
}

#[test]
fn include_trace_replays_inputs_activity_and_outputs_per_turn() {
    let text = lookup(&stack(), by_task(vec![Include::Trace], None));
    assert!(text.contains(&format!("trace: turn 1 ({}, completed)", TURNS[0])));
    assert!(text.contains("inputs:"));
    assert!(text.contains("prompt: create hello"));
    assert!(text.contains("activity:"));
    assert!(text.contains("worker.agent_message"));
    assert!(text.contains("worker.file_change"));
    assert!(text.contains("outputs:"));
    assert!(text.contains("changed files: hello.txt"));
    assert!(text.contains("artifact:"));
}

#[test]
fn scope_turn_id_narrows_expansions_to_one_turn() {
    let scope = Scope { turn_id: Some(TURNS[1].into()), last: None };
    let text = lookup(&stack(), by_task(vec![Include::Turns], Some(scope)));
    assert!(!text.contains(">> create hello"));
    assert!(text.contains(">> extend hello"));
}

#[test]
fn scope_last_n_returns_only_the_trailing_exchanges() {
    let scope = Scope { turn_id: None, last: Some(1) };
    let text = lookup(&stack(), by_task(vec![Include::Turns], Some(scope)));
    assert!(!text.contains(">> create hello"));
    assert!(text.contains(">> extend hello"));
}

#[test]
fn include_artifacts_lists_handles_not_payloads() {
    let text = lookup(&stack(), by_task(vec![Include::Artifacts], None));
    assert!(text.contains("worker-events"));
    assert!(text.contains("art_turn_a1  worker-events"));
    assert!(text.contains("bytes"));
}

#[test]
fn include_audit_lists_per_turn_audit_rows() {
    let text = lookup(&stack(), by_task(vec![Include::Audit], None));
    assert!(text.contains(&format!("audit: turn 1 ({}", TURNS[0])));
    assert!(text.contains("worker.turn.completed"));
}

#[test]
fn project_lookup_lists_tasks_most_recent_first_without_creating_records() {
    let st = stack();
    let count = || -> i64 {
        st.index.db.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap()
    };
    let before = count();
    let text = lookup(&st, LookupArgs { project: Some(st.repo()), ..Default::default() });
    assert!(text.contains("project: "));
    assert!(text.contains(TASK));
    assert!(text.contains("completed"));
    assert_eq!(count(), before);
}

#[test]
fn errors_match_the_contract() {
    let st = stack();
    let fails_with = |args: LookupArgs, needle: &str| {
        let err = lookup_task(&st.deps(), &args).unwrap_err();
        assert!(err.message.contains(needle), "{}", err.message);
    };
    fails_with(LookupArgs::default(), "taskId or project");
    fails_with(LookupArgs { task_id: Some("task_nope".into()), ..Default::default() }, "no task");
    fails_with(
        LookupArgs { project: Some("/nope/nothing".into()), ..Default::default() },
        "no tasks recorded",
    );
    let scope = Scope { turn_id: Some("turn_nope".into()), last: None };
    fails_with(by_task(vec![], Some(scope)), "no turn");
}
