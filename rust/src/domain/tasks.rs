//! Read-side helpers over the derived index. Turn statuses are
//! running | completed | failed | canceled; a task mirrors its latest turn.

use std::fs;

use rusqlite::{OptionalExtension, Row, ToSql, params, params_from_iter};
use serde_json::Value;

use crate::storage::index::StateIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHandle {
    pub artifact_id: String,
    pub kind: String,
    pub label: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub locator: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnInfo {
    pub turn_id: String,
    pub idx: i64,
    pub prompt: String,
    pub response: Option<String>,
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub changed_files: Vec<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub project_root: String,
    pub worker: String,
    pub prompt_summary: String,
    pub status: String,
    pub tier: Option<String>,
    pub allow_domains: Vec<String>,
    pub approval_state: String,
    pub updated_at: String,
    pub worker_session_id: Option<String>,
    pub turn_count: i64,
    pub latest_turn: Option<TurnInfo>,
}

fn turn_from_row(row: &Row) -> rusqlite::Result<TurnInfo> {
    let changed_files: Option<String> = row.get("changed_files")?;
    Ok(TurnInfo {
        turn_id: row.get("id")?,
        idx: row.get("idx")?,
        prompt: row.get("prompt")?,
        response: row.get("response")?,
        status: row.get("status")?,
        error_code: row.get("error_code")?,
        error_message: row.get("error_message")?,
        changed_files: changed_files.map(|json| string_list(&json)).unwrap_or_default(),
        started_at: row.get("started_at")?,
        completed_at: row.get("completed_at")?,
    })
}

/// A JSON array of strings as written by the fold; anything else is empty.
fn string_list(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn list_turns(index: &StateIndex, task_id: &str) -> rusqlite::Result<Vec<TurnInfo>> {
    let mut stmt = index.db.prepare("SELECT * FROM turns WHERE task_id = ? ORDER BY idx")?;
    stmt.query_map([task_id], turn_from_row)?.collect()
}

pub fn get_task_snapshot(
    index: &StateIndex,
    task_id: &str,
) -> rusqlite::Result<Option<TaskSnapshot>> {
    let db = &index.db;
    let task = db
        .query_row(
            "SELECT t.*, p.root AS project_root FROM tasks t JOIN projects p ON p.id = t.project_id
             WHERE t.id = ?",
            [task_id],
            |row| {
                let allow_domains: Option<String> = row.get("allow_domains")?;
                Ok(TaskSnapshot {
                    task_id: row.get("id")?,
                    project_root: row.get("project_root")?,
                    worker: row.get("worker")?,
                    prompt_summary: row.get("prompt_summary")?,
                    status: row.get("status")?,
                    tier: row.get("tier")?,
                    allow_domains: allow_domains.map(|json| string_list(&json)).unwrap_or_default(),
                    approval_state: row.get("approval_state")?,
                    updated_at: row.get("updated_at")?,
                    worker_session_id: None,
                    turn_count: 0,
                    latest_turn: None,
                })
            },
        )
        .optional()?;
    let Some(mut task) = task else { return Ok(None) };

    task.latest_turn = db
        .query_row(
            "SELECT * FROM turns WHERE task_id = ? ORDER BY idx DESC LIMIT 1",
            [task_id],
            turn_from_row,
        )
        .optional()?;
    task.turn_count =
        db.query_row("SELECT COUNT(*) FROM turns WHERE task_id = ?", [task_id], |row| row.get(0))?;
    task.worker_session_id = db
        .query_row(
            "SELECT native_session_id FROM worker_sessions WHERE task_id = ?
             ORDER BY recorded_at DESC, id DESC LIMIT 1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(Some(task))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMatch {
    pub project_id: String,
    pub root: String,
}

/// Read-only project lookup: never creates records (unlike resolve_project).
pub fn find_project_by_path(
    index: &StateIndex,
    path: &str,
) -> rusqlite::Result<Option<ProjectMatch>> {
    let mut candidates = vec![path.to_string()];
    // The path may no longer exist; alias lookup can still hit.
    if let Ok(real) = fs::canonicalize(path) {
        candidates.push(real.to_string_lossy().into_owned());
    }
    for candidate in candidates {
        let hit = index
            .db
            .query_row(
                "SELECT p.id AS project_id, p.root FROM project_aliases a
                 JOIN projects p ON p.id = a.project_id WHERE a.path = ?",
                [&candidate],
                |row| Ok(ProjectMatch { project_id: row.get(0)?, root: row.get(1)? }),
            )
            .optional()?;
        if hit.is_some() {
            return Ok(hit);
        }
    }
    Ok(None)
}

pub fn list_task_snapshots(
    index: &StateIndex,
    project_id: &str,
    limit: i64,
) -> rusqlite::Result<Vec<TaskSnapshot>> {
    let mut stmt = index.db.prepare(
        "SELECT id FROM tasks WHERE project_id = ? ORDER BY updated_at DESC, id DESC LIMIT ?",
    )?;
    let ids: Vec<String> =
        stmt.query_map(params![project_id, limit], |row| row.get(0))?.collect::<Result<_, _>>()?;
    let mut snapshots = Vec::new();
    for id in ids {
        if let Some(snapshot) = get_task_snapshot(index, &id)? {
            snapshots.push(snapshot);
        }
    }
    Ok(snapshots)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    pub ts: String,
    pub kind: String,
    pub payload: Value,
}

pub fn get_turn_audit(index: &StateIndex, turn_id: &str) -> rusqlite::Result<Vec<AuditRow>> {
    let mut stmt = index
        .db
        .prepare("SELECT ts, kind, payload FROM audit_events WHERE turn_id = ? ORDER BY ts, id")?;
    stmt.query_map([turn_id], |row| {
        let payload: String = row.get(2)?;
        Ok(AuditRow {
            ts: row.get(0)?,
            kind: row.get(1)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        })
    })?
    .collect()
}

pub fn get_turn_artifacts(
    index: &StateIndex,
    turn_id: &str,
) -> rusqlite::Result<Vec<ArtifactHandle>> {
    let mut stmt = index.db.prepare(
        "SELECT a.id AS artifact_id, a.kind, a.label, a.media_type, a.size_bytes, a.sha256, a.locator
         FROM artifact_links l JOIN artifacts a ON a.id = l.artifact_id
         WHERE l.turn_id = ? ORDER BY a.created_at, a.id",
    )?;
    stmt.query_map([turn_id], |row| {
        Ok(ArtifactHandle {
            artifact_id: row.get(0)?,
            kind: row.get(1)?,
            label: row.get(2)?,
            media_type: row.get(3)?,
            size_bytes: row.get(4)?,
            sha256: row.get(5)?,
            locator: row.get(6)?,
        })
    })?
    .collect()
}

/// One swept transcript record belonging to a task's worker session(s). The
/// tool_* and prompt_idx columns are projection-time facts (see facts.rs),
/// carried here so a renderer never has to re-parse the content blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub role: String,
    pub kind: String,
    pub content: String,
    pub native_ts: Option<String>,
    pub native_session_id: String,
    pub prompt_idx: i64,
    pub tool_name: Option<String>,
    pub tool_target: Option<String>,
    pub is_error: Option<i64>,
}

/// Default cap on messages rendered for one task, so a lookup can't dump a
/// whole delegated turn's interior in a single response.
pub const DEFAULT_TRANSCRIPT_LIMIT: i64 = 500;

/// How many messages a transcript read keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageLimit {
    /// The newest [`DEFAULT_TRANSCRIPT_LIMIT`].
    #[default]
    Default,
    /// The whole session — the timeline view's default, since a capped audit
    /// trail is not an audit trail.
    All,
    /// The newest N.
    Newest(i64),
}

/// Shared scoping for the two transcript readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageQuery {
    pub limit: MessageLimit,
    /// Restrict to one addressable exchange: a real user prompt and everything
    /// that followed it, up to the next one.
    pub prompt_idx: Option<i64>,
}

/// Messages in chronological order, and whether older ones were dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub messages: Vec<TranscriptMessage>,
    pub capped: bool,
}

const MESSAGE_COLUMNS: &str = "role, kind, content, native_ts, native_session_id,
                               prompt_idx, tool_name, tool_target, is_error";

fn message_from_row(row: &Row) -> rusqlite::Result<TranscriptMessage> {
    Ok(TranscriptMessage {
        role: row.get(0)?,
        kind: row.get(1)?,
        content: row.get(2)?,
        native_ts: row.get(3)?,
        native_session_id: row.get(4)?,
        prompt_idx: row.get(5)?,
        tool_name: row.get(6)?,
        tool_target: row.get(7)?,
        is_error: row.get(8)?,
    })
}

/// Reads the newest messages matching `scope`, then presents them oldest-first.
/// `LIMIT -1` is SQLite's "no limit"; fetching one past the cap is what tells
/// the caller whether older messages were dropped.
fn read_messages(
    index: &StateIndex,
    scope: &str,
    scope_params: Vec<Box<dyn ToSql>>,
    query: MessageQuery,
) -> rusqlite::Result<MessagePage> {
    let cap = match query.limit {
        MessageLimit::Default => Some(DEFAULT_TRANSCRIPT_LIMIT),
        MessageLimit::All => None,
        MessageLimit::Newest(n) => Some(n),
    };
    let prompt_filter = if query.prompt_idx.is_some() { "AND prompt_idx = ?" } else { "" };
    let mut params = scope_params;
    if let Some(prompt_idx) = query.prompt_idx {
        params.push(Box::new(prompt_idx));
    }
    params.push(Box::new(cap.map_or(-1, |n| n + 1)));

    let mut stmt = index.db.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS}
           FROM messages
          WHERE {scope}
            {prompt_filter}
          ORDER BY native_ts DESC, recorded_at DESC, id DESC
          LIMIT ?"
    ))?;
    let mut messages: Vec<TranscriptMessage> = stmt
        .query_map(params_from_iter(params.iter()), message_from_row)?
        .collect::<Result<_, _>>()?;
    let capped = cap.is_some_and(|n| messages.len() as i64 > n);
    if let Some(n) = cap {
        messages.truncate(n as usize);
    }
    messages.reverse(); // newest-first for the cap, chronological for display
    Ok(MessagePage { messages, capped })
}

const TASK_SESSIONS: &str = "native_session_id IN (
    SELECT DISTINCT native_session_id FROM worker_sessions WHERE task_id = ?
)";

/// A task's archived transcript: every message swept from the worker
/// session(s) this task ran under, joined via
/// `worker_sessions.native_session_id → messages.native_session_id`. Returned
/// in chronological order, but capped to the most recent `limit` — a delegated
/// turn can archive thousands of records and the tail is what a caller usually
/// wants.
pub fn get_task_messages(
    index: &StateIndex,
    task_id: &str,
    query: MessageQuery,
) -> rusqlite::Result<MessagePage> {
    read_messages(index, TASK_SESSIONS, vec![Box::new(task_id.to_string())], query)
}

/// One session's messages in chronological order, keyed directly on
/// source+native_session_id so it works for a host session that no task links.
/// Same cap semantics as [`get_task_messages`].
pub fn get_session_messages(
    index: &StateIndex,
    source: &str,
    native_session_id: &str,
    query: MessageQuery,
) -> rusqlite::Result<MessagePage> {
    read_messages(
        index,
        "source = ? AND native_session_id = ?",
        vec![Box::new(source.to_string()), Box::new(native_session_id.to_string())],
        query,
    )
}

/// One ingested transcript session (a distinct source+native_session_id = one
/// Claude Code jsonl / Codex rollout), aggregated in `transcript_sessions`.
/// `task_id` is set only for a worker session linked to a task; host sessions
/// (Claude Code / Codex conversations on this machine) carry none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub source: String,
    pub native_session_id: String,
    pub project_path: Option<String>,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub first_recorded_at: String,
    pub last_recorded_at: String,
    pub message_count: i64,
    pub task_id: Option<String>,
}

pub const DEFAULT_SESSION_LIMIT: i64 = 20;

#[derive(Debug, Clone, Default)]
pub struct SessionListQuery {
    pub project: Option<String>,
    pub native_session_id: Option<String>,
    pub limit: Option<i64>,
    pub before: Option<String>,
}

/// Ingested transcript sessions, most recent first. Recency is
/// `COALESCE(last_ts, last_recorded_at)` so a session whose format carries no
/// per-record timestamp still orders by when it was ingested. `project`
/// filters to one project root; `native_session_id` narrows to a single
/// session id (used to resolve a bare id to its source). Task linkage is a
/// correlated subquery, not a join, so a session linked to several tasks stays
/// one row.
pub fn list_sessions(
    index: &StateIndex,
    query: &SessionListQuery,
) -> rusqlite::Result<Vec<SessionInfo>> {
    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(project) = &query.project {
        clauses.push("s.project_path = ?");
        params.push(Box::new(project.clone()));
    }
    if let Some(id) = &query.native_session_id {
        clauses.push("s.native_session_id = ?");
        params.push(Box::new(id.clone()));
    }
    if let Some(before) = &query.before {
        clauses.push("COALESCE(s.last_ts, s.last_recorded_at) < ?");
        params.push(Box::new(before.clone()));
    }
    let where_clause =
        if clauses.is_empty() { String::new() } else { format!("WHERE {}", clauses.join(" AND ")) };
    params.push(Box::new(query.limit.unwrap_or(DEFAULT_SESSION_LIMIT)));

    let mut stmt = index.db.prepare(&format!(
        "SELECT s.id, s.source, s.native_session_id, s.project_path,
                s.first_ts, s.last_ts, s.first_recorded_at, s.last_recorded_at,
                s.message_count,
                (SELECT ws.task_id FROM worker_sessions ws
                  WHERE ws.native_session_id = s.native_session_id
                  ORDER BY ws.recorded_at, ws.id LIMIT 1) AS task_id
           FROM transcript_sessions s
           {where_clause}
          ORDER BY COALESCE(s.last_ts, s.last_recorded_at) DESC, s.id DESC
          LIMIT ?"
    ))?;
    stmt.query_map(params_from_iter(params.iter()), |row| {
        Ok(SessionInfo {
            id: row.get(0)?,
            source: row.get(1)?,
            native_session_id: row.get(2)?,
            project_path: row.get(3)?,
            first_ts: row.get(4)?,
            last_ts: row.get(5)?,
            first_recorded_at: row.get(6)?,
            last_recorded_at: row.get(7)?,
            message_count: row.get(8)?,
            task_id: row.get(9)?,
        })
    })?
    .collect()
}

/// One hit from corpus-wide transcript search. `task_id` is present only when
/// the matched message came from a worker session linked to a task; host
/// session messages match too and carry none. `prompt_idx` makes a hit
/// drillable: it is the address to re-read the exchange it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptHit {
    pub task_id: Option<String>,
    pub source: String,
    pub role: String,
    pub kind: String,
    pub native_ts: Option<String>,
    pub native_session_id: String,
    pub project_path: Option<String>,
    pub prompt_idx: i64,
    pub tool_name: Option<String>,
    pub tool_target: Option<String>,
    pub is_error: Option<i64>,
    /// The matched text with the hit bracketed; None for a filter-only search,
    /// which never touches the full-text index and so has nothing to highlight.
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    /// Relevance.
    #[default]
    Rank,
    /// Newest native_ts first.
    Recent,
}

/// Optional scoping for [`search_messages`].
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub project: Option<String>,
    /// Restrict to these native session ids. Takes precedence over `last_sessions`.
    pub sessions: Option<Vec<String>>,
    /// Restrict to the most-recent N sessions (within `project` when set).
    pub last_sessions: Option<i64>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub role: Option<String>,
    pub kind: Option<String>,
    /// Restrict to calls of one tool, e.g. "Edit". Matches tool_use records only.
    pub tool: Option<String>,
    /// Substring of the path or command a call acted on; SQL LIKE wildcards apply.
    pub target: Option<String>,
    /// true: the call failed. false: it succeeded. Records that state no
    /// outcome (rejections, aborts) are excluded either way — see facts.rs.
    pub failed: Option<bool>,
    /// Ignored without a query: a filter-only search has no relevance to rank by.
    pub sort: SearchSort,
}

pub const DEFAULT_SEARCH_LIMIT: i64 = 20;

/// A call and its result are one thing, so a tool_use record inherits the
/// failure state of the result it is paired with, while a result states its
/// own. Lets `failed` combine with `tool`/`target`, which live on the *call*
/// record.
pub(crate) const ERROR_STATE: &str = "COALESCE(m.is_error,
      (SELECT r.is_error FROM messages r
        WHERE r.tool_use_id = m.tool_use_id AND r.kind = 'tool_result'))";

/// Columns the fts table carries unindexed, so a text search can filter
/// without reaching through the join.
const FTS_COLUMNS: [&str; 5] = ["source", "role", "kind", "native_ts", "native_session_id"];

/// Search across ingested transcript messages. With a `query` this is FTS5
/// over message text; with none it is a structured scan over the facts
/// promoted to columns — which is what makes "every Edit under src/shim" a
/// query rather than a grep over JSON blobs. Either way the same filters apply
/// and the same hit shape comes back.
///
/// Task attribution and every non-content field come from a 1:1 join to
/// `messages` (keyed on the unique message_id), so an fts hit is never
/// multiplied. Fails on malformed FTS5 query syntax — callers map that to a
/// client error.
pub fn search_messages(
    index: &StateIndex,
    query: Option<&str>,
    limit: i64,
    filters: &SearchFilters,
) -> rusqlite::Result<Vec<TranscriptHit>> {
    let fts = query.is_some_and(|q| !q.is_empty());
    let col = |name: &str| {
        if fts && FTS_COLUMNS.contains(&name) { format!("f.{name}") } else { format!("m.{name}") }
    };
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    if fts {
        clauses.push("messages_fts MATCH ?".into());
        params.push(Box::new(query.unwrap_or_default().to_string()));
    }

    let sessions = match (&filters.sessions, filters.last_sessions) {
        (Some(ids), _) => Some(ids.clone()),
        (None, Some(n)) => {
            let recent = list_sessions(
                index,
                &SessionListQuery {
                    project: filters.project.clone(),
                    limit: Some(n),
                    ..Default::default()
                },
            )?;
            Some(recent.into_iter().map(|s| s.native_session_id).collect())
        }
        (None, None) => None,
    };
    if let Some(ids) = sessions {
        if ids.is_empty() {
            return Ok(Vec::new()); // scoped to no session ⇒ no hits
        }
        let marks = vec!["?"; ids.len()].join(", ");
        clauses.push(format!("{} IN ({marks})", col("native_session_id")));
        params.extend(ids.into_iter().map(|id| Box::new(id) as Box<dyn ToSql>));
    }
    let text_filters = [
        (filters.project.as_ref(), "m.project_path = ?".to_string()),
        (filters.role.as_ref(), format!("{} = ?", col("role"))),
        (filters.kind.as_ref(), format!("{} = ?", col("kind"))),
        (filters.since.as_ref(), format!("{} >= ?", col("native_ts"))),
        (filters.until.as_ref(), format!("{} <= ?", col("native_ts"))),
        (filters.tool.as_ref(), "m.tool_name = ?".to_string()),
    ];
    for (value, clause) in text_filters {
        if let Some(value) = value {
            clauses.push(clause);
            params.push(Box::new(value.clone()));
        }
    }
    if let Some(target) = &filters.target {
        clauses.push("m.tool_target LIKE ?".into());
        params.push(Box::new(format!("%{target}%")));
    }
    if let Some(failed) = filters.failed {
        clauses.push(format!("{ERROR_STATE} = ?"));
        params.push(Box::new(i64::from(failed)));
        // Both halves of a pair carry the state, so a filter-only search would
        // report one failure twice. The call is the useful half — it names the
        // tool and the target. A text search is left alone: there the caller
        // asked for whichever record their words appear in.
        if !fts && filters.kind.is_none() {
            clauses.push("m.kind = 'tool_use'".into());
        }
    }

    let from =
        if fts { "messages_fts f JOIN messages m ON m.id = f.message_id" } else { "messages m" };
    let snippet = if fts { "snippet(messages_fts, 0, '[', ']', '…', 12)" } else { "NULL" };
    let order = match (fts, filters.sort) {
        (true, SearchSort::Recent) => "f.native_ts DESC, rank",
        (true, SearchSort::Rank) => "rank",
        (false, _) => "m.native_ts DESC, m.id DESC",
    };
    params.push(Box::new(limit));

    let mut stmt = index.db.prepare(&format!(
        "SELECT
           (SELECT ws.task_id FROM worker_sessions ws
             WHERE ws.native_session_id = {session_col}
             ORDER BY ws.recorded_at, ws.id LIMIT 1) AS task_id,
           {source}            AS source,
           {role}              AS role,
           {kind}              AS kind,
           {native_ts}         AS native_ts,
           {session_col}       AS native_session_id,
           m.project_path      AS project_path,
           m.prompt_idx        AS prompt_idx,
           m.tool_name         AS tool_name,
           m.tool_target       AS tool_target,
           {ERROR_STATE}       AS is_error,
           {snippet}           AS snippet
         FROM {from}
         WHERE {where_clause}
         ORDER BY {order}
         LIMIT ?",
        session_col = col("native_session_id"),
        source = col("source"),
        role = col("role"),
        kind = col("kind"),
        native_ts = col("native_ts"),
        where_clause = clauses.join(" AND "),
    ))?;
    stmt.query_map(params_from_iter(params.iter()), |row| {
        Ok(TranscriptHit {
            task_id: row.get(0)?,
            source: row.get(1)?,
            role: row.get(2)?,
            kind: row.get(3)?,
            native_ts: row.get(4)?,
            native_session_id: row.get(5)?,
            project_path: row.get(6)?,
            prompt_idx: row.get(7)?,
            tool_name: row.get(8)?,
            tool_target: row.get(9)?,
            is_error: row.get(10)?,
            snippet: row.get(11)?,
        })
    })?
    .collect()
}

/// The scannable index of a session: one entry per prompt, reply and tool
/// call, with no message bodies read beyond a leading slice of prose. That is
/// what keeps an outline cheap enough to be the default view — a session that
/// costs ~28k tokens to read in full outlines in well under a tenth of that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub prompt_idx: i64,
    pub role: String,
    pub kind: String,
    pub native_ts: Option<String>,
    /// Leading slice of a prose message; None on a tool call, whose body is
    /// deliberately never loaded.
    pub head: Option<String>,
    pub tool_name: Option<String>,
    pub tool_target: Option<String>,
    /// Failure state of the call, resolved from the result it is paired with.
    pub is_error: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutline {
    /// Every archived message, including the tool results the outline omits.
    pub message_count: i64,
    pub prompt_count: i64,
    pub tool_count: i64,
    pub entries: Vec<OutlineEntry>,
}

/// Prose kept per outline entry. Rendering truncates well below this; the
/// slice exists so a 40k-character preamble is never pulled out of the database.
const OUTLINE_HEAD: usize = 400;

/// Tool results are excluded: their failure state is already folded onto the
/// call that produced them, and a result carries nothing else an index needs.
fn outline_for(
    index: &StateIndex,
    scope: &str,
    scope_params: Vec<Box<dyn ToSql>>,
    prompt_idx: Option<i64>,
) -> rusqlite::Result<SessionOutline> {
    let filter = if prompt_idx.is_some() { "AND m.prompt_idx = ?" } else { "" };
    let mut params = scope_params;
    if let Some(prompt_idx) = prompt_idx {
        params.push(Box::new(prompt_idx));
    }
    let (message_count, prompt_count, tool_count) = index.db.query_row(
        &format!(
            "SELECT COUNT(*) AS message_count,
                    COALESCE(MAX(m.prompt_idx), 0) AS prompt_count,
                    COALESCE(SUM(m.kind = 'tool_use'), 0) AS tool_count
               FROM messages m
              WHERE {scope} {filter}"
        ),
        params_from_iter(params.iter()),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut stmt = index.db.prepare(&format!(
        "SELECT m.prompt_idx, m.role, m.kind, m.native_ts, m.tool_name, m.tool_target,
                CASE WHEN m.kind = 'tool_use' THEN NULL
                     ELSE substr(m.content, 1, {OUTLINE_HEAD}) END AS head,
                {ERROR_STATE} AS is_error
           FROM messages m
          WHERE {scope} {filter}
            AND m.kind != 'tool_result'
          ORDER BY m.native_ts, m.recorded_at, m.id"
    ))?;
    let entries = stmt
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(OutlineEntry {
                prompt_idx: row.get(0)?,
                role: row.get(1)?,
                kind: row.get(2)?,
                native_ts: row.get(3)?,
                tool_name: row.get(4)?,
                tool_target: row.get(5)?,
                head: row.get(6)?,
                is_error: row.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(SessionOutline { message_count, prompt_count, tool_count, entries })
}

pub fn get_session_outline(
    index: &StateIndex,
    source: &str,
    native_session_id: &str,
    prompt_idx: Option<i64>,
) -> rusqlite::Result<SessionOutline> {
    outline_for(
        index,
        "m.source = ? AND m.native_session_id = ?",
        vec![Box::new(source.to_string()), Box::new(native_session_id.to_string())],
        prompt_idx,
    )
}

pub fn get_task_outline(
    index: &StateIndex,
    task_id: &str,
    prompt_idx: Option<i64>,
) -> rusqlite::Result<SessionOutline> {
    outline_for(
        index,
        "m.native_session_id IN (
            SELECT DISTINCT native_session_id FROM worker_sessions WHERE task_id = ?
        )",
        vec![Box::new(task_id.to_string())],
        prompt_idx,
    )
}
