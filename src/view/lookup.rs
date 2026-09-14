//! lookup-task semantics: compact summaries by default; expansion blocks only
//! for requested include fields; history as paired exchanges, never loose
//! audit rows; trace replays inputs, observable worker activity, and outputs
//! per in-scope turn; transcript surfaces the archived interior of the
//! worker's own session(s). Plus search-transcripts and lookup-session, which
//! read the same archive.

use crate::domain::errors::ToolError;
use crate::domain::tasks::{
    MessageLimit, MessagePage, MessageQuery, SearchFilters, SessionInfo, SessionListQuery,
    SessionOutline, TaskSnapshot, TranscriptHit, TurnInfo, find_project_by_path,
    get_session_messages, get_session_outline, get_task_messages, get_task_outline,
    get_task_snapshot, get_turn_artifacts, get_turn_audit, list_sessions, list_task_snapshots,
    list_turns, search_messages,
};
use crate::js;
use crate::storage::artifacts::ArtifactStore;
use crate::storage::index::StateIndex;
use crate::view::transcript::{
    DEFAULT_TOOL_LINES, MessageView, TranscriptView, compact_payload, render_messages,
    render_outline, truncate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Include {
    Turns,
    Artifacts,
    Audit,
    Diff,
    Trace,
    Transcript,
}

impl Include {
    pub fn parse(value: &str) -> Option<Include> {
        match value {
            "turns" => Some(Include::Turns),
            "artifacts" => Some(Include::Artifacts),
            "audit" => Some(Include::Audit),
            "diff" => Some(Include::Diff),
            "trace" => Some(Include::Trace),
            "transcript" => Some(Include::Transcript),
            _ => None,
        }
    }
}

/// Which turns an expansion covers: one by id, or the trailing N.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub turn_id: Option<String>,
    pub last: Option<i64>,
}

/// How a transcript is rendered; shared by lookup-task and lookup-session.
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewArgs {
    /// See [`resolve_view`].
    pub view: Option<TranscriptView>,
    pub tool_lines: Option<usize>,
    pub prompt_idx: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct LookupArgs {
    pub task_id: Option<String>,
    pub project: Option<String>,
    pub include: Vec<Include>,
    pub scope: Option<Scope>,
    pub limit: Option<i64>,
    pub view: ViewArgs,
}

/// Which view an unset `view` means. The outline is the default because
/// reading a whole session should be a decision, not an accident — but the
/// two ways a caller can already narrow a read say plainly what they want,
/// and outlining them instead would be a regression:
///
///   prompt N   drilling into one exchange means reading it → timeline
///   last N     a bounded read of the newest messages → compact, as before
fn resolve_view(view: ViewArgs, last: Option<i64>) -> TranscriptView {
    if let Some(view) = view.view {
        return view;
    }
    if view.prompt_idx.is_some() {
        return TranscriptView::Timeline;
    }
    if last.is_some() { TranscriptView::Compact } else { TranscriptView::Outline }
}

/// How many messages a transcript read returns. The timeline is an audit
/// view, so it is uncapped unless the caller narrows it; compact keeps its
/// 500 default. An explicit `last` always wins.
fn message_query(view: MessageView, last: Option<i64>, prompt_idx: Option<i64>) -> MessageQuery {
    let limit = match (last, view) {
        (Some(n), _) => MessageLimit::Newest(n),
        (None, MessageView::Timeline) => MessageLimit::All,
        (None, MessageView::Compact) => MessageLimit::Default,
    };
    MessageQuery { limit, prompt_idx }
}

/// Shared tail of the two outline renderings: the index itself, or a plain
/// statement that there is nothing to index.
fn outline_section(outline: &SessionOutline, prompt_idx: Option<i64>) -> Vec<String> {
    if outline.message_count == 0 {
        return vec![no_messages("(no transcript recorded)", prompt_idx)];
    }
    render_outline(outline)
}

fn no_messages(when_unscoped: &str, prompt_idx: Option<i64>) -> String {
    match prompt_idx {
        Some(n) => format!("  (no messages at prompt {n})"),
        None => format!("  {when_unscoped}"),
    }
}

pub struct LookupDeps<'a> {
    pub index: &'a StateIndex,
    pub artifacts: &'a ArtifactStore,
}

const DIFF_INLINE_LIMIT: usize = 50_000;

pub fn lookup_task(deps: &LookupDeps, args: &LookupArgs) -> Result<String, ToolError> {
    if let Some(task_id) = &args.task_id {
        return lookup_single_task(deps, task_id, args);
    }
    if let Some(project) = &args.project {
        return lookup_project_tasks(deps, project, args.limit.unwrap_or(10));
    }
    Err(ToolError::invalid_request("provide taskId or project"))
}

fn lookup_single_task(
    deps: &LookupDeps,
    task_id: &str,
    args: &LookupArgs,
) -> Result<String, ToolError> {
    let index = deps.index;
    let snapshot = get_task_snapshot(index, task_id)?
        .ok_or_else(|| ToolError::not_found(format!("no task {task_id}")))?;
    let turns = apply_scope(list_turns(index, task_id)?, args.scope.as_ref(), task_id)?;
    let has = |field| args.include.contains(&field);

    let mut sections = vec![render_summary(&snapshot)];
    if has(Include::Turns) {
        sections.push(render_exchanges(&turns));
    }
    if has(Include::Trace) {
        for turn in &turns {
            sections.push(render_trace(deps, turn)?);
        }
    }
    if has(Include::Audit) {
        for turn in &turns {
            sections.push(render_audit(index, turn)?);
        }
    }
    if has(Include::Artifacts) {
        sections.push(render_artifacts(index, &turns)?);
    }
    if has(Include::Diff) {
        sections.push(render_diffs(deps, &turns)?);
    }
    if has(Include::Transcript) {
        sections.push(render_transcript(index, task_id, args)?);
    }
    Ok(sections.join("\n\n"))
}

fn lookup_project_tasks(
    deps: &LookupDeps,
    project_path: &str,
    limit: i64,
) -> Result<String, ToolError> {
    let project = find_project_by_path(deps.index, project_path)?.ok_or_else(|| {
        ToolError::not_found(format!("no tasks recorded for project {project_path}"))
    })?;
    let snapshots = list_task_snapshots(deps.index, &project.project_id, limit)?;
    let mut lines =
        vec![format!("project: {}", project.root), format!("tasks: {}", snapshots.len())];
    for s in &snapshots {
        lines.push(format!(
            "  {}  {}  {}  turns={}  {}  {}",
            s.task_id,
            js::pad_end(&s.status, 9),
            js::pad_end(&s.worker, 6),
            s.turn_count,
            s.updated_at,
            s.prompt_summary
        ));
    }
    Ok(lines.join("\n"))
}

fn apply_scope(
    turns: Vec<TurnInfo>,
    scope: Option<&Scope>,
    task_id: &str,
) -> Result<Vec<TurnInfo>, ToolError> {
    let Some(scope) = scope else { return Ok(turns) };
    if let Some(turn_id) = &scope.turn_id {
        let hit: Vec<TurnInfo> = turns.into_iter().filter(|t| &t.turn_id == turn_id).collect();
        if hit.is_empty() {
            return Err(ToolError::not_found(format!("no turn {turn_id} in task {task_id}")));
        }
        return Ok(hit);
    }
    match scope.last {
        // `turns.slice(-last)`: the trailing N, and all of them when N is 0.
        Some(last) if last > 0 => {
            let keep = usize::try_from(last).unwrap_or(usize::MAX).min(turns.len());
            Ok(turns[turns.len() - keep..].to_vec())
        }
        _ => Ok(turns),
    }
}

fn render_summary(s: &TaskSnapshot) -> String {
    [
        format!("task: {}", s.task_id),
        format!("project: {}", s.project_root),
        format!("worker: {}{}", s.worker, native_session_suffix(s.worker_session_id.as_deref())),
        format!("status: {}", s.status),
        format!("about: {}", s.prompt_summary),
        format!("turns: {}", s.turn_count),
        format!("updated: {}", s.updated_at),
    ]
    .join("\n")
}

pub fn native_session_suffix(worker_session_id: Option<&str>) -> String {
    worker_session_id.map_or(String::new(), |id| format!(" (native session {id})"))
}

/// Ordered prompt/response exchange pairs, never loose audit rows.
fn render_exchanges(turns: &[TurnInfo]) -> String {
    let mut lines = vec!["exchanges:".to_string()];
    for turn in turns {
        lines.push(String::new());
        lines.push(format!("--- turn {} ({}, {})", turn.idx + 1, turn.turn_id, turn.status));
        lines.push(format!(">> {}", turn.prompt));
        match &turn.response {
            Some(response) => lines.push(format!("<< {response}")),
            None if turn.status == "running" => lines.push("<< (turn still running)".to_string()),
            None => {}
        }
        if turn.status == "failed" {
            lines.push(format!(
                "error {}: {}",
                turn.error_code.as_deref().unwrap_or("null"),
                turn.error_message.as_deref().unwrap_or("")
            ));
        }
        if turn.status == "canceled" {
            let reason = turn.error_message.as_deref().map_or(String::new(), |m| format!(": {m}"));
            lines.push(format!("canceled{reason}"));
        }
    }
    lines.join("\n")
}

/// End-to-end replay of one turn: inputs, worker activity, outputs.
fn render_trace(deps: &LookupDeps, turn: &TurnInfo) -> Result<String, ToolError> {
    let mut lines =
        vec![format!("trace: turn {} ({}, {})", turn.idx + 1, turn.turn_id, turn.status)];
    lines.push("inputs:".to_string());
    lines.push(format!("  prompt: {}", turn.prompt));
    lines.push(format!("  started: {}", turn.started_at));
    lines.push("activity:".to_string());
    let audit = get_turn_audit(deps.index, &turn.turn_id)?;
    if audit.is_empty() {
        lines.push("  (none captured)".to_string());
    }
    for row in &audit {
        lines.push(format!("  {}  {}  {}", row.ts, row.kind, compact_payload(&row.payload)));
    }
    lines.push("outputs:".to_string());
    lines.push(format!("  status: {}", turn.status));
    if let Some(response) = &turn.response {
        lines.push(format!("  response: {response}"));
    }
    if let Some(code) = &turn.error_code {
        lines.push(format!("  error {code}: {}", turn.error_message.as_deref().unwrap_or("")));
    }
    if !turn.changed_files.is_empty() {
        lines.push(format!("  changed files: {}", turn.changed_files.join(", ")));
    }
    for a in get_turn_artifacts(deps.index, &turn.turn_id)? {
        lines.push(format!(
            "  artifact: {}  {}  ({}, {} bytes)",
            a.artifact_id, a.kind, a.media_type, a.size_bytes
        ));
    }
    if let Some(completed) = &turn.completed_at {
        lines.push(format!("  completed: {completed}"));
    }
    Ok(lines.join("\n"))
}

fn render_audit(index: &StateIndex, turn: &TurnInfo) -> Result<String, ToolError> {
    let rows = get_turn_audit(index, &turn.turn_id)?;
    let mut lines =
        vec![format!("audit: turn {} ({}, {} events)", turn.idx + 1, turn.turn_id, rows.len())];
    for row in &rows {
        lines.push(format!("  {}  {}  {}", row.ts, row.kind, compact_payload(&row.payload)));
    }
    Ok(lines.join("\n"))
}

fn render_artifacts(index: &StateIndex, turns: &[TurnInfo]) -> Result<String, ToolError> {
    let mut lines = vec!["artifacts:".to_string()];
    for turn in turns {
        for a in get_turn_artifacts(index, &turn.turn_id)? {
            lines.push(format!(
                "  {}  {}  {}  ({}, {} bytes, sha256 {}…, turn {})",
                a.artifact_id,
                a.kind,
                a.label,
                a.media_type,
                a.size_bytes,
                js::slice_to(&a.sha256, 12),
                turn.idx + 1
            ));
        }
    }
    if lines.len() == 1 {
        lines.push("  (none)".to_string());
    }
    Ok(lines.join("\n"))
}

fn render_diffs(deps: &LookupDeps, turns: &[TurnInfo]) -> Result<String, ToolError> {
    let mut lines = vec!["diffs:".to_string()];
    for turn in turns {
        for a in get_turn_artifacts(deps.index, &turn.turn_id)? {
            if a.kind != "diff" {
                continue;
            }
            lines.push(format!("--- turn {} ({})", turn.idx + 1, a.artifact_id));
            let mut text = String::from_utf8_lossy(&deps.artifacts.read(&a.locator)?).into_owned();
            if js::len(&text) > DIFF_INLINE_LIMIT {
                text = format!(
                    "{}\n… truncated; full diff in artifact {}",
                    js::slice_to(&text, DIFF_INLINE_LIMIT),
                    a.artifact_id
                );
            }
            lines.push(js::trim_end(&text).to_string());
        }
    }
    if lines.len() == 1 {
        lines.push("  (no diff artifacts in scope)".to_string());
    }
    Ok(lines.join("\n"))
}

/// The worker's archived transcript for a task: every swept message from the
/// session(s) it ran under, in the requested view. Task-level — `scope.turn_id`
/// does not sub-select it; `scope.last` caps the number of messages shown, and
/// `prompt_idx` narrows to one exchange.
fn render_transcript(
    index: &StateIndex,
    task_id: &str,
    args: &LookupArgs,
) -> Result<String, ToolError> {
    let last = args.scope.as_ref().and_then(|s| s.last);
    let prompt_idx = args.view.prompt_idx;
    let view = match resolve_view(args.view, last) {
        TranscriptView::Outline => {
            let outline = get_task_outline(index, task_id, prompt_idx)?;
            let mut lines = vec!["transcript:".to_string()];
            lines.extend(outline_section(&outline, prompt_idx));
            return Ok(lines.join("\n"));
        }
        TranscriptView::Compact => MessageView::Compact,
        TranscriptView::Timeline => MessageView::Timeline,
    };
    let page = get_task_messages(index, task_id, message_query(view, last, prompt_idx))?;
    let mut lines = vec!["transcript:".to_string()];
    lines.extend(message_section(&page, "(no transcript recorded)", view, args.view));
    Ok(lines.join("\n"))
}

/// The rendered messages, or the empty statement; plus the cap notice.
fn message_section(
    page: &MessagePage,
    when_empty: &str,
    view: MessageView,
    args: ViewArgs,
) -> Vec<String> {
    if page.messages.is_empty() {
        return vec![no_messages(when_empty, args.prompt_idx)];
    }
    let mut lines =
        render_messages(&page.messages, view, args.tool_lines.unwrap_or(DEFAULT_TOOL_LINES));
    if page.capped {
        lines.push(format!(
            "  … capped at {} messages (raise scope.last for more)",
            page.messages.len()
        ));
    }
    lines
}

/// Corpus-wide transcript search rendered for the search-transcripts tool.
/// The query is optional — the structured filters stand on their own — but a
/// search with neither is a session listing, which lookup-session already
/// does better. Every hit prints its session and prompt index, so a result is
/// an address to drill into and not just a sighting.
pub fn search_transcripts(
    index: &StateIndex,
    query: Option<&str>,
    limit: i64,
    filters: &SearchFilters,
) -> Result<String, ToolError> {
    let structured = filters.tool.is_some() || filters.target.is_some() || filters.failed.is_some();
    let query = query.filter(|q| !q.is_empty());
    if query.is_none() && !structured {
        return Err(ToolError::invalid_request(
            "provide a query, or a tool / target / failed filter",
        ));
    }
    let hits = search_messages(index, query, limit, filters)
        .map_err(|err| ToolError::invalid_request(format!("invalid search query: {err}")))?;
    let what = describe_search(query, filters);
    if hits.is_empty() {
        return Ok(format!("no transcript matches for: {what}"));
    }
    let mut lines = vec![format!("transcript matches ({}) for {what}:", hits.len())];
    for h in &hits {
        let where_ = match &h.task_id {
            Some(task_id) => format!("task {task_id}"),
            None => format!("{} session {}", h.source, h.native_session_id),
        };
        let proj = h.project_path.as_deref().map_or(String::new(), |p| format!(" · {p}"));
        let ts = h.native_ts.as_deref().map_or(String::new(), |t| format!(" · {t}"));
        lines.push(format!(
            "  {where_}{proj} · {}/{} · prompt {}{ts}",
            h.role, h.kind, h.prompt_idx
        ));
        lines.push(format!("    {}", hit_body(h)));
    }
    Ok(lines.join("\n"))
}

/// The snippet where a text search highlighted one; otherwise the call
/// itself, which is the whole content of a structured hit.
fn hit_body(h: &TranscriptHit) -> String {
    if let Some(snippet) = &h.snippet {
        return truncate(snippet, 200);
    }
    // Truncated per part: collapsing the pair together would eat the gap that
    // separates the tool from what it acted on.
    let call = [&h.tool_name, &h.tool_target]
        .into_iter()
        .flatten()
        .map(|part| truncate(part, 200))
        .collect::<Vec<_>>()
        .join("  ");
    let label = if call.is_empty() { h.kind.clone() } else { call };
    format!("{label}{}", if h.is_error == Some(1) { "  ✗" } else { "" })
}

/// Echoes back what was actually searched for, so a filter-only search does
/// not report "no matches for: null".
fn describe_search(query: Option<&str>, filters: &SearchFilters) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(query) = query {
        parts.push(query.to_string());
    }
    if let Some(tool) = &filters.tool {
        parts.push(format!("tool {tool}"));
    }
    if let Some(target) = &filters.target {
        parts.push(format!("target ~ {target}"));
    }
    if let Some(failed) = filters.failed {
        parts.push(if failed { "failed" } else { "succeeded" }.to_string());
    }
    parts.join(", ")
}

#[derive(Debug, Clone, Default)]
pub struct SessionLookupArgs {
    pub session_id: Option<String>,
    pub project: Option<String>,
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub last: Option<i64>,
    /// Rendering of a single session's history; see [`resolve_view`].
    pub view: ViewArgs,
}

/// lookup-session: no session_id lists ingested transcript sessions
/// most-recent first (optionally filtered to a project); a session_id returns
/// that session's full history in order. Works for host sessions that no task
/// links, unlike lookup-task's transcript include. A bare id matching several
/// sources is not guessed — the candidates are listed for the caller to
/// disambiguate.
pub fn lookup_session(index: &StateIndex, args: &SessionLookupArgs) -> Result<String, ToolError> {
    let Some(session_id) = &args.session_id else {
        let sessions = list_sessions(
            index,
            &SessionListQuery {
                project: args.project.clone(),
                limit: args.limit,
                ..Default::default()
            },
        )?;
        return Ok(render_session_list(&sessions));
    };

    let matches = list_sessions(
        index,
        &SessionListQuery {
            native_session_id: Some(session_id.clone()),
            limit: Some(50),
            ..Default::default()
        },
    )?;
    let candidates: Vec<&SessionInfo> = match &args.source {
        Some(source) => matches.iter().filter(|m| &m.source == source).collect(),
        None => matches.iter().collect(),
    };
    let info = match candidates.as_slice() {
        [] => {
            let what =
                args.source.as_deref().map_or("session".to_string(), |s| format!("{s} session"));
            return Err(ToolError::not_found(format!("no ingested {what} {session_id}")));
        }
        [only] => *only,
        several => {
            let mut lines = vec![format!(
                "session id {session_id} matches {} sources; re-run with source=<one of>:",
                several.len()
            )];
            lines.extend(
                several
                    .iter()
                    .map(|m| format!("  source={}  ({} messages)", m.source, m.message_count)),
            );
            return Ok(lines.join("\n"));
        }
    };

    let prompt_idx = args.view.prompt_idx;
    let view = match resolve_view(args.view, args.last) {
        TranscriptView::Outline => {
            let outline =
                get_session_outline(index, &info.source, &info.native_session_id, prompt_idx)?;
            let mut lines = vec![session_header(info, prompt_idx)];
            lines.extend(outline_section(&outline, prompt_idx));
            return Ok(lines.join("\n"));
        }
        TranscriptView::Compact => MessageView::Compact,
        TranscriptView::Timeline => MessageView::Timeline,
    };
    let page = get_session_messages(
        index,
        &info.source,
        &info.native_session_id,
        message_query(view, args.last, prompt_idx),
    )?;
    let mut lines = vec![session_header(info, prompt_idx)];
    lines.extend(message_section(&page, "(no messages recorded)", view, args.view));
    Ok(lines.join("\n"))
}

fn render_session_list(sessions: &[SessionInfo]) -> String {
    if sessions.is_empty() {
        return "sessions: (none ingested)".to_string();
    }
    let mut lines = vec![format!("sessions ({}):", sessions.len())];
    for s in sessions {
        let when = s.last_ts.as_deref().unwrap_or(&s.last_recorded_at);
        let proj = s.project_path.as_deref().unwrap_or("(no project)");
        let task = s.task_id.as_deref().map_or(String::new(), |t| format!("  [task {t}]"));
        lines.push(format!(
            "  {}  {}  {when}  msgs={}  {proj}{task}",
            s.native_session_id,
            js::pad_end(&s.source, 11),
            s.message_count
        ));
    }
    lines.join("\n")
}

fn session_header(info: &SessionInfo, prompt_idx: Option<i64>) -> String {
    let proj = info.project_path.as_deref().map_or(String::new(), |p| format!(", {p}"));
    let at = prompt_idx.map_or(String::new(), |n| format!(" · prompt {n}"));
    format!("session {} ({}{proj}){at}:", info.native_session_id, info.source)
}
