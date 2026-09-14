//! Periodic transcript sweeper. It reads new lines out of each configured
//! source's transcript files (resuming from a persisted byte offset), parses
//! them, and appends a message.recorded for every record it has not seen.
//!
//! Idempotency is enforced BEFORE the append: message ids are deterministic,
//! so a record already in the `messages` table is skipped. The byte offsets
//! are only a performance cache in a sidecar JSON file — deleting it forces a
//! full re-scan that dedupe makes harmless, and the event log stays the sole
//! source of truth for the index.
//!
//! The sweep is synchronous and runs on a blocking thread; the daemon's
//! socket is served elsewhere, so a long backfill never delays it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::parser::{FileContext, TranscriptParser, message_id};
use super::registry::parser_for_format;
use crate::storage::events::EventBody;

/// One transcript source for a given parser format. Either a set of host
/// `dirs` (scanned directly), or a Docker `volume`+`subdir` whose subtree is
/// copied to staging first (worker auth volumes, which the host cannot mount
/// on macOS). `image` names the already-built worker image used as the
/// copy-out vehicle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSource {
    pub format: String,
    pub dirs: Vec<String>,
    pub volume: Option<String>,
    pub subdir: Option<String>,
    pub image: Option<String>,
}

/// Per-file resume state persisted in the sidecar. Field names are the file
/// format: an existing `ingest-state.json` must keep loading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileState {
    /// Byte offset just past the last fully-consumed line.
    offset: usize,
    /// Number of complete lines already consumed (stable codex record ids).
    line_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_path: Option<String>,
}

type SweepState = BTreeMap<String, FileState>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepStats {
    pub files_scanned: usize,
    pub recorded: usize,
    pub errors: usize,
}

/// Where swept messages go. The daemon implements it over its log and index,
/// taking its lock per call so a backfill never holds it for long.
pub trait Archive: Send + Sync {
    fn has_message(&self, message_id: &str) -> anyhow::Result<bool>;
    /// Appends without waiting for fsync; the sweeper calls `flush` once per
    /// sweep, before it persists offsets.
    fn record_unsynced(&self, body: EventBody) -> anyhow::Result<()>;
    /// Forces every recorded event durable. Called before offsets are
    /// persisted, so the sidecar can never claim a record the log has not
    /// durably kept.
    fn flush(&self) -> anyhow::Result<()>;
}

/// Copies `<volume>/<subdir>` to a host dir using `image` as the mount
/// vehicle. Injected so the sweeper stays Docker-agnostic and unit-testable.
pub type CopyVolume = dyn Fn(&str, &str, &str, &Path) -> anyhow::Result<()> + Send + Sync;

/// Diagnostics sink.
pub type LogSink = dyn Fn(&str) + Send + Sync;

pub struct SweeperDeps {
    pub sources: Vec<IngestSource>,
    pub archive: Box<dyn Archive>,
    /// Sidecar path, e.g. <state root>/ingest-state.json.
    pub state_file: PathBuf,
    /// Base dir for volume copy-outs, e.g. <state root>/ingest-staging.
    pub staging_dir: Option<PathBuf>,
    /// Required for volume sources.
    pub copy_volume: Option<Box<CopyVolume>>,
    /// Defaults to stderr.
    pub on_log: Option<Box<LogSink>>,
}

/// Expands a leading ~ to the user's home directory.
pub fn expand_home(dir: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if dir == "~" {
        home()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(dir)
    }
}

pub struct TranscriptSweeper {
    deps: SweeperDeps,
}

impl TranscriptSweeper {
    pub fn new(deps: SweeperDeps) -> TranscriptSweeper {
        TranscriptSweeper { deps }
    }

    /// Runs one sweep. `host_only` restricts to host-directory sources
    /// (skipping the Docker volume copy-out), so a query path can cheaply
    /// refresh live host transcripts without paying for a full worker-volume
    /// sweep. Never fails as a whole: a source that cannot be read is counted
    /// in `errors` and the others are swept.
    pub fn sweep(&self, host_only: bool) -> SweepStats {
        let mut state = self.load_state();
        let mut stats = SweepStats::default();
        let sources =
            self.deps.sources.iter().filter(|source| !host_only || source.volume.is_none());
        for source in sources {
            let Some(parser) = parser_for_format(&source.format) else {
                self.log(&format!("ingest: no parser for format '{}', skipping", source.format));
                continue;
            };
            let dirs = match self.resolve_dirs(source) {
                Ok(dirs) => dirs,
                Err(err) => {
                    // Copy-out failure (Docker down, missing volume): skip this
                    // source, leave the others untouched.
                    stats.errors += 1;
                    self.log(&format!(
                        "ingest: {}: {err}",
                        source.volume.as_deref().unwrap_or(&source.format)
                    ));
                    continue;
                }
            };
            for file in parser.enumerate(&dirs) {
                match self.sweep_file(parser, &source.format, &file, &mut state) {
                    Ok(recorded) => {
                        stats.recorded += recorded;
                        stats.files_scanned += 1;
                    }
                    Err(err) => {
                        stats.errors += 1;
                        self.log(&format!("ingest: error sweeping {}: {err}", file.display()));
                    }
                }
            }
        }
        // Offsets may only ever advance over durably-logged records: a lost log
        // tail combined with an advanced offset would skip those messages
        // forever, since dedupe would never see them again.
        if let Err(err) = self.deps.archive.flush().and_then(|()| self.save_state(&state)) {
            self.log(&format!("ingest: could not persist offsets: {err}"));
        }
        stats
    }

    /// The host directories to scan for a source. A volume source is first
    /// copied out to a per-volume staging dir (whose stable path lets the
    /// offset cache stay incremental across copies); a host source scans its
    /// dirs directly. Fails if a volume source cannot be resolved.
    fn resolve_dirs(&self, source: &IngestSource) -> anyhow::Result<Vec<PathBuf>> {
        let Some(volume) = &source.volume else {
            return Ok(source.dirs.iter().map(|dir| expand_home(dir)).collect());
        };
        let (Some(copy_volume), Some(staging_dir)) =
            (&self.deps.copy_volume, &self.deps.staging_dir)
        else {
            anyhow::bail!("volume source configured but no copy-out available");
        };
        // An empty subdir would copy the volume *root*, and a worker auth volume
        // holds that worker's credentials. Refuse rather than stage secrets.
        let subdir = source.subdir.as_deref().filter(|s| !s.is_empty());
        let Some(subdir) = subdir else { anyhow::bail!("volume source {volume} has no subdir") };
        let Some(image) = source.image.as_deref().filter(|s| !s.is_empty()) else {
            anyhow::bail!("volume source {volume} has no image")
        };
        let dest = staging_dir.join(volume);
        copy_volume(volume, subdir, image, &dest)?;
        Ok(vec![dest])
    }

    /// Reads new complete lines from one file and records unseen messages.
    fn sweep_file(
        &self,
        parser: &dyn TranscriptParser,
        format: &str,
        file: &Path,
        state: &mut SweepState,
    ) -> anyhow::Result<usize> {
        let bytes = fs::read(file)?;
        let key = file.to_string_lossy().into_owned();
        let mut resume = state.get(&key).cloned().unwrap_or_default();
        // Shorter than where we left off means the file was truncated or
        // rewritten; re-read from the top. Dedupe keeps that harmless.
        if bytes.len() < resume.offset {
            resume = FileState::default();
        }
        let mut ctx = FileContext {
            file_path: file.to_path_buf(),
            line_index: resume.line_index,
            session_id: resume.session_id.filter(|s| !s.is_empty()),
            project_path: resume.project_path.filter(|p| !p.is_empty()),
        };

        let mut pos = resume.offset;
        let mut recorded = 0;
        // A trailing partial line is left for the next sweep.
        while let Some(newline) = bytes[pos..].iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&bytes[pos..pos + newline]);
            for msg in parser.parse(&line, &mut ctx) {
                let id = message_id(format, &msg.native_session_id, &msg.native_record_id);
                if self.deps.archive.has_message(&id)? {
                    continue;
                }
                self.deps.archive.record_unsynced(EventBody::MessageRecorded {
                    message_id: id,
                    source: format.to_string(),
                    native_session_id: msg.native_session_id,
                    native_record_id: msg.native_record_id,
                    role: msg.role,
                    kind: msg.kind,
                    content: msg.content,
                    native_ts: msg.native_ts.filter(|t| !t.is_empty()),
                    project_path: msg.project_path.filter(|p| !p.is_empty()),
                })?;
                recorded += 1;
            }
            pos += newline + 1;
            ctx.line_index += 1;
        }

        state.insert(
            key,
            FileState {
                offset: pos,
                line_index: ctx.line_index,
                session_id: ctx.session_id.filter(|s| !s.is_empty()),
                project_path: ctx.project_path.filter(|p| !p.is_empty()),
            },
        );
        Ok(recorded)
    }

    fn load_state(&self) -> SweepState {
        // Missing or unparseable: start fresh. A full re-scan is dedupe-safe.
        fs::read(&self.deps.state_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, state: &SweepState) -> anyhow::Result<()> {
        let tmp = self.deps.state_file.with_extension("json.tmp");
        if let Some(dir) = self.deps.state_file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&tmp, serde_json::to_vec(state)?)?;
        fs::rename(&tmp, &self.deps.state_file)?;
        Ok(())
    }

    fn log(&self, message: &str) {
        match &self.deps.on_log {
            Some(sink) => sink(message),
            None => eprintln!("{message}"),
        }
    }
}
