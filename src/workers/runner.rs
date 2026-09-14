//! Per-turn execution runtime for worker processes. Harnesses build a worker
//! argv (codex/claude CLI invocations) and the runner runs it inside a Docker
//! container with the workspace mounted at /workspace behind the egress
//! proxy. `RunnerKind::Host` exists only for the local test runner that
//! exercises harnesses without Docker.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::ResourceLimits;
use crate::domain::errors::{ErrorCode, ToolError};
use crate::harnesses::AuthMount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Host,
    Docker,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerSpawnSpec {
    /// Logical worker argv, e.g. ["codex", "exec", ...].
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// A started worker: its process, plus the extra termination step a
/// container needs (the docker CLI proxies signals, but killing the
/// container by name is the reliable path when the attach stream is wedged).
pub struct RunningWorker {
    pub child: Child,
    on_kill: Option<Box<dyn Fn() + Send + Sync>>,
}

impl RunningWorker {
    pub fn new(child: Child) -> RunningWorker {
        RunningWorker { child, on_kill: None }
    }

    pub fn with_kill_step(mut self, step: impl Fn() + Send + Sync + 'static) -> RunningWorker {
        self.on_kill = Some(Box::new(step));
        self
    }

    /// Terminates the worker; used on cancel and timeout.
    pub fn kill(&mut self) {
        if let Some(step) = &self.on_kill {
            step();
        }
        let _ = self.child.start_kill();
    }
}

#[async_trait]
pub trait WorkerRunner: Send + Sync {
    fn kind(&self) -> RunnerKind;
    /// Workspace path as the worker process sees it.
    fn workspace_path(&self) -> &str;
    async fn start(&self, spec: WorkerSpawnSpec) -> Result<RunningWorker, ToolError>;
    /// Post-turn cleanup; the scheduler calls this exactly once.
    async fn dispose(&self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressDecision {
    pub allowed: bool,
    pub host: String,
    pub port: i64,
    /// Why the proxy refused, e.g. "allowlist" or "private-address".
    pub reason: Option<String>,
}

pub type OnEgress = dyn Fn(EgressDecision) + Send + Sync;

pub struct DockerRunnerOptions {
    /// Host path of the task workspace (a task-local clone).
    pub workspace_dir: PathBuf,
    /// Uniquifies container/network names; the turn id.
    pub scope_id: String,
    pub image: String,
    /// Docker volume with the worker's own login, e.g. taskrunner-codex-home.
    pub auth_volume: Option<String>,
    /// Which parts of the auth volume to mount, and where.
    pub auth_mounts: Vec<AuthMount>,
    pub proxy_image: String,
    /// Egress allowlist for this turn: worker defaults plus approved additions.
    pub allowed_domains: Vec<String>,
    /// Resource ceilings applied to the worker container.
    pub limits: ResourceLimits,
    pub on_egress: Option<Arc<OnEgress>>,
    pub docker_command: String,
}

const PROXY_PORT: u16 = 3128;

/// Builds --mount arguments for the worker's auth material. Subpath mounts
/// expose only the login/session paths a harness actually uses, so a task
/// cannot plant state elsewhere in the volume (shell rc files, ~/.config)
/// for a later turn to trust. Credential files stay writable because both
/// CLIs rotate tokens and persist native sessions in place; a task can
/// therefore still read them — scoping that away needs a credential broker,
/// which this layer does not attempt.
pub fn auth_mount_args(volume: &str, mounts: &[AuthMount]) -> Vec<String> {
    let mut args = Vec::new();
    for mount in mounts {
        let mut parts = vec![
            "type=volume".to_string(),
            format!("src={volume}"),
            format!("dst={}", mount.container_path),
        ];
        if let Some(subpath) = mount.subpath {
            parts.push(format!("volume-subpath={subpath}"));
        }
        if mount.read_only {
            parts.push("readonly".to_string());
        }
        args.push("--mount".to_string());
        args.push(parts.join(","));
    }
    args
}

/// Docker flags that bound a worker container. The resource ceilings come
/// from config; `no-new-privileges` is unconditional hardening — the non-root
/// worker user never needs to escalate, so denying it costs nothing and
/// blocks setuid escalation from anything the turn runs.
pub fn resource_limit_args(limits: &ResourceLimits) -> Vec<String> {
    [
        "--memory",
        &limits.memory,
        "--cpus",
        &crate::js::number(&serde_json::Number::from_f64(limits.cpus).unwrap_or_else(|| 0.into())),
        "--pids-limit",
        &limits.pids.to_string(),
        "--security-opt",
        "no-new-privileges",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

async fn docker(command: &str, args: &[&str]) -> Output {
    match Command::new(command).args(args).stdin(Stdio::null()).output().await {
        Ok(output) => Output {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(err) => Output { ok: false, stdout: String::new(), stderr: err.to_string() },
    }
}

/// Runs the worker in a container on an internal Docker network (no route to
/// the outside). A dual-homed egress proxy sidecar is the only way out and
/// forwards only allowlisted domains; every decision it makes is surfaced via
/// `on_egress` for the audit log.
pub struct DockerRunner {
    options: DockerRunnerOptions,
    network_name: String,
    proxy_name: String,
    worker_name: String,
    state: tokio::sync::Mutex<ProxyState>,
}

#[derive(Default)]
struct ProxyState {
    network_created: bool,
    proxy_started: bool,
    logs: Option<Child>,
}

impl DockerRunner {
    pub fn new(options: DockerRunnerOptions) -> DockerRunner {
        let scope = &options.scope_id;
        DockerRunner {
            network_name: format!("taskrunner-egress-{scope}"),
            proxy_name: format!("taskrunner-proxy-{scope}"),
            worker_name: format!("taskrunner-worker-{scope}"),
            state: tokio::sync::Mutex::new(ProxyState::default()),
            options,
        }
    }

    fn docker_cmd(&self) -> &str {
        &self.options.docker_command
    }

    /// Checks docker, image, and auth volume upfront for clear errors.
    async fn preflight(&self) -> Result<(), ToolError> {
        let not_configured = |message: String| ToolError::new(ErrorCode::NotConfigured, message);
        let docker_cmd = self.docker_cmd();
        if self.options.image.is_empty() {
            return Err(not_configured(
                "this worker has no Docker image configured; set [worker.<name>] image".into(),
            ));
        }
        if !docker(docker_cmd, &["version", "--format", "{{.Server.Version}}"]).await.ok {
            return Err(ToolError::new(
                ErrorCode::WorkerUnavailable,
                "Docker is not available; start Docker Desktop",
            ));
        }
        if !docker(docker_cmd, &["image", "inspect", &self.options.image]).await.ok {
            return Err(not_configured(format!(
                "worker image '{}' is not built; run: sh scripts/build-images.sh",
                self.options.image
            )));
        }
        if !docker(docker_cmd, &["image", "inspect", &self.options.proxy_image]).await.ok {
            return Err(not_configured(format!(
                "egress proxy image '{}' is not built; run: sh scripts/build-images.sh",
                self.options.proxy_image
            )));
        }
        if let Some(volume) = &self.options.auth_volume
            && !docker(docker_cmd, &["volume", "inspect", volume]).await.ok
        {
            return Err(not_configured(format!(
                "worker auth volume '{volume}' does not exist; create it and log the worker in (see README § Worker sign-in)"
            )));
        }
        Ok(())
    }

    async fn start_proxy(&self) -> Result<String, ToolError> {
        let internal = |message: String| ToolError::new(ErrorCode::InternalError, message);
        let docker_cmd = self.docker_cmd();
        let mut state = self.state.lock().await;

        let created =
            docker(docker_cmd, &["network", "create", "--internal", &self.network_name]).await;
        if !created.ok {
            return Err(internal(format!(
                "failed to create egress network: {}",
                created.stderr.trim()
            )));
        }
        state.network_created = true;

        let allowed =
            serde_json::to_string(&self.options.allowed_domains).expect("strings serialize");
        let run = docker(
            docker_cmd,
            &[
                "run",
                "-d",
                "--name",
                &self.proxy_name,
                "--network",
                &self.network_name,
                "-e",
                &format!("TASKRUNNER_ALLOWED_DOMAINS={allowed}"),
                &self.options.proxy_image,
            ],
        )
        .await;
        if !run.ok {
            return Err(internal(format!("failed to start egress proxy: {}", run.stderr.trim())));
        }
        state.proxy_started = true;

        // Second leg: the proxy (and only the proxy) can reach the outside.
        let connected =
            docker(docker_cmd, &["network", "connect", "bridge", &self.proxy_name]).await;
        if !connected.ok {
            return Err(internal(format!(
                "failed to connect egress proxy to the outside network: {}",
                connected.stderr.trim()
            )));
        }

        let template = format!(
            "{{{{(index .NetworkSettings.Networks \"{}\").IPAddress}}}}",
            self.network_name
        );
        let inspected = docker(docker_cmd, &["inspect", "-f", &template, &self.proxy_name]).await;
        let proxy_ip = inspected.stdout.trim().to_string();
        if !inspected.ok || proxy_ip.is_empty() {
            return Err(internal("could not determine egress proxy address".into()));
        }

        state.logs = Some(self.watch_proxy_logs().await?);
        Ok(proxy_ip)
    }

    /// Streams proxy decisions; resolves once the proxy reports it is listening.
    async fn watch_proxy_logs(&self) -> Result<Child, ToolError> {
        let mut logs = Command::new(self.docker_cmd())
            .args(["logs", "-f", &self.proxy_name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                ToolError::new(
                    ErrorCode::InternalError,
                    format!("could not follow proxy logs: {err}"),
                )
            })?;
        let stdout = logs.stdout.take().expect("piped stdout");
        let (listening_tx, listening_rx) = tokio::sync::oneshot::channel();
        let on_egress = self.options.on_egress.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut listening = Some(listening_tx);
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if obj.get("proxy").and_then(|v| v.as_str()) == Some("listening") {
                    if let Some(tx) = listening.take() {
                        let _ = tx.send(());
                    }
                    continue;
                }
                if let (Some(egress), Some(on_egress)) =
                    (obj.get("egress").and_then(|v| v.as_str()), &on_egress)
                {
                    on_egress(EgressDecision {
                        allowed: egress == "allowed",
                        host: obj.get("host").map(json_string).unwrap_or_default(),
                        port: obj.get("port").and_then(|v| v.as_i64()).unwrap_or(0),
                        reason: obj.get("reason").and_then(|v| v.as_str()).map(str::to_string),
                    });
                }
            }
        });
        match tokio::time::timeout(Duration::from_secs(10), listening_rx).await {
            Ok(Ok(())) => Ok(logs),
            _ => {
                Err(ToolError::new(ErrorCode::InternalError, "egress proxy did not start in time"))
            }
        }
    }
}

/// `String(v)` for a JSON value that is usually already a string.
fn json_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[async_trait]
impl WorkerRunner for DockerRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Docker
    }

    fn workspace_path(&self) -> &str {
        "/workspace"
    }

    async fn start(&self, spec: WorkerSpawnSpec) -> Result<RunningWorker, ToolError> {
        self.preflight().await?;
        let proxy_ip = self.start_proxy().await?;

        let proxy_url = format!("http://{proxy_ip}:{PROXY_PORT}");
        let mut args: Vec<String> = vec![
            "run".into(),
            "-i".into(),
            "--rm".into(),
            "--name".into(),
            self.worker_name.clone(),
        ];
        args.extend(resource_limit_args(&self.options.limits));
        args.extend([
            "--network".to_string(),
            self.network_name.clone(),
            "-v".into(),
            format!("{}:/workspace", self.options.workspace_dir.display()),
            "-w".into(),
            "/workspace".into(),
            "-e".into(),
            format!("HTTP_PROXY={proxy_url}"),
            "-e".into(),
            format!("HTTPS_PROXY={proxy_url}"),
            "-e".into(),
            format!("http_proxy={proxy_url}"),
            "-e".into(),
            format!("https_proxy={proxy_url}"),
            "-e".into(),
            "NO_PROXY=localhost,127.0.0.1".into(),
        ]);
        if let Some(volume) = &self.options.auth_volume {
            let default_mount =
                [AuthMount { container_path: "/home/worker", subpath: None, read_only: false }];
            let mounts = if self.options.auth_mounts.is_empty() {
                &default_mount[..]
            } else {
                &self.options.auth_mounts
            };
            args.extend(auth_mount_args(volume, mounts));
        }
        for (key, value) in &spec.env {
            args.push("-e".into());
            args.push(format!("{key}={value}"));
        }
        args.push(self.options.image.clone());
        args.extend(spec.argv);

        let child = Command::new(self.docker_cmd())
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                ToolError::new(ErrorCode::WorkerFailed, format!("failed to start docker: {err}"))
            })?;
        let docker_cmd = self.options.docker_command.clone();
        let worker_name = self.worker_name.clone();
        Ok(RunningWorker::new(child).with_kill_step(move || {
            let _ = std::process::Command::new(&docker_cmd).args(["kill", &worker_name]).output();
        }))
    }

    async fn dispose(&self) {
        let docker_cmd = self.docker_cmd();
        let mut state = self.state.lock().await;
        // Give the log stream a beat to flush trailing egress decisions.
        if let Some(mut logs) = state.logs.take() {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = logs.start_kill();
        }
        docker(docker_cmd, &["rm", "-f", &self.worker_name]).await;
        if state.proxy_started {
            docker(docker_cmd, &["rm", "-f", &self.proxy_name]).await;
        }
        if state.network_created {
            docker(docker_cmd, &["network", "rm", &self.network_name]).await;
        }
    }
}
