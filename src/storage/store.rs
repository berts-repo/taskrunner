//! The log and the index behind one lock: every write appends to the log and
//! folds into the index in that order, and nothing else writes either.

use std::sync::{Arc, Mutex, MutexGuard};

use super::Recorder;
use super::events::{EventBody, EventLog, LogEvent};
use super::index::StateIndex;
use crate::ingest::sweep::Archive;

pub struct Store {
    pub log: EventLog,
    pub index: StateIndex,
}

impl Store {
    /// The single durable write path: append to the log, fold into the index.
    pub fn record(&mut self, body: EventBody) -> anyhow::Result<LogEvent> {
        let event = self.log.append(body)?;
        self.index.apply(&event)?;
        Ok(event)
    }

    /// Bulk write path for reconstructible events (see `EventLog::append_unsynced`).
    pub fn record_unsynced(&mut self, body: EventBody) -> anyhow::Result<LogEvent> {
        let event = self.log.append_unsynced(body)?;
        self.index.apply(&event)?;
        Ok(event)
    }
}

/// A handle every part of the daemon holds. The lock is taken per call and
/// released at once — a sweep locks per message, a query per statement — so
/// a long backfill never keeps the socket waiting.
#[derive(Clone)]
pub struct SharedStore(Arc<Mutex<Store>>);

impl SharedStore {
    pub fn new(log: EventLog, index: StateIndex) -> SharedStore {
        SharedStore(Arc::new(Mutex::new(Store { log, index })))
    }

    pub fn lock(&self) -> MutexGuard<'_, Store> {
        // A poisoned lock means a panic mid-write; there is no sane way to
        // continue serving, and the log repairs its tail on the next start.
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Recorder for SharedStore {
    fn record(&self, body: EventBody) -> anyhow::Result<LogEvent> {
        self.lock().record(body)
    }
}

impl Archive for SharedStore {
    fn has_message(&self, message_id: &str) -> anyhow::Result<bool> {
        let store = self.lock();
        let n: i64 = store.index.db.query_row(
            "SELECT COUNT(*) FROM messages WHERE id = ?",
            [message_id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    fn record_unsynced(&self, body: EventBody) -> anyhow::Result<()> {
        self.lock().record_unsynced(body).map(|_| ())
    }

    fn flush(&self) -> anyhow::Result<()> {
        Ok(self.lock().log.flush()?)
    }
}
