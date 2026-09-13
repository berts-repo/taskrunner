//! Sweeps host transcript directories into a fresh event log: the Rust half
//! of scripts/parity-sweep.sh. Usage: sweep <out-dir> <claude-dir> <codex-dir>

use std::path::PathBuf;
use std::sync::Mutex;

use taskrunner::ingest::sweep::{Archive, IngestSource, SweeperDeps, TranscriptSweeper};
use taskrunner::storage::events::{EventBody, EventLog};
use taskrunner::storage::index::{IN_MEMORY, StateIndex};

struct FreshArchive {
    log: Mutex<EventLog>,
    index: Mutex<StateIndex>,
}

impl Archive for FreshArchive {
    fn has_message(&self, id: &str) -> anyhow::Result<bool> {
        let n: i64 = self.index.lock().unwrap().db.query_row(
            "SELECT COUNT(*) FROM messages WHERE id = ?",
            [id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    fn record_unsynced(&self, body: EventBody) -> anyhow::Result<()> {
        let event = self.log.lock().unwrap().append_unsynced(body)?;
        self.index.lock().unwrap().apply(&event)?;
        Ok(())
    }

    fn flush(&self) -> anyhow::Result<()> {
        Ok(self.log.lock().unwrap().flush()?)
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, out_dir, claude_dir, codex_dir] = args.as_slice() else {
        anyhow::bail!("usage: sweep <out-dir> <claude-dir> <codex-dir>");
    };
    let out = PathBuf::from(out_dir);
    let source = |format: &str, dir: &str| IngestSource {
        format: format.into(),
        dirs: vec![dir.into()],
        ..Default::default()
    };
    let sweeper = TranscriptSweeper::new(SweeperDeps {
        sources: vec![source("claude-code", claude_dir), source("codex", codex_dir)],
        archive: Box::new(FreshArchive {
            log: Mutex::new(EventLog::open(&out.join("events.jsonl"))?),
            index: Mutex::new(StateIndex::open(IN_MEMORY)?),
        }),
        state_file: out.join("ingest-state.json"),
        staging_dir: None,
        copy_volume: None,
        on_log: None,
    });
    let stats = sweeper.sweep(false);
    println!(
        "swept {} files, {} messages, {} errors",
        stats.files_scanned, stats.recorded, stats.errors
    );
    Ok(())
}
