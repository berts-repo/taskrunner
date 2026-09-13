//! The harness table: everything that keys on a harness *kind* (codex, claude)
//! rather than on a worker's name, so config-only workers inherit their loop's
//! layout. Adding a harness is one row in each table here.

use std::collections::HashSet;

use crate::config::{Config, HarnessKind, worker_config};
use crate::ingest::sweep::IngestSource;

/// One mount of (a subpath of) the worker's auth volume into its container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMount {
    /// Where the (sub)volume lands inside the container.
    pub container_path: &'static str,
    /// Path inside the auth volume to mount; None mounts the volume root.
    pub subpath: Option<&'static str>,
    pub read_only: bool,
}

/// The kind a worker runs as: its `harness` key, else its own name. None for
/// a name that is not a harness kind and names none.
pub fn worker_kind(config: &Config, worker: &str) -> Option<HarnessKind> {
    worker_config(config, worker).harness.or_else(|| HarnessKind::parse(worker))
}

/// Every worker name the daemon knows: the built-ins plus each config section.
pub fn worker_names(config: &Config) -> Vec<String> {
    let mut seen = HashSet::new();
    ["codex", "claude"]
        .into_iter()
        .map(str::to_string)
        .chain(config.worker.keys().cloned())
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

/// Container mounts for each harness kind's auth material.
pub fn auth_mounts(kind: HarnessKind) -> Vec<AuthMount> {
    match kind {
        // codex keeps everything under ~/.codex.
        HarnessKind::Codex => vec![AuthMount {
            container_path: "/home/worker/.codex",
            subpath: None,
            read_only: false,
        }],
        // claude spreads login state across the home directory, but only these
        // two paths carry it: ~/.claude (credentials, sessions, settings) and
        // ~/.claude.json. Subpath mounts expose exactly those, so a task cannot
        // persist shell rc files, ~/.config, or ~/.npmrc into the volume for a
        // later turn to execute. The login flow still mounts the volume root at
        // /home/worker, which keeps both paths present (see README § Worker
        // sign-in).
        HarnessKind::Claude => vec![
            AuthMount {
                container_path: "/home/worker/.claude",
                subpath: Some(".claude"),
                read_only: false,
            },
            AuthMount {
                container_path: "/home/worker/.claude.json",
                subpath: Some(".claude.json"),
                read_only: false,
            },
        ],
    }
}

/// Image fallback so config-only workers need no image key.
pub fn default_image(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Codex => "taskrunner/codex-worker",
        HarnessKind::Claude => "taskrunner/claude-worker",
    }
}

/// Where each harness kind keeps its native transcripts inside its auth
/// volume, and which parser format reads them. Verified against
/// `auth_mounts`: the codex volume root is ~/.codex, the claude volume root is
/// the worker home.
pub fn transcript_layout(kind: HarnessKind) -> (&'static str, &'static str) {
    match kind {
        HarnessKind::Codex => ("sessions", "codex"),
        HarnessKind::Claude => (".claude/projects", "claude-code"),
    }
}

/// Resolves the transcript sources the sweeper ingests: the configured host
/// sources, plus one derived volume source per worker that has an auth volume
/// and a known transcript layout.
///
/// Volume sources are derived rather than configured. Reaching into a volume
/// needs a container to mount it in, and the only image guaranteed to exist
/// for a given auth volume is the worker's own — so taking the volume, the
/// image and the subdir from one worker entry keeps them in agreement by
/// construction, and leaves nothing for a config file to get wrong.
pub fn ingest_sources(config: &Config) -> Vec<IngestSource> {
    let mut sources: Vec<IngestSource> = config
        .ingest
        .sources
        .values()
        .map(|source| IngestSource {
            format: source.format.clone().unwrap_or_default(),
            dirs: source.dirs.clone(),
            ..Default::default()
        })
        .collect();
    let mut seen_volumes = HashSet::new();
    for name in worker_names(config) {
        let cfg = worker_config(config, &name);
        let Some(kind) = worker_kind(config, &name) else { continue };
        let Some(volume) = cfg.auth_volume else { continue };
        // Two workers may share an auth volume (same harness, different
        // model); ingesting it once is enough.
        if !seen_volumes.insert(volume.clone()) {
            continue;
        }
        let (subdir, format) = transcript_layout(kind);
        sources.push(IngestSource {
            format: format.into(),
            volume: Some(volume),
            subdir: Some(subdir.into()),
            image: Some(cfg.image.unwrap_or_else(|| default_image(kind).into())),
            ..Default::default()
        });
    }
    sources
}
