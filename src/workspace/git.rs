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
use std::sync::Mutex;
use std::time::Duration;

use crate::config::HarnessKind;
use crate::harnesses::default_image;
use crate::process::run;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(120);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(60);

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

/// Everything the host needs to know about a finished turn, read out of the
/// workspace in one pass: which paths changed, the diff, the task branch's
/// tip, and a bundle of the commits the host does not have yet.
///
/// `$1` workspace, `$2` output directory, `$3` the commit the clone started
/// from (empty on a workspace made before bases were recorded), `$4` the task
/// branch. Every command is read-only except `add -N`, which only marks
/// untracked files in the clone's own disposable index so that files the turn
/// *created* show up in the diff rather than silently missing from the record.
/// Failures are left to the host to notice as a missing or empty file: this
/// runs where a hostile `.git` can make git do odd things, so it reports
/// nothing it would have to be trusted about.
pub const INSPECT_SCRIPT: &str = r#"
set -u
ws=$1; out=$2; base=$3; branch=$4
cd "$ws" || exit 1
# safe.directory: the clone is owned by the host user, and this may run as
# another; it is command-line config, which a repository's own config cannot
# override.
g() { git -c safe.directory='*' --no-pager "$@"; }
g status --porcelain=v1 -z --untracked-files=all > "$out/status" 2>/dev/null
g add -A -N >/dev/null 2>&1
g diff --no-ext-diff --no-textconv HEAD > "$out/diff" 2>/dev/null
g rev-parse --verify -q "refs/heads/$branch" > "$out/tip" 2>/dev/null
if [ -n "$base" ]; then
  g bundle create "$out/bundle" "refs/heads/$branch" --not "$base" >/dev/null 2>&1 || rm -f "$out/bundle"
else
  g bundle create "$out/bundle" "refs/heads/$branch" >/dev/null 2>&1 || rm -f "$out/bundle"
fi
exit 0
"#;

/// Reads a worker's workspace without letting its `.git` reach the host.
pub trait WorkspaceGit: Send + Sync {
    /// Runs [`INSPECT_SCRIPT`] over `workspace_dir`, leaving its output files
    /// in `out_dir`. Errors are reported for logging only — the host treats a
    /// missing output file as "nothing to report" either way.
    fn inspect(
        &self,
        workspace_dir: &Path,
        out_dir: &Path,
        base: &str,
        branch: &str,
    ) -> Result<(), String>;
}

/// Production: the inspection runs in a throwaway container over the mounted
/// clone. Anything the clone's config makes git execute runs in there — with
/// no network, no credentials, and nothing of the host but the clone and the
/// output directory — and dies with the container.
pub struct ContainerGit {
    docker: String,
    image: Mutex<Option<String>>,
}

impl ContainerGit {
    pub fn new(docker: &str) -> ContainerGit {
        ContainerGit { docker: docker.to_string(), image: Mutex::new(None) }
    }

    /// Any built worker image will do — they all ship git — so this needs no
    /// configuration of its own and works whichever workers exist. Resolved
    /// once per daemon.
    fn image(&self) -> Option<String> {
        let mut cached = self.image.lock().unwrap_or_else(|p| p.into_inner());
        if cached.is_none() {
            for candidate in [default_image(HarnessKind::Codex), default_image(HarnessKind::Claude)]
            {
                if run(&self.docker, &["image", "inspect", candidate], DOCKER_TIMEOUT).ok {
                    *cached = Some(candidate.to_string());
                    break;
                }
            }
        }
        cached.clone()
    }

    /// The container arguments, separated out so a test can assert the
    /// isolation without needing Docker.
    pub fn docker_args(workspace_dir: &Path, out_dir: &Path, image: &str) -> Vec<String> {
        [
            "run",
            "--rm",
            "--network",
            "none",
            "--security-opt",
            "no-new-privileges",
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
        .map(|s| s.to_string())
        .collect()
    }
}

impl WorkspaceGit for ContainerGit {
    fn inspect(
        &self,
        workspace_dir: &Path,
        out_dir: &Path,
        base: &str,
        branch: &str,
    ) -> Result<(), String> {
        let image = self.image().ok_or_else(|| {
            "no worker image is built, so the workspace cannot be inspected safely".to_string()
        })?;
        let mut args = ContainerGit::docker_args(workspace_dir, out_dir, &image);
        args.extend(
            ["-c", INSPECT_SCRIPT, "sh", "/workspace", "/out", base, branch]
                .iter()
                .map(|s| s.to_string()),
        );
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = run(&self.docker, &argv, INSPECT_TIMEOUT);
        if output.ok { Ok(()) } else { Err(output.stderr.trim().to_string()) }
    }
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
        );
        assert!(args.windows(2).any(|w| w == ["--network", "none"]));
        assert!(args.contains(&"/state/workspaces/task_1:/workspace".to_string()));
        assert!(args.contains(&"/state/workspaces/.inspect-turn_1:/out".to_string()));
        // Nothing else of the host, and no worker credentials.
        assert_eq!(args.iter().filter(|arg| *arg == "-v").count(), 2);
        assert!(!args.iter().any(|arg| arg.contains("--mount")));
    }
}
