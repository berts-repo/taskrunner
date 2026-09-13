use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use taskrunner::workers::claude::{ClaudeHarness, ClaudeHarnessOptions};
use taskrunner::workers::harness::{TurnRequest, WorkerHarness};
use tokio_util::sync::CancellationToken;

use crate::helpers::{LocalRunner, fake_claude};
use crate::{collect, kinds};

// realpath matters: the fake claude reports file_path from its resolved cwd
// (/private/var/... on macOS), and prefix stripping compares string paths.
fn workspace_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    (dir, real)
}

fn fake_runner(workspace: &Path) -> LocalRunner {
    LocalRunner::new(workspace, Some(&fake_claude()))
}

fn harness() -> ClaudeHarness {
    ClaudeHarness::new(ClaudeHarnessOptions::default())
}

async fn run(
    prompt: &str,
    resume: Option<&str>,
    cancel: CancellationToken,
) -> (
    Result<taskrunner::workers::harness::TurnResult, taskrunner::domain::errors::ToolError>,
    Vec<String>,
) {
    let (_dir, workspace) = workspace_dir();
    let (events, on_event) = collect();
    let result = harness()
        .run_turn(TurnRequest {
            runner: &fake_runner(&workspace),
            prompt: prompt.into(),
            native_session_id: resume.map(Into::into),
            cancel,
            on_event: &on_event,
        })
        .await;
    let kinds = kinds(&events);
    (result, kinds)
}

#[tokio::test]
async fn parses_stream_json() {
    let (result, kinds) = run("create hello", None, CancellationToken::new()).await;
    let result = result.unwrap();
    assert!(result.native_session_id.as_deref().unwrap().starts_with("sess-"));
    assert!(result.response.contains("started sess-"));
    // Workspace-relative: the absolute file_path prefix is stripped.
    assert_eq!(result.changed_files, vec!["hello.txt"]);
    assert_eq!(result.usage, Some(json!({ "input_tokens": 7, "output_tokens": 3 })));
    // Event kinds quote Claude's own line types verbatim.
    assert_eq!(kinds, vec!["system", "assistant", "user", "result"]);
}

#[tokio::test]
async fn passes_the_native_session_id_on_resume() {
    let (result, _) = run("continue please", Some("sess-existing"), CancellationToken::new()).await;
    let result = result.unwrap();
    assert_eq!(result.native_session_id.as_deref(), Some("sess-existing"));
    assert!(result.response.contains("resumed sess-existing"));
}

#[tokio::test]
async fn fails_when_claude_reports_an_error_result_despite_exit_0() {
    let (result, _) = run("result-error", None, CancellationToken::new()).await;
    assert!(result.unwrap_err().message.contains("fake claude task failed"));
}

#[tokio::test]
async fn fails_with_stderr_detail_on_nonzero_exit() {
    let (result, _) = run("exit-nonzero", None, CancellationToken::new()).await;
    let message = result.unwrap_err().message;
    assert!(message.contains("code 3") && message.contains("fake claude blew up"), "{message}");
}

#[tokio::test]
async fn kills_the_worker_on_abort() {
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        canceller.cancel();
    });
    let (result, _) = run("hang", None, cancel).await;
    assert!(result.unwrap_err().message.contains("terminated by abort"));
}
