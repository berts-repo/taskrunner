//! One MCP service per connected client. A connection is a session: the
//! `session.started` event is recorded when the client's `initialize`
//! completes and `session.ended` when the connection closes. Tools arrive in
//! step 7 of the port.

use std::sync::Mutex;

use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::service::{NotificationContext, RoleServer};
use rmcp::{ServerHandler, model::Implementation};

use crate::config::Config;
use crate::ids::{IdPrefix, new_id};
use crate::storage::Recorder;
use crate::storage::events::EventBody;
use crate::storage::store::SharedStore;

pub const VERSION: &str = "0.1.0";

/// Server-level cheatsheet delivered to every client in the MCP handshake.
/// Generated from config so the advertised workers and their egress defaults
/// can never drift from what the daemon actually runs.
pub fn build_instructions(config: &Config) -> String {
    let workers = config.worker.iter().map(|(name, cfg)| {
        let model = cfg.model.as_deref().map_or(String::new(), |m| format!(" (model: {m})"));
        let domains = if cfg.allowed_domains.is_empty() {
            "none".to_string()
        } else {
            cfg.allowed_domains.join(", ")
        };
        format!("- {name}{model} — default egress: {domains}")
    });
    let mut lines = vec![
        "Taskrunner runs delegated coding tasks in isolated Docker containers, one workspace per task.".to_string(),
        String::new(),
        "Configured workers (the `worker` argument to assign-task):".to_string(),
    ];
    lines.extend(workers);
    lines.extend([
        String::new(),
        "Network access: a worker reaches only its default egress domains above. To grant more, \
         pass allowDomains on assign-task; the value \"*\" means the entire public internet. \
         Loopback, LAN, and other private addresses stay blocked regardless. Any allowDomains \
         value requires the user's explicit yes in conversation, relayed via userApproved: true. \
         Every connection attempt a worker makes is audit-logged."
            .to_string(),
        String::new(),
        "Lifecycle: assign-task starts a task (wait: true blocks for the result); lookup-task \
         fetches status, output, and audit records; continue-task sends a follow-up prompt to \
         an existing task; cancel-task stops a running turn."
            .to_string(),
        String::new(),
        "Transcripts: worker turns and host agent sessions are archived, and searchable. \
         Work them in two steps — find, then drill — instead of reading a session whole:\n  \
         1. find. search-transcripts searches the corpus by text, and/or by structured \
         filters: tool (which tool was called), target (the path or command it acted on), \
         failed (whether it errored). Scope with project, sessions, lastSessions, since/until. \
         Or list sessions with lookup-session, then read one as an outline — one line per \
         prompt, reply and tool call, each exchange addressed [N].\n  \
         2. drill. Every hit and outline entry carries that address; pass prompt N to \
         lookup-session (or lookup-task) to read that one exchange in full. Reach for \
         view \"timeline\" on a whole session only when you truly need all of it."
            .to_string(),
        String::new(),
        "Worker credentials live in Docker volumes on this host. If a turn fails with a \
         login or auth error, the user must re-run the worker login procedure on the host \
         (documented in the taskrunner README); it cannot be fixed through these tools."
            .to_string(),
    ]);
    lines.join("\n")
}

/// The durable session record behind one connection.
pub struct SessionRecord {
    pub session_id: String,
    started: Mutex<bool>,
}

impl SessionRecord {
    pub fn new() -> SessionRecord {
        SessionRecord { session_id: new_id(IdPrefix::Session), started: Mutex::new(false) }
    }

    /// Records session.started once; safe to call on every dispatch.
    pub fn ensure_started(&self, store: &SharedStore, client: Option<String>) {
        let mut started = self.started.lock().unwrap_or_else(|p| p.into_inner());
        if *started {
            return;
        }
        *started = true;
        let _ = store.record(EventBody::SessionStarted {
            session_id: self.session_id.clone(),
            project_id: None,
            client,
        });
    }

    /// Records session.ended, if the session ever started.
    pub fn end(&self, store: &SharedStore) {
        if *self.started.lock().unwrap_or_else(|p| p.into_inner()) {
            let _ = store.record(EventBody::SessionEnded { session_id: self.session_id.clone() });
        }
    }
}

impl Default for SessionRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct McpService {
    pub store: SharedStore,
    pub instructions: String,
    pub session: std::sync::Arc<SessionRecord>,
}

impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("taskrunner", VERSION))
            .with_instructions(self.instructions.clone())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let client = context.peer.peer_info().map(|info| info.client_info.name.clone());
        self.session.ensure_started(&self.store, client);
    }
}
