//! Codex worker harness over the live-verified control surface:
//!   start:  codex -a never -s <sandbox> exec --json -C <workspace> <prompt>
//!   resume: codex -a never -s <sandbox> exec resume --json <thread_id> <prompt>
//! On the host, codex's own workspace-write sandbox is the boundary. In
//! Docker the container plus egress proxy are the boundary, and codex's
//! sandbox (Landlock) is unavailable inside containers, so the CLI sandbox is
//! disabled there. Events are one JSON object per stdout line; the parser is
//! deliberately tolerant of shape differences across codex versions.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::Value;

use super::harness::{TurnRequest, TurnResult, WorkerEvent, WorkerHarness, drive};
use super::runner::{RunnerKind, WorkerSpawnSpec};
use crate::config::Provider;
use crate::domain::errors::ToolError;

fn derive_kind(obj: &Value) -> String {
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("unknown");
    if kind.starts_with("item.")
        && let Some(item) = obj.get("item")
        && let Some(item_type) =
            item.get("item_type").or_else(|| item.get("type")).and_then(Value::as_str)
    {
        return item_type.to_string();
    }
    kind.to_string()
}

/// `obj.item ?? obj`, the object a codex line describes.
fn item_of(obj: &Value) -> &Value {
    obj.get("item").filter(|item| !item.is_null()).unwrap_or(obj)
}

fn extract_text(obj: &Value) -> Option<String> {
    let item = item_of(obj);
    ["text", "message", "content"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn extract_changed_files(obj: &Value, into: &mut BTreeSet<String>) {
    let item = item_of(obj);
    let changes = item.get("changes").filter(|c| !c.is_null()).or_else(|| obj.get("changes"));
    if let Some(Value::Array(changes)) = changes {
        for change in changes {
            match change {
                Value::String(path) => into.insert(path.clone()),
                other => match other.get("path").and_then(Value::as_str) {
                    Some(path) => into.insert(path.to_string()),
                    None => false,
                },
            };
        }
    } else if let Some(path) = item.get("path").and_then(Value::as_str) {
        into.insert(path.to_string());
    }
}

/// Codex reports paths as the worker saw them; make them workspace-relative.
pub(crate) fn relative_to_workspace(path: &str, workspace_path: &str) -> String {
    let prefix = if workspace_path.ends_with('/') {
        workspace_path.to_string()
    } else {
        format!("{workspace_path}/")
    };
    path.strip_prefix(&prefix).unwrap_or(path).to_string()
}

#[derive(Debug, Clone, Default)]
pub struct CodexHarnessOptions {
    /// Model to request (cloud or local), passed as `-m`.
    pub model: Option<String>,
    /// Local model server type; presence switches codex into --oss mode.
    pub provider: Option<Provider>,
}

pub struct CodexHarness {
    options: CodexHarnessOptions,
}

impl CodexHarness {
    pub fn new(options: CodexHarnessOptions) -> CodexHarness {
        CodexHarness { options }
    }

    fn spawn_spec(&self, request: &TurnRequest<'_>) -> WorkerSpawnSpec {
        let in_docker = request.runner.kind() == RunnerKind::Docker;
        let sandbox = if in_docker { "danger-full-access" } else { "workspace-write" };
        let mut spec = WorkerSpawnSpec::default();
        let mut push = |arg: &str| spec.argv.push(arg.to_string());
        for arg in ["codex", "-a", "never", "-s", sandbox] {
            push(arg);
        }
        if let Some(provider) = self.options.provider {
            let (name, port) = match provider {
                Provider::Ollama => ("ollama", 11434),
                Provider::Lmstudio => ("lmstudio", 1234),
            };
            for arg in ["--oss", "--local-provider", name] {
                push(arg);
            }
            if in_docker {
                // Inside a container, "localhost" is the container; the model
                // server sits on the host, reached only through the egress proxy.
                spec.env.insert(
                    "CODEX_OSS_BASE_URL".into(),
                    format!("http://host.docker.internal:{port}/v1"),
                );
            }
        }
        if let Some(model) = &self.options.model {
            push("-m");
            push(model);
        }
        push("exec");
        match &request.native_session_id {
            Some(thread) => {
                for arg in ["resume", "--json", thread, &request.prompt] {
                    push(arg);
                }
            }
            None => {
                for arg in ["--json", "-C", request.runner.workspace_path(), &request.prompt] {
                    push(arg);
                }
            }
        }
        spec
    }
}

#[async_trait]
impl WorkerHarness for CodexHarness {
    fn name(&self) -> &str {
        "codex"
    }

    async fn run_turn(&self, request: TurnRequest<'_>) -> Result<TurnResult, ToolError> {
        // ToolErrors from runner preflight (docker down, image or auth volume
        // missing) propagate with their error codes intact.
        let worker = request.runner.start(self.spawn_spec(&request)).await?;

        let mut thread_id = request.native_session_id.clone();
        let mut last_agent_message: Option<String> = None;
        let mut usage: Option<Value> = None;
        let mut changed_files = BTreeSet::new();
        let exit = drive::run(worker, &request.cancel, |line| {
            let Ok(obj) = serde_json::from_str::<Value>(line) else {
                (request.on_event)(WorkerEvent {
                    kind: "unparsed_output".into(),
                    payload: serde_json::json!({ "line": line }),
                });
                return;
            };
            let kind = derive_kind(&obj);
            (request.on_event)(WorkerEvent { kind: kind.clone(), payload: obj.clone() });
            if let Some(id) = obj.get("thread_id").and_then(Value::as_str) {
                thread_id = Some(id.to_string());
            }
            match kind.as_str() {
                "agent_message" => {
                    if let Some(text) = extract_text(&obj) {
                        last_agent_message = Some(text);
                    }
                }
                "file_change" => extract_changed_files(&obj, &mut changed_files),
                "turn.completed" => {
                    if let Some(u) = obj.get("usage") {
                        usage = Some(u.clone());
                    }
                }
                _ => {}
            }
        })
        .await;

        if exit.aborted || exit.code != Some(0) {
            return Err(drive::failure("codex", &exit, None));
        }
        Ok(TurnResult {
            response: last_agent_message.unwrap_or_default(),
            native_session_id: thread_id.filter(|t| !t.is_empty()),
            changed_files: changed_files
                .iter()
                .map(|f| relative_to_workspace(f, request.runner.workspace_path()))
                .collect(),
            usage,
        })
    }
}
