//! Global TOML config at <state root>/config.toml. Keys are user-facing;
//! keep this schema minimal.

use std::fs;
use std::num::NonZeroU64;
use std::path::Path;

use anyhow::Context;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Which harness code drives a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    Codex,
    Claude,
}

impl HarnessKind {
    pub fn parse(name: &str) -> Option<HarnessKind> {
        match name {
            "codex" => Some(HarnessKind::Codex),
            "claude" => Some(HarnessKind::Claude),
            _ => None,
        }
    }
}

/// Local model server type; setting it puts the codex harness in --oss mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Ollama,
    Lmstudio,
}

/// Per-container resource ceilings. A worker runs delegated (and in Docker,
/// unsandboxed) code, so these bound the blast radius of a runaway turn:
/// Docker kills the container at the limit instead of the host freezing.
/// Inherited by built-in and custom workers alike.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceLimits {
    /// Docker --memory value, e.g. "4g", "512m".
    pub memory: String,
    /// Fractional CPUs (docker --cpus), e.g. 2 or 1.5.
    pub cpus: f64,
    /// Max process/thread count (docker --pids-limit), guards fork bombs.
    pub pids: NonZeroU64,
}

impl Default for ResourceLimits {
    fn default() -> ResourceLimits {
        ResourceLimits {
            memory: "4g".into(),
            cpus: 2.0,
            pids: NonZeroU64::new(512).expect("nonzero"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct WorkerConfig {
    /// Defaults to the worker's own name, so `[worker.codex]` needs nothing;
    /// a custom worker names the loop it reuses (e.g. `[worker.qwen] harness = "codex"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessKind>,
    /// Model the harness asks for (e.g. "gpt-oss:20b", "qwen2.5-coder:32b").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_volume: Option<String>,
    /// Egress allowlist defaults: the worker's own API domains.
    pub allowed_domains: Vec<String>,
    pub limits: ResourceLimits,
}

/// As written in the file: a list left out is filled with a built-in default,
/// a list written empty stays empty. That distinction is why these are Options
/// here and plain lists in `WorkerConfig`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct RawWorker {
    harness: Option<HarnessKind>,
    model: Option<String>,
    provider: Option<Provider>,
    image: Option<String>,
    auth_volume: Option<String>,
    allowed_domains: Option<Vec<String>>,
    limits: ResourceLimits,
}

impl RawWorker {
    fn into_worker(self, default_domains: &[&str]) -> WorkerConfig {
        WorkerConfig {
            harness: self.harness,
            model: self.model,
            provider: self.provider,
            image: self.image,
            auth_volume: self.auth_volume,
            allowed_domains: self
                .allowed_domains
                .unwrap_or_else(|| default_domains.iter().map(|d| d.to_string()).collect()),
            limits: self.limits,
        }
    }
}

/// A transcript ingestion source: a set of *host* directories read by the
/// parser its `format` names. Worker transcripts living inside Docker auth
/// volumes are not configured — they are derived from each worker's own
/// `auth_volume` and `image` (see `harnesses::ingest_sources`), so the volume
/// name, the image used to reach into it, and the per-harness subdir can never
/// drift out of agreement with the worker they belong to.
///
/// Strict on purpose: an unknown key here — a stray `volume`, a `dir` typo —
/// would otherwise be dropped in silence, and a source that silently ingests
/// nothing is worse than one that refuses to load.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IngestSourceConfig {
    /// Parser format; a custom source names the parser it reuses.
    pub format: String,
    pub dirs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawSource {
    format: Option<String>,
    dirs: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskConfig {
    pub turn_timeout_seconds: NonZeroU64,
}

impl Default for TaskConfig {
    fn default() -> TaskConfig {
        TaskConfig { turn_timeout_seconds: NonZeroU64::new(1800).expect("nonzero") }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EgressConfig {
    pub proxy_image: String,
}

impl Default for EgressConfig {
    fn default() -> EgressConfig {
        EgressConfig { proxy_image: "taskrunner/egress-proxy".into() }
    }
}

/// Interval and sources live under one [ingest] section; sources nest one
/// level deeper so the scalar interval_seconds does not collide with the
/// source catch-all.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IngestConfig {
    /// How often the daemon sweeps transcript sources into the event log.
    pub interval_seconds: NonZeroU64,
    pub sources: IndexMap<String, IngestSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct RawIngest {
    interval_seconds: NonZeroU64,
    sources: IndexMap<String, RawSource>,
}

impl Default for RawIngest {
    fn default() -> RawIngest {
        RawIngest {
            interval_seconds: NonZeroU64::new(300).expect("nonzero"),
            sources: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    pub task: TaskConfig,
    /// Workers are pluggable: any `[worker.<name>]` section needs only a
    /// `harness` key to come alive. The built-in codex and claude entries are
    /// always present, with their images, volumes and API domains defaulted.
    pub worker: IndexMap<String, WorkerConfig>,
    pub egress: EgressConfig,
    pub ingest: IngestConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    task: TaskConfig,
    worker: IndexMap<String, RawWorker>,
    egress: EgressConfig,
    ingest: RawIngest,
}

impl Config {
    /// What an empty config file loads as: every default filled in.
    pub fn default_loaded() -> Config {
        parse_config("").expect("an empty config always parses")
    }
}

/// Parses config text and fills in the built-in workers and sources.
pub fn parse_config(text: &str) -> anyhow::Result<Config> {
    let raw: RawConfig = toml::from_str(text)?;
    for (name, worker) in &raw.worker {
        if worker.limits.cpus <= 0.0 {
            anyhow::bail!("worker.{name}.limits.cpus must be positive");
        }
    }
    Ok(Config {
        task: raw.task,
        worker: with_builtin_workers(raw.worker),
        egress: raw.egress,
        ingest: IngestConfig {
            interval_seconds: raw.ingest.interval_seconds,
            sources: with_builtin_sources(raw.ingest.sources)?,
        },
    })
}

pub fn load_config(path: &Path) -> anyhow::Result<Config> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    parse_config(&text).with_context(|| format!("loading {}", path.display()))
}

/// Worker settings, with defaults for a worker not named in the config.
pub fn worker_config(config: &Config, worker: &str) -> WorkerConfig {
    config.worker.get(worker).cloned().unwrap_or_default()
}

/// The built-in workers first (so they are always present and listed first),
/// then every custom section in file order.
fn with_builtin_workers(
    mut configured: IndexMap<String, RawWorker>,
) -> IndexMap<String, WorkerConfig> {
    let mut builtin = |name: &str, image: &str, volume: &str, domains: &[&str]| {
        let mut worker = configured.shift_remove(name).unwrap_or_default().into_worker(domains);
        worker.image.get_or_insert_with(|| image.to_string());
        worker.auth_volume.get_or_insert_with(|| volume.to_string());
        (name.to_string(), worker)
    };
    let mut workers = IndexMap::from([
        builtin(
            "codex",
            "taskrunner/codex-worker",
            "taskrunner-codex-home",
            &["api.openai.com", "auth.openai.com", "chatgpt.com", "*.chatgpt.com"],
        ),
        // platform.claude.com serves the OAuth token refresh; blocking it
        // strands the worker with 401s once its access token ages out.
        builtin(
            "claude",
            "taskrunner/claude-worker",
            "taskrunner-claude-home",
            &["api.anthropic.com", "*.anthropic.com", "claude.ai", "platform.claude.com"],
        ),
    ]);
    for (name, worker) in configured {
        workers.insert(name, worker.into_worker(&[]));
    }
    workers
}

/// The built-in sources first, then every custom section in file order. A
/// custom source must name its parser format.
fn with_builtin_sources(
    mut configured: IndexMap<String, RawSource>,
) -> anyhow::Result<IndexMap<String, IngestSourceConfig>> {
    let mut builtin = |name: &str, dir: &str| {
        let raw = configured.shift_remove(name).unwrap_or_default();
        let source = IngestSourceConfig {
            format: raw.format.unwrap_or_else(|| name.to_string()),
            dirs: raw.dirs.unwrap_or_else(|| vec![dir.to_string()]),
        };
        (name.to_string(), source)
    };
    let mut sources = IndexMap::from([
        builtin("claude-code", "~/.claude/projects"),
        builtin("codex", "~/.codex/sessions"),
    ]);
    for (name, raw) in configured {
        let Some(format) = raw.format else {
            anyhow::bail!("ingest.sources.{name} needs a format")
        };
        sources.insert(name, IngestSourceConfig { format, dirs: raw.dirs.unwrap_or_default() });
    }
    Ok(sources)
}
