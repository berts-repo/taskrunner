//! The six MCP tools. Their names, descriptions and argument schemas are the
//! frozen contract with Claude Code, so they are data — `tools.json`, captured
//! from the original server's `tools/list` output — served verbatim and used to
//! validate every call. Nothing here can drift from what the client sees.

use std::sync::Arc;

use jsonschema::Validator;
use rmcp::model::{JsonObject, Tool};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::Daemon;
use super::scheduler::AssignArgs;
use crate::domain::errors::ToolError;
use crate::domain::tasks::{SearchFilters, SearchSort};
use crate::view::lookup::{
    Include, LookupArgs, LookupDeps, Scope, SessionLookupArgs, ViewArgs, lookup_session,
    lookup_task, search_transcripts,
};
use crate::view::render::{render_cancel, render_outcome};
use crate::view::transcript::TranscriptView;

const TOOLS_JSON: &str = include_str!("tools.json");

pub struct ToolTable {
    pub tools: Vec<Tool>,
    validators: Vec<(String, Validator, Arc<JsonObject>)>,
}

impl ToolTable {
    pub fn load() -> ToolTable {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Entry {
            name: String,
            description: String,
            input_schema: JsonObject,
        }
        let entries: Vec<Entry> = serde_json::from_str(TOOLS_JSON).expect("tools.json is valid");
        let mut tools = Vec::new();
        let mut validators = Vec::new();
        for entry in entries {
            let schema = Arc::new(entry.input_schema);
            let validator = jsonschema::validator_for(&Value::Object((*schema).clone()))
                .expect("tool schema compiles");
            tools.push(Tool::new(entry.name.clone(), entry.description, schema.clone()));
            validators.push((entry.name, validator, schema));
        }
        ToolTable { tools, validators }
    }

    /// Checks a call's arguments against the tool's schema and returns them
    /// in the schema's property order, so audit records keep the argument
    /// order the archive's earlier records have.
    pub fn check(&self, name: &str, args: &JsonObject) -> Result<JsonObject, ToolCallError> {
        let Some((_, validator, schema)) = self.validators.iter().find(|(n, ..)| n == name) else {
            return Err(ToolCallError::UnknownTool);
        };
        let instance = Value::Object(args.clone());
        if let Err(err) = validator.validate(&instance) {
            return Err(ToolCallError::InvalidArguments(format!("{}: {err}", err.instance_path)));
        }
        Ok(in_schema_order(args, schema))
    }
}

pub enum ToolCallError {
    UnknownTool,
    InvalidArguments(String),
}

fn in_schema_order(args: &JsonObject, schema: &JsonObject) -> JsonObject {
    let Some(Value::Object(properties)) = schema.get("properties") else { return args.clone() };
    let mut ordered = Map::new();
    for (key, property) in properties {
        let Some(value) = args.get(key) else { continue };
        let value = match (value, property) {
            (Value::Object(nested), Value::Object(nested_schema)) => {
                Value::Object(in_schema_order(nested, nested_schema))
            }
            (value, _) => value.clone(),
        };
        ordered.insert(key.clone(), value);
    }
    ordered
}

// ---- argument shapes, one per tool ----------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewFields {
    view: Option<TranscriptView>,
    tool_lines: Option<usize>,
    /// The wire name `prompt`, mapped onto the renderer's `prompt_idx`
    /// because `prompt` already means the worker instruction on assign-task.
    prompt: Option<i64>,
}

impl ViewFields {
    fn args(&self) -> ViewArgs {
        ViewArgs { view: self.view, tool_lines: self.tool_lines, prompt_idx: self.prompt }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignTask {
    project: String,
    worker: String,
    prompt: String,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    allow_domains: Vec<String>,
    #[serde(default)]
    user_approved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueTask {
    task_id: String,
    prompt: String,
    #[serde(default)]
    wait: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeFields {
    turn_id: Option<String>,
    last: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupTask {
    task_id: Option<String>,
    project: Option<String>,
    #[serde(default)]
    include: Vec<String>,
    scope: Option<ScopeFields>,
    limit: Option<i64>,
    #[serde(flatten)]
    view: ViewFields,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupSession {
    session_id: Option<String>,
    project: Option<String>,
    source: Option<String>,
    limit: Option<i64>,
    scope: Option<ScopeFields>,
    #[serde(flatten)]
    view: ViewFields,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchTranscripts {
    query: Option<String>,
    tool: Option<String>,
    target: Option<String>,
    failed: Option<bool>,
    project: Option<String>,
    sessions: Option<Vec<String>>,
    last_sessions: Option<i64>,
    since: Option<String>,
    until: Option<String>,
    role: Option<String>,
    kind: Option<String>,
    sort: Option<SearchSort>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelTask {
    task_id: String,
    reason: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de>>(args: &JsonObject) -> Result<T, ToolError> {
    serde_json::from_value(Value::Object(args.clone()))
        .map_err(|err| ToolError::invalid_request(err.to_string()))
}

/// Runs one validated tool call and renders its text.
pub async fn call(
    daemon: &Daemon,
    session_id: &str,
    name: &str,
    args: &JsonObject,
) -> Result<String, ToolError> {
    match name {
        "assign-task" => {
            let a: AssignTask = parse(args)?;
            let outcome = daemon
                .scheduler
                .assign_task(AssignArgs {
                    project: a.project,
                    worker: a.worker,
                    prompt: a.prompt,
                    session_id: Some(session_id.into()),
                    wait: a.wait,
                    allow_domains: a.allow_domains,
                    user_approved: a.user_approved,
                })
                .await?;
            Ok(render_outcome(&outcome))
        }
        "continue-task" => {
            let a: ContinueTask = parse(args)?;
            Ok(render_outcome(
                &daemon.scheduler.continue_task(&a.task_id, &a.prompt, a.wait).await?,
            ))
        }
        "lookup-task" => {
            let a: LookupTask = parse(args)?;
            let store = daemon.store.lock();
            lookup_task(
                &LookupDeps { index: &store.index, artifacts: &daemon.artifacts },
                &LookupArgs {
                    task_id: a.task_id,
                    project: a.project,
                    include: a.include.iter().filter_map(|i| Include::parse(i)).collect(),
                    scope: a.scope.map(|s| Scope { turn_id: s.turn_id, last: s.last }),
                    limit: a.limit,
                    view: a.view.args(),
                },
            )
        }
        "lookup-session" => {
            let a: LookupSession = parse(args)?;
            daemon.sweep_host_transcripts().await;
            let store = daemon.store.lock();
            lookup_session(
                &store.index,
                &SessionLookupArgs {
                    session_id: a.session_id,
                    project: a.project,
                    source: a.source,
                    limit: a.limit,
                    last: a.scope.and_then(|s| s.last),
                    view: a.view.args(),
                },
            )
        }
        "search-transcripts" => {
            let a: SearchTranscripts = parse(args)?;
            let filters = SearchFilters {
                project: a.project,
                sessions: a.sessions,
                last_sessions: a.last_sessions,
                since: a.since,
                until: a.until,
                role: a.role,
                kind: a.kind,
                tool: a.tool,
                target: a.target,
                failed: a.failed,
                sort: a.sort.unwrap_or_default(),
            };
            // lastSessions ranks by recency, so refresh host sessions first.
            if filters.last_sessions.is_some() {
                daemon.sweep_host_transcripts().await;
            }
            let store = daemon.store.lock();
            search_transcripts(&store.index, a.query.as_deref(), a.limit.unwrap_or(20), &filters)
        }
        "cancel-task" => {
            let a: CancelTask = parse(args)?;
            let reason = a.reason.filter(|r| !r.is_empty());
            Ok(render_cancel(&daemon.scheduler.cancel_task(&a.task_id, reason).await?))
        }
        _ => Err(ToolError::not_found(format!("unknown tool {name}"))),
    }
}
