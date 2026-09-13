//! The event log is the only write path; the SQLite index is folded from it.

pub mod artifacts;
pub mod events;
pub mod facts;
pub mod index;
