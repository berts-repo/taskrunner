//! The control socket's HTTP side: `/status`, and the read-only query routes
//! the CLI reaches so a session / task / search lookup runs in a terminal
//! without spending an MCP session. They call the exact same renderers as the
//! tools, so output is identical.

use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::Daemon;
use super::mcp::VERSION;
use crate::domain::errors::{ErrorCode, ToolError};
use crate::domain::tasks::{SearchFilters, SearchSort};
use crate::view::lookup::{
    Include, LookupArgs, LookupDeps, Scope, SessionLookupArgs, ViewArgs, lookup_session,
    lookup_task, search_transcripts,
};
use crate::view::render::render_wait;
use crate::view::transcript::{TRANSCRIPT_VIEWS, TranscriptView};

pub fn router(daemon: Daemon) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/lookup-session", get(|s, q| read(s, q, Route::LookupSession)))
        .route("/search-transcripts", get(|s, q| read(s, q, Route::SearchTranscripts)))
        .route("/lookup-task", get(|s, q| read(s, q, Route::LookupTask)))
        .route("/wait-task", get(wait_task))
        .fallback(|| async { (StatusCode::NOT_FOUND, r#"{"error":"not_found"}"#) })
        .with_state(daemon)
}

async fn status(State(daemon): State<Daemon>) -> Response {
    let counts = {
        let store = daemon.store.lock();
        let mut stmt =
            match store.index.db.prepare("SELECT status, COUNT(*) FROM tasks GROUP BY status") {
                Ok(stmt) => stmt,
                Err(err) => return internal(err.to_string()),
            };
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)));
        match rows.and_then(|rows| rows.collect::<Result<Vec<_>, _>>()) {
            Ok(rows) => rows
                .into_iter()
                .map(|(status, n)| (status, serde_json::Value::from(n)))
                .collect::<serde_json::Map<_, _>>(),
            Err(err) => return internal(err.to_string()),
        }
    };
    let body = serde_json::json!({
        "pid": std::process::id(),
        "version": VERSION,
        "state_root": daemon.paths.root,
        "tasks": counts,
        "active_mcp_sessions": daemon.active_sessions(),
    });
    ([(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

fn internal(message: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
}

#[derive(Clone, Copy)]
enum Route {
    LookupSession,
    SearchTranscripts,
    LookupTask,
}

/// Query params, first value wins, as `URLSearchParams.get` reads them.
struct Params(Vec<(String, String)>);

impl Params {
    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// A present, non-empty string.
    fn text(&self, name: &str) -> Option<String> {
        self.get(name).filter(|v| !v.is_empty()).map(str::to_string)
    }

    /// `Number(v)`: a finite number, else a client error.
    fn number(&self, name: &str) -> Result<Option<f64>, ToolError> {
        let Some(raw) = self.get(name) else { return Ok(None) };
        let value: f64 = raw
            .trim()
            .parse()
            .map_err(|_| ToolError::invalid_request(format!("{name} must be a number")))?;
        if !value.is_finite() {
            return Err(ToolError::invalid_request(format!("{name} must be a number")));
        }
        Ok(Some(value))
    }

    fn num(&self, name: &str) -> Result<Option<i64>, ToolError> {
        Ok(self.number(name)?.map(|n| n as i64))
    }

    fn whole(&self, name: &str) -> Result<Option<i64>, ToolError> {
        match self.number(name)? {
            Some(n) if n.fract() != 0.0 || n < 0.0 => {
                Err(ToolError::invalid_request(format!("{name} must be a non-negative integer")))
            }
            other => Ok(other.map(|n| n as i64)),
        }
    }

    fn bool(&self, name: &str) -> Result<Option<bool>, ToolError> {
        match self.get(name) {
            None => Ok(None),
            Some("true") => Ok(Some(true)),
            Some("false") => Ok(Some(false)),
            Some(_) => Err(ToolError::invalid_request(format!("{name} must be true or false"))),
        }
    }

    /// Transcript rendering params, shared by /lookup-session and /lookup-task.
    fn view(&self) -> Result<ViewArgs, ToolError> {
        let view = match self.get("view") {
            None => None,
            Some(name) => Some(TranscriptView::parse(name).ok_or_else(|| {
                ToolError::invalid_request(format!(
                    "view must be one of: {}",
                    TRANSCRIPT_VIEWS.join(", ")
                ))
            })?),
        };
        Ok(ViewArgs {
            view,
            tool_lines: self.whole("toolLines")?.map(|n| n as usize),
            prompt_idx: self.whole("prompt")?,
        })
    }
}

async fn read(
    State(daemon): State<Daemon>,
    Query(params): Query<Vec<(String, String)>>,
    route: Route,
) -> Response {
    match render(&daemon, &Params(params), route).await {
        Ok(text) => {
            let text = if text.ends_with('\n') { text } else { format!("{text}\n") };
            ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], text)
                .into_response()
        }
        Err(err) => tool_error(err),
    }
}

fn tool_error(err: ToolError) -> Response {
    let status = match err.code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], format!("{err}\n"))
        .into_response()
}

/// Long-polls a task: answers when its running turn ends, or once `timeout`
/// seconds pass, with the short result `taskrunner wait` prints. JSON, so the
/// CLI takes its exit code from the status field rather than from the text.
async fn wait_task(
    State(daemon): State<Daemon>,
    Query(params): Query<Vec<(String, String)>>,
) -> Response {
    let q = Params(params);
    let waited = async {
        let task_id =
            q.text("taskId").ok_or_else(|| ToolError::invalid_request("taskId is required"))?;
        let timeout = q.whole("timeout")?.map(|secs| Duration::from_secs(secs as u64));
        daemon.scheduler.wait_for(&task_id, timeout).await
    }
    .await;
    match waited {
        Ok(outcome) => {
            let body =
                serde_json::json!({ "status": outcome.status, "text": render_wait(&outcome) });
            ([(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string())
                .into_response()
        }
        Err(err) => tool_error(err),
    }
}

async fn render(daemon: &Daemon, q: &Params, route: Route) -> Result<String, ToolError> {
    match route {
        Route::LookupSession => {
            daemon.sweep_host_transcripts().await;
            let store = daemon.store.lock();
            lookup_session(
                &store.index,
                &SessionLookupArgs {
                    session_id: q.text("sessionId"),
                    project: q.text("project"),
                    source: q.text("source"),
                    limit: q.num("limit")?,
                    last: q.num("last")?,
                    view: q.view()?,
                },
            )
        }
        Route::SearchTranscripts => {
            let sort = match q.get("sort") {
                None | Some("rank") => SearchSort::Rank,
                Some("recent") => SearchSort::Recent,
                Some(_) => {
                    return Err(ToolError::invalid_request("sort must be 'rank' or 'recent'"));
                }
            };
            let last_sessions = q.num("lastSessions")?;
            let filters = SearchFilters {
                project: q.text("project"),
                sessions: q
                    .text("sessions")
                    .map(|s| s.split(',').filter(|v| !v.is_empty()).map(str::to_string).collect()),
                last_sessions,
                since: q.text("since"),
                until: q.text("until"),
                role: q.text("role"),
                kind: q.text("kind"),
                tool: q.text("tool"),
                target: q.text("target"),
                failed: q.bool("failed")?,
                sort,
            };
            if last_sessions.is_some() {
                daemon.sweep_host_transcripts().await;
            }
            let store = daemon.store.lock();
            search_transcripts(
                &store.index,
                q.get("query"),
                q.num("limit")?.unwrap_or(20),
                &filters,
            )
        }
        Route::LookupTask => {
            let include = q
                .text("include")
                .map(|s| {
                    s.split(',').filter(|v| !v.is_empty()).filter_map(Include::parse).collect()
                })
                .unwrap_or_default();
            let scope = match (q.text("turnId"), q.num("last")?) {
                (Some(turn_id), _) => Some(Scope { turn_id: Some(turn_id), last: None }),
                (None, Some(last)) => Some(Scope { turn_id: None, last: Some(last) }),
                (None, None) => None,
            };
            let store = daemon.store.lock();
            lookup_task(
                &LookupDeps { index: &store.index, artifacts: &daemon.artifacts },
                &LookupArgs {
                    task_id: q.text("taskId"),
                    project: q.text("project"),
                    include,
                    scope,
                    limit: q.num("limit")?,
                    view: q.view()?,
                },
            )
        }
    }
}
