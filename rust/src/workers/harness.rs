//! Shared worker harness contract: start a turn, resume by worker-native
//! session id, stream structured events, capture the final response and
//! changed files.

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::runner::WorkerRunner;
use crate::domain::errors::ToolError;

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerEvent {
    pub kind: String,
    pub payload: Value,
}

pub type OnEvent = dyn Fn(WorkerEvent) + Send + Sync;

pub struct TurnRequest<'a> {
    /// Where and how the worker process runs (host spawn or Docker container).
    pub runner: &'a dyn WorkerRunner,
    pub prompt: String,
    /// Resume this worker-native session when present.
    pub native_session_id: Option<String>,
    /// Cancelled by the scheduler on cancellation or timeout.
    pub cancel: CancellationToken,
    /// Streamed as events arrive; the scheduler appends them to the audit log.
    pub on_event: &'a OnEvent,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnResult {
    pub response: String,
    pub native_session_id: Option<String>,
    /// Harness-reported changed files; the workspace git fallback fills gaps.
    pub changed_files: Vec<String>,
    pub usage: Option<Value>,
}

#[async_trait]
pub trait WorkerHarness: Send + Sync {
    fn name(&self) -> &str;
    /// Fails on worker failure, keeping a preflight error's code; anything
    /// else is `worker_failed`. Terminates the worker when `cancel` fires.
    async fn run_turn(&self, request: TurnRequest<'_>) -> Result<TurnResult, ToolError>;
}

/// Reads a worker's stdout line by line and stderr as a tail, until it exits.
/// Shared by both harnesses: they differ only in what they make of each line.
pub(crate) mod drive {
    use std::process::ExitStatus;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    use tokio_util::sync::CancellationToken;

    use super::super::runner::RunningWorker;
    use crate::domain::errors::{ErrorCode, ToolError};

    /// How a worker run ended.
    pub struct Exit {
        pub code: Option<i32>,
        pub stderr_tail: String,
        pub aborted: bool,
    }

    /// Feeds every non-blank stdout line to `on_line`, kills the worker if
    /// `cancel` fires, and reports how it exited.
    pub async fn run(
        mut worker: RunningWorker,
        cancel: &CancellationToken,
        mut on_line: impl FnMut(&str),
    ) -> Exit {
        let stdout = worker.child.stdout.take();
        let stderr = worker.child.stderr.take();
        let stderr_tail = tokio::spawn(async move {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_string(&mut text).await;
            }
            // Keep the last 4 KB, as the TypeScript harness does.
            let cut = text.len().saturating_sub(4096);
            text.split_off(cut)
        });
        let lines = async {
            if let Some(stdout) = stdout {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        on_line(&line);
                    }
                }
            }
        };
        let aborted = tokio::select! {
            _ = lines => false,
            _ = cancel.cancelled() => {
                worker.kill();
                true
            }
        };
        let status: Option<ExitStatus> = worker.child.wait().await.ok();
        Exit {
            code: status.and_then(|s| s.code()),
            stderr_tail: stderr_tail.await.unwrap_or_default(),
            aborted: aborted || cancel.is_cancelled(),
        }
    }

    /// The failure a nonzero exit (or an abort) turns into.
    pub fn failure(worker: &str, exit: &Exit, detail: Option<&str>) -> ToolError {
        if exit.aborted {
            return ToolError::new(
                ErrorCode::WorkerFailed,
                format!("{worker} worker terminated by abort"),
            );
        }
        let code = exit.code.map_or("null".to_string(), |c| c.to_string());
        let detail =
            detail.map(str::to_string).unwrap_or_else(|| exit.stderr_tail.trim().to_string());
        let suffix = if detail.is_empty() { String::new() } else { format!(": {detail}") };
        ToolError::new(ErrorCode::WorkerFailed, format!("{worker} exited with code {code}{suffix}"))
    }
}
