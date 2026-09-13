//! Building an in-memory index from event bodies, with a per-test clock.

use std::cell::Cell;

use taskrunner::storage::events::{EventBody, LogEvent};
use taskrunner::storage::index::{IN_MEMORY, StateIndex, rebuild_index};

/// A monotonic counter each test module turns into timestamps and record ids.
pub struct Clock(Cell<u64>);

impl Clock {
    pub fn new() -> Clock {
        Clock(Cell::new(0))
    }

    pub fn tick(&self) -> u64 {
        self.0.set(self.0.get() + 1);
        self.0.get()
    }

    pub fn now(&self) -> u64 {
        self.0.get()
    }
}

/// Stamps ids and the given timestamps onto bodies and folds them into a
/// fresh in-memory index.
pub fn index_of(bodies: Vec<EventBody>, ts: impl Fn() -> String) -> StateIndex {
    let events: Vec<LogEvent> = bodies
        .into_iter()
        .enumerate()
        .map(|(n, body)| LogEvent { id: format!("evt-{n}"), ts: ts(), body })
        .collect();
    rebuild_index(IN_MEMORY, &events).unwrap()
}

pub fn project_created() -> EventBody {
    EventBody::ProjectCreated { project_id: "p1".into(), root: "/repo".into() }
}

/// The fields a seeded message.recorded varies; the rest are fixed.
pub struct Msg<'a> {
    pub source: &'a str,
    pub session: &'a str,
    pub record_id: String,
    pub role: &'a str,
    pub kind: &'a str,
    pub content: String,
    pub native_ts: Option<String>,
    pub project_path: Option<&'a str>,
}

impl Msg<'_> {
    pub fn body(self) -> EventBody {
        EventBody::MessageRecorded {
            message_id: format!("msg:{}:{}:{}", self.source, self.session, self.record_id),
            source: self.source.into(),
            native_session_id: self.session.into(),
            native_record_id: self.record_id,
            role: self.role.into(),
            kind: self.kind.into(),
            content: self.content,
            native_ts: self.native_ts,
            project_path: self.project_path.map(str::to_string),
        }
    }
}
