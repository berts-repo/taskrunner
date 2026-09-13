//! Worker tests: one module per TypeScript test file.

#[path = "../helpers/mod.rs"]
mod helpers;

mod claude;
mod codex;
mod docker;
mod runner;

use std::sync::{Arc, Mutex};

use taskrunner::workers::harness::{OnEvent, WorkerEvent};

/// Collects the events a harness streams.
pub fn collect() -> (Arc<Mutex<Vec<WorkerEvent>>>, Box<OnEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    (events, Box::new(move |event| sink.lock().unwrap().push(event)))
}

pub fn kinds(events: &Arc<Mutex<Vec<WorkerEvent>>>) -> Vec<String> {
    events.lock().unwrap().iter().map(|e| e.kind.clone()).collect()
}
