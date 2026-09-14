// One real two-turn task built through the whole stack (fake codex binary +
// clone workspaces), then lookup assertions against it.

use std::collections::HashMap;
use std::sync::Arc;

use taskrunner::config::parse_config;
use taskrunner::daemon::scheduler::{AssignArgs, Scheduler, SchedulerDeps};
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::EventLog;
use taskrunner::storage::index::{IN_MEMORY, StateIndex};
use taskrunner::storage::store::SharedStore;
use taskrunner::view::lookup::{Include, LookupArgs, LookupDeps, lookup_task};
use taskrunner::workers::codex::{CodexHarness, CodexHarnessOptions};
use taskrunner::workers::harness::WorkerHarness;
use taskrunner::workspace::clone::CloneWorkspaces;
use taskrunner::workspace::git::HostGit;

use crate::helpers::{LocalRunner, fake_codex, init_git_repo};

struct Stack {
    scheduler: Scheduler,
    store: SharedStore,
    artifacts: Arc<ArtifactStore>,
    workspaces_dir: std::path::PathBuf,
    _root: tempfile::TempDir,
}

fn make_stack() -> Stack {
    let root = tempfile::tempdir().unwrap();
    let store = SharedStore::new(
        EventLog::open(&root.path().join("events.jsonl")).unwrap(),
        StateIndex::open(IN_MEMORY).unwrap(),
    );
    let artifacts = Arc::new(ArtifactStore::new(&root.path().join("artifacts")));
    let workspaces_dir = root.path().join("workspaces");
    let clones = CloneWorkspaces::new(
        &workspaces_dir,
        artifacts.clone(),
        Arc::new(store.clone()),
        Arc::new(HostGit),
    );
    let mut harnesses: HashMap<String, Arc<dyn WorkerHarness>> = HashMap::new();
    harnesses.insert("codex".into(), Arc::new(CodexHarness::new(CodexHarnessOptions::default())));
    let scheduler = Scheduler::new(SchedulerDeps {
        config: Arc::new(parse_config("").unwrap()),
        store: store.clone(),
        harnesses,
        workspaces: Arc::new(clones),
        make_runner: Arc::new(|ctx| {
            Box::new(LocalRunner::new(&ctx.workspace_dir, Some(&fake_codex())))
        }),
        artifacts: artifacts.clone(),
    });
    Stack { scheduler, store, artifacts, workspaces_dir, _root: root }
}

#[tokio::test]
async fn delegates_edits_in_the_task_workspace_and_resumes_the_same_thread() {
    let repo = init_git_repo();
    let st = make_stack();

    let first = st
        .scheduler
        .assign_task(AssignArgs {
            project: repo.path().to_string_lossy().into_owned(),
            worker: "codex".into(),
            prompt: "create hello".into(),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(first.status, "completed");
    assert_eq!(first.changed_files, vec!["hello.txt"]);
    assert!(first.worker_session_id.as_deref().unwrap().starts_with("thread-"));

    // The edit landed in the task workspace, not the project.
    let workspace = st.workspaces_dir.join(&first.task_id);
    assert_eq!(std::fs::read_to_string(workspace.join("hello.txt")).unwrap(), "line one\n");
    assert!(!repo.path().join("hello.txt").exists());

    // Raw worker events were captured as a linked artifact.
    assert!(first.artifacts.iter().any(|a| a.kind == "worker-events"));

    let second =
        st.scheduler.continue_task(&first.task_id, "add another line", true).await.unwrap();
    assert_eq!(second.status, "completed");
    assert!(
        second
            .summary
            .as_deref()
            .unwrap()
            .contains(&format!("resumed {}", first.worker_session_id.as_deref().unwrap()))
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("hello.txt")).unwrap(),
        "line one\nline two\n"
    );

    // Same native session: no second worker_sessions row.
    let n: i64 = st
        .store
        .lock()
        .index
        .db
        .query_row("SELECT COUNT(*) FROM worker_sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);

    // And the lookup renders what the scheduler recorded, end to end.
    let store = st.store.lock();
    let deps = LookupDeps { index: &store.index, artifacts: &st.artifacts };
    let text = lookup_task(
        &deps,
        &LookupArgs {
            task_id: Some(first.task_id.clone()),
            include: vec![Include::Turns, Include::Trace, Include::Artifacts, Include::Diff],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(text.contains("turns: 2"));
    assert!(text.contains(">> create hello") && text.contains(">> add another line"));
    assert!(text.contains("worker.file_change"));
    assert!(text.contains("changed files: hello.txt"));
    assert!(text.contains("worker-events"));
    // A file the turn created is untracked, and used to be missing from the
    // record entirely. The inspection marks it intent-to-add, so what the turn
    // wrote is in the diff like any other change.
    assert!(!text.contains("(no diff artifacts in scope)"), "{text}");
    assert!(text.contains("+line two"), "{text}");
}
