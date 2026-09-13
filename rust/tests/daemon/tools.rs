//! The MCP tool surface, through a real connection to the daemon's MCP socket.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Implementation,
};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Map, Value, json};
use taskrunner::client;
use taskrunner::daemon::{Daemon, DaemonOptions};
use taskrunner::paths::StatePaths;
use taskrunner::storage::events::{EventBody, LogEvent, read_events};
use taskrunner::workers::harness::WorkerHarness;

use crate::helpers::{FakeHarness, ProjectRootWorkspaces};
use crate::short_root;

struct Stack {
    daemon: Daemon,
    client: RunningService<RoleClient, ClientInfo>,
    paths: StatePaths,
    project: tempfile::TempDir,
    _dir: tempfile::TempDir,
}

async fn stack() -> Stack {
    let (dir, paths) = short_root();
    let mut harnesses: HashMap<String, Arc<dyn WorkerHarness>> = HashMap::new();
    harnesses.insert("fake".into(), Arc::new(FakeHarness::default()));
    let daemon = Daemon::start(
        paths.clone(),
        DaemonOptions {
            ingest_sources: Some(vec![]),
            harnesses: Some(harnesses),
            workspaces: Some(Arc::new(ProjectRootWorkspaces)),
            make_runner: None,
        },
    )
    .await
    .unwrap();
    let stream = tokio::net::UnixStream::connect(&paths.mcp_socket_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let info =
        ClientInfo::new(ClientCapabilities::default(), Implementation::new("tools-test", "0.0.1"));
    let client = info.serve((reader, writer)).await.unwrap();
    Stack { daemon, client, paths, project: tempfile::tempdir().unwrap(), _dir: dir }
}

impl Stack {
    async fn call(&self, name: &str, args: Value) -> CallToolResult {
        let Value::Object(args) = args else { panic!("arguments must be an object") };
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(args);
        self.client.peer().call_tool(params).await.unwrap()
    }

    async fn close(self) {
        self.client.cancel().await.unwrap();
        self.daemon.stop().await;
    }
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_id_in(text: &str) -> String {
    text.lines().find_map(|line| line.strip_prefix("task: ")).expect("a task line").to_string()
}

#[tokio::test]
async fn lists_the_delegation_and_transcript_tools() {
    let st = stack().await;
    let tools = st.client.peer().list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "assign-task",
            "cancel-task",
            "continue-task",
            "lookup-session",
            "lookup-task",
            "search-transcripts"
        ]
    );
    st.close().await;
}

#[tokio::test]
async fn assign_task_with_wait_then_lookup_task_shows_the_exchange() {
    let st = stack().await;
    let project = st.project.path().to_string_lossy().into_owned();
    let assign = st
        .call(
            "assign-task",
            json!({ "project": project, "worker": "fake", "prompt": "make it so", "wait": true }),
        )
        .await;
    let assign_text = text(&assign);
    assert_ne!(assign.is_error, Some(true));
    assert!(assign_text.contains("status: completed"));
    let task_id = task_id_in(&assign_text);

    let lookup = st.call("lookup-task", json!({ "taskId": task_id, "include": ["turns"] })).await;
    let lookup_text = text(&lookup);
    assert!(lookup_text.contains(">> make it so"));
    assert!(lookup_text.contains("<< echo: make it so"));
    st.close().await;
}

#[tokio::test]
async fn continue_task_on_a_running_turn_reports_conflict_and_cancel_task_stops_it() {
    let st = stack().await;
    let project = st.project.path().to_string_lossy().into_owned();
    let assign = st
        .call(
            "assign-task",
            json!({ "project": project, "worker": "fake", "prompt": "sleep:10000" }),
        )
        .await;
    let task_id = task_id_in(&text(&assign));

    let conflict = st.call("continue-task", json!({ "taskId": task_id, "prompt": "more" })).await;
    assert_eq!(conflict.is_error, Some(true));
    assert!(text(&conflict).contains("error conflict:"));

    let cancel =
        st.call("cancel-task", json!({ "taskId": task_id, "reason": "test cleanup" })).await;
    assert!(text(&cancel).contains("status: canceled"));
    st.close().await;
}

#[tokio::test]
async fn renders_a_transcript_identically_for_the_tool_and_the_clis_read_route() {
    let st = stack().await;
    // Seed the projection directly: this asserts about rendering, not ingest.
    let records =
        [("user", "why did it fail"), ("assistant", "because the socket path was too long")];
    for (i, (role, content)) in records.iter().enumerate() {
        let event = LogEvent {
            id: format!("evt-{i}"),
            ts: format!("2026-07-25T00:00:0{i}.000Z"),
            body: EventBody::MessageRecorded {
                message_id: format!("msg-{i}"),
                source: "claude-code".into(),
                native_session_id: "sess-mcp".into(),
                native_record_id: format!("r{i}"),
                role: role.to_string(),
                kind: "message".into(),
                content: content.to_string(),
                native_ts: Some(format!("2026-07-25T00:00:0{i}.000Z")),
                project_path: None,
            },
        };
        st.daemon.store.lock().index.apply(&event).unwrap();
    }

    let via_tool = text(
        &st.call(
            "lookup-session",
            json!({ "sessionId": "sess-mcp", "view": "timeline", "prompt": 1 }),
        )
        .await,
    );
    let via_route = client::get(
        &st.paths.socket_path,
        "/lookup-session?sessionId=sess-mcp&view=timeline&prompt=1",
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap()
    .body;
    assert!(via_tool.contains("── [1] user"));
    assert!(via_tool.contains("because the socket path was too long"));
    assert_eq!(via_route.trim(), via_tool.trim());
    st.close().await;
}

#[tokio::test]
async fn rejects_an_unknown_view() {
    let st = stack().await;
    let res = client::get(
        &st.paths.socket_path,
        "/lookup-session?sessionId=sess-mcp&view=verbose",
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(res.status, 400);
    assert!(res.body.contains("view must be one of"));
    st.close().await;
}

#[tokio::test]
async fn maps_unknown_workers_to_not_configured_and_audits_tool_calls() {
    let st = stack().await;
    let project = st.project.path().to_string_lossy().into_owned();
    let result = st
        .call("assign-task", json!({ "project": project, "worker": "gemini", "prompt": "x" }))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("error not_configured:"));

    let kinds: Vec<String> = read_events(&st.paths.events_log)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AuditRecorded { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&"tool.assign-task".to_string()));
    st.close().await;
}

#[tokio::test]
async fn refuses_arguments_the_schema_does_not_allow_before_running_anything() {
    let st = stack().await;
    let params = CallToolRequestParams::new("lookup-task")
        .with_arguments(Map::from_iter([("limit".to_string(), json!(999))]));
    let err = st.client.peer().call_tool(params).await.unwrap_err();
    assert!(err.to_string().contains("invalid arguments for lookup-task"), "{err}");
    // Nothing was audited: the call never reached the tool.
    let audited = read_events(&st.paths.events_log)
        .unwrap()
        .iter()
        .any(|e| matches!(e.body, EventBody::AuditRecorded { .. }));
    assert!(!audited);
    st.close().await;
}
