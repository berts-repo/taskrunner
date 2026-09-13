//! Clone workspace tests.

#[path = "../helpers/mod.rs"]
mod helpers;

use std::path::Path;
use std::sync::{Arc, Mutex};

use taskrunner::storage::Recorder;
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::{EventBody, LogEvent};
use taskrunner::workspace::clone::{CloneWorkspaces, WorkspaceProvider};

use crate::helpers::init_git_repo;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Keeps every recorded body, for assertions.
#[derive(Default)]
struct Recorded(Mutex<Vec<EventBody>>);

impl Recorder for Recorded {
    fn record(&self, body: EventBody) -> anyhow::Result<LogEvent> {
        self.0.lock().unwrap().push(body.clone());
        Ok(LogEvent { id: "evt_x".into(), ts: "2026-01-01T00:00:00.000Z".into(), body })
    }
}

struct Provider {
    workspaces: CloneWorkspaces,
    recorded: Arc<Recorded>,
    _root: tempfile::TempDir,
}

fn make_provider() -> Provider {
    let root = tempfile::tempdir().unwrap();
    let recorded = Arc::new(Recorded::default());
    let artifacts = Arc::new(ArtifactStore::new(&root.path().join("artifacts")));
    let workspaces =
        CloneWorkspaces::new(&root.path().join("workspaces"), artifacts, recorded.clone());
    Provider { workspaces, recorded, _root: root }
}

#[test]
fn creates_a_self_contained_clone_on_a_task_branch_with_no_remotes() {
    let repo = init_git_repo();
    let p = make_provider();
    let dir = p.workspaces.ensure_workspace("task_c1", repo.path()).unwrap();

    // A real .git directory (self-contained), not a worktree pointer file.
    assert!(dir.join(".git/HEAD").exists());
    assert_eq!(git_in(&dir, &["branch", "--show-current"]), "taskrunner/task_c1");
    assert_eq!(git_in(&dir, &["remote"]), "");
    // Idempotent: a second call reuses the same workspace.
    assert_eq!(p.workspaces.ensure_workspace("task_c1", repo.path()).unwrap(), dir);
}

#[test]
fn rejects_projects_that_are_not_git_repositories() {
    let plain = tempfile::tempdir().unwrap();
    let p = make_provider();
    let err = p.workspaces.ensure_workspace("task_c2", plain.path()).unwrap_err();
    assert_eq!(err.code, taskrunner::domain::errors::ErrorCode::InvalidRequest);
}

#[test]
fn collects_uncommitted_changes_and_captures_a_diff_artifact() {
    let repo = init_git_repo();
    let p = make_provider();
    let dir = p.workspaces.ensure_workspace("task_c3", repo.path()).unwrap();

    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    assert_eq!(p.workspaces.collect_changes(&dir), vec!["README.md"]);

    p.workspaces.after_turn("task_c3", "turn_c3", &dir, repo.path());
    let recorded = p.recorded.0.lock().unwrap();
    let stored = recorded.iter().find_map(|e| match e {
        EventBody::ArtifactStored { kind, .. } => Some(kind.clone()),
        _ => None,
    });
    assert_eq!(stored.as_deref(), Some("diff"));
    assert!(recorded.iter().any(|e| matches!(e, EventBody::ArtifactLinked { .. })));
}

#[test]
fn lands_committed_work_on_the_host_repo_under_the_task_branch() {
    let repo = init_git_repo();
    let p = make_provider();
    let dir = p.workspaces.ensure_workspace("task_c4", repo.path()).unwrap();

    std::fs::write(dir.join("new.txt"), "worker output\n").unwrap();
    git_in(&dir, &["add", "."]);
    git_in(&dir, &["commit", "-qm", "worker commit"]);
    let tip = git_in(&dir, &["rev-parse", "HEAD"]);

    p.workspaces.after_turn("task_c4", "turn_c4", &dir, repo.path());
    assert_eq!(git_in(repo.path(), &["rev-parse", "taskrunner/task_c4"]), tip);
    // The host branch is a real local branch; the working tree is untouched.
    assert!(!repo.path().join("new.txt").exists());

    // Re-running after_turn without new commits is a no-op.
    p.workspaces.after_turn("task_c4", "turn_c4b", &dir, repo.path());
    assert_eq!(git_in(repo.path(), &["rev-parse", "taskrunner/task_c4"]), tip);
}
