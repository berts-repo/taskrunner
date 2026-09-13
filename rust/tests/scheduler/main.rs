//! Scheduler tests: the async contract, and risk tiers and approvals.

#[path = "../helpers/mod.rs"]
mod helpers;

mod integration;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use taskrunner::config::{Config, parse_config};
use taskrunner::daemon::scheduler::{AssignArgs, Scheduler, SchedulerDeps};
use taskrunner::domain::errors::ErrorCode;
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::{EventBody, EventLog, read_events};
use taskrunner::storage::index::{IN_MEMORY, StateIndex};
use taskrunner::storage::store::SharedStore;
use taskrunner::workers::harness::WorkerHarness;

use crate::helpers::{FakeHarness, LocalRunner, ProjectRootWorkspaces};

pub struct Stack {
    pub scheduler: Scheduler,
    pub store: SharedStore,
    pub log_path: std::path::PathBuf,
    pub project: tempfile::TempDir,
    _root: tempfile::TempDir,
}

pub fn make_scheduler(config: Config) -> Stack {
    let root = tempfile::tempdir().unwrap();
    let log_path = root.path().join("events.jsonl");
    let store =
        SharedStore::new(EventLog::open(&log_path).unwrap(), StateIndex::open(IN_MEMORY).unwrap());
    let mut harnesses: HashMap<String, Arc<dyn WorkerHarness>> = HashMap::new();
    harnesses.insert("fake".into(), Arc::new(FakeHarness::default()));
    let scheduler = Scheduler::new(SchedulerDeps {
        config: Arc::new(config),
        store: store.clone(),
        harnesses,
        workspaces: Arc::new(ProjectRootWorkspaces),
        // FakeHarness never spawns a process, so a local runner suffices.
        make_runner: Arc::new(|ctx| Box::new(LocalRunner::new(&ctx.workspace_dir, None))),
        artifacts: Arc::new(ArtifactStore::new(&root.path().join("artifacts"))),
    });
    Stack { scheduler, store, log_path, project: tempfile::tempdir().unwrap(), _root: root }
}

fn assign(project: &Path, prompt: &str) -> AssignArgs {
    AssignArgs {
        project: project.to_string_lossy().into_owned(),
        worker: "fake".into(),
        prompt: prompt.into(),
        ..Default::default()
    }
}

pub async fn wait_for_terminal(scheduler: &Scheduler, task_id: &str) -> String {
    for _ in 0..200 {
        let status = scheduler.outcome(task_id).unwrap().status;
        if status != "running" && status != "created" {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("turn never reached a terminal status");
}

fn audit_kinds(log: &Path) -> Vec<String> {
    read_events(log)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AuditRecorded { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn assign_task_returns_immediately_with_running_then_completes() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome =
        s.scheduler.assign_task(assign(s.project.path(), "sleep:200 do something")).await.unwrap();
    assert_eq!(outcome.status, "running");
    assert!(outcome.task_id.starts_with("task_"));
    assert!(outcome.turn_id.as_deref().unwrap().starts_with("turn_"));

    assert_eq!(wait_for_terminal(&s.scheduler, &outcome.task_id).await, "completed");
    let done = s.scheduler.outcome(&outcome.task_id).unwrap();
    assert!(done.summary.as_deref().unwrap().contains("echo:"));
    assert!(done.worker_session_id.as_deref().unwrap().starts_with("fake-"));
    assert_eq!(done.error, None);
}

#[tokio::test]
async fn wait_true_blocks_until_the_turn_is_terminal() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "quick") })
        .await
        .unwrap();
    assert_eq!(outcome.status, "completed");
    assert!(outcome.summary.as_deref().unwrap().contains("turn 1"));
}

#[tokio::test]
async fn continue_task_resumes_the_same_worker_native_session() {
    let s = make_scheduler(parse_config("").unwrap());
    let first = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "start") })
        .await
        .unwrap();
    let second = s.scheduler.continue_task(&first.task_id, "again", true).await.unwrap();
    assert_eq!(second.task_id, first.task_id);
    assert_ne!(second.turn_id, first.turn_id);
    assert_eq!(second.worker_session_id, first.worker_session_id);
    assert!(second.summary.as_deref().unwrap().contains("turn 2"));
}

#[tokio::test]
async fn continue_task_during_a_running_turn_returns_conflict() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s.scheduler.assign_task(assign(s.project.path(), "sleep:2000")).await.unwrap();
    let err = s.scheduler.continue_task(&outcome.task_id, "more", false).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    s.scheduler.cancel_task(&outcome.task_id, None).await.unwrap();
}

#[tokio::test]
async fn times_out_a_turn_and_records_worker_failed_with_audit_retained() {
    let s = make_scheduler(parse_config("[task]\nturn_timeout_seconds = 1\n").unwrap());
    let outcome = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "sleep:10000") })
        .await
        .unwrap();
    assert_eq!(outcome.status, "failed");
    let error = outcome.error.unwrap();
    assert_eq!(error.code, "worker_failed");
    assert!(error.message.contains("timed out after 1s"));
    // The pre-timeout streamed audit events survive.
    assert!(audit_kinds(&s.log_path).contains(&"worker.agent_message".to_string()));
}

#[tokio::test]
async fn cancel_task_cancels_the_running_turn_and_preserves_the_audit_trail() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s.scheduler.assign_task(assign(s.project.path(), "sleep:10000")).await.unwrap();
    let result =
        s.scheduler.cancel_task(&outcome.task_id, Some("changed my mind".into())).await.unwrap();
    assert_eq!(result.task_id, outcome.task_id);
    assert_eq!(result.turn_id, outcome.turn_id);
    assert_eq!(result.status, "canceled");
    let events = read_events(&s.log_path).unwrap();
    let reason = events.iter().find_map(|e| match &e.body {
        EventBody::TurnCanceled { reason, .. } => Some(reason.clone()),
        _ => None,
    });
    assert_eq!(reason, Some(Some("changed my mind".into())));
    assert!(!audit_kinds(&s.log_path).is_empty());
}

#[tokio::test]
async fn cancel_task_on_an_idle_task_reports_the_current_status_without_a_turn() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "quick") })
        .await
        .unwrap();
    let result = s.scheduler.cancel_task(&outcome.task_id, None).await.unwrap();
    assert_eq!((result.turn_id, result.status.as_str()), (None, "completed"));
}

#[tokio::test]
async fn records_worker_crashes_as_failed_turns() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "please fail") })
        .await
        .unwrap();
    assert_eq!(outcome.status, "failed");
    assert!(outcome.error.unwrap().message.contains("fake worker failure"));
}

#[tokio::test]
async fn rejects_unknown_workers_tasks_and_empty_prompts() {
    let s = make_scheduler(parse_config("").unwrap());
    let code = |r: Result<_, taskrunner::domain::errors::ToolError>| r.unwrap_err().code;
    assert_eq!(
        code(
            s.scheduler
                .assign_task(AssignArgs { worker: "nope".into(), ..assign(s.project.path(), "x") })
                .await
        ),
        ErrorCode::NotConfigured
    );
    assert_eq!(
        code(s.scheduler.continue_task("task_missing", "x", false).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        s.scheduler.cancel_task("task_missing", None).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        code(s.scheduler.assign_task(assign(s.project.path(), "   ")).await),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        code(s.scheduler.assign_task(assign(Path::new("relative/path"), "x")).await),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        code(s.scheduler.assign_task(assign(&s.project.path().join("missing"), "x")).await),
        ErrorCode::InvalidRequest
    );
}

#[tokio::test]
async fn runs_different_tasks_concurrently() {
    let s = make_scheduler(parse_config("").unwrap());
    let a = s.scheduler.assign_task(assign(s.project.path(), "sleep:10000 a")).await.unwrap();
    let b = s.scheduler.assign_task(assign(s.project.path(), "sleep:10000 b")).await.unwrap();
    // Two different tasks hold running turns at the same time (within one
    // task this is impossible: continue-task returns conflict).
    assert!(s.scheduler.has_running_turn(&a.task_id));
    assert!(s.scheduler.has_running_turn(&b.task_id));
    assert_eq!(s.scheduler.cancel_task(&a.task_id, None).await.unwrap().status, "canceled");
    assert_eq!(s.scheduler.cancel_task(&b.task_id, None).await.unwrap().status, "canceled");
}

#[tokio::test]
async fn reuses_one_project_record_across_tasks() {
    let s = make_scheduler(parse_config("").unwrap());
    s.scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "a") })
        .await
        .unwrap();
    s.scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "b") })
        .await
        .unwrap();
    let n: i64 = s
        .store
        .lock()
        .index
        .db
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

// ---- risk tiers and approvals ------------------------------------------

#[tokio::test]
async fn records_workspace_write_tier_on_plain_tasks() {
    let s = make_scheduler(parse_config("").unwrap());
    let outcome = s
        .scheduler
        .assign_task(AssignArgs { wait: true, ..assign(s.project.path(), "quick") })
        .await
        .unwrap();
    assert_eq!(outcome.tier.as_deref(), Some("workspace-write"));
    assert_eq!(outcome.approval_state, "none");
}

#[tokio::test]
async fn networked_without_user_approved_is_rejected_with_approval_required() {
    let s = make_scheduler(parse_config("").unwrap());
    let args = AssignArgs {
        allow_domains: vec!["registry.npmjs.org".into()],
        ..assign(s.project.path(), "install deps")
    };
    assert_eq!(s.scheduler.assign_task(args).await.unwrap_err().code, ErrorCode::ApprovalRequired);
}

#[tokio::test]
async fn networked_with_user_approved_runs_and_records_an_agent_relayed_approval() {
    let s = make_scheduler(parse_config("").unwrap());
    let args = AssignArgs {
        allow_domains: vec!["registry.npmjs.org".into()],
        user_approved: true,
        wait: true,
        ..assign(s.project.path(), "install deps")
    };
    let outcome = s.scheduler.assign_task(args).await.unwrap();
    assert_eq!(outcome.tier.as_deref(), Some("networked"));
    assert_eq!(outcome.approval_state, "approved");
    assert_eq!(outcome.status, "completed");
    let approval = read_events(&s.log_path).unwrap().into_iter().find_map(|e| match e.body {
        EventBody::ApprovalRecorded { via, domains, .. } => Some((via, domains)),
        _ => None,
    });
    let (via, domains) = approval.unwrap();
    assert_eq!(via.as_str(), "agent");
    assert_eq!(domains, Some(vec!["registry.npmjs.org".to_string()]));
}
