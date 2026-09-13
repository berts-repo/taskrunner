//! Folds an event log into a fresh index file. The Rust half of
//! scripts/parity-index.sh. Usage: cargo run --example refold -- <events.jsonl> <out.db>

use std::path::Path;

use taskrunner::storage::events::read_events;
use taskrunner::storage::index::rebuild_index;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, log, out] = args.as_slice() else {
        anyhow::bail!("usage: refold <events.jsonl> <out.db>");
    };
    let events = read_events(Path::new(log))?;
    rebuild_index(out, &events)?;
    println!("refold clean ({} events)", events.len());
    Ok(())
}
