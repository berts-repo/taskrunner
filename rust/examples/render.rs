//! Renders a fixed set of views over an index: the Rust half of
//! scripts/parity-views.sh, whose TypeScript half renders the same set.
//! Usage: cargo run --example render -- <index.db>

use taskrunner::domain::errors::ToolError;
use taskrunner::domain::tasks::{SearchFilters, SearchSort, SessionListQuery, list_sessions};
use taskrunner::storage::artifacts::ArtifactStore;
use taskrunner::storage::index::StateIndex;
use taskrunner::view::lookup::{
    Include, LookupArgs, LookupDeps, SessionLookupArgs, ViewArgs, lookup_session, lookup_task,
    search_transcripts,
};
use taskrunner::view::transcript::TranscriptView;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: render <index.db>");
    let index = StateIndex::open(&path)?;
    let artifacts = ArtifactStore::new(std::path::Path::new("/nonexistent"));
    let deps = LookupDeps { index: &index, artifacts: &artifacts };
    let mut out: Vec<String> = Vec::new();
    let mut section = |name: String, result: Result<String, ToolError>| {
        out.push(format!("===== {name}"));
        out.push(match result {
            Ok(text) => text,
            Err(err) => format!("ERR {}", err.message),
        });
    };
    let session = |id: &str,
                   source: &str,
                   view: Option<TranscriptView>,
                   prompt_idx: Option<i64>,
                   last: Option<i64>| SessionLookupArgs {
        session_id: Some(id.into()),
        source: Some(source.into()),
        last,
        view: ViewArgs { view, prompt_idx, tool_lines: None },
        ..Default::default()
    };

    section(
        "sessions".into(),
        lookup_session(&index, &SessionLookupArgs { limit: Some(50), ..Default::default() }),
    );
    for s in list_sessions(&index, &SessionListQuery { limit: Some(6), ..Default::default() })? {
        let (id, src) = (s.native_session_id.as_str(), s.source.as_str());
        section(
            format!("outline {id}"),
            lookup_session(&index, &session(id, src, None, None, None)),
        );
        section(
            format!("compact {id}"),
            lookup_session(&index, &session(id, src, Some(TranscriptView::Compact), None, None)),
        );
        section(
            format!("timeline {id}"),
            lookup_session(&index, &session(id, src, Some(TranscriptView::Timeline), None, None)),
        );
        section(
            format!("prompt2 {id}"),
            lookup_session(&index, &session(id, src, None, Some(2), None)),
        );
        section(
            format!("last3 {id}"),
            lookup_session(&index, &session(id, src, None, None, Some(3))),
        );
    }
    let searches = [
        ("search rust", Some("rust"), SearchFilters::default()),
        (
            "search rust recent",
            Some("rust"),
            SearchFilters {
                sort: SearchSort::Recent,
                role: Some("user".into()),
                ..Default::default()
            },
        ),
        (
            "search tool Edit",
            None,
            SearchFilters { tool: Some("Edit".into()), ..Default::default() },
        ),
        ("search failed", None, SearchFilters { failed: Some(true), ..Default::default() }),
        (
            "search target",
            None,
            SearchFilters {
                target: Some("proxy".into()),
                failed: Some(false),
                ..Default::default()
            },
        ),
        (
            "search last sessions",
            Some("proxy"),
            SearchFilters { last_sessions: Some(5), ..Default::default() },
        ),
    ];
    for (name, query, filters) in searches {
        section(name.into(), search_transcripts(&index, query, 30, &filters));
    }

    let mut stmt = index.db.prepare("SELECT id FROM tasks ORDER BY id")?;
    let tasks: Vec<String> = stmt.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
    for task_id in tasks {
        let all = vec![
            Include::Turns,
            Include::Trace,
            Include::Audit,
            Include::Artifacts,
            Include::Transcript,
        ];
        section(
            format!("task {task_id}"),
            lookup_task(
                &deps,
                &LookupArgs { task_id: Some(task_id.clone()), include: all, ..Default::default() },
            ),
        );
        let view = ViewArgs {
            view: Some(TranscriptView::Timeline),
            tool_lines: Some(5),
            prompt_idx: None,
        };
        section(
            format!("task tl {task_id}"),
            lookup_task(
                &deps,
                &LookupArgs {
                    task_id: Some(task_id),
                    include: vec![Include::Transcript],
                    view,
                    ..Default::default()
                },
            ),
        );
    }
    println!("{}", out.join("\n"));
    Ok(())
}
