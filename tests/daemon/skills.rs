//! Skills over MCP (SEP-2640), through a real connection to the daemon's MCP
//! socket: the capability, `skills/list`, `skills/get`, the skill resources,
//! rendering per host, and the audit record sync reads.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{
    ClientInfo, ClientRequest, CustomRequest, ReadResourceRequestParams, ResourceContents,
    ServerResult,
};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use taskrunner::daemon::{Daemon, DaemonOptions, HOST_PREAMBLE};
use taskrunner::paths::StatePaths;
use taskrunner::storage::events::{EventBody, read_events};
use tokio::io::AsyncWriteExt;

use crate::{client_info, short_root};

type Client = RunningService<RoleClient, ClientInfo>;

async fn daemon_with(config: &str) -> (tempfile::TempDir, StatePaths, Daemon) {
    let (dir, paths) = short_root();
    std::fs::write(&paths.config_file, config).unwrap();
    let options = DaemonOptions { ingest_sources: Some(vec![]), ..Default::default() };
    let daemon = Daemon::start(paths.clone(), options).await.unwrap();
    (dir, paths, daemon)
}

/// Connects the way a registered shim does: an optional host line, then MCP.
async fn connect(paths: &StatePaths, host: Option<&str>) -> Client {
    let mut stream = tokio::net::UnixStream::connect(&paths.mcp_socket_path).await.unwrap();
    if let Some(host) = host {
        stream.write_all(format!("{HOST_PREAMBLE}{host}\n").as_bytes()).await.unwrap();
    }
    let (reader, writer) = stream.into_split();
    client_info("skills-test").serve((reader, writer)).await.unwrap()
}

async fn custom(client: &Client, method: &str, params: Value) -> Result<Value, String> {
    let request = ClientRequest::CustomRequest(CustomRequest::new(method, Some(params)));
    match client.peer().send_request(request).await {
        Ok(ServerResult::CustomResult(result)) => Ok(result.0),
        Ok(other) => panic!("expected a custom result, got {other:?}"),
        Err(err) => Err(err.to_string()),
    }
}

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["frontmatter"]["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {list}"))
}

#[tokio::test]
async fn declares_the_extension_and_serves_every_skill_with_matching_digests() {
    let (_dir, paths, daemon) = daemon_with("").await;
    let client = connect(&paths, None).await;

    let capabilities = &client.peer_info().expect("server info").capabilities;
    assert!(capabilities.resources.is_some(), "the extension requires resources");
    let extensions = capabilities.extensions.as_ref().expect("extensions declared");
    assert!(extensions.contains_key("io.modelcontextprotocol/skills"));

    let list = custom(&client, "skills/list", json!({})).await.unwrap();
    assert_eq!(list["resultType"], "complete");
    let mut names: Vec<&str> = list["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["frontmatter"]["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["archive-search", "delegate-task", "handoff", "setup-harness", "worker-login"]
    );

    let delegate = entry(&list, "delegate-task");
    let uri = delegate["uri"].as_str().unwrap();
    assert_eq!(uri, "skill://delegate-task/SKILL.md");
    let read = client.peer().read_resource(ReadResourceRequestParams::new(uri)).await.unwrap();
    let ResourceContents::TextResourceContents { text, mime_type, .. } = &read.contents[0] else {
        panic!("a skill is served as text");
    };
    assert_eq!(mime_type.as_deref(), Some("text/markdown"));
    let digest = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
    assert_eq!(delegate["resources"][0]["digest"], digest);
    assert_eq!(delegate["resources"][0]["size"], text.len());
    assert_eq!(delegate["digest"], digest, "the digest Claude Code's client checks");

    let listed = client.peer().list_all_resources().await.unwrap();
    assert_eq!(listed.len(), 5);
    let audited: Vec<String> = read_events(&paths.events_log)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AuditRecorded { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    for kind in ["skills.list", "resource.read", "resources.list"] {
        assert!(audited.iter().any(|k| k == kind), "{kind} not audited: {audited:?}");
    }

    client.cancel().await.unwrap();
    daemon.stop().await;
}

#[tokio::test]
async fn renders_skills_for_the_connections_host_and_records_the_host() {
    let config = "[host.hermes]\nconnected = true\ndelegation = \"on-request\"\n";
    let (_dir, paths, daemon) = daemon_with(config).await;
    let hermes = connect(&paths, Some("hermes")).await;
    let unlabelled = connect(&paths, None).await;

    let for_hermes = custom(&hermes, "skills/list", json!({})).await.unwrap();
    let description = entry(&for_hermes, "delegate-task")["frontmatter"]["description"].to_string();
    assert!(description.contains("only when the user explicitly asks"), "{description}");
    let for_anyone = custom(&unlabelled, "skills/list", json!({})).await.unwrap();
    let description = entry(&for_anyone, "delegate-task")["frontmatter"]["description"].to_string();
    assert!(description.contains("offer it"), "{description}");

    let events = read_events(&paths.events_log).unwrap();
    let hermes_session = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::SessionStarted { session_id, host: Some(host), .. } if host == "hermes" => {
                Some(session_id.clone())
            }
            _ => None,
        })
        .expect("the hermes session is recorded with its host");
    assert!(events.iter().any(|e| matches!(
        &e.body,
        EventBody::AuditRecorded { session_id: Some(s), kind, .. }
            if *s == hermes_session && kind == "skills.list"
    )));

    hermes.cancel().await.unwrap();
    unlabelled.cancel().await.unwrap();
    daemon.stop().await;
}

#[tokio::test]
async fn skills_get_answers_for_a_served_skill_and_refuses_what_it_does_not_serve() {
    let (_dir, paths, daemon) = daemon_with("").await;
    let client = connect(&paths, None).await;

    let got = custom(&client, "skills/get", json!({ "uri": "skill://worker-login/SKILL.md" }))
        .await
        .unwrap();
    assert_eq!(got["skill"]["frontmatter"]["name"], "worker-login");

    let unknown = custom(&client, "skills/get", json!({ "uri": "skill://nope/SKILL.md" })).await;
    assert!(unknown.unwrap_err().contains("no skill at skill://nope/SKILL.md"));
    let unread =
        client.peer().read_resource(ReadResourceRequestParams::new("skill://nope/SKILL.md")).await;
    assert!(unread.is_err());
    assert!(custom(&client, "skills/nothing", json!({})).await.is_err());

    client.cancel().await.unwrap();
    daemon.stop().await;
}

#[tokio::test]
async fn a_client_whose_first_message_starts_with_whitespace_is_still_served() {
    let (_dir, paths, daemon) = daemon_with("").await;
    let mut stream = tokio::net::UnixStream::connect(&paths.mcp_socket_path).await.unwrap();
    // Valid JSON-RPC may lead with whitespace; it is not a host line.
    stream.write_all(b" ").await.unwrap();
    let (reader, writer) = stream.into_split();
    let client =
        tokio::time::timeout(Duration::from_secs(5), client_info("spaced").serve((reader, writer)))
            .await
            .expect("the initialize message was swallowed")
            .unwrap();
    client.peer().list_tools(None).await.unwrap();
    client.cancel().await.unwrap();
    daemon.stop().await;
}
