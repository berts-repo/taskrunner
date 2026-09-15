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

/// Shown wherever a turn's result is, when its workspace couldn't be read back.
pub fn inspection_warning(error: &str) -> String {
    format!(
        "warning: the worker's workspace could not be read back, so this turn's diff and commits were not captured ({error})"
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
    if let Some(branch) = &outcome.branch {
        lines.push(format!("branch: {branch} (not merged — review it before merging)"));
    }
    if !outcome.uncommitted.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "not included: {} uncommitted file(s) — the worker's clone starts from the last commit:",
            outcome.uncommitted.len()
        ));
        lines.extend(outcome.uncommitted.iter().take(10).map(|f| format!("  {f}")));
        if outcome.uncommitted.len() > 10 {
            lines.push(format!("  … and {} more", outcome.uncommitted.len() - 10));
        }
    }
    if let Some(error) = &outcome.inspection_error {
        lines.push(String::new());
        lines.push(inspection_warning(error));
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

/// The short result `taskrunner wait` prints when a wait ends. It lands in an
/// agent's context, so it carries only enough to decide what to read next.
pub fn render_wait(outcome: &TurnOutcome) -> String {
    let mut lines =
        vec![format!("task: {}", outcome.task_id), format!("status: {}", outcome.status)];
    if let Some(branch) = &outcome.branch {
        lines.push(format!("branch: {branch} (not merged — review it before merging)"));
    }
    if let Some(error) = &outcome.inspection_error {
        lines.push(inspection_warning(error));
    }
    if let Some(error) = &outcome.error {
        lines.push(format!("error {}: {}", error.code, error.message));
    }
    if !outcome.changed_files.is_empty() {
        lines.push(format!("changed files: {}", outcome.changed_files.len()));
    }
    let first_line =
        outcome.summary.as_deref().and_then(|s| s.lines().find(|l| !l.trim().is_empty()));
    if let Some(line) = first_line {
        let short: String = line.chars().take(200).collect();
        let cut = if line.chars().count() > 200 { "…" } else { "" };
        lines.push(format!("summary: {short}{cut}"));
    }
    if outcome.status == "running" || outcome.status == "created" {
        lines.push("still running: wait again, or check with lookup-task later".to_string());
    } else {
        lines.push(format!(
            "full result: lookup-task {} with include [\"turns\", \"diff\"]",
            outcome.task_id
        ));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn render_cancel(result: &CancelResult) -> String {
    let mut lines = vec![format!("task: {}", result.task_id), format!("status: {}", result.status)];
    match &result.turn_id {
        Some(turn_id) => lines.insert(1, format!("turn: {turn_id}")),
        None => lines.push("no turn was running".to_string()),
    }
    lines.join("\n")
}
