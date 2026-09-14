use std::num::NonZeroU64;

use taskrunner::config::ResourceLimits;
use taskrunner::harnesses::AuthMount;
use taskrunner::workers::runner::{auth_mount_args, resource_limit_args};

fn mount(
    container_path: &'static str,
    subpath: Option<&'static str>,
    read_only: bool,
) -> AuthMount {
    AuthMount { container_path, subpath, read_only }
}

#[test]
fn mounts_the_volume_root_when_no_subpath_is_given() {
    assert_eq!(
        auth_mount_args("taskrunner-codex-home", &[mount("/home/worker/.codex", None, false)]),
        vec!["--mount", "type=volume,src=taskrunner-codex-home,dst=/home/worker/.codex"]
    );
}

#[test]
fn mounts_only_the_named_subpaths_of_the_volume() {
    let mounts = [
        mount("/home/worker/.claude", Some(".claude"), false),
        mount("/home/worker/.claude.json", Some(".claude.json"), false),
    ];
    assert_eq!(
        auth_mount_args("taskrunner-claude-home", &mounts),
        vec![
            "--mount",
            "type=volume,src=taskrunner-claude-home,dst=/home/worker/.claude,volume-subpath=.claude",
            "--mount",
            "type=volume,src=taskrunner-claude-home,dst=/home/worker/.claude.json,volume-subpath=.claude.json",
        ]
    );
}

#[test]
fn marks_read_only_mounts() {
    assert_eq!(
        auth_mount_args("vol", &[mount("/x", Some("y"), true)]),
        vec!["--mount", "type=volume,src=vol,dst=/x,volume-subpath=y,readonly"]
    );
}

fn limits(memory: &str, cpus: f64, pids: u64) -> ResourceLimits {
    ResourceLimits { memory: memory.into(), cpus, pids: NonZeroU64::new(pids).unwrap() }
}

#[test]
fn emits_the_configured_ceilings_as_docker_flags() {
    assert_eq!(
        resource_limit_args(&limits("4g", 2.0, 512)),
        vec![
            "--memory",
            "4g",
            "--cpus",
            "2",
            "--pids-limit",
            "512",
            "--security-opt",
            "no-new-privileges"
        ]
    );
}

#[test]
fn passes_through_custom_and_fractional_values() {
    assert_eq!(
        resource_limit_args(&limits("512m", 1.5, 128)),
        vec![
            "--memory",
            "512m",
            "--cpus",
            "1.5",
            "--pids-limit",
            "128",
            "--security-opt",
            "no-new-privileges"
        ]
    );
}

#[test]
fn always_hardens_with_no_new_privileges_regardless_of_the_limits() {
    assert!(
        resource_limit_args(&limits("8g", 4.0, 1024)).contains(&"no-new-privileges".to_string())
    );
}
