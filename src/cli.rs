//! The `taskrunner` command line. Usage text and error messages are fixed
//! output that a parser library would reword, so arguments are parsed by hand:
//! a command, positionals, and `--key value` flags anywhere.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::client;
use crate::config::HostKind;
use crate::daemon::mcp::VERSION;
use crate::daemon::{AlreadyRunning, Daemon, DaemonOptions};
use crate::doctor::run_doctor;
use crate::paths::{StatePaths, default_root, state_paths};
use crate::shim::run_shim;
use crate::storage::chain::{self, Claim};
use crate::sync::{SyncOptions, run_sync};

pub const USAGE: &str = "Usage: taskrunner <command> [args] [--state-root <dir>]

Commands:
  up        Start the Taskrunner daemon in the foreground.
  down      Stop the running daemon.
  status    Report daemon status.
  doctor    Diagnose Docker, worker images/auth, harness setup, and ingestion.
  sync [--connect claude,codex] [--skip hermes] [--delegation suggest|on-request]
            Connect the agent harnesses on this machine to taskrunner and give
            them its skills. Asks once about each new harness; the flags
            answer instead. Safe to run again.
  mcp [--host claude|codex|hermes]
            Run the stdio MCP shim (auto-starts the daemon). --host names the
            harness it serves.
  wait <task-id> [--timeout S]
            Wait until a delegated task's running turn ends, then print a short
            result. Exit 0 completed, 1 failed or canceled, 2 still running
            when --timeout (seconds) ran out.
  verify [--anchor \"<saved line>\"]
            Check that the event log has not changed: its chain, every
            automatic anchor, and an anchor you saved. Exit 0 verified, 1 not.
  anchor    Print the log's current fingerprint, to keep off this machine and
            check later with verify --anchor.

Query (read the ingested corpus without an MCP session):
  sessions [--project P] [--limit N]
              List ingested transcript sessions, most recent first.
  session <id> [--source S] [--last N] [--prompt N]
               [--view timeline|outline|compact] [--tool-lines N]
              Print one session (host or worker). Defaults to the timeline:
              prompts, replies and reasoning in full, tool output capped at 20
              lines (--tool-lines 0 for all). --prompt N prints one exchange.
              --view outline is the scannable index, one line per prompt and
              tool call; --view compact is one truncated line per message.
              Pipe to less.
  search [\"<fts>\"] [--tool T] [--target S] [--failed true|false]
                   [--project P] [--sessions a,b] [--last-sessions N]
                   [--role R] [--kind K] [--since T] [--until T]
                   [--sort rank|recent] [--limit N]
              Search transcripts by text, by what a tool call did, or both.
              --target matches the path or command a call acted on, e.g.
              --tool Edit --target src/shim/proxy.ts. Every hit prints the
              prompt index to drill into with session <id> --prompt N.
  task <id> [--include turns,trace,audit,artifacts,diff,transcript]
            [--turn <turnId>] [--last N] [--prompt N]
            [--view timeline|outline|compact] [--tool-lines N]
              Look up one task; tasks --project P lists a project's tasks.
              --include transcript prints the worker's interior as a timeline,
              with the same rendering flags as session.
";

pub struct Args {
    pub command: Option<String>,
    /// Positional arguments after the command (e.g. session <id>).
    pub rest: Vec<String>,
    /// `--key value` options after the command.
    pub flags: BTreeMap<String, String>,
    pub paths: StatePaths,
}

impl Args {
    fn flag(&self, name: &str) -> Option<String> {
        self.flags.get(name).cloned()
    }

    /// Transcript rendering params for the query routes. The terminal defaults
    /// to the timeline — an audit view is what a person at a shell wants, and
    /// a person can page and grep — while the routes themselves default to
    /// the outline, which is what an agent paying per token needs. Always sent
    /// explicitly, so the two defaults never have to agree.
    fn render_flags(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("view", Some(self.flag("view").unwrap_or_else(|| "timeline".into()))),
            ("toolLines", self.flag("tool-lines")),
            ("prompt", self.flag("prompt")),
        ]
    }
}

/// Writes command output to stdout. A reader that stops early (`| head`)
/// closes the pipe: that is its choice, not our failure, and `print!` would
/// panic on it — so exactly that error is ignored and the command keeps its
/// exit code.
pub fn say(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()) {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// Fetches a read-only query route from the daemon over the control socket
/// and prints the plain-text body. Mirrors `status`: a down daemon is a soft
/// failure.
async fn read_query(
    paths: &StatePaths,
    path: &str,
    params: &[(&str, Option<String>)],
) -> Result<i32, anyhow::Error> {
    let query: Vec<String> = params
        .iter()
        .filter_map(|(key, value)| {
            value.as_deref().filter(|v| !v.is_empty()).map(|v| format!("{key}={}", url_encode(v)))
        })
        .collect();
    match client::get(
        &paths.socket_path,
        &format!("{path}?{}", query.join("&")),
        Duration::from_secs(30),
    )
    .await
    {
        Ok(res) => {
            let body =
                if res.body.ends_with('\n') { res.body.clone() } else { format!("{}\n", res.body) };
            say(&body)?;
            Ok(if res.ok() { 0 } else { 1 })
        }
        Err(_) => {
            say("taskrunner daemon is not running\n")?;
            Ok(1)
        }
    }
}

/// Reads the files directly rather than asking the daemon, so the record can
/// be checked with the daemon stopped.
fn verify_log(paths: &StatePaths, saved: Option<String>) -> anyhow::Result<i32> {
    let claim = match saved {
        None => None,
        Some(text) => Some(
            Claim::parse(&text)
                .context("--anchor needs a fingerprint like sha256:<64 hex digits>")?,
        ),
    };
    let report = chain::verify(&paths.events_log, claim.as_ref())?;
    say(&report.render())?;
    Ok(if report.ok() { 0 } else { 1 })
}

fn print_anchor(paths: &StatePaths) -> anyhow::Result<i32> {
    let Some(anchor) = chain::current(&paths.events_log)? else {
        eprintln!("taskrunner: the event log is empty; there is nothing to anchor yet");
        return Ok(1);
    };
    say(&format!("{}\n", anchor.display()))?;
    Ok(0)
}

/// `URLSearchParams` encoding: application/x-www-form-urlencoded.
fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub fn parse_args(argv: &[String]) -> anyhow::Result<Args> {
    let mut command = None;
    let mut rest = Vec::new();
    let mut flags = BTreeMap::new();
    let mut root = std::env::var_os("TASKRUNNER_STATE_ROOT").map(PathBuf::from);
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        // Help takes no value, so it must be caught before the rule that every
        // other --flag does.
        if arg == "--help" || arg == "-h" {
            command = Some("help".to_string());
        } else if arg == "--state-root" {
            let dir = args
                .next()
                .filter(|d| !d.is_empty())
                .context("--state-root requires a directory argument")?;
            root = Some(PathBuf::from(dir));
        } else if let Some(flag) = arg.strip_prefix("--") {
            let value = args.next().with_context(|| format!("{arg} requires a value"))?;
            flags.insert(flag.to_string(), value.clone());
        } else if command.is_none() {
            command = Some(arg.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    let paths = state_paths(&root.unwrap_or_else(default_root));
    Ok(Args { command, rest, flags, paths })
}

pub async fn main(argv: &[String]) -> i32 {
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(err) => {
            eprint!("taskrunner: {err}\n\n{USAGE}");
            return 1;
        }
    };
    let outcome = match args.command.as_deref() {
        Some("up") => up(&args.paths).await,
        Some("down") => down(&args.paths).await,
        Some("status") => status(&args.paths).await,
        Some("doctor") => run_doctor(&args.paths).await,
        Some("mcp") => {
            let host = match args.flag("host") {
                None => None,
                Some(name) => match HostKind::parse(&name) {
                    Some(host) => Some(host),
                    None => {
                        eprintln!(
                            "taskrunner: --host must be claude, codex or hermes, not '{name}'"
                        );
                        return 1;
                    }
                },
            };
            run_shim(&args.paths, host).await
        }
        Some("sync") => {
            SyncOptions::parse(args.flag("connect"), args.flag("skip"), args.flag("delegation"))
                .and_then(|options| run_sync(&args.paths, &options))
        }
        Some("wait") => {
            let Some(id) = args.rest.first() else {
                eprintln!("taskrunner: wait <task-id> requires a task id");
                return 1;
            };
            wait_task(&args.paths, id, args.flag("timeout")).await
        }
        Some("verify") => verify_log(&args.paths, args.flag("anchor")),
        Some("anchor") => print_anchor(&args.paths),
        Some("sessions") => {
            read_query(
                &args.paths,
                "/lookup-session",
                &[("project", args.flag("project")), ("limit", args.flag("limit"))],
            )
            .await
        }
        Some("session") => {
            let Some(id) = args.rest.first() else {
                eprintln!("taskrunner: session <id> requires a session id");
                return 1;
            };
            let mut params = vec![
                ("sessionId", Some(id.clone())),
                ("source", args.flag("source")),
                ("last", args.flag("last")),
            ];
            params.extend(args.render_flags());
            read_query(&args.paths, "/lookup-session", &params).await
        }
        Some("search") => {
            let query = args.rest.first().cloned();
            let structured =
                ["tool", "target", "failed"].iter().any(|f| args.flags.contains_key(*f));
            if query.is_none() && !structured {
                eprintln!("taskrunner: search needs a query, or --tool / --target / --failed");
                return 1;
            }
            let params = [
                ("query", query),
                ("tool", args.flag("tool")),
                ("target", args.flag("target")),
                ("failed", args.flag("failed")),
                ("project", args.flag("project")),
                ("sessions", args.flag("sessions")),
                ("lastSessions", args.flag("last-sessions")),
                ("role", args.flag("role")),
                ("kind", args.flag("kind")),
                ("since", args.flag("since")),
                ("until", args.flag("until")),
                ("sort", args.flag("sort")),
                ("limit", args.flag("limit")),
            ];
            read_query(&args.paths, "/search-transcripts", &params).await
        }
        Some("task") => {
            let Some(id) = args.rest.first() else {
                eprintln!("taskrunner: task <id> requires a task id");
                return 1;
            };
            let mut params = vec![
                ("taskId", Some(id.clone())),
                ("include", args.flag("include")),
                ("turnId", args.flag("turn")),
                ("last", args.flag("last")),
            ];
            params.extend(args.render_flags());
            read_query(&args.paths, "/lookup-task", &params).await
        }
        Some("tasks") => {
            read_query(
                &args.paths,
                "/lookup-task",
                &[("project", args.flag("project")), ("limit", args.flag("limit"))],
            )
            .await
        }
        None | Some("help") => say(USAGE)
            .map(|()| if args.command.is_none() { 1 } else { 0 })
            .map_err(anyhow::Error::from),
        Some(other) => {
            eprint!("taskrunner: unknown command '{other}'\n\n{USAGE}");
            Ok(1)
        }
    };
    match outcome {
        Ok(code) => code,
        Err(err) => {
            eprintln!("taskrunner: {err:#}");
            1
        }
    }
}

async fn up(paths: &StatePaths) -> anyhow::Result<i32> {
    let daemon = match Daemon::start(paths.clone(), DaemonOptions::default()).await {
        Ok(daemon) => daemon,
        Err(err) if err.is::<AlreadyRunning>() => {
            eprintln!("{err}");
            return Ok(2);
        }
        Err(err) => return Err(err),
    };
    say(&format!("taskrunner daemon {VERSION} listening on {}\n", paths.socket_path.display()))?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    daemon.stop().await;
    Ok(0)
}

async fn down(paths: &StatePaths) -> anyhow::Result<i32> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let Some(pid) = crate::daemon::read_pid(&paths.pid_file) else {
        say("taskrunner daemon is not running\n")?;
        return Ok(0);
    };
    if kill(Pid::from_raw(pid), Signal::SIGTERM).is_err() {
        say("taskrunner daemon is not running (stale pid file)\n")?;
        return Ok(0);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !crate::daemon::is_process_alive(pid) {
            say(&format!("taskrunner daemon stopped (pid {pid})\n"))?;
            return Ok(0);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    eprintln!("taskrunner daemon (pid {pid}) did not stop within 5s");
    Ok(1)
}

async fn status(paths: &StatePaths) -> anyhow::Result<i32> {
    let fetched = client::get(&paths.socket_path, "/status", Duration::from_secs(2)).await;
    let body: serde_json::Value =
        match fetched.ok().filter(|r| r.ok()).and_then(|r| serde_json::from_str(&r.body).ok()) {
            Some(body) => body,
            None => {
                say("taskrunner daemon is not running\n")?;
                return Ok(1);
            }
        };
    let tasks = body["tasks"]
        .as_object()
        .map(|counts| {
            counts.iter().map(|(status, n)| format!("{status}={n}")).collect::<Vec<_>>().join(" ")
        })
        .unwrap_or_default();
    say(&format!(
        "taskrunner daemon {} running (pid {})\nstate root: {}\nactive mcp sessions: {}\ntasks: {}\n",
        body["version"].as_str().unwrap_or(""),
        body["pid"],
        body["state_root"].as_str().unwrap_or(""),
        body["active_mcp_sessions"],
        if tasks.is_empty() { "none".to_string() } else { tasks }
    ))?;
    Ok(0)
}

/// How long one wait request may block. The CLI keeps asking in rounds, so a
/// single HTTP request never has to outlive a daemon restart by much.
const WAIT_ROUND_SECS: u64 = 300;

/// Blocks until a task's running turn ends (or `--timeout` seconds pass) and
/// prints the short result. Silent until then: it is meant to run in the
/// background and cost one notification when it ends.
async fn wait_task(
    paths: &StatePaths,
    task_id: &str,
    timeout: Option<String>,
) -> anyhow::Result<i32> {
    let limit = match timeout {
        None => None,
        Some(secs) => {
            Some(secs.parse::<u64>().context("--timeout must be a whole number of seconds")?)
        }
    };
    let started = Instant::now();
    loop {
        let left = limit.map(|limit| limit.saturating_sub(started.elapsed().as_secs()));
        let round = left.map_or(WAIT_ROUND_SECS, |left| left.min(WAIT_ROUND_SECS));
        let path = format!("/wait-task?taskId={}&timeout={round}", url_encode(task_id));
        let Ok(res) = client::get(&paths.socket_path, &path, Duration::from_secs(round + 30)).await
        else {
            say("taskrunner daemon is not running\n")?;
            return Ok(1);
        };
        if !res.ok() {
            eprint!("taskrunner: {}", res.body);
            return Ok(1);
        }
        let body: serde_json::Value = serde_json::from_str(&res.body)?;
        let status = body["status"].as_str().unwrap_or_default();
        let running = status == "running" || status == "created";
        if !running || left == Some(0) || limit.is_some_and(|l| started.elapsed().as_secs() >= l) {
            say(body["text"].as_str().unwrap_or_default())?;
            return Ok(match status {
                "completed" => 0,
                _ if running => 2,
                _ => 1,
            });
        }
    }
}
