//! Daemon tests: the control socket, the lock, crash recovery, MCP sessions,
//! and the stdio shim. Each test gets its own short state root — unix socket
//! paths are capped around 104 bytes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[path = "../helpers/mod.rs"]
mod helpers;

use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation};
use serde_json::{Value, json};
use taskrunner::client;
use taskrunner::daemon::scheduler::AssignArgs;
use taskrunner::daemon::{AlreadyRunning, Daemon, DaemonOptions};
use taskrunner::paths::{StatePaths, state_paths};
use taskrunner::storage::events::{EventBody, EventLog, read_events};

/// A throwaway state root under /tmp: short, so the socket path fits.
fn short_root() -> (tempfile::TempDir, StatePaths) {
    let dir = tempfile::Builder::new().prefix("tr-").tempdir_in("/tmp").unwrap();
    let paths = state_paths(dir.path());
    (dir, paths)
}

/// Default off: tests must never sweep the developer's real host transcripts.
async fn start(paths: &StatePaths) -> Daemon {
    Daemon::start(
        paths.clone(),
        DaemonOptions { ingest_sources: Some(vec![]), ..Default::default() },
    )
    .await
    .unwrap()
}

async fn get(paths: &StatePaths, path: &str) -> client::Response {
    client::get(&paths.socket_path, path, Duration::from_secs(5)).await.unwrap()
}

fn message_recorded() -> EventBody {
    EventBody::MessageRecorded {
        message_id: "m1".into(),
        source: "claude-code".into(),
        native_session_id: "host-9".into(),
        native_record_id: "r1".into(),
        role: "user".into(),
        kind: "message".into(),
        content: "hunt for the flux capacitor".into(),
        native_ts: Some("2026-07-24T00:00:01.000Z".into()),
        project_path: Some("/repo".into()),
    }
}

#[tokio::test]
async fn serves_status_on_the_unix_socket_and_cleans_up_on_stop() {
    let (_dir, paths) = short_root();
    let daemon = start(&paths).await;

    let res = get(&paths, "/status").await;
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).unwrap();
    assert_eq!(body["pid"], json!(std::process::id()));
    assert_eq!(body["state_root"], json!(paths.root.to_string_lossy()));

    daemon.stop().await;
    assert!(!paths.socket_path.exists());
    assert!(!paths.mcp_socket_path.exists());
    assert!(!paths.pid_file.exists());
    assert!(!paths.lock_file.exists());
}

#[tokio::test]
async fn serves_read_only_query_routes_over_the_socket() {
    let (_dir, paths) = short_root();
    let daemon = start(&paths).await;
    daemon
        .record(EventBody::ProjectCreated { project_id: "p1".into(), root: "/repo".into() })
        .unwrap();
    daemon.record(message_recorded()).unwrap();

    let sessions = get(&paths, "/lookup-session").await;
    assert_eq!(sessions.status, 200);
    assert!(sessions.body.contains("host-9"));

    let history = get(&paths, "/lookup-session?sessionId=host-9").await;
    assert!(history.body.contains("flux capacitor"));

    let search = get(&paths, "/search-transcripts?query=flux").await;
    assert!(search.body.contains("host-9"));

    // A missing required query param is a client error, not a crash.
    let bad = get(&paths, "/search-transcripts").await;
    assert_eq!(bad.status, 400);
    daemon.stop().await;
}

#[tokio::test]
async fn refuses_a_second_daemon_on_the_same_state_root() {
    let (_dir, paths) = short_root();
    let first = start(&paths).await;
    let second = Daemon::start(
        paths.clone(),
        DaemonOptions { ingest_sources: Some(vec![]), ..Default::default() },
    )
    .await;
    let Err(err) = second else { panic!("second daemon started") };
    assert!(err.is::<AlreadyRunning>());
    first.stop().await;
}

#[tokio::test]
async fn allows_a_restart_after_a_clean_stop() {
    let (_dir, paths) = short_root();
    start(&paths).await.stop().await;
    start(&paths).await.stop().await;
}

#[tokio::test]
async fn fails_turns_left_running_by_a_crash_keeping_their_audit_trail() {
    let (_dir, paths) = short_root();
    let mut log = EventLog::open(&paths.events_log).unwrap();
    log.append(EventBody::ProjectCreated { project_id: "proj_a".into(), root: "/repo".into() })
        .unwrap();
    log.append(EventBody::TaskCreated {
        task_id: "task_a".into(),
        project_id: "proj_a".into(),
        session_id: None,
        worker: "codex".into(),
        prompt_summary: "x".into(),
        tier: None,
        runtime: None,
        allow_domains: None,
    })
    .unwrap();
    log.append(EventBody::TurnStarted {
        turn_id: "turn_a1".into(),
        task_id: "task_a".into(),
        prompt: "go".into(),
    })
    .unwrap();
    log.append(EventBody::AuditRecorded {
        session_id: None,
        task_id: Some("task_a".into()),
        turn_id: Some("turn_a1".into()),
        kind: "worker.command_execution".into(),
        payload: json!({ "command": "sleep 999" }),
    })
    .unwrap();
    drop(log);

    let daemon = start(&paths).await;
    let (status, error_code): (String, String) = daemon
        .store
        .lock()
        .index
        .db
        .query_row("SELECT status, error_code FROM turns WHERE id = 'turn_a1'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((status.as_str(), error_code.as_str()), ("failed", "worker_failed"));

    let events = read_events(&paths.events_log).unwrap();
    assert!(matches!(events.last().unwrap().body, EventBody::TurnFailed { .. }));
    // The pre-crash audit trail is untouched.
    assert_eq!(
        events.iter().filter(|e| matches!(e.body, EventBody::AuditRecorded { .. })).count(),
        1
    );
    daemon.stop().await;
}

fn client_info(name: &str) -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), Implementation::new(name, "0.0.1"))
}

#[tokio::test]
async fn records_session_started_and_ended_for_mcp_sessions() {
    let (_dir, paths) = short_root();
    let daemon = start(&paths).await;

    let stream = tokio::net::UnixStream::connect(&paths.mcp_socket_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let client = client_info("daemon-test-client").serve((reader, writer)).await.unwrap();
    client.peer().list_tools(None).await.unwrap();

    let events = read_events(&paths.events_log).unwrap();
    let started = events.iter().find_map(|e| match &e.body {
        EventBody::SessionStarted { session_id, client, .. } => {
            Some((session_id.clone(), client.clone()))
        }
        _ => None,
    });
    let (session_id, client_name) = started.expect("session.started recorded");
    assert_eq!(client_name.as_deref(), Some("daemon-test-client"));
    assert_eq!(daemon.active_sessions(), 1);

    client.cancel().await.unwrap();
    // The daemon records the end when the connection closes.
    let ended = wait_for(|| {
        read_events(&paths.events_log).unwrap().into_iter().find_map(|e| match e.body {
            EventBody::SessionEnded { session_id } => Some(session_id),
            _ => None,
        })
    })
    .await;
    assert_eq!(ended, session_id);
    daemon.stop().await;
}

async fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(value) = probe() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not met within 5s");
}

#[tokio::test]
async fn runs_a_worker_that_exists_only_in_config_end_to_end() {
    // The pluggability acceptance test: no injected harnesses, no code that
    // knows this worker's name — just a [worker.<name>] config section. Only
    // the runner factory is stubbed so the fake codex binary runs sans Docker.
    let (_dir, paths) = short_root();
    std::fs::create_dir_all(&paths.root).unwrap();
    std::fs::write(&paths.config_file, "[worker.configling]\nharness = \"codex\"\n").unwrap();
    let repo = helpers::init_git_repo();

    let daemon = Daemon::start(
        paths.clone(),
        DaemonOptions {
            ingest_sources: Some(vec![]),
            make_runner: Some(Arc::new(|ctx| {
                Box::new(helpers::LocalRunner::new(
                    &ctx.workspace_dir,
                    Some(&helpers::fake_codex()),
                ))
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let assigned = daemon
        .scheduler
        .assign_task(AssignArgs {
            project: repo.path().to_string_lossy().into_owned(),
            worker: "configling".into(),
            prompt: "create hello".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(assigned.tier.as_deref(), Some("workspace-write"));
    let status = wait_for(|| {
        let status = daemon.scheduler.outcome(&assigned.task_id).unwrap().status;
        (status != "running" && status != "created").then_some(status)
    })
    .await;
    assert_eq!(status, "completed");
    let done = daemon.scheduler.outcome(&assigned.task_id).unwrap();
    assert!(done.summary.as_deref().unwrap().contains("create hello"));
    assert_eq!(done.changed_files, vec!["hello.txt"]);
    daemon.stop().await;
}

// ---- the stdio shim ------------------------------------------------------

fn taskrunner_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_taskrunner"))
}

async fn shim(
    root: &Path,
    name: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, ClientInfo> {
    let mut cmd = tokio::process::Command::new(taskrunner_bin());
    cmd.args(["mcp", "--state-root"]).arg(root);
    let transport = rmcp::transport::TokioChildProcess::new(cmd).unwrap();
    client_info(name).serve(transport).await.unwrap()
}

#[tokio::test]
async fn auto_starts_one_daemon_even_when_two_shims_race_and_both_connect() {
    let (_dir, paths) = short_root();
    // Keep the auto-started daemon off the developer's real transcripts.
    std::fs::write(
        &paths.config_file,
        "[ingest.sources.claude-code]\ndirs = []\n[ingest.sources.codex]\ndirs = []\n",
    )
    .unwrap();

    let (a, b) = tokio::join!(shim(&paths.root, "shim-a"), shim(&paths.root, "shim-b"));
    a.peer().list_tools(None).await.unwrap();
    b.peer().list_tools(None).await.unwrap();

    let pid = taskrunner::daemon::read_pid(&paths.pid_file).expect("pid file");
    // Both shims proxied to the same daemon; pid file is stable.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(taskrunner::daemon::read_pid(&paths.pid_file), Some(pid));

    a.cancel().await.unwrap();
    b.cancel().await.unwrap();

    // The daemon outlives its shims.
    assert!(taskrunner::daemon::is_process_alive(pid));
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM)
        .unwrap();
}
