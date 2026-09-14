//! Running a child process to completion with a timeout, capturing both pipes.
//! Shared by the callers that shell out to `docker` and `git` from blocking
//! code: a hung child must never become a hung daemon.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    /// stdout as text, for the commands whose output is known to be text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Runs a command to completion. A missing binary or a timeout reports as a
/// failure with the reason in `stderr`, never a panic.
pub fn run(command: &str, args: &[&str], timeout: Duration) -> Output {
    let failed = |reason: String| Output { ok: false, stdout: Vec::new(), stderr: reason };
    let spawned = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => return failed(err.to_string()),
    };
    // Drain both pipes on their own threads so a chatty child can never fill
    // one and block before it exits.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            return failed(format!("{command} timed out after {}s", timeout.as_secs()));
        }
        Err(err) => return failed(err.to_string()),
    };
    Output {
        ok: status.success(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned(),
    }
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

use wait_timeout::ChildExt;
