//! Shared test scaffolding.
//! Included by several test binaries, so each uses only part of it.
#![allow(dead_code)]

use std::cell::Cell;

use taskrunner::storage::events::{EventBody, LogEvent};

thread_local! {
    static SEQ: Cell<u32> = const { Cell::new(0) };
}

/// Builds a LogEvent with deterministic id/ts without going through a log file.
pub fn evt(body: EventBody) -> LogEvent {
    let seq = SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let ts = chrono::DateTime::from_timestamp(1_767_225_600 + i64::from(seq), 0)
        .expect("fixed epoch")
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    LogEvent { id: format!("evt_{seq:08}"), ts, body }
}

/// A realistic event sequence: one project, session, task, and completed turn.
pub fn sample_sequence() -> Vec<LogEvent> {
    vec![
        evt(EventBody::ProjectCreated { project_id: "proj_a".into(), root: "/repo".into() }),
        evt(EventBody::ProjectAliasAdded {
            project_id: "proj_a".into(),
            path: "/repo-symlink".into(),
        }),
        evt(EventBody::SessionStarted {
            session_id: "sess_a".into(),
            project_id: Some("proj_a".into()),
            client: Some("claude-code".into()),
            host: None,
        }),
        evt(EventBody::TaskCreated {
            task_id: "task_a".into(),
            project_id: "proj_a".into(),
            session_id: Some("sess_a".into()),
            worker: "codex".into(),
            prompt_summary: "add greeting file".into(),
            tier: None,
            runtime: None,
            allow_domains: None,
        }),
        evt(EventBody::TurnStarted {
            turn_id: "turn_a1".into(),
            task_id: "task_a".into(),
            prompt: "create hello.txt".into(),
        }),
        evt(EventBody::AuditRecorded {
            session_id: None,
            task_id: Some("task_a".into()),
            turn_id: Some("turn_a1".into()),
            kind: "worker.command_execution".into(),
            payload: serde_json::json!({ "command": "touch hello.txt" }),
        }),
        evt(EventBody::WorkerSessionRecorded {
            worker_session_id: "wsess_a".into(),
            task_id: "task_a".into(),
            worker: "codex".into(),
            native_session_id: "019e28f6-9f73-73d0-b601-33505b06d3f5".into(),
            turn_id: None,
        }),
        evt(EventBody::ArtifactStored {
            artifact_id: "art_a".into(),
            kind: "worker-events".into(),
            label: "raw codex events".into(),
            media_type: "application/jsonl".into(),
            size_bytes: 123,
            sha256: "ab".repeat(32),
            locator: format!("ab/{}", "ab".repeat(32)),
        }),
        evt(EventBody::ArtifactLinked {
            artifact_id: "art_a".into(),
            session_id: None,
            task_id: Some("task_a".into()),
            turn_id: Some("turn_a1".into()),
            audit_event_id: None,
        }),
        evt(EventBody::TurnCompleted {
            turn_id: "turn_a1".into(),
            task_id: "task_a".into(),
            response: "created hello.txt".into(),
            changed_files: vec!["hello.txt".into()],
            usage: None,
        }),
        evt(EventBody::SessionEnded { session_id: "sess_a".into() }),
    ]
}

/// A message.recorded body with only the fields a test cares about set.
pub fn message(
    message_id: &str,
    source: &str,
    session: &str,
    role: &str,
    kind: &str,
    content: &str,
) -> EventBody {
    EventBody::MessageRecorded {
        message_id: message_id.into(),
        source: source.into(),
        native_session_id: session.into(),
        native_record_id: message_id.into(),
        role: role.into(),
        kind: kind.into(),
        content: content.into(),
        native_ts: None,
        project_path: None,
    }
}

// ---- worker test seams ----------------------------------------------------

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use taskrunner::domain::errors::{ErrorCode, ToolError};
use taskrunner::workers::runner::{RunnerKind, RunningWorker, WorkerRunner, WorkerSpawnSpec};

/// Absolute path of a file under tests/fixtures.
pub fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(relative)
}

/// The scripted stand-ins for the codex and claude CLIs (Node scripts).
pub fn fake_codex() -> PathBuf {
    fixture_path("fake-codex.cjs")
}

pub fn fake_claude() -> PathBuf {
    fixture_path("fake-claude.cjs")
}

/// Throwaway git repo with one committed README, for clone-workspace tests.
pub fn init_git_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(repo.path().join("README.md"), "hi\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    repo
}

/// Spawns the worker argv directly in the workspace: the no-Docker test runner.
pub struct LocalRunner {
    pub workspace: PathBuf,
    /// Overrides argv[0], e.g. a fake codex binary path.
    pub command: Option<PathBuf>,
}

impl LocalRunner {
    pub fn new(workspace: &Path, command: Option<&Path>) -> LocalRunner {
        LocalRunner { workspace: workspace.to_path_buf(), command: command.map(Path::to_path_buf) }
    }
}

#[async_trait]
impl WorkerRunner for LocalRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Host
    }

    fn workspace_path(&self) -> &str {
        self.workspace.to_str().unwrap()
    }

    async fn start(&self, spec: WorkerSpawnSpec) -> Result<RunningWorker, ToolError> {
        let (logical, rest) = spec.argv.split_first().expect("argv has a command");
        let program = self.command.clone().unwrap_or_else(|| PathBuf::from(logical));
        let child = tokio::process::Command::new(&program)
            .args(rest)
            .current_dir(&self.workspace)
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                ToolError::new(
                    ErrorCode::WorkerFailed,
                    format!("failed to start {} worker: {err}", logical),
                )
            })?;
        Ok(RunningWorker::new(child))
    }

    async fn dispose(&self) {}
}

// ---- scheduler test seams -------------------------------------------------

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use taskrunner::workers::harness::{TurnRequest, TurnResult, WorkerEvent, WorkerHarness};
use taskrunner::workspace::clone::WorkspaceProvider;

/// Runs workers directly in the project root: no clone, no isolation.
pub struct ProjectRootWorkspaces;

impl WorkspaceProvider for ProjectRootWorkspaces {
    fn ensure_workspace(&self, _task_id: &str, project_root: &Path) -> Result<PathBuf, ToolError> {
        Ok(project_root.to_path_buf())
    }
    fn after_turn(
        &self,
        _task_id: &str,
        _turn_id: &str,
        _workspace_dir: &Path,
        _project_root: &Path,
    ) -> Vec<String> {
        vec![]
    }
}

/// Scripted in-process harness for tests. Behavior is driven by directives in
/// the prompt: `sleep:<ms>` delays (abortably), `fail` fails. Native session
/// ids are `fake-<n>` and each resumed turn increments a per-session counter
/// so continuation is observable.
#[derive(Default)]
pub struct FakeHarness {
    next_session: AtomicU32,
    turn_counts: Mutex<std::collections::HashMap<String, u32>>,
}

#[async_trait]
impl WorkerHarness for FakeHarness {
    fn name(&self) -> &str {
        "fake"
    }

    async fn run_turn(&self, request: TurnRequest<'_>) -> Result<TurnResult, ToolError> {
        (request.on_event)(WorkerEvent {
            kind: "agent_message".into(),
            payload: serde_json::json!({ "text": "fake worker starting" }),
        });

        if let Some(ms) = request
            .prompt
            .split_whitespace()
            .find_map(|w| w.strip_prefix("sleep:"))
            .and_then(|n| n.parse::<u64>().ok())
        {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(ms)) => {}
                _ = request.cancel.cancelled() => {}
            }
        }
        if request.cancel.is_cancelled() {
            return Err(ToolError::new(ErrorCode::WorkerFailed, "fake worker aborted"));
        }
        if request.prompt.contains("fail") {
            return Err(ToolError::new(ErrorCode::WorkerFailed, "fake worker failure"));
        }

        let session_id = request.native_session_id.clone().unwrap_or_else(|| {
            format!("fake-{}", self.next_session.fetch_add(1, Ordering::Relaxed) + 1)
        });
        let turn = {
            let mut counts = self.turn_counts.lock().unwrap();
            let n = counts.entry(session_id.clone()).or_insert(0);
            *n += 1;
            *n
        };
        (request.on_event)(WorkerEvent {
            kind: "command_execution".into(),
            payload: serde_json::json!({ "command": "true" }),
        });
        Ok(TurnResult {
            response: format!("echo: {} (session {session_id}, turn {turn})", request.prompt),
            native_session_id: Some(session_id),
            changed_files: vec![],
            usage: None,
        })
    }
}

// ---- workspace test seams -------------------------------------------------

use taskrunner::storage::Recorder;

/// Keeps nothing: for tests that need a workspace provider but assert on the
/// daemon's own store rather than on what the provider recorded.
pub struct DiscardRecorder;

impl Recorder for DiscardRecorder {
    fn record(&self, body: EventBody) -> anyhow::Result<LogEvent> {
        Ok(LogEvent { id: "evt_discarded".into(), ts: "2026-01-01T00:00:00.000Z".into(), body })
    }
}
