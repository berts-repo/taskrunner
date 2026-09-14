//! Task-local clone workspaces for Docker workers: a worktree's `.git` file
//! points back into the main repository, so mounting one into a container
//! either breaks git or exposes the host `.git`. A clone is fully
//! self-contained; the container mounts only the clone directory.
//!
//! `--no-hardlinks` matters: a default local clone hardlinks object files,
//! which share inodes with the host repository — a worker writing through
//! one could corrupt host history. Committed work lands host-side via fetch
//! in `after_turn`; the worker never touches the real repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::git::{capture_diff_artifact, collect_git_changes, git};
use crate::domain::errors::{ErrorCode, ToolError};
use crate::storage::Recorder;
use crate::storage::artifacts::ArtifactStore;

/// Provides the per-task isolated workspace.
pub trait WorkspaceProvider: Send + Sync {
    fn ensure_workspace(&self, task_id: &str, project_root: &Path) -> Result<PathBuf, ToolError>;
    /// Git-based fallback when the harness reports nothing.
    fn collect_changes(&self, workspace_dir: &Path) -> Vec<String>;
    /// Post-turn hook: diff artifacts and clone-branch landing.
    fn after_turn(&self, task_id: &str, turn_id: &str, workspace_dir: &Path, project_root: &Path);
}

pub struct CloneWorkspaces {
    workspaces_dir: PathBuf,
    artifacts: Arc<ArtifactStore>,
    recorder: Arc<dyn Recorder>,
}

impl CloneWorkspaces {
    pub fn new(
        workspaces_dir: &Path,
        artifacts: Arc<ArtifactStore>,
        recorder: Arc<dyn Recorder>,
    ) -> CloneWorkspaces {
        CloneWorkspaces { workspaces_dir: workspaces_dir.to_path_buf(), artifacts, recorder }
    }
}

impl WorkspaceProvider for CloneWorkspaces {
    fn ensure_workspace(&self, task_id: &str, project_root: &Path) -> Result<PathBuf, ToolError> {
        let dir = self.workspaces_dir.join(task_id);
        if dir.join(".git").exists() {
            return Ok(dir);
        }
        if !git(project_root, &["rev-parse", "--git-dir"]).ok {
            return Err(ToolError::invalid_request(format!(
                "project is not a git repository, so no task workspace can be created: {}",
                project_root.display()
            )));
        }
        fs::create_dir_all(&self.workspaces_dir)?;
        let cloned = git(
            Path::new("."),
            &["clone", "--no-hardlinks", &project_root.to_string_lossy(), &dir.to_string_lossy()],
        );
        if !cloned.ok {
            return Err(ToolError::new(
                ErrorCode::InternalError,
                format!("failed to create task workspace clone: {}", cloned.stderr.trim()),
            ));
        }
        let branch = git(&dir, &["checkout", "-b", &format!("taskrunner/{task_id}")]);
        if !branch.ok {
            return Err(ToolError::new(
                ErrorCode::InternalError,
                format!("failed to create task branch in clone: {}", branch.stderr.trim()),
            ));
        }
        // The origin remote points at the host repository by absolute path;
        // that path is meaningless (and must stay unreachable) inside the
        // container.
        git(&dir, &["remote", "remove", "origin"]);
        // The container's non-root worker user has a different uid than the
        // host user who owns this clone (a bind mount keeps host uids, and
        // the base image's own "node" user often already claims uid 1000),
        // so every file and dir here looks owned-by-someone-else to the
        // worker: it can create nothing and edit nothing. World-writable is
        // fine: this is a disposable, single-task clone with no secrets,
        // discarded once the turn lands its changes host-side. `o+rwX` (not
        // `o+rwx`) only adds execute where a file already had one, so plain
        // files don't spuriously become executable.
        let chmodded = std::process::Command::new("chmod")
            .args(["-R", "o+rwX"])
            .arg(&dir)
            .output()
            .map_err(|err| ToolError::new(ErrorCode::InternalError, err.to_string()))?;
        if !chmodded.status.success() {
            return Err(ToolError::new(
                ErrorCode::InternalError,
                format!(
                    "failed to make task workspace writable by the container user: {}",
                    String::from_utf8_lossy(&chmodded.stderr).trim()
                ),
            ));
        }
        Ok(dir)
    }

    fn collect_changes(&self, workspace_dir: &Path) -> Vec<String> {
        collect_git_changes(workspace_dir)
    }

    /// Diff artifact for uncommitted work, then the host-side landing step:
    /// commits the worker made on taskrunner/<task_id> are fetched into the
    /// host repository under the same branch name for review.
    fn after_turn(&self, task_id: &str, turn_id: &str, workspace_dir: &Path, project_root: &Path) {
        if let Err(err) = capture_diff_artifact(
            workspace_dir,
            task_id,
            turn_id,
            &self.artifacts,
            self.recorder.as_ref(),
        ) {
            eprintln!("taskrunner: could not capture the workspace diff: {err}");
        }
        let tip = git(workspace_dir, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        if tip.is_empty() {
            return;
        }
        // Skip when the host already has the tip (no new commits in the clone).
        if git(project_root, &["cat-file", "-e", &tip]).ok {
            return;
        }
        let branch = format!("taskrunner/{task_id}");
        git(
            project_root,
            &[
                "fetch",
                &workspace_dir.to_string_lossy(),
                &format!("+refs/heads/{branch}:refs/heads/{branch}"),
            ],
        );
    }
}
