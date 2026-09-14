//! Record ids are prefixed lowercase ULIDs.

use ulid::Ulid;

/// The kinds of record that get an id, so a typo can't mint an unknown prefix.
#[derive(Debug, Clone, Copy)]
pub enum IdPrefix {
    Project,
    Session,
    Task,
    Turn,
    WorkerSession,
    Event,
    Artifact,
    Approval,
}

impl IdPrefix {
    fn as_str(self) -> &'static str {
        match self {
            IdPrefix::Project => "proj",
            IdPrefix::Session => "sess",
            IdPrefix::Task => "task",
            IdPrefix::Turn => "turn",
            IdPrefix::WorkerSession => "wsess",
            IdPrefix::Event => "evt",
            IdPrefix::Artifact => "art",
            IdPrefix::Approval => "appr",
        }
    }
}

pub fn new_id(prefix: IdPrefix) -> String {
    format!("{}_{}", prefix.as_str(), Ulid::new().to_string().to_lowercase())
}
