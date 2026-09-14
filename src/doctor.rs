//! `taskrunner doctor`: a read-only preflight over the pieces a delegated
//! turn needs — Docker, worker images, auth volumes, the egress proxy image —
//! plus ingestion health and best-effort worker-credential freshness. It
//! reuses the same config-driven worker enumeration the daemon uses, so it can
//! never drift from what actually runs. Nothing here mutates state.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::client;
use crate::config::{Config, HarnessKind, load_config, worker_config};
use crate::daemon::mcp::VERSION;
use crate::harnesses::{default_image, ingest_sources, worker_kind, worker_names};
use crate::ingest::sweep::expand_home;
use crate::paths::StatePaths;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn mark(self) -> &'static str {
        match self {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => "✗",
        }
    }
}

struct Check {
    level: Level,
    label: String,
    detail: String,
}

fn check(level: Level, label: impl Into<String>, detail: impl Into<String>) -> Check {
    Check { level, label: label.into(), detail: detail.into() }
}

struct DockerOutput {
    ok: bool,
    stdout: String,
}

/// Runs a docker subcommand, capturing output; never fails.
fn docker(args: &[&str]) -> DockerOutput {
    match Command::new("docker").args(args).stdin(Stdio::null()).output() {
        Ok(output) => DockerOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        },
        Err(_) => DockerOutput { ok: false, stdout: String::new() },
    }
}

async fn check_daemon(paths: &StatePaths, checks: &mut Vec<Check>) {
    let fetched = client::get(&paths.socket_path, "/status", Duration::from_secs(2)).await;
    let version = fetched
        .ok()
        .and_then(|res| serde_json::from_str::<Value>(&res.body).ok())
        .and_then(|body| body["version"].as_str().map(str::to_string));
    checks.push(match version {
        Some(version) if version == VERSION => {
            check(Level::Ok, "daemon", format!("running, version {version}"))
        }
        Some(version) => check(
            Level::Warn,
            "daemon",
            format!(
                "running version {version}, but this CLI is {VERSION}; restart the daemon to update"
            ),
        ),
        None => {
            check(Level::Warn, "daemon", "not running (it auto-starts on the next MCP connection)")
        }
    });
}

fn check_docker(checks: &mut Vec<Check>) -> bool {
    let info = docker(&["version", "--format", "{{.Server.Version}}"]);
    if info.ok {
        checks.push(check(Level::Ok, "docker", format!("engine {}", info.stdout.trim())));
        return true;
    }
    checks.push(check(
        Level::Fail,
        "docker",
        "not available; start Docker Desktop (workers and hub cannot run without it)",
    ));
    false
}

fn check_workers(config: &Config, docker_up: bool, checks: &mut Vec<Check>) {
    for name in worker_names(config) {
        let cfg = worker_config(config, &name);
        let kind = worker_kind(config, &name);
        let image = cfg
            .image
            .clone()
            .or_else(|| kind.map(|k| default_image(k).to_string()))
            .unwrap_or_default();
        if image.is_empty() {
            checks.push(check(Level::Fail, format!("worker {name}"), "no image configured"));
            continue;
        }
        if docker_up {
            let has = docker(&["image", "inspect", &image]).ok;
            let detail = if has {
                image.clone()
            } else {
                format!("{image} not built; run: sh scripts/build-images.sh")
            };
            checks.push(check(
                if has { Level::Ok } else { Level::Fail },
                format!("worker {name} image"),
                detail,
            ));
        }
        if let (Some(volume), true) = (&cfg.auth_volume, docker_up) {
            let has = docker(&["volume", "inspect", volume]).ok;
            let detail = if has {
                volume.clone()
            } else {
                format!(
                    "auth volume '{volume}' missing; log the worker in (see README § Worker sign-in)"
                )
            };
            checks.push(check(
                if has { Level::Ok } else { Level::Fail },
                format!("worker {name} auth"),
                detail,
            ));
            if has && let Some(kind) = kind {
                check_credential(&name, kind, volume, &image, checks);
            }
        }
    }
    if docker_up {
        let proxy = &config.egress.proxy_image;
        let has = docker(&["image", "inspect", proxy]).ok;
        let detail = if has {
            proxy.clone()
        } else {
            format!("{proxy} not built; run: sh scripts/build-images.sh")
        };
        checks.push(check(if has { Level::Ok } else { Level::Fail }, "egress proxy", detail));
    }
}

/// Where each harness kind keeps its credential file inside its auth volume.
fn credential_file(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Codex => "auth.json",
        HarnessKind::Claude => ".claude/.credentials.json",
    }
}

/// Best-effort: report credential presence and any parseable expiry. Never
/// downgrades to fail — a stale read here should not block anything.
fn check_credential(
    name: &str,
    kind: HarnessKind,
    volume: &str,
    image: &str,
    checks: &mut Vec<Check>,
) {
    let rel = credential_file(kind);
    let read = docker(&[
        "run",
        "--rm",
        "-v",
        &format!("{volume}:/v:ro"),
        image,
        "cat",
        &format!("/v/{rel}"),
    ]);
    let label = format!("worker {name} credential");
    if !read.ok {
        checks.push(check(
            Level::Warn,
            label,
            format!("no {rel} in the auth volume; the worker may need to log in"),
        ));
        return;
    }
    let Some(expiry) = parse_expiry(&read.stdout) else {
        checks.push(check(Level::Ok, label, "present"));
        return;
    };
    let now = chrono::Utc::now().timestamp_millis();
    let stamp = chrono::DateTime::from_timestamp_millis(expiry)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default();
    if expiry <= now {
        checks.push(check(Level::Warn, label, format!("expired {stamp}; re-run the worker login")));
    } else {
        checks.push(check(Level::Ok, label, format!("valid until {stamp}")));
    }
}

/// Pulls a millisecond expiry out of known credential shapes, if present.
fn parse_expiry(text: &str) -> Option<i64> {
    let json: Value = serde_json::from_str(text).ok()?;
    if let Some(ms) = json["claudeAiOauth"]["expiresAt"].as_i64() {
        return Some(ms);
    }
    let expires_at = json["tokens"]["expires_at"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(expires_at).ok().map(|t| t.timestamp_millis())
}

fn check_ingest(config: &Config, paths: &StatePaths, checks: &mut Vec<Check>) {
    for source in ingest_sources(config) {
        if let Some(volume) = &source.volume {
            // Volume sources are copied out via Docker at sweep time; presence
            // is covered by the worker auth-volume check above, so just note
            // the route.
            let label = format!("ingest {} (volume)", source.format);
            checks.push(check(
                Level::Ok,
                label,
                format!("{volume}/{}", source.subdir.as_deref().unwrap_or("")),
            ));
            continue;
        }
        let present = source.dirs.iter().filter(|dir| expand_home(dir).exists()).count();
        let label = format!("ingest {}", source.format);
        if present > 0 {
            checks.push(check(
                Level::Ok,
                label,
                format!("{present}/{} source dir(s) present", source.dirs.len()),
            ));
        } else {
            let detail = format!(
                "no source dirs found ({}); nothing to archive yet",
                source.dirs.join(", ")
            );
            checks.push(check(Level::Warn, label, detail));
        }
    }
    if paths.ingest_state_file.exists() {
        let parseable = std::fs::read(&paths.ingest_state_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some();
        checks.push(if parseable {
            check(Level::Ok, "ingest state", "sidecar parseable")
        } else {
            check(
                Level::Warn,
                "ingest state",
                "sidecar unparseable; it will be rebuilt on the next sweep (harmless)",
            )
        });
    }
}

pub async fn run_doctor(paths: &StatePaths) -> anyhow::Result<i32> {
    let config = load_config(&paths.config_file)?;
    let mut checks = Vec::new();
    check_daemon(paths, &mut checks).await;
    let docker_up = check_docker(&mut checks);
    check_workers(&config, docker_up, &mut checks);
    check_ingest(&config, paths, &mut checks);

    for c in &checks {
        println!("  {} {}: {}", c.level.mark(), c.label, c.detail);
    }
    let failures = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warnings = checks.iter().filter(|c| c.level == Level::Warn).count();
    println!(
        "\ntaskrunner doctor: {failures} failing, {warnings} warning(s), {} ok",
        checks.len() - failures - warnings
    );
    Ok(if failures > 0 { 1 } else { 0 })
}
