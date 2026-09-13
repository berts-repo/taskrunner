//! Thin stdio shim: the `mcp` command. Ensures a daemon is serving, then pumps
//! the client's stdin to the daemon's MCP socket and the socket back to
//! stdout, byte for byte. It never interprets messages — the daemon speaks
//! the same newline-delimited JSON-RPC the client does.

use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::net::UnixStream;

use crate::client;
use crate::paths::StatePaths;

async fn daemon_is_up(paths: &StatePaths) -> bool {
    client::get(&paths.socket_path, "/status", Duration::from_secs(1)).await.is_ok_and(|r| r.ok())
}

/// Ensures a daemon is serving on the socket, spawning `taskrunner up`
/// detached if needed. Losing an auto-start race is fine: the loser's `up`
/// exits on the lock, and both shims connect to the winner.
async fn ensure_daemon(paths: &StatePaths) -> anyhow::Result<()> {
    if daemon_is_up(paths).await {
        return Ok(());
    }
    fs::create_dir_all(&paths.logs_dir)?;
    let log =
        fs::OpenOptions::new().create(true).append(true).open(paths.logs_dir.join("daemon.log"))?;
    let exe = std::env::current_exe().context("cannot determine taskrunner executable")?;
    Command::new(exe)
        .args(["up", "--state-root"])
        .arg(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .process_group(0) // outlives this shim
        .spawn()
        .context("spawning the daemon")?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut delay = Duration::from_millis(50);
    while Instant::now() < deadline {
        if daemon_is_up(paths).await {
            return Ok(());
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_millis(500));
    }
    anyhow::bail!("taskrunner daemon did not become ready on {}", paths.socket_path.display())
}

/// Runs until the client hangs up (exit 0) or the daemon connection fails (exit 1).
pub async fn run_shim(paths: &StatePaths) -> anyhow::Result<i32> {
    ensure_daemon(paths).await?;
    let stream =
        UnixStream::connect(&paths.mcp_socket_path).await.context("connecting to the daemon")?;
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        // Client hung up (stdin closed): dropping the socket ends the MCP
        // session at the daemon too.
        _ = tokio::io::copy(&mut stdin, &mut to_daemon) => Ok(0),
        copied = tokio::io::copy(&mut from_daemon, &mut stdout) => match copied {
            Ok(_) => Ok(0),
            Err(err) => {
                eprintln!("taskrunner mcp: daemon connection error: {err}");
                Ok(1)
            }
        },
    }
}
