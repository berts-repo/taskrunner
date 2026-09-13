//! Thin git wrapper and the two workspace facts every provider shares.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::ids::{IdPrefix, new_id};
use crate::storage::Recorder;
use crate::storage::artifacts::ArtifactStore;
use crate::storage::events::EventBody;

pub struct GitOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn git(cwd: &Path, args: &[&str]) -> GitOutput {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).stdin(Stdio::null()).output();
    match output {
        Ok(output) => GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(err) => GitOutput { ok: false, stdout: String::new(), stderr: err.to_string() },
    }
}

/// Fallback changed-file detection shared by all workspace providers.
pub fn collect_git_changes(workspace_dir: &Path) -> Vec<String> {
    let status = git(workspace_dir, &["status", "--porcelain"]);
    status
        .stdout
        .split('\n')
        .filter(|line| line.len() >= 4)
        .map(|line| {
            let path = &line[3..];
            // A rename lists both names; the new one is what changed.
            path.split_once(" -> ").map_or(path, |(_, renamed)| renamed).to_string()
        })
        .collect()
}

/// Captures the turn's cumulative uncommitted diff as a linked artifact.
pub fn capture_diff_artifact(
    workspace_dir: &Path,
    task_id: &str,
    turn_id: &str,
    artifacts: &ArtifactStore,
    recorder: &dyn Recorder,
) -> anyhow::Result<()> {
    let diff = git(workspace_dir, &["diff", "HEAD"]);
    if !diff.ok || diff.stdout.trim().is_empty() {
        return Ok(());
    }
    let stored = artifacts.store(diff.stdout.as_bytes())?;
    let artifact_id = new_id(IdPrefix::Artifact);
    recorder.record(EventBody::ArtifactStored {
        artifact_id: artifact_id.clone(),
        kind: "diff".into(),
        label: "workspace diff after turn".into(),
        media_type: "text/x-diff".into(),
        size_bytes: stored.size_bytes,
        sha256: stored.sha256,
        locator: stored.locator,
    })?;
    recorder.record(EventBody::ArtifactLinked {
        artifact_id,
        session_id: None,
        task_id: Some(task_id.into()),
        turn_id: Some(turn_id.into()),
        audit_event_id: None,
    })?;
    Ok(())
}
