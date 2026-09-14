//! One daemon per state root, enforced by a lock file that carries its pid.

use std::fs;
use std::io;

use nix::sys::signal::kill;
use nix::unistd::Pid;

use crate::paths::StatePaths;

#[derive(Debug)]
pub struct AlreadyRunning {
    pub pid: i32,
}

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "taskrunner daemon already running (pid {})", self.pid)
    }
}

impl std::error::Error for AlreadyRunning {}

pub fn acquire(paths: &StatePaths) -> anyhow::Result<()> {
    let pid = std::process::id();
    for _attempt in 0..2 {
        // link() publishes the lock atomically WITH its pid content, so a
        // racing daemon can never observe an empty lock file and misjudge it
        // stale.
        let tmp = paths.lock_file.with_extension(format!("{pid}.tmp"));
        fs::write(&tmp, pid.to_string())?;
        let linked = fs::hard_link(&tmp, &paths.lock_file);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => {
                fs::write(&paths.pid_file, pid.to_string())?;
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
        let Some(holder) = read_pid(&paths.lock_file) else {
            continue; // lock vanished between link() and read; retry
        };
        if is_process_alive(holder) {
            return Err(AlreadyRunning { pid: holder }.into());
        }
        // Stale lock from a crashed daemon; clear it and retry once.
        let _ = fs::remove_file(&paths.lock_file);
        let _ = fs::remove_file(&paths.pid_file);
    }
    anyhow::bail!("could not acquire daemon lock")
}

/// Removes lock/pid files only if this process still owns them.
pub fn release(paths: &StatePaths) {
    let own = std::process::id() as i32;
    for file in [&paths.lock_file, &paths.pid_file] {
        if read_pid(file) == Some(own) {
            let _ = fs::remove_file(file);
        }
    }
}

pub fn read_pid(file: &std::path::Path) -> Option<i32> {
    fs::read_to_string(file).ok()?.trim().parse().ok().filter(|pid| *pid > 0)
}

pub fn is_process_alive(pid: i32) -> bool {
    match kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}
