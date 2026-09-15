use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use taskrunner::config::Provider;
use taskrunner::domain::errors::ToolError;
use taskrunner::workers::codex::{CodexHarness, CodexHarnessOptions};
use taskrunner::workers::harness::{TurnRequest, WorkerHarness};
use taskrunner::workers::runner::{RunnerKind, RunningWorker, WorkerRunner, WorkerSpawnSpec};
use tokio_util::sync::CancellationToken;

use crate::helpers::{LocalRunner, fake_codex};
use crate::{collect, kinds};

/// Local runner whose command override points at the fake codex script.
fn fake_runner(workspace: &Path) -> LocalRunner {
    LocalRunner::new(workspace, Some(&fake_codex()))
}

fn harness() -> CodexHarness {
    CodexHarness::new(CodexHarnessOptions::default())
}

#[tokio::test]
async fn parses_codex_exec_jsonl() {
    let workspace = tempfile::tempdir().unwrap();
    let (events, on_event) = collect();
    let result = harness()
        .run_turn(TurnRequest {
            runner: &fake_runner(workspace.path()),
            prompt: "create hello".into(),
            native_session_id: None,
            cancel: CancellationToken::new(),
            on_event: &on_event,
        })
        .await
        .unwrap();

    assert!(result.native_session_id.as_deref().unwrap().starts_with("thread-"));
    assert!(result.response.contains("started thread-"));
    assert_eq!(result.changed_files, vec!["hello.txt"]);
    assert_eq!(result.usage, Some(json!({ "input_tokens": 10, "output_tokens": 5 })));
    assert_eq!(
        kinds(&events),
        vec![
            "thread.started",
            "turn.started",
            "command_execution",
            "file_change",
            "agent_message",
            "turn.completed"
        ]
    );
}

#[tokio::test]
async fn passes_the_native_session_id_on_resume() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    let result = harness()
        .run_turn(TurnRequest {
            runner: &fake_runner(workspace.path()),
            prompt: "continue please".into(),
            native_session_id: Some("thread-existing".into()),
            cancel: CancellationToken::new(),
            on_event: &on_event,
        })
        .await
        .unwrap();
    assert_eq!(result.native_session_id.as_deref(), Some("thread-existing"));
    assert!(result.response.contains("resumed thread-existing"));
}

#[tokio::test]
async fn fails_with_stderr_detail_on_nonzero_exit() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    let err = harness()
        .run_turn(TurnRequest {
            runner: &fake_runner(workspace.path()),
            prompt: "exit-nonzero".into(),
            native_session_id: None,
            cancel: CancellationToken::new(),
            on_event: &on_event,
        })
        .await
        .unwrap_err();
    assert!(
        err.message.contains("code 3") && err.message.contains("fake codex blew up"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn kills_the_worker_on_abort() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        canceller.cancel();
    });
    let err = harness()
        .run_turn(TurnRequest {
            runner: &fake_runner(workspace.path()),
            prompt: "hang".into(),
            native_session_id: None,
            cancel,
            on_event: &on_event,
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("terminated by abort"), "{}", err.message);
}

#[tokio::test]
async fn cancels_a_worker_that_closed_its_output_and_kept_running() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        canceller.cancel();
    });
    // Reading this worker's output ends on its own, which is where cancel
    // used to stop being watched: the wait for exit then never returned, so
    // neither cancel-task nor the turn timeout could end the turn.
    let codex = harness();
    let runner = fake_runner(workspace.path());
    let turn = codex.run_turn(TurnRequest {
        runner: &runner,
        prompt: "close-stdout-and-hang".into(),
        native_session_id: None,
        cancel,
        on_event: &on_event,
    });
    let err = tokio::time::timeout(Duration::from_secs(5), turn)
        .await
        .expect("cancel did not end the turn")
        .unwrap_err();
    assert!(err.message.contains("terminated by abort"), "{}", err.message);
}

#[tokio::test]
async fn a_worker_that_exits_but_leaves_stderr_open_still_ends_its_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    // The worker exits at once; a child it started holds stderr for 30 s.
    let codex = harness();
    let runner = fake_runner(workspace.path());
    let turn = codex.run_turn(TurnRequest {
        runner: &runner,
        prompt: "exit-leaving-stderr-open".into(),
        native_session_id: None,
        cancel: CancellationToken::new(),
        on_event: &on_event,
    });
    let result = tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .expect("the turn waited on a pipe the worker no longer held")
        .unwrap();
    assert!(result.response.contains("done"), "{}", result.response);
}

/// A runner that records the spec and runs nothing (well, `true`).
struct CapturingRunner {
    captured: Mutex<Vec<WorkerSpawnSpec>>,
}

#[async_trait]
impl WorkerRunner for CapturingRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Docker
    }
    fn workspace_path(&self) -> &str {
        "/workspace"
    }
    async fn start(&self, spec: WorkerSpawnSpec) -> Result<RunningWorker, ToolError> {
        self.captured.lock().unwrap().push(spec);
        let child = tokio::process::Command::new("true")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        Ok(RunningWorker::new(child))
    }
    async fn dispose(&self) {}
}

#[tokio::test]
async fn builds_oss_argv_for_local_model_workers() {
    let runner = Arc::new(CapturingRunner { captured: Mutex::new(Vec::new()) });
    let (_, on_event) = collect();
    let harness = CodexHarness::new(CodexHarnessOptions {
        model: Some("gpt-oss:20b".into()),
        provider: Some(Provider::Ollama),
    });
    harness
        .run_turn(TurnRequest {
            runner: runner.as_ref(),
            prompt: "hello".into(),
            native_session_id: None,
            cancel: CancellationToken::new(),
            on_event: &on_event,
        })
        .await
        .unwrap();
    let captured = runner.captured.lock().unwrap();
    assert_eq!(
        captured[0].argv,
        vec![
            "codex",
            "-a",
            "never",
            "-s",
            "danger-full-access",
            "--oss",
            "--local-provider",
            "ollama",
            "-m",
            "gpt-oss:20b",
            "exec",
            "--json",
            "-C",
            "/workspace",
            "hello",
        ]
    );
    // The model server sits on the host; localhost would be the container.
    assert_eq!(
        captured[0].env.get("CODEX_OSS_BASE_URL").map(String::as_str),
        Some("http://host.docker.internal:11434/v1")
    );
    assert_eq!(captured[0].env.len(), 1);
}

#[tokio::test]
async fn fails_clearly_when_the_codex_binary_is_missing() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, on_event) = collect();
    let err = harness()
        .run_turn(TurnRequest {
            runner: &LocalRunner::new(
                workspace.path(),
                Some(Path::new("/nonexistent/codex-binary")),
            ),
            prompt: "x".into(),
            native_session_id: None,
            cancel: CancellationToken::new(),
            on_event: &on_event,
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("failed to start codex worker"), "{}", err.message);
}
