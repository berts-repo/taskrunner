//! One daemon per state root owns the event log, index, and worker lifecycle.
//! Single-writer is enforced by construction: all durable writes go through
//! the shared store, and the lock file keeps a second daemon out.
//!
//! Two unix sockets under the runtime dir: `daemon.sock` serves HTTP
//! (`/status` and the read-only query routes), `mcp.sock` serves one MCP
//! session per connection as newline-delimited JSON-RPC — the shim pumps a
//! client's stdio to it byte for byte. A connection is a session.

mod http;
mod lock;
pub mod mcp;
pub mod scheduler;
mod sweep_gate;
mod tools;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context;
use rmcp::ServiceExt;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub use lock::{AlreadyRunning, is_process_alive, read_pid};

/// The Docker runner for a turn. Mounts only the worker's own auth material,
/// never broad host state; mounts and image fallbacks key on the harness
/// kind, so config-only workers inherit their loop's layout. A missing image
/// surfaces from the runner's preflight at start time, so harnesses that
/// never spawn a process (tests) are unaffected. Every egress decision the
/// proxy makes is recorded against the turn.
fn docker_runner_factory(config: Arc<Config>, store: SharedStore) -> Arc<MakeRunner> {
    Arc::new(move |ctx: RunnerContext| -> Box<dyn WorkerRunner> {
        let cfg = worker_config(&config, &ctx.worker);
        let kind = worker_kind(&config, &ctx.worker);
        let store = store.clone();
        let (task_id, turn_id) = (ctx.task_id.clone(), ctx.turn_id.clone());
        Box::new(DockerRunner::new(DockerRunnerOptions {
            workspace_dir: ctx.workspace_dir,
            scope_id: ctx.turn_id,
            image: cfg
                .image
                .or_else(|| kind.map(|k| default_image(k).to_string()))
                .unwrap_or_default(),
            auth_volume: cfg.auth_volume,
            auth_mounts: kind.map(auth_mounts).unwrap_or_default(),
            proxy_image: config.egress.proxy_image.clone(),
            allowed_domains: ctx.allowed_domains,
            limits: cfg.limits,
            on_egress: Some(Arc::new(move |decision| {
                let mut payload =
                    serde_json::json!({ "host": decision.host, "port": decision.port });
                if let Some(reason) = decision.reason {
                    payload["reason"] = serde_json::Value::String(reason);
                }
                let _ = store.record(EventBody::AuditRecorded {
                    session_id: None,
                    task_id: Some(task_id.clone()),
                    turn_id: Some(turn_id.clone()),
                    kind: if decision.allowed { "egress.allowed" } else { "egress.refused" }.into(),
                    payload,
                });
            })),
            docker_command: "docker".into(),
        }))
    })
}
use mcp::{McpService, SessionRecord};
use sweep_gate::SweepGate;
use tools::ToolTable;

use crate::config::{Config, load_config, worker_config};
use crate::harnesses::{auth_mounts, build_harnesses, default_image, ingest_sources, worker_kind};
use crate::ingest::sweep::{IngestSource, SweepStats, SweeperDeps, TranscriptSweeper};
use crate::ingest::volume::{docker_copy_out, reap_copy_out_containers};
use crate::paths::StatePaths;
use crate::storage::Recorder;
use crate::storage::artifacts::ArtifactStore;
use crate::storage::events::{EventBody, EventLog, read_events};
use crate::storage::index::rebuild_index;
use crate::storage::store::SharedStore;
use crate::workers::harness::WorkerHarness;
use crate::workers::runner::{DockerRunner, DockerRunnerOptions, WorkerRunner};
use crate::workspace::clone::{CloneWorkspaces, WorkspaceProvider};
use scheduler::{MakeRunner, RunnerContext, Scheduler, SchedulerDeps};
use std::collections::HashMap;

#[derive(Default)]
pub struct DaemonOptions {
    /// Test seam: overrides the config-derived transcript sources. Pass an
    /// empty list to keep the startup sweep from touching real host
    /// transcript directories.
    pub ingest_sources: Option<Vec<IngestSource>>,
    /// Configured worker harnesses; tests may inject a fake.
    pub harnesses: Option<HashMap<String, Arc<dyn WorkerHarness>>>,
    pub workspaces: Option<Arc<dyn WorkspaceProvider>>,
    /// Test seam: replaces the Docker runner factory.
    pub make_runner: Option<Arc<MakeRunner>>,
}

/// A handle on the running daemon. Clones share it; `stop` takes one down.
#[derive(Clone)]
pub struct Daemon {
    pub paths: Arc<StatePaths>,
    pub config: Arc<Config>,
    pub store: SharedStore,
    pub artifacts: Arc<ArtifactStore>,
    pub scheduler: Scheduler,
    tools: Arc<ToolTable>,
    sweeps: SweepGate,
    sessions: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    tasks: Arc<tokio::sync::Mutex<JoinSet<()>>>,
}

impl Daemon {
    pub async fn start(paths: StatePaths, options: DaemonOptions) -> anyhow::Result<Daemon> {
        // sun_path is ~104 bytes on macOS; fail with a clear message instead
        // of a bare EINVAL from bind().
        if paths.socket_path.as_os_str().len() > 100 {
            anyhow::bail!(
                "state root produces a unix socket path longer than the OS limit: {}",
                paths.socket_path.display()
            );
        }
        fs::create_dir_all(&paths.runtime_dir)?;
        fs::create_dir_all(&paths.logs_dir)?;
        // The state root holds every ingested transcript and the control
        // socket that drives workers (spending their stored credentials).
        // Owner-only, so another local user cannot read the archive or assign
        // tasks. The execute bit on the root gates traversal into every child.
        fs::set_permissions(&paths.root, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&paths.runtime_dir, fs::Permissions::from_mode(0o700))?;
        lock::acquire(&paths)?;

        match Self::boot(paths.clone(), options).await {
            Ok(daemon) => Ok(daemon),
            Err(err) => {
                lock::release(&paths);
                Err(err)
            }
        }
    }

    async fn boot(paths: StatePaths, options: DaemonOptions) -> anyhow::Result<Daemon> {
        let log = EventLog::open(&paths.events_log)?;
        let index_path = paths.index_db.to_string_lossy().into_owned();
        let index = rebuild_index(&index_path, &read_events(&paths.events_log)?)?;
        let config = load_config(&paths.config_file)?;
        let store = SharedStore::new(log, index);
        let artifacts = Arc::new(ArtifactStore::new(&paths.artifacts_dir));

        let sweeper = TranscriptSweeper::new(SweeperDeps {
            sources: options.ingest_sources.unwrap_or_else(|| ingest_sources(&config)),
            archive: Box::new(store.clone()),
            state_file: paths.ingest_state_file.clone(),
            staging_dir: Some(paths.ingest_staging_dir.clone()),
            copy_volume: Some(Box::new(|volume, subdir, image, dest| {
                docker_copy_out(volume, subdir, dest, image, "docker")
            })),
            on_log: None,
        });
        let config = Arc::new(config);
        // Turns run in self-contained clones the container can mount safely.
        let workspaces = options.workspaces.unwrap_or_else(|| {
            Arc::new(CloneWorkspaces::new(
                &paths.workspaces_dir,
                artifacts.clone(),
                Arc::new(store.clone()),
            ))
        });
        let make_runner = options
            .make_runner
            .unwrap_or_else(|| docker_runner_factory(config.clone(), store.clone()));
        let scheduler = Scheduler::new(SchedulerDeps {
            config: config.clone(),
            store: store.clone(),
            harnesses: options.harnesses.unwrap_or_else(|| build_harnesses(&config)),
            workspaces,
            make_runner,
            artifacts: artifacts.clone(),
        });
        let daemon = Daemon {
            paths: Arc::new(paths),
            config,
            store,
            artifacts,
            scheduler,
            tools: Arc::new(ToolTable::load()),
            sweeps: SweepGate::new(sweeper),
            sessions: Arc::new(AtomicUsize::new(0)),
            shutdown: CancellationToken::new(),
            tasks: Arc::new(tokio::sync::Mutex::new(JoinSet::new())),
        };

        daemon.recover_crashed_turns()?;
        let http = daemon.bind(&daemon.paths.socket_path)?;
        let mcp = daemon.bind(&daemon.paths.mcp_socket_path)?;
        let mut tasks = daemon.tasks.lock().await;
        tasks.spawn(daemon.clone().serve_http(http));
        tasks.spawn(daemon.clone().serve_mcp(mcp));
        drop(tasks);

        // Before the first sweep, and only here: a copy-out container that is
        // still around is garbage by definition at this point, but one created
        // by a sweep in flight would not be. Never fatal — a stray container
        // costs nothing, so failing to clear it must not keep the daemon down.
        if let Ok(Err(err)) =
            tokio::task::spawn_blocking(|| reap_copy_out_containers("docker")).await
        {
            eprintln!("taskrunner: copy-out container reap failed: {err}");
        }
        // Only once the sockets are accepting: the first backfill can run for
        // minutes, and shims give the daemon a bounded window to become ready.
        daemon.tasks.lock().await.spawn(daemon.clone().keep_sweeping());
        Ok(daemon)
    }

    /// Appends to the event log and folds into the index — the only write path.
    pub fn record(&self, body: EventBody) -> anyhow::Result<crate::storage::events::LogEvent> {
        self.store.record(body)
    }

    pub fn active_sessions(&self) -> usize {
        self.sessions.load(Ordering::Relaxed)
    }

    /// Sweeps host transcript dirs on demand (skips the worker-volume
    /// copy-out), so a session-recency query reflects the live conversation.
    pub async fn sweep_host_transcripts(&self) -> SweepStats {
        self.sweeps.sweep(true).await
    }

    fn bind(&self, socket: &std::path::Path) -> anyhow::Result<UnixListener> {
        let _ = fs::remove_file(socket);
        let listener =
            UnixListener::bind(socket).with_context(|| format!("binding {}", socket.display()))?;
        // Owner-only: the socket is an unauthenticated control channel, so its
        // filesystem permissions are the access boundary.
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Turns left `running` by a crash are failed with their audit retained.
    fn recover_crashed_turns(&self) -> anyhow::Result<()> {
        let orphans: Vec<(String, String)> = {
            let store = self.store.lock();
            let mut stmt =
                store.index.db.prepare("SELECT id, task_id FROM turns WHERE status = 'running'")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (turn_id, task_id) in orphans {
            self.record(EventBody::TurnFailed {
                turn_id,
                task_id,
                error_code: "worker_failed".into(),
                error_message: "daemon restarted while the turn was running".into(),
            })?;
        }
        Ok(())
    }

    async fn serve_http(self, listener: UnixListener) {
        let shutdown = self.shutdown.clone();
        let served = axum::serve(listener, http::router(self))
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await;
        if let Err(err) = served {
            eprintln!("taskrunner: control socket failed: {err}");
        }
    }

    async fn serve_mcp(self, listener: UnixListener) {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => { connections.spawn(self.clone().serve_session(stream)); }
                    Err(err) => eprintln!("taskrunner: mcp socket accept failed: {err}"),
                },
                _ = self.shutdown.cancelled() => break,
            }
        }
        // Cancelled services end their connections; wait so every
        // session.ended lands before the log closes.
        while connections.join_next().await.is_some() {}
    }

    async fn serve_session(self, stream: tokio::net::UnixStream) {
        self.sessions.fetch_add(1, Ordering::Relaxed);
        let session = Arc::new(SessionRecord::new());
        let service = McpService {
            daemon: self.clone(),
            tools: self.tools.clone(),
            session: session.clone(),
        };
        let (reader, writer) = stream.into_split();
        match service.serve_with_ct((reader, writer), self.shutdown.child_token()).await {
            Ok(running) => {
                let _ = running.waiting().await;
            }
            Err(err) => eprintln!("taskrunner: mcp session failed to initialize: {err}"),
        }
        session.end(&self.store);
        self.sessions.fetch_sub(1, Ordering::Relaxed);
    }

    /// Sweeps host transcripts into the event log once on startup (this is
    /// where the historical backfill lands) and then on the configured
    /// interval. Sweep failures are logged, never fatal.
    async fn keep_sweeping(self) {
        let period = Duration::from_secs(self.config.ingest.interval_seconds.get());
        loop {
            self.sweeps.sweep(false).await;
            tokio::select! {
                _ = tokio::time::sleep(period) => {}
                _ = self.shutdown.cancelled() => return,
            }
        }
    }

    pub async fn stop(self) {
        if self.shutdown.is_cancelled() {
            return;
        }
        // Stop new sweeps and let any in-flight sweep finish appending before
        // the log closes.
        self.shutdown.cancel();
        self.sweeps.settle().await;
        // Cancel running turns so their terminal events land in the log too.
        self.scheduler.shutdown().await;
        let mut tasks = self.tasks.lock().await;
        while tasks.join_next().await.is_some() {}
        let _ = fs::remove_file(&self.paths.socket_path);
        let _ = fs::remove_file(&self.paths.mcp_socket_path);
        lock::release(&self.paths);
    }
}
