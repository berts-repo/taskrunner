//! One MCP service per connected client. A connection is a session: the
//! `session.started` event is recorded when the client's `initialize`
//! completes (or on its first request, whichever comes first) and
//! `session.ended` when the connection closes. Every tool call is audited
//! as `tool.<name>` with its arguments, then dispatched to `tools::call`.
//!
//! The service also serves taskrunner's skills (SEP-2640): `skills/list` and
//! `skills/get` as custom methods, and each skill file as a resource, rendered
//! for the session's host. Skills requests are audited like tool calls — that
//! record is how `taskrunner sync` sees a harness getting skills this way.

use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomRequest,
    CustomResult, ErrorCode, ExtensionCapabilities, JsonObject, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, model::Implementation};
use serde_json::{Value, json};

use super::Daemon;
use super::tools::{ToolCallError, ToolTable};
use crate::config::{Config, HostKind};
use crate::ids::{IdPrefix, new_id};
use crate::skills::{self, Skill};
use crate::storage::Recorder;
use crate::storage::events::EventBody;
use crate::storage::store::SharedStore;

pub const VERSION: &str = "0.1.0";

const MARKDOWN: &str = "text/markdown";

/// Server-level cheatsheet delivered to every client in the MCP handshake.
/// Generated from config so the advertised workers and their egress defaults
/// can never drift from what the daemon actually runs. Only some harnesses
/// show it to the model (Claude Code does, Codex and Hermes don't), so rules
/// that matter everywhere live in the tool descriptions, and procedures live
/// in the skills.
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
         an existing task; cancel-task stops a running turn. The delegate-task skill has the \
         whole routine."
            .to_string(),
        String::new(),
        "Transcripts: worker turns and host agent sessions are archived and searchable with \
         search-transcripts and lookup-session. Find first, then read one exchange at a time; \
         the archive-search skill has the routine."
            .to_string(),
        String::new(),
        "Worker credentials live in Docker volumes on this host. If a turn fails with a \
         login or auth error, the user must sign that worker in again on the host (the \
         worker-login skill has the steps); it cannot be fixed through these tools."
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
    pub fn ensure_started(
        &self,
        store: &SharedStore,
        client: Option<String>,
        host: Option<HostKind>,
    ) {
        let mut started = self.started.lock().unwrap_or_else(|p| p.into_inner());
        if *started {
            return;
        }
        *started = true;
        let _ = store.record(EventBody::SessionStarted {
            session_id: self.session_id.clone(),
            project_id: None,
            client,
            host: host.map(|h| h.as_str().to_string()),
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
    pub daemon: Daemon,
    pub tools: Arc<ToolTable>,
    pub session: Arc<SessionRecord>,
    /// Which harness this connection is, when its registration said so.
    pub host: Option<HostKind>,
}

impl McpService {
    /// Records session.started once, from the handshake's client name.
    fn ensure_started(&self, peer: &rmcp::service::Peer<RoleServer>) {
        let client = peer.peer_info().map(|info| info.client_info.name.clone());
        self.session.ensure_started(&self.daemon.store, client, self.host);
    }

    fn audit(&self, kind: &str, payload: Value) {
        let _ = self.daemon.store.record(EventBody::AuditRecorded {
            session_id: Some(self.session.session_id.clone()),
            task_id: None,
            turn_id: None,
            kind: kind.to_string(),
            payload,
        });
    }

    /// Taskrunner's skills as this session's host should see them.
    fn skills(&self) -> Vec<Skill> {
        skills::render(self.daemon.config.delegation(self.host))
    }

    /// An unknown skill URI is invalid params (-32602), as SEP-2640 requires.
    fn skill_at(&self, uri: &str) -> Result<Skill, ErrorData> {
        self.skills()
            .into_iter()
            .find(|skill| skill.uri() == uri)
            .ok_or_else(|| ErrorData::invalid_params(format!("no skill at {uri}"), None))
    }
}

impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        let skills_extension =
            ExtensionCapabilities::from([(skills::EXTENSION.to_string(), JsonObject::new())]);
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_extensions_with(skills_extension)
            .build();
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("taskrunner", VERSION))
            .with_instructions(build_instructions(&self.daemon.config))
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.ensure_started(&context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // The initialized notification and the first tool call arrive
        // separately; whichever lands first opens the session record.
        self.ensure_started(&context.peer);
        let args = request.arguments.unwrap_or_default();
        // Invalid arguments are a protocol error: the tool never runs and
        // nothing is audited.
        let args = match self.tools.check(&request.name, &args) {
            Ok(args) => args,
            Err(ToolCallError::UnknownTool) => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool {}", request.name),
                    None,
                ));
            }
            Err(ToolCallError::InvalidArguments(detail)) => {
                return Err(ErrorData::invalid_params(
                    format!("invalid arguments for {}: {detail}", request.name),
                    None,
                ));
            }
        };
        self.audit(&format!("tool.{}", request.name), Value::Object(args.clone()));
        let text =
            match super::tools::call(&self.daemon, &self.session.session_id, &request.name, &args)
                .await
            {
                Ok(text) => {
                    return Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into());
                }
                Err(err) => err.to_string(),
            };
        Ok(CallToolResult::error(vec![ContentBlock::text(text)]).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.ensure_started(&context.peer);
        self.audit("resources.list", json!({}));
        let resources = self
            .skills()
            .into_iter()
            .map(|skill| {
                Resource::new(skill.uri(), skill.name.clone())
                    .with_description(skill.description.clone())
                    .with_mime_type(MARKDOWN)
                    .with_size(skill.text.len() as u64)
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.ensure_started(&context.peer);
        let skill = self.skill_at(&request.uri)?;
        self.audit("resource.read", json!({ "uri": request.uri }));
        let contents = ResourceContents::text(skill.text, request.uri).with_mime_type(MARKDOWN);
        Ok(ReadResourceResult::new(vec![contents]).into())
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        self.ensure_started(&context.peer);
        match request.method.as_str() {
            "skills/list" => {
                self.audit("skills.list", json!({}));
                let entries: Vec<Value> = self.skills().iter().map(Skill::entry).collect();
                Ok(CustomResult::new(json!({ "resultType": "complete", "skills": entries })))
            }
            "skills/get" => {
                let uri =
                    request.params.as_ref().and_then(|p| p.get("uri")).and_then(Value::as_str);
                let skill = self.skill_at(uri.unwrap_or_default())?;
                self.audit("skills.get", json!({ "uri": skill.uri() }));
                Ok(CustomResult::new(json!({ "resultType": "complete", "skill": skill.entry() })))
            }
            _ => Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND, request.method, None)),
        }
    }
}
