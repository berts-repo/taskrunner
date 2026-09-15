//! Clone workspace tests.

#[path = "../helpers/mod.rs"]
mod helpers;

use std::path::Path;
use std::sync::{Arc, Mutex};

use taskrunner::storage::Recorder;
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::events::{EventBody, LogEvent};
use taskrunner::workspace::clone::{CloneWorkspaces, WorkspaceProvider};
use taskrunner::workspace::git::{ContainerGit, HostGit, InspectWith, WorkspaceGit};

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
    root: tempfile::TempDir,
}

/// The inspection runs on the host here: these tests own the repository they
/// point it at. The one test that hands git a repository written against it
/// uses the real [`ContainerGit`] instead.
fn make_provider() -> Provider {
    provider_with(Arc::new(HostGit))
}

fn provider_with(git: Arc<dyn WorkspaceGit>) -> Provider {
    let root = tempfile::tempdir().unwrap();
    let recorded = Arc::new(Recorded::default());
    let artifacts = Arc::new(ArtifactStore::new(&root.path().join("artifacts")));
    let workspaces =
        CloneWorkspaces::new(&root.path().join("workspaces"), artifacts, recorded.clone(), git);
    Provider { workspaces, recorded, root }
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
fn reports_edited_and_created_files_and_captures_a_diff_artifact() {
    let repo = init_git_repo();
    let p = make_provider();
    let dir = p.workspaces.ensure_workspace("task_c3", repo.path()).unwrap();

    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    // A file the turn created through the shell: untracked, and just as much
    // part of what the turn did.
    std::fs::write(dir.join("created.txt"), "brand new\n").unwrap();

    let changed = p
        .workspaces
        .after_turn("task_c3", "turn_c3", &dir, repo.path(), &InspectWith::default())
        .changed_files;
    assert_eq!(changed, vec!["README.md", "created.txt"]);

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

    p.workspaces.after_turn("task_c4", "turn_c4", &dir, repo.path(), &InspectWith::default());
    assert_eq!(git_in(repo.path(), &["rev-parse", "taskrunner/task_c4"]), tip);
    // The host branch is a real local branch; the working tree is untouched.
    assert!(!repo.path().join("new.txt").exists());

    // Re-running after_turn without new commits is a no-op.
    p.workspaces.after_turn("task_c4", "turn_c4b", &dir, repo.path(), &InspectWith::default());
    assert_eq!(git_in(repo.path(), &["rev-parse", "taskrunner/task_c4"]), tip);
}

#[test]
fn leaves_no_inspection_directory_behind() {
    let repo = init_git_repo();
    let p = make_provider();
    let dir = p.workspaces.ensure_workspace("task_c6", repo.path()).unwrap();
    p.workspaces.after_turn("task_c6", "turn_c6", &dir, repo.path(), &InspectWith::default());

    let leftovers: Vec<_> = std::fs::read_dir(p.root.path().join("workspaces"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".inspect-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// The security property, against the real container inspection: a turn can
/// write its clone's `.git/config`, and git config can name programs for git
/// to run. Whatever it plants must fire inside the container, if at all —
/// never on the host, where it would run as the user with no boundary left.
///
/// Gated: needs Docker and a built worker image (`sh scripts/build-images.sh`).
#[test]
fn never_runs_commands_planted_in_the_clones_git_config_on_the_host() {
    if std::env::var("TASKRUNNER_LIVE_DOCKER").as_deref() != Ok("1") {
        eprintln!("skipped: TASKRUNNER_LIVE_DOCKER is not set");
        return;
    }
    let repo = init_git_repo();
    let p = provider_with(Arc::new(ContainerGit::new("docker")));
    let dir = p.workspaces.ensure_workspace("task_c5", repo.path()).unwrap();

    let marker = p.root.path().join("escaped");
    let plant = format!("touch {}", marker.display());
    // Two of git's several config keys that name a command to run: one fires
    // on `git diff`, the other on `git status`.
    git_in(&dir, &["config", "diff.external", &plant]);
    git_in(&dir, &["config", "core.fsmonitor", &plant]);
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();

    let changed = p
        .workspaces
        .after_turn("task_c5", "turn_c5", &dir, repo.path(), &InspectWith::default())
        .changed_files;

    assert!(!marker.exists(), "a command planted in the clone's git config ran on the host");
    // And the inspection still did its job inside the container.
    assert!(changed.contains(&"README.md".to_string()), "{changed:?}");
}
