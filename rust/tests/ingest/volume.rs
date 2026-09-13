use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use taskrunner::ingest::volume::{COPY_OUT_LABEL, reap_copy_out_containers};

/// A stand-in `docker` that appends its argv to a log and replays a scripted
/// stdout for `ps`. Exercises the real spawn/exit-code plumbing, which is
/// where the reap's failure handling lives.
struct FakeDocker {
    path: PathBuf,
    log: PathBuf,
    _root: tempfile::TempDir,
}

impl FakeDocker {
    fn new(ps_stdout: &str, exit_code: i32) -> FakeDocker {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("calls.log");
        fs::write(&log, "").unwrap();
        let path = root.path().join("docker");
        // Record argv one line per invocation, tab-separated.
        let script = format!(
            "#!/bin/sh\nprintf '%s\\t' \"$@\" >> {log}\nprintf '\\n' >> {log}\nif [ \"$1\" = \"ps\" ]; then printf '%s' '{ps_stdout}'; fi\nexit {exit_code}\n",
            log = log.display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        // Tests run on parallel threads, and a child forked by another thread
        // while this file was open for writing keeps it open until it execs;
        // executing it before then fails with "text file busy". Probe until
        // it runs, so the test proper never hits that window.
        for _ in 0..100 {
            if Command::new(&path).arg("probe").output().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        fs::write(&log, "").unwrap();
        FakeDocker { path, log, _root: root }
    }

    fn command(&self) -> &str {
        self.path.to_str().unwrap()
    }

    /// Argv of each invocation, in order.
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim_end_matches('\t').split('\t').map(str::to_string).collect())
            .collect()
    }
}

#[test]
fn removes_every_container_carrying_the_copy_out_label() {
    let docker = FakeDocker::new("abc123\ndef456\n", 0);
    assert_eq!(reap_copy_out_containers(docker.command()).unwrap(), 2);
    let calls = docker.calls();
    assert_eq!(calls[0], vec!["ps", "-aq", "--filter", &format!("label={COPY_OUT_LABEL}")]);
    assert_eq!(calls[1], vec!["rm", "-f", "abc123", "def456"]);
}

// The label filter is the whole safety story: without it the reap would be
// an unfiltered `docker rm -f` against every container on the host.
#[test]
fn never_issues_an_rm_when_nothing_carries_the_label() {
    let docker = FakeDocker::new("", 0);
    assert_eq!(reap_copy_out_containers(docker.command()).unwrap(), 0);
    assert_eq!(docker.calls().iter().map(|c| c[0].as_str()).collect::<Vec<_>>(), vec!["ps"]);
}

#[test]
fn ignores_blank_lines_rather_than_passing_an_empty_id_to_rm() {
    let docker = FakeDocker::new("\n\n", 0);
    assert_eq!(reap_copy_out_containers(docker.command()).unwrap(), 0);
    assert_eq!(docker.calls().iter().map(|c| c[0].as_str()).collect::<Vec<_>>(), vec!["ps"]);
}

#[test]
fn fails_when_docker_fails_so_the_caller_can_log_and_carry_on() {
    let docker = FakeDocker::new("", 1);
    let err = reap_copy_out_containers(docker.command()).unwrap_err();
    assert!(err.to_string().contains("docker ps failed"));
}

#[test]
fn fails_when_docker_is_missing_entirely() {
    let err = reap_copy_out_containers("/nonexistent/docker").unwrap_err();
    assert!(err.to_string().contains("docker ps failed"));
}
