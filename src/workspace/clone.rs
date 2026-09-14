//! Task-local clone workspaces for Docker workers: a worktree's `.git` file
//! points back into the main repository, so mounting one into a container
//! either breaks git or exposes the host `.git`. A clone is fully
//! self-contained; the container mounts only the clone directory.
//!
//! `--no-hardlinks` matters: a default local clone hardlinks object files,
//! which share inodes with the host repository — a worker writing through
//! one could corrupt host history.
//!
//! Once a turn has run, the clone is a repository a worker controlled, so
//! nothing here reads it with host git: the inspection runs through a
//! [`WorkspaceGit`], and its commits land on the host from a bundle, which
//! git only ever reads as data (see [`super::git`]).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::git::{Inspection, WorkspaceGit, git};
use crate::domain::errors::{ErrorCode, ToolError};
use crate::ids::{IdPrefix, new_id};
use crate::storage::Recorder;
use crate::storage::artifacts::ArtifactStore;
use crate::storage::events::EventBody;

/// Provides the per-task isolated workspace.
pub trait WorkspaceProvider: Send + Sync {
    fn ensure_workspace(&self, task_id: &str, project_root: &Path) -> Result<PathBuf, ToolError>;
    /// Post-turn: records the diff artifact, lands committed work on the host
    /// repository, and reports the paths git saw change — which is the whole
    /// truth about the workspace, whatever the worker said it touched.
    fn after_turn(
        &self,
        task_id: &str,
        turn_id: &str,
        workspace_dir: &Path,
        project_root: &Path,
    ) -> Vec<String>;
}

pub struct CloneWorkspaces {
    workspaces_dir: PathBuf,
    artifacts: Arc<ArtifactStore>,
    recorder: Arc<dyn Recorder>,
    git: Arc<dyn WorkspaceGit>,
}

impl CloneWorkspaces {
    pub fn new(
        workspaces_dir: &Path,
        artifacts: Arc<ArtifactStore>,
        recorder: Arc<dyn Recorder>,
        git: Arc<dyn WorkspaceGit>,
    ) -> CloneWorkspaces {
        CloneWorkspaces { workspaces_dir: workspaces_dir.to_path_buf(), artifacts, recorder, git }
    }

    /// The commit a task's clone started from, kept beside the clone rather
    /// than inside it: the host decides which commits are new, so a worker
    /// must not be able to edit the answer.
    fn base_file(&self, task_id: &str) -> PathBuf {
        self.workspaces_dir.join(format!("{task_id}.base"))
    }

    fn store_diff(&self, task_id: &str, turn_id: &str, diff: &[u8]) -> anyhow::Result<()> {
        let stored = self.artifacts.store(diff)?;
        let artifact_id = new_id(IdPrefix::Artifact);
        self.recorder.record(EventBody::ArtifactStored {
            artifact_id: artifact_id.clone(),
            kind: "diff".into(),
            label: "workspace diff after turn".into(),
            media_type: "text/x-diff".into(),
            size_bytes: stored.size_bytes,
            sha256: stored.sha256,
            locator: stored.locator,
        })?;
        self.recorder.record(EventBody::ArtifactLinked {
            artifact_id,
            session_id: None,
            task_id: Some(task_id.into()),
            turn_id: Some(turn_id.into()),
            audit_event_id: None,
        })?;
        Ok(())
    }

    /// Fetches the turn's commits from the bundle the inspection produced.
    /// The bundle is a file of objects, not a repository: git reads it
    /// without consulting any config the worker could have written, and
    /// `transfer.fsckObjects` rejects malformed objects at the door.
    fn land_bundle(&self, task_id: &str, project_root: &Path, out_dir: &Path, tip: &str) {
        // Already on the host: the turn added no commits of its own.
        if git(project_root, &["cat-file", "-e", &format!("{tip}^{{commit}}")]).ok {
            return;
        }
        let branch = format!("taskrunner/{task_id}");
        let bundle = out_dir.join("bundle");
        let landed = git(
            project_root,
            &[
                "-c",
                "transfer.fsckObjects=true",
                "fetch",
                "--no-tags",
                &bundle.to_string_lossy(),
                &format!("+refs/heads/{branch}:refs/heads/{branch}"),
            ],
        );
        if !landed.ok {
            eprintln!(
                "taskrunner: could not land {branch} on the host repository: {}",
                landed.stderr.trim()
            );
        }
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
        // Read before the worker has ever run here, so this is still a
        // repository taskrunner controls.
        let base = git(&dir, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        if !base.is_empty() {
            let _ = fs::write(self.base_file(task_id), format!("{base}\n"));
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
        // discarded once the turn's changes land host-side. `o+rwX` (not
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

    fn after_turn(
        &self,
        task_id: &str,
        turn_id: &str,
        workspace_dir: &Path,
        project_root: &Path,
    ) -> Vec<String> {
        let out_dir = self.workspaces_dir.join(format!(".inspect-{turn_id}"));
        let _ = fs::remove_dir_all(&out_dir);
        if let Err(err) = fs::create_dir_all(&out_dir) {
            eprintln!("taskrunner: could not prepare the workspace inspection: {err}");
            return vec![];
        }
        // The inspection may run as another user (the container's), so it has
        // to be able to write its results here.
        let _ = fs::set_permissions(&out_dir, fs::Permissions::from_mode(0o777));

        let base = fs::read_to_string(self.base_file(task_id)).unwrap_or_default();
        let branch = format!("taskrunner/{task_id}");
        if let Err(err) = self.git.inspect(workspace_dir, &out_dir, base.trim(), &branch) {
            eprintln!("taskrunner: could not inspect the task workspace: {err}");
        }

        let inspection = Inspection::read(&out_dir);
        if !inspection.diff.is_empty()
            && let Err(err) = self.store_diff(task_id, turn_id, &inspection.diff)
        {
            eprintln!("taskrunner: could not capture the workspace diff: {err}");
        }
        if let (Some(tip), true) = (&inspection.tip, inspection.has_bundle) {
            self.land_bundle(task_id, project_root, &out_dir, tip);
        }
        let _ = fs::remove_dir_all(&out_dir);
        inspection.changed_files
    }
}
