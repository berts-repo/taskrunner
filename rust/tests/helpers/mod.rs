//! Shared test scaffolding, mirroring the TypeScript tests/helpers.ts.
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

/// Absolute path of a file under the repository's tests/fixtures, which the
/// TypeScript tests share.
pub fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures").join(relative)
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
