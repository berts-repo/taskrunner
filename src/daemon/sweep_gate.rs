//! Runs sweeps on a blocking thread, one at a time. A caller that arrives
//! while a sweep is in flight waits for that sweep and gets its result:
//! first-caller-wins coalescing, so a burst of session lookups never queues
//! up a burst of sweeps.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::ingest::sweep::{SweepStats, TranscriptSweeper};

#[derive(Clone)]
pub struct SweepGate {
    sweeper: Arc<TranscriptSweeper>,
    in_flight: Arc<Mutex<Option<watch::Receiver<Option<SweepStats>>>>>,
}

impl SweepGate {
    pub fn new(sweeper: TranscriptSweeper) -> SweepGate {
        SweepGate { sweeper: Arc::new(sweeper), in_flight: Arc::new(Mutex::new(None)) }
    }

    /// One sweep, or the one already running. An in-flight host-only sweep
    /// can absorb a concurrent full request for that tick — harmless, since
    /// the interval sweep repeats and reaches the volumes on its next run.
    pub async fn sweep(&self, host_only: bool) -> SweepStats {
        let mut rx = {
            let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(rx) = in_flight.as_ref() {
                rx.clone()
            } else {
                let (tx, rx) = watch::channel(None);
                *in_flight = Some(rx.clone());
                let sweeper = self.sweeper.clone();
                let gate = self.in_flight.clone();
                tokio::task::spawn_blocking(move || {
                    let stats = sweeper.sweep(host_only);
                    *gate.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    let _ = tx.send(Some(stats));
                });
                rx
            }
        };
        // The sender is dropped when the sweep thread finishes, so this
        // resolves either way; a panicked sweep counts as an empty one.
        loop {
            if let Some(stats) = rx.borrow().clone() {
                return stats;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone().unwrap_or_default();
            }
        }
    }

    /// Resolves once any in-flight sweep has finished (for daemon shutdown).
    pub async fn settle(&self) {
        let rx = self.in_flight.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(mut rx) = rx {
            while rx.borrow().is_none() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}
