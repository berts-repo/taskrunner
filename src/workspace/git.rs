//! Git against the two kinds of repository taskrunner deals with: the ones it
//! controls (the project root, a clone no worker has touched yet), and the
//! ones a worker has had its hands on.
//!
//! The difference is a security boundary, not a style choice. A repository's
//! own `.git/config` can name programs for git to run — `diff.external`,
//! `core.fsmonitor`, `uploadpack.packObjectsHook`, filter drivers reached
//! through `.gitattributes` — and a turn can write that file inside its
//! workspace like any other. Running git there on the host would run those
//! programs as the user who runs taskrunner, outside the container, the
//! network firewall and every other boundary a turn has. So a worker's clone
//! is only ever read through a [`WorkspaceGit`], and the host side takes the
//! result as data.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::{HarnessKind, ResourceLimits};
use crate::process::{remove_labelled_containers, run};
use crate::workers::runner::resource_limit_args;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(120);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(60);

/// Marks inspection containers so a later daemon can reap the ones a crash
/// left behind, without ever touching a worker container.
pub const INSPECT_LABEL: &str = "taskrunner.role=workspace-inspect";

/// What [`INSPECT_SCRIPT`] says on stderr when the image it runs in has no git.
const NO_GIT: &str = "taskrunner-inspect: no git";

pub struct GitOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs git on the host, in a repository taskrunner controls. Never point
/// this at a workspace a worker has run in — use a [`WorkspaceGit`].
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

/// The branch a task's commits landed on in the user's repository, if they
/// did. Reads the user's own refs, never a worker's clone.
pub fn task_branch(project_root: &Path, task_id: &str) -> Option<String> {
    let branch = format!("taskrunner/{task_id}");
    let refname = format!("refs/heads/{branch}");
    git(project_root, &["rev-parse", "--verify", "--quiet", &refname]).ok.then_some(branch)
}

/// Paths with uncommitted changes, untracked files included, in the user's
/// repository: what a task's clone, taken from the last commit, won't have.
pub fn uncommitted_files(project_root: &Path) -> Vec<String> {
    let status = git(project_root, &["status", "--porcelain=v1", "--untracked-files=all"]);
    if !status.ok {
        return vec![];
    }
    status.stdout.lines().filter_map(|line| line.get(3..)).map(str::to_string).collect()
}

/// Everything the host needs to know about a finished turn, read out of the
/// workspace in one pass: which paths changed, the diff, the task branch's
/// tip, and a bundle of the commits the host does not have yet.
///
/// `$1` workspace, `$2` output directory, `$3` the commit the clone started
/// from (empty on a workspace made before bases were recorded), `$4` the task
/// branch. Every command is read-only except `add -N`, which only marks
/// untracked files in the clone's own disposable index so that files the turn
/// *created* show up in the diff rather than silently missing from the record.
/// A failure that would leave the record empty — `status`, `diff`, the task
/// branch — stops the script, so the host reports it rather than reading it as
/// a turn that changed nothing. The messages are fixed text: git's own stderr
/// here comes from a `.git` the worker controlled, so none of it is passed on.
pub const INSPECT_SCRIPT: &str = r#"
set -u
ws=$1; out=$2; base=$3; branch=$4
fail() { echo "taskrunner-inspect: $1" >&2; exit 4; }
cd "$ws" || fail "cannot enter the workspace"
command -v git >/dev/null 2>&1 || { echo "taskrunner-inspect: no git" >&2; exit 3; }
# safe.directory: the clone is owned by the host user, and this may run as
# another; it is command-line config, which a repository's own config cannot
# override.
g() { git -c safe.directory='*' --no-pager "$@"; }
g status --porcelain=v1 -z --untracked-files=all > "$out/status" 2>/dev/null || fail "git status failed"
g add -A -N >/dev/null 2>&1
# A repository with no commits yet has nothing to diff against and no branch tip.
if g rev-parse --verify -q HEAD >/dev/null 2>&1; then
  g diff --no-ext-diff --no-textconv HEAD > "$out/diff" 2>/dev/null || fail "git diff failed"
  g rev-parse --verify -q "refs/heads/$branch" > "$out/tip" 2>/dev/null || fail "the task branch $branch is gone"
fi
if [ -n "$base" ]; then
  g bundle create "$out/bundle" "refs/heads/$branch" --not "$base" >/dev/null 2>&1 || rm -f "$out/bundle"
else
  g bundle create "$out/bundle" "refs/heads/$branch" >/dev/null 2>&1 || rm -f "$out/bundle"
fi
exit 0
"#;

/// What an inspection runs with, taken from the worker whose turn it reads.
#[derive(Debug, Clone, Default)]
pub struct InspectWith {
    /// The worker's own image, tried before the built-in ones.
    pub image: Option<String>,
    /// The worker's ceilings. Whatever the turn planted in its clone runs
    /// during the inspection, so it gets no more room than the turn had.
    pub limits: ResourceLimits,
}

/// Reads a worker's workspace without letting its `.git` reach the host.
pub trait WorkspaceGit: Send + Sync {
    /// Runs [`INSPECT_SCRIPT`] over `workspace_dir`, leaving its output files
    /// in `out_dir`. `turn_id` names the run. An error means nothing could be
    /// read back, so the turn's diff and commits were not captured.
    fn inspect(
        &self,
        workspace_dir: &Path,
        out_dir: &Path,
        base: &str,
        branch: &str,
        turn_id: &str,
        with: &InspectWith,
    ) -> Result<(), String>;
}

/// Production: the inspection runs in a throwaway container over the mounted
/// clone. Anything the clone's config makes git execute runs in there — with
/// no network, no credentials, nothing of the host but the clone and the
/// output directory, and the worker's resource limits — and dies with the
/// container.
pub struct ContainerGit {
    docker: String,
    timeout: Duration,
}

impl ContainerGit {
    pub fn new(docker: &str) -> ContainerGit {
        ContainerGit { docker: docker.to_string(), timeout: INSPECT_TIMEOUT }
    }

    /// Test seam: a shorter limit than a real inspection gets.
    pub fn with_timeout(mut self, timeout: Duration) -> ContainerGit {
        self.timeout = timeout;
        self
    }

    pub fn container_name(turn_id: &str) -> String {
        format!("taskrunner-inspect-{turn_id}")
    }

    /// The images to try, in order: the worker's own, then the built-in ones
    /// (a custom image may not ship git). Only images that are built.
    fn images(&self, with: &InspectWith) -> Vec<String> {
        let mut images: Vec<String> = with.image.iter().cloned().collect();
        for kind in [HarnessKind::Codex, HarnessKind::Claude] {
            let builtin = kind.defaults().image.to_string();
            if !images.contains(&builtin) {
                images.push(builtin);
            }
        }
        images
            .into_iter()
            .filter(|image| run(&self.docker, &["image", "inspect", image], DOCKER_TIMEOUT).ok)
            .collect()
    }

    /// The container arguments, separated out so a test can assert the
    /// isolation without needing Docker.
    pub fn docker_args(
        workspace_dir: &Path,
        out_dir: &Path,
        image: &str,
        name: &str,
        limits: &ResourceLimits,
    ) -> Vec<String> {
        let mut args: Vec<String> = ["run", "--rm", "--name", name, "--label", INSPECT_LABEL]
            .iter()
            .chain(&["--network", "none"])
            .map(|s| s.to_string())
            .collect();
        args.extend(resource_limit_args(limits));
        args.extend(
            [
                "-v",
                &format!("{}:/workspace", workspace_dir.display()),
                "-v",
                &format!("{}:/out", out_dir.display()),
                "-w",
                "/workspace",
                "-e",
                "HOME=/tmp",
                "--entrypoint",
                "sh",
                image,
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        args
    }
}

impl WorkspaceGit for ContainerGit {
    fn inspect(
        &self,
        workspace_dir: &Path,
        out_dir: &Path,
        base: &str,
        branch: &str,
        turn_id: &str,
        with: &InspectWith,
    ) -> Result<(), String> {
        let images = self.images(with);
        if images.is_empty() {
            return Err(
                "no worker image is built, so the workspace cannot be inspected safely".to_string()
            );
        }
        let name = ContainerGit::container_name(turn_id);
        let mut without_git = vec![];
        for image in &images {
            let mut args =
                ContainerGit::docker_args(workspace_dir, out_dir, image, &name, &with.limits);
            args.extend(
                ["-c", INSPECT_SCRIPT, "sh", "/workspace", "/out", base, branch]
                    .iter()
                    .map(|s| s.to_string()),
            );
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = run(&self.docker, &argv, self.timeout);
            if output.ok {
                return Ok(());
            }
            // Killing a timed-out `docker run` leaves its container running,
            // so it goes by name. Harmless when `--rm` already removed it.
            run(&self.docker, &["rm", "-f", &name], DOCKER_TIMEOUT);
            if !output.stderr.contains(NO_GIT) {
                return Err(output.stderr.trim().to_string());
            }
            without_git.push(image.as_str());
        }
        Err(format!("no built image has git ({})", without_git.join(", ")))
    }
}

/// Removes inspection containers left behind by a daemon that was killed
/// mid-inspection. Called at startup, before any turn can have finished.
pub fn reap_inspection_containers(docker: &str) -> anyhow::Result<usize> {
    remove_labelled_containers(docker, INSPECT_LABEL, "inspection", DOCKER_TIMEOUT)
}

/// Runs the same script directly on the host. For tests over repositories the
/// test itself wrote: it gives a worker's `.git` the host, which is the whole
/// thing [`ContainerGit`] exists to prevent.
pub struct HostGit;

impl WorkspaceGit for HostGit {
    fn inspect(
        &self,
        workspace_dir: &Path,
        out_dir: &Path,
        base: &str,
        branch: &str,
        _turn_id: &str,
        _with: &InspectWith,
    ) -> Result<(), String> {
        let output = run(
            "sh",
            &[
                "-c",
                INSPECT_SCRIPT,
                "sh",
                &workspace_dir.to_string_lossy(),
                &out_dir.to_string_lossy(),
                base,
                branch,
            ],
            INSPECT_TIMEOUT,
        );
        if output.ok { Ok(()) } else { Err(output.stderr.trim().to_string()) }
    }
}

/// One inspection's results, already reduced to plain data.
#[derive(Default)]
pub struct Inspection {
    pub changed_files: Vec<String>,
    pub diff: Vec<u8>,
    /// The task branch's tip, only when it is a well-formed object id.
    pub tip: Option<String>,
    /// Whether a bundle of new commits was produced.
    pub has_bundle: bool,
}

/// Largest output files the host will read back. Generous for a real turn,
/// bounded so a turn cannot make the daemon read an unbounded file.
const MAX_STATUS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIFF_BYTES: u64 = 256 * 1024 * 1024;

impl Inspection {
    /// Reads what the script produced. Every file is worker-influenced data:
    /// oversized or non-regular files (a planted symlink) are ignored rather
    /// than followed.
    pub fn read(out_dir: &Path) -> Inspection {
        let status = read_capped(&out_dir.join("status"), MAX_STATUS_BYTES).unwrap_or_default();
        let tip = read_capped(&out_dir.join("tip"), 1024)
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .filter(|tip| is_object_id(tip));
        Inspection {
            changed_files: parse_status(&String::from_utf8_lossy(&status)),
            diff: read_capped(&out_dir.join("diff"), MAX_DIFF_BYTES).unwrap_or_default(),
            tip,
            has_bundle: is_regular_file(&out_dir.join("bundle")),
        }
    }
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|meta| meta.is_file()).unwrap_or(false)
}

fn read_capped(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    fs::read(path).ok()
}

/// A hex object id and nothing else — this string is handed to git on the
/// host, where a leading `-` would be read as an option.
fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// `status --porcelain=v1 -z`: NUL-terminated `XY <path>` records, where a
/// rename or copy is followed by a second record holding the *old* path.
fn parse_status(text: &str) -> Vec<String> {
    let mut changed = Vec::new();
    let mut records = text.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let (state, path) = record.split_at(3);
        if state.starts_with('R') || state.starts_with('C') {
            records.next(); // the old path; the new one is what changed
        }
        changed.push(path.to_string());
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_new_path_of_a_rename_and_skips_the_old_one() {
        let status = "RM b.txt\0a.txt\0 M src/lib.rs\0?? new.txt\0";
        assert_eq!(parse_status(status), vec!["b.txt", "src/lib.rs", "new.txt"]);
    }

    #[test]
    fn accepts_only_hex_object_ids() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0".repeat(64)));
        assert!(!is_object_id("--upload-pack=touch /tmp/x"));
        assert!(!is_object_id(""));
    }

    #[test]
    fn the_inspection_container_gets_no_network_and_only_the_two_mounts() {
        let args = ContainerGit::docker_args(
            Path::new("/state/workspaces/task_1"),
            Path::new("/state/workspaces/.inspect-turn_1"),
            "taskrunner/codex-worker",
            "taskrunner-inspect-turn_1",
            &ResourceLimits::default(),
        );
        assert!(args.windows(2).any(|w| w == ["--network", "none"]));
        assert!(args.contains(&"/state/workspaces/task_1:/workspace".to_string()));
        assert!(args.contains(&"/state/workspaces/.inspect-turn_1:/out".to_string()));
        // Nothing else of the host, and no worker credentials.
        assert_eq!(args.iter().filter(|arg| *arg == "-v").count(), 2);
        assert!(!args.iter().any(|arg| arg.contains("--mount")));
        // The worker's ceilings, and a name and label to find it by.
        assert!(args.windows(2).any(|w| w == ["--memory", "4g"]));
        assert!(args.windows(2).any(|w| w == ["--pids-limit", "512"]));
        assert!(args.windows(2).any(|w| w == ["--security-opt", "no-new-privileges"]));
        assert!(args.windows(2).any(|w| w == ["--name", "taskrunner-inspect-turn_1"]));
        assert!(args.windows(2).any(|w| w == ["--label", INSPECT_LABEL]));
    }

    /// A stand-in `docker` that logs every call beside itself and runs `body`
    /// for `docker run`.
    fn fake_docker(dir: &Path, run_body: &str) -> (String, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("docker");
        let log = dir.join("calls");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1\" in\n  run) {run_body} ;;\n  ps) echo stale1 ;;\nesac\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // Tests run on parallel threads, and a child forked by another thread
        // while this file was open for writing keeps it open until it execs;
        // executing it before then fails with "text file busy". Probe until
        // it runs, so the test proper never hits that window.
        for _ in 0..100 {
            if Command::new(&script).arg("probe").output().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_file(&log);
        (script.display().to_string(), log)
    }

    #[test]
    fn a_timed_out_inspection_removes_its_container_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let (docker, log) = fake_docker(dir.path(), "exec sleep 5");
        let git = ContainerGit::new(&docker).with_timeout(Duration::from_millis(300));
        let err = git
            .inspect(dir.path(), dir.path(), "", "b", "turn_t", &InspectWith::default())
            .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.contains("rm -f taskrunner-inspect-turn_t"), "{calls}");
    }

    #[test]
    fn falls_back_to_a_built_in_image_when_the_workers_has_no_git() {
        let dir = tempfile::tempdir().unwrap();
        let body = "case \"$*\" in *' custom/image -c'*) echo 'taskrunner-inspect: no git' >&2; exit 3 ;; esac";
        let (docker, log) = fake_docker(dir.path(), body);
        let with = InspectWith { image: Some("custom/image".into()), ..Default::default() };
        ContainerGit::new(&docker)
            .inspect(dir.path(), dir.path(), "", "b", "turn_f", &with)
            .unwrap();
        let calls = fs::read_to_string(log).unwrap();
        let custom = calls.find("sh custom/image -c").expect("tried the worker's image");
        let builtin = calls.find("sh taskrunner/codex-worker -c").expect("fell back to a built-in");
        assert!(custom < builtin);
    }

    #[test]
    fn reaps_only_containers_carrying_the_inspection_label() {
        let dir = tempfile::tempdir().unwrap();
        let (docker, log) = fake_docker(dir.path(), "exit 0");
        assert_eq!(reap_inspection_containers(&docker).unwrap(), 1);
        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.contains(&format!("ps -aq --filter label={INSPECT_LABEL}")), "{calls}");
        assert!(calls.contains("rm -f stale1"), "{calls}");
    }

    #[test]
    fn an_inspection_that_cannot_read_the_repository_fails_instead_of_reporting_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (ws, out) = (dir.path().join("ws"), dir.path().join("out"));
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&out).unwrap();
        let git_in = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&ws)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        let inspect = || {
            HostGit.inspect(&ws, &out, "", "taskrunner/task_1", "turn_1", &InspectWith::default())
        };
        git_in(&["init", "-q", "-b", "taskrunner/task_1"]);
        fs::write(ws.join("a.txt"), "a\n").unwrap();
        git_in(&["add", "a.txt"]);
        git_in(&["commit", "-qm", "first"]);
        inspect().unwrap();

        // The turn moved off its branch and deleted it: its commits can't land.
        git_in(&["checkout", "-qb", "elsewhere"]);
        git_in(&["branch", "-qD", "taskrunner/task_1"]);
        assert!(inspect().unwrap_err().contains("the task branch taskrunner/task_1 is gone"));

        // The turn broke the repository outright.
        fs::write(ws.join(".git/HEAD"), "garbage\n").unwrap();
        assert!(inspect().unwrap_err().contains("git status failed"));
    }
}
