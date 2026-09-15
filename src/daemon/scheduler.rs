//! Turn lifecycle owner: assign/continue return immediately with a running
//! status unless `wait`; one running turn per task; every turn has a timeout;
//! networked tasks need a relayed user approval before they run.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, worker_config};
use crate::domain::errors::{ErrorCode, ToolError};
use crate::domain::policy::{RiskTier, resolve_tier};
use crate::domain::projects::resolve_project;
use crate::domain::tasks::{
    ArtifactHandle, INSPECTION_FAILED, TaskSnapshot, get_inspection_error, get_task_snapshot,
    get_turn_artifacts,
};
use crate::harnesses::worker_kind;
use crate::ids::{IdPrefix, new_id};
use crate::js;
use crate::storage::Recorder;
use crate::storage::artifacts::ArtifactStore;
use crate::storage::events::{Decision, EventBody, Via};
use crate::storage::store::SharedStore;
use crate::workers::harness::{TurnRequest, WorkerHarness};
use crate::workers::runner::WorkerRunner;
use crate::workspace::clone::WorkspaceProvider;
use crate::workspace::git::{InspectWith, task_branch, uncommitted_files};

/// Result contract shared by assign-task and continue-task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub task_id: String,
    pub turn_id: Option<String>,
    pub status: String,
    pub worker: String,
    pub worker_session_id: Option<String>,
    pub tier: Option<String>,
    /// none | pending | approved | denied
    pub approval_state: String,
    pub summary: Option<String>,
    pub changed_files: Vec<String>,
    pub artifacts: Vec<ArtifactHandle>,
    pub error: Option<TurnErrorInfo>,
    /// The branch the task's commits landed on in the user's repository.
    pub branch: Option<String>,
    /// Files uncommitted in the user's repository when the task was assigned.
    /// The worker's clone starts from the last commit, so it doesn't have them.
    pub uncommitted: Vec<String>,
    /// Why the turn's workspace couldn't be read back, when it couldn't: its
    /// diff and commits were then not captured.
    pub inspection_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnErrorInfo {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelResult {
    pub task_id: String,
    pub turn_id: Option<String>,
    pub status: String,
}

/// Everything the runner factory needs to place one turn's worker process.
pub struct RunnerContext {
    pub worker: String,
    pub workspace_dir: PathBuf,
    pub task_id: String,
    pub turn_id: String,
    /// Egress allowlist: worker API defaults plus approved task additions.
    pub allowed_domains: Vec<String>,
}

pub type MakeRunner = dyn Fn(RunnerContext) -> Box<dyn WorkerRunner> + Send + Sync;

pub struct SchedulerDeps {
    pub config: Arc<Config>,
    pub store: SharedStore,
    pub harnesses: HashMap<String, Arc<dyn WorkerHarness>>,
    pub workspaces: Arc<dyn WorkspaceProvider>,
    pub make_runner: Arc<MakeRunner>,
    pub artifacts: Arc<ArtifactStore>,
}

#[derive(Debug, Clone, Default)]
pub struct AssignArgs {
    pub project: String,
    pub worker: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub wait: bool,
    pub allow_domains: Vec<String>,
    pub user_approved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelReason {
    Cancel,
    Timeout,
}

/// Why a turn was aborted, decided by whoever aborts it first.
#[derive(Default)]
struct Cancellation {
    reason: Option<CancelReason>,
    note: Option<String>,
}

#[derive(Clone)]
struct RunningTurn {
    turn_id: String,
    cancel: CancellationToken,
    cancellation: Arc<Mutex<Cancellation>>,
    done: watch::Receiver<bool>,
}

impl RunningTurn {
    fn abort(&self, reason: CancelReason, note: Option<String>) {
        let mut cancellation = self.cancellation.lock().unwrap_or_else(|p| p.into_inner());
        cancellation.reason.get_or_insert(reason);
        if let Some(note) = note {
            cancellation.note.get_or_insert(note);
        }
        self.cancel.cancel();
    }

    async fn wait(&self) {
        let mut done = self.done.clone();
        while !*done.borrow() {
            if done.changed().await.is_err() {
                return; // the turn task is gone; nothing more will change
            }
        }
    }
}

#[derive(Clone)]
pub struct Scheduler {
    deps: Arc<SchedulerDeps>,
    running: Arc<Mutex<HashMap<String, RunningTurn>>>,
}

/// Both halves of the truth about a turn: a harness lists what its own edit
/// tool touched, git lists what actually ended up different — a turn that
/// edits through the shell as well as the tool is only fully described by the
/// two together. Harness paths can be absolute in the worker's view of the
/// workspace; git's are relative to it.
fn merge_changed_files(
    reported: Vec<String>,
    observed: Vec<String>,
    workspace_path: &str,
) -> Vec<String> {
    let prefix = format!("{}/", workspace_path.trim_end_matches('/'));
    let mut merged: BTreeSet<String> = reported
        .into_iter()
        .map(|path| path.strip_prefix(&prefix).unwrap_or(&path).to_string())
        .collect();
    merged.extend(observed);
    merged.into_iter().collect()
}

impl Scheduler {
    pub fn new(deps: SchedulerDeps) -> Scheduler {
        Scheduler { deps: Arc::new(deps), running: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub async fn assign_task(&self, args: AssignArgs) -> Result<TurnOutcome, ToolError> {
        let harness = self.harness_for(&args.worker)?;
        if js::trim(&args.prompt).is_empty() {
            return Err(ToolError::invalid_request("prompt must not be empty"));
        }
        let tier = resolve_tier(&args.allow_domains);
        if tier == RiskTier::Networked && !args.user_approved {
            return Err(ToolError::new(
                ErrorCode::ApprovalRequired,
                format!(
                    "this task needs outbound network access to: {}. Ask the user for permission first, then retry with userApproved: true.",
                    args.allow_domains.join(", ")
                ),
            ));
        }

        let project = resolve_project(&mut self.deps.store.lock(), &args.project)?;
        let task_id = new_id(IdPrefix::Task);
        self.deps.store.record(EventBody::TaskCreated {
            task_id: task_id.clone(),
            project_id: project.project_id,
            session_id: args.session_id.clone(),
            worker: args.worker.clone(),
            prompt_summary: summarize(&args.prompt),
            tier: Some(tier.as_str().into()),
            runtime: None,
            allow_domains: if args.allow_domains.is_empty() {
                None
            } else {
                Some(args.allow_domains.clone())
            },
        })?;
        if tier == RiskTier::Networked {
            // The calling agent relayed the user's yes; the record says so.
            self.deps.store.record(EventBody::ApprovalRecorded {
                approval_id: new_id(IdPrefix::Approval),
                task_id: task_id.clone(),
                decision: Decision::Approved,
                via: Via::Agent,
                domains: Some(args.allow_domains.clone()),
                session_id: args.session_id.clone(),
            })?;
        }
        let root = PathBuf::from(&project.root);
        let uncommitted = tokio::task::spawn_blocking(move || uncommitted_files(&root))
            .await
            .map_err(|err| ToolError::new(ErrorCode::InternalError, err.to_string()))?;
        let mut outcome =
            self.start_turn(task_id, project.root, harness, args.prompt, args.wait).await?;
        outcome.uncommitted = uncommitted;
        Ok(outcome)
    }

    pub async fn continue_task(
        &self,
        task_id: &str,
        prompt: &str,
        wait: bool,
    ) -> Result<TurnOutcome, ToolError> {
        let snapshot = self.snapshot_for(task_id)?;
        if self.has_running_turn(task_id) {
            return Err(ToolError::new(
                ErrorCode::Conflict,
                format!(
                    "task {task_id} already has a running turn; cancel it or wait for it to finish"
                ),
            ));
        }
        if js::trim(prompt).is_empty() {
            return Err(ToolError::invalid_request("prompt must not be empty"));
        }
        // Legacy state: tasks denied under the removed human-approval flow stay denied.
        if snapshot.approval_state == "denied" {
            return Err(ToolError::new(
                ErrorCode::PolicyDenied,
                format!("task {} was denied by the user and cannot run", snapshot.task_id),
            ));
        }
        let harness = self.harness_for(&snapshot.worker)?;
        self.start_turn(
            task_id.to_string(),
            snapshot.project_root,
            harness,
            prompt.to_string(),
            wait,
        )
        .await
    }

    pub async fn cancel_task(
        &self,
        task_id: &str,
        reason: Option<String>,
    ) -> Result<CancelResult, ToolError> {
        let snapshot = self.snapshot_for(task_id)?;
        let entry = self.running.lock().unwrap_or_else(|p| p.into_inner()).get(task_id).cloned();
        let Some(entry) = entry else {
            return Ok(CancelResult {
                task_id: task_id.into(),
                turn_id: None,
                status: snapshot.status,
            });
        };
        entry.abort(CancelReason::Cancel, reason);
        entry.wait().await;
        let after = self.snapshot_for(task_id).ok();
        Ok(CancelResult {
            task_id: task_id.into(),
            turn_id: Some(entry.turn_id),
            status: after.and_then(|s| s.latest_turn).map_or("canceled".into(), |t| t.status),
        })
    }

    /// Waits until the task has no running turn, or `timeout` passes, then
    /// reports its outcome. With no timeout it waits as long as the turn runs;
    /// every turn has its own time limit.
    pub async fn wait_for(
        &self,
        task_id: &str,
        timeout: Option<Duration>,
    ) -> Result<TurnOutcome, ToolError> {
        self.snapshot_for(task_id)?;
        let entry = self.running.lock().unwrap_or_else(|p| p.into_inner()).get(task_id).cloned();
        if let Some(entry) = entry {
            match timeout {
                Some(timeout) => {
                    let _ = tokio::time::timeout(timeout, entry.wait()).await;
                }
                None => entry.wait().await,
            }
        }
        self.outcome(task_id)
    }

    pub fn has_running_turn(&self, task_id: &str) -> bool {
        self.running.lock().unwrap_or_else(|p| p.into_inner()).contains_key(task_id)
    }

    /// Aborts all running turns and waits for their terminal records.
    pub async fn shutdown(&self) {
        let entries: Vec<RunningTurn> =
            self.running.lock().unwrap_or_else(|p| p.into_inner()).values().cloned().collect();
        for entry in &entries {
            entry.abort(CancelReason::Cancel, Some("daemon stopping".into()));
        }
        for entry in &entries {
            entry.wait().await;
        }
    }

    fn snapshot_for(&self, task_id: &str) -> Result<TaskSnapshot, ToolError> {
        get_task_snapshot(&self.deps.store.lock().index, task_id)?
            .ok_or_else(|| ToolError::not_found(format!("no task {task_id}")))
    }

    fn harness_for(&self, worker: &str) -> Result<Arc<dyn WorkerHarness>, ToolError> {
        self.deps.harnesses.get(worker).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorCode::NotConfigured,
                format!("worker '{worker}' is not configured on this workstation"),
            )
        })
    }

    async fn start_turn(
        &self,
        task_id: String,
        project_root: String,
        harness: Arc<dyn WorkerHarness>,
        prompt: String,
        wait: bool,
    ) -> Result<TurnOutcome, ToolError> {
        let turn_id = new_id(IdPrefix::Turn);
        self.deps.store.record(EventBody::TurnStarted {
            turn_id: turn_id.clone(),
            task_id: task_id.clone(),
            prompt: prompt.clone(),
        })?;

        let (done_tx, done_rx) = watch::channel(false);
        let entry = RunningTurn {
            turn_id: turn_id.clone(),
            cancel: CancellationToken::new(),
            cancellation: Arc::new(Mutex::new(Cancellation::default())),
            done: done_rx,
        };
        self.running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.clone(), entry.clone());
        let scheduler = self.clone();
        let running_entry = entry.clone();
        let task = task_id.clone();
        tokio::spawn(async move {
            scheduler.run_turn(&running_entry, &task, &project_root, harness, &prompt).await;
            scheduler.running.lock().unwrap_or_else(|p| p.into_inner()).remove(&task);
            let _ = done_tx.send(true);
        });

        if wait {
            entry.wait().await;
        }
        self.outcome_of(&task_id, Some(&turn_id))
    }

    async fn run_turn(
        &self,
        entry: &RunningTurn,
        task_id: &str,
        project_root: &str,
        harness: Arc<dyn WorkerHarness>,
        prompt: &str,
    ) {
        let timeout_seconds = self.deps.config.task.turn_timeout_seconds.get();
        let timer = {
            let entry = entry.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(timeout_seconds)).await;
                entry.abort(CancelReason::Timeout, None);
            })
        };

        let raw_events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut runner: Option<Box<dyn WorkerRunner>> = None;
        let attempt = self
            .attempt_turn(entry, task_id, project_root, harness, prompt, &raw_events, &mut runner)
            .await;

        if let Err(err) = attempt {
            let cancellation = entry.cancellation.lock().unwrap_or_else(|p| p.into_inner());
            let terminal = match cancellation.reason {
                Some(CancelReason::Cancel) => EventBody::TurnCanceled {
                    turn_id: entry.turn_id.clone(),
                    task_id: task_id.into(),
                    reason: cancellation.note.clone(),
                },
                Some(CancelReason::Timeout) => EventBody::TurnFailed {
                    turn_id: entry.turn_id.clone(),
                    task_id: task_id.into(),
                    error_code: "worker_failed".into(),
                    error_message: format!("turn timed out after {timeout_seconds}s"),
                },
                None => EventBody::TurnFailed {
                    turn_id: entry.turn_id.clone(),
                    task_id: task_id.into(),
                    error_code: err.code.as_str().into(),
                    error_message: err.message,
                },
            };
            let _ = self.deps.store.record(terminal);
        }

        timer.abort();
        if let Some(runner) = runner {
            runner.dispose().await;
        }
        // Bulky raw event streams are end-of-turn copy-out; the per-event
        // audit records are already durable.
        let lines = std::mem::take(&mut *raw_events.lock().unwrap_or_else(|p| p.into_inner()));
        if !lines.is_empty() {
            self.store_raw_events(task_id, &entry.turn_id, &lines);
        }
    }

    /// The turn from workspace to `turn.completed`; any failure is classified
    /// by the caller.
    #[allow(clippy::too_many_arguments)]
    async fn attempt_turn(
        &self,
        entry: &RunningTurn,
        task_id: &str,
        project_root: &str,
        harness: Arc<dyn WorkerHarness>,
        prompt: &str,
        raw_events: &Arc<Mutex<Vec<String>>>,
        runner_slot: &mut Option<Box<dyn WorkerRunner>>,
    ) -> Result<(), ToolError> {
        let snapshot = self.snapshot_for(task_id)?;
        let workspaces = self.deps.workspaces.clone();
        let workspace_dir = {
            let (task, root) = (task_id.to_string(), PathBuf::from(project_root));
            tokio::task::spawn_blocking(move || workspaces.ensure_workspace(&task, &root))
                .await
                .map_err(|err| ToolError::new(ErrorCode::InternalError, err.to_string()))??
        };
        let previous_native_id = snapshot.worker_session_id.clone();

        let worker_cfg = worker_config(&self.deps.config, &snapshot.worker);
        let approved = if snapshot.approval_state == "approved" {
            snapshot.allow_domains.clone()
        } else {
            vec![]
        };
        let runner = (self.deps.make_runner)(RunnerContext {
            worker: snapshot.worker.clone(),
            workspace_dir: workspace_dir.clone(),
            task_id: task_id.into(),
            turn_id: entry.turn_id.clone(),
            allowed_domains: worker_cfg.allowed_domains.iter().chain(&approved).cloned().collect(),
        });
        let runner = runner_slot.insert(runner);
        let workspace_path = runner.workspace_path().to_string();

        // Streamed into the audit log as they arrive, not buffered, so a
        // crashed turn keeps its partial trail.
        let store = self.deps.store.clone();
        let (task, turn) = (task_id.to_string(), entry.turn_id.clone());
        let raw = raw_events.clone();
        let on_event = move |event: crate::workers::harness::WorkerEvent| {
            raw.lock().unwrap_or_else(|p| p.into_inner()).push(
                serde_json::json!({ "kind": event.kind, "payload": event.payload }).to_string(),
            );
            let _ = store.record(EventBody::AuditRecorded {
                session_id: None,
                task_id: Some(task.clone()),
                turn_id: Some(turn.clone()),
                kind: format!("worker.{}", event.kind),
                payload: event.payload,
            });
        };
        let result = harness
            .run_turn(TurnRequest {
                runner: runner.as_ref(),
                prompt: prompt.into(),
                native_session_id: previous_native_id.clone(),
                cancel: entry.cancel.clone(),
                on_event: &on_event,
            })
            .await?;

        if let Some(native) = &result.native_session_id
            && Some(native) != previous_native_id.as_ref()
        {
            self.deps.store.record(EventBody::WorkerSessionRecorded {
                worker_session_id: new_id(IdPrefix::WorkerSession),
                task_id: task_id.into(),
                worker: harness.name().into(),
                native_session_id: native.clone(),
                turn_id: Some(entry.turn_id.clone()),
            })?;
        }

        // The inspection gets what the worker had: its image, its limits.
        let with = InspectWith {
            image: worker_cfg.image.clone().or_else(|| {
                worker_kind(&self.deps.config, &snapshot.worker)
                    .map(|kind| kind.defaults().image.to_string())
            }),
            limits: worker_cfg.limits.clone(),
        };
        let workspaces = self.deps.workspaces.clone();
        let (task, turn, dir, root) = (
            task_id.to_string(),
            entry.turn_id.clone(),
            workspace_dir.clone(),
            PathBuf::from(project_root),
        );
        let after = tokio::task::spawn_blocking(move || {
            workspaces.after_turn(&task, &turn, &dir, &root, &with)
        })
        .await
        .map_err(|err| ToolError::new(ErrorCode::InternalError, err.to_string()))?;
        if let Some(error) = &after.inspection_error {
            self.deps.store.record(EventBody::AuditRecorded {
                session_id: None,
                task_id: Some(task_id.into()),
                turn_id: Some(entry.turn_id.clone()),
                kind: INSPECTION_FAILED.into(),
                payload: serde_json::json!({ "error": error }),
            })?;
        }
        let changed_files =
            merge_changed_files(result.changed_files, after.changed_files, &workspace_path);

        self.deps.store.record(EventBody::TurnCompleted {
            turn_id: entry.turn_id.clone(),
            task_id: task_id.into(),
            response: result.response,
            changed_files,
            usage: result.usage,
        })?;
        Ok(())
    }

    fn store_raw_events(&self, task_id: &str, turn_id: &str, lines: &[String]) {
        let stored = match self.deps.artifacts.store(format!("{}\n", lines.join("\n")).as_bytes()) {
            Ok(stored) => stored,
            Err(err) => {
                eprintln!("taskrunner: could not store raw worker events: {err}");
                return;
            }
        };
        let artifact_id = new_id(IdPrefix::Artifact);
        let _ = self.deps.store.record(EventBody::ArtifactStored {
            artifact_id: artifact_id.clone(),
            kind: "worker-events".into(),
            label: "raw worker events".into(),
            media_type: "application/jsonl".into(),
            size_bytes: stored.size_bytes,
            sha256: stored.sha256,
            locator: stored.locator,
        });
        let _ = self.deps.store.record(EventBody::ArtifactLinked {
            artifact_id,
            session_id: None,
            task_id: Some(task_id.into()),
            turn_id: Some(turn_id.into()),
            audit_event_id: None,
        });
    }

    /// The current outcome of a task, or of one specific turn of it.
    pub fn outcome(&self, task_id: &str) -> Result<TurnOutcome, ToolError> {
        self.outcome_of(task_id, None)
    }

    fn outcome_of(&self, task_id: &str, turn_id: Option<&str>) -> Result<TurnOutcome, ToolError> {
        let store = self.deps.store.lock();
        let snapshot = get_task_snapshot(&store.index, task_id)?.ok_or_else(|| {
            ToolError::new(ErrorCode::InternalError, format!("task {task_id} vanished"))
        })?;
        let turn = match (turn_id, snapshot.latest_turn) {
            (Some(wanted), Some(latest)) if latest.turn_id != wanted => None,
            (_, latest) => latest,
        };
        let artifacts = match &turn {
            Some(turn) => get_turn_artifacts(&store.index, &turn.turn_id)?,
            None => vec![],
        };
        let error = turn.as_ref().and_then(|t| match (&t.error_code, t.status.as_str()) {
            (Some(code), "failed") => Some(TurnErrorInfo {
                code: code.clone(),
                message: t.error_message.clone().unwrap_or_default(),
            }),
            _ => None,
        });
        let inspection_error = match &turn {
            Some(turn) => get_inspection_error(&store.index, &turn.turn_id)?,
            None => None,
        };
        // Git runs outside the store lock.
        drop(store);
        let branch = task_branch(Path::new(&snapshot.project_root), task_id);
        Ok(TurnOutcome {
            task_id: task_id.into(),
            turn_id: turn
                .as_ref()
                .map(|t| t.turn_id.clone())
                .or_else(|| turn_id.map(str::to_string)),
            status: turn.as_ref().map_or(snapshot.status, |t| t.status.clone()),
            worker: snapshot.worker,
            worker_session_id: snapshot.worker_session_id,
            tier: snapshot.tier,
            approval_state: snapshot.approval_state,
            summary: turn.as_ref().and_then(|t| t.response.clone()),
            changed_files: turn.as_ref().map(|t| t.changed_files.clone()).unwrap_or_default(),
            artifacts,
            error,
            branch,
            uncommitted: vec![],
            inspection_error,
        })
    }
}

fn summarize(prompt: &str) -> String {
    let line = js::collapse_whitespace(prompt);
    if js::len(&line) <= 100 { line } else { format!("{}...", js::slice_to(&line, 97)) }
}
