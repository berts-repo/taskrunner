//! Claude Code worker harness over the live-verified control surface:
//!   start:  claude --print --output-format stream-json --verbose <perm> <prompt>
//!   resume: same, plus --resume <session_id>
//! Non-interactive runs MUST set a permission flag or they hang on permission
//! prompts. In Docker the container plus egress proxy are the boundary, so
//! Claude's own permission prompts are disabled; on the host, acceptEdits
//! allows file edits but nothing broader.
//!
//! Claude emits no dedicated file-change event; changed files are derived
//! from tool_use inputs (Write/Edit/MultiEdit/NotebookEdit), with the
//! workspace git fallback covering anything shell commands touched.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::Value;

use super::codex::relative_to_workspace;
use super::harness::{TurnRequest, TurnResult, WorkerEvent, WorkerHarness, drive};
use super::runner::{RunnerKind, WorkerSpawnSpec};
use crate::domain::errors::ToolError;

const EDIT_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

fn extract_edited_files(message: &Value, workspace_path: &str, into: &mut BTreeSet<String>) {
    let Some(Value::Array(content)) = message.get("content") else { return };
    for block in content {
        let is_edit = block.get("type").and_then(Value::as_str) == Some("tool_use")
            && block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| EDIT_TOOLS.contains(&name));
        if !is_edit {
            continue;
        }
        let input = block.get("input").filter(|v| !v.is_null());
        let path = input.and_then(|i| {
            i.get("file_path").filter(|v| !v.is_null()).or_else(|| i.get("notebook_path"))
        });
        if let Some(path) = path.and_then(Value::as_str) {
            into.insert(relative_to_workspace(path, workspace_path));
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeHarnessOptions {
    /// Model to request, passed as `--model`.
    pub model: Option<String>,
}

pub struct ClaudeHarness {
    options: ClaudeHarnessOptions,
}

impl ClaudeHarness {
    pub fn new(options: ClaudeHarnessOptions) -> ClaudeHarness {
        ClaudeHarness { options }
    }

    fn spawn_spec(&self, request: &TurnRequest<'_>) -> WorkerSpawnSpec {
        let mut argv: Vec<String> =
            ["claude", "--print", "--output-format", "stream-json", "--verbose"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        if let Some(model) = &self.options.model {
            argv.extend(["--model".to_string(), model.clone()]);
        }
        if request.runner.kind() == RunnerKind::Docker {
            argv.push("--dangerously-skip-permissions".into());
        } else {
            argv.extend(["--permission-mode".to_string(), "acceptEdits".to_string()]);
        }
        if let Some(session) = &request.native_session_id {
            argv.extend(["--resume".to_string(), session.clone()]);
        }
        argv.push(request.prompt.clone());
        WorkerSpawnSpec { argv, env: Default::default() }
    }
}

#[async_trait]
impl WorkerHarness for ClaudeHarness {
    fn name(&self) -> &str {
        "claude"
    }

    async fn run_turn(&self, request: TurnRequest<'_>) -> Result<TurnResult, ToolError> {
        let worker = request.runner.start(self.spawn_spec(&request)).await?;

        let mut session_id = request.native_session_id.clone();
        let mut response = String::new();
        let mut usage: Option<Value> = None;
        let mut result_error: Option<String> = None;
        let mut changed_files = BTreeSet::new();
        let exit = drive::run(worker, &request.cancel, |line| {
            let Ok(obj) = serde_json::from_str::<Value>(line) else {
                (request.on_event)(WorkerEvent {
                    kind: "unparsed_output".into(),
                    payload: serde_json::json!({ "line": line }),
                });
                return;
            };
            // Event kinds quote Claude's own line types verbatim
            // (system, assistant, user, result, rate_limit_event, ...).
            let kind = obj.get("type").and_then(Value::as_str).unwrap_or("unknown").to_string();
            (request.on_event)(WorkerEvent { kind: kind.clone(), payload: obj.clone() });

            if let Some(id) = obj.get("session_id").and_then(Value::as_str) {
                session_id = Some(id.to_string());
            }
            if kind == "assistant"
                && let Some(message) = obj.get("message").filter(|m| !m.is_null())
            {
                extract_edited_files(message, request.runner.workspace_path(), &mut changed_files);
            }
            if kind == "result" {
                if let Some(text) = obj.get("result").and_then(Value::as_str) {
                    response = text.to_string();
                }
                if let Some(u) = obj.get("usage") {
                    usage = Some(u.clone());
                }
                if obj.get("is_error") == Some(&Value::Bool(true)) {
                    result_error = Some(match obj.get("result").and_then(Value::as_str) {
                        Some(text) => text.to_string(),
                        None => format!(
                            "claude reported {}",
                            obj.get("subtype").map(json_string).unwrap_or("an error".into())
                        ),
                    });
                }
            }
        })
        .await;

        if exit.aborted || exit.code != Some(0) || result_error.is_some() {
            return Err(drive::failure("claude", &exit, result_error.as_deref()));
        }
        Ok(TurnResult {
            response,
            native_session_id: session_id.filter(|s| !s.is_empty()),
            changed_files: changed_files.into_iter().collect(),
            usage,
        })
    }
}

/// `String(v)`.
fn json_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
