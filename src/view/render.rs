//! Tool responses are compact readable text with handles, not raw JSON blobs.

use crate::daemon::scheduler::{CancelResult, TurnOutcome};
use crate::domain::tasks::ArtifactHandle;
use crate::view::lookup::native_session_suffix;

fn artifact_line(a: &ArtifactHandle) -> String {
    format!(
        "  {}  {}  {} ({}, {} bytes)",
        a.artifact_id, a.kind, a.label, a.media_type, a.size_bytes
    )
}

pub fn render_outcome(outcome: &TurnOutcome) -> String {
    let mut lines = vec![
        format!("task: {}", outcome.task_id),
        format!("turn: {}", outcome.turn_id.as_deref().unwrap_or("-")),
        format!("status: {}", outcome.status),
        format!(
            "worker: {}{}",
            outcome.worker,
            native_session_suffix(outcome.worker_session_id.as_deref())
        ),
    ];
    if let Some(tier) = outcome.tier.as_deref().filter(|t| !t.is_empty()) {
        lines.push(format!("tier: {tier}"));
    }
    if outcome.approval_state != "none" {
        lines.push(format!("approval: {}", outcome.approval_state));
    }
    if let Some(summary) = outcome.summary.as_deref().filter(|s| !s.is_empty()) {
        lines.push(String::new());
        lines.push(summary.to_string());
    }
    if !outcome.changed_files.is_empty() {
        lines.push(String::new());
        lines.push("changed files:".to_string());
        lines.extend(outcome.changed_files.iter().map(|f| format!("  {f}")));
    }
    if !outcome.artifacts.is_empty() {
        lines.push(String::new());
        lines.push("artifacts:".to_string());
        lines.extend(outcome.artifacts.iter().map(artifact_line));
    }
    if let Some(error) = &outcome.error {
        lines.push(String::new());
        lines.push(format!("error {}: {}", error.code, error.message));
    }
    if outcome.status == "running" {
        lines.push(String::new());
        lines.push(
            "Turn is running. Use lookup-task with this task id to retrieve the result."
                .to_string(),
        );
    }
    lines.join("\n")
}

pub fn render_cancel(result: &CancelResult) -> String {
    let mut lines = vec![format!("task: {}", result.task_id), format!("status: {}", result.status)];
    match &result.turn_id {
        Some(turn_id) => lines.insert(1, format!("turn: {turn_id}")),
        None => lines.push("no turn was running".to_string()),
    }
    lines.join("\n")
}
