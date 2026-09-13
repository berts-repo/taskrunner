//! The event log is the only write path; the SQLite index is folded from it.

pub mod artifacts;
pub mod events;
pub mod facts;
pub mod index;
pub mod store;

use events::{EventBody, LogEvent};

/// The single durable write path: append to the log, fold into the index.
/// The daemon implements it; tests implement it over an in-memory stack.
pub trait Recorder {
    fn record(&self, body: EventBody) -> anyhow::Result<LogEvent>;
}
