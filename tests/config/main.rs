//! Config, paths and the harness table.

use std::num::NonZeroU64;
use std::path::Path;

use taskrunner::config::{
    Config, Delegation, HarnessKind, HostKind, Provider, load_config, parse_config, worker_config,
};
use taskrunner::harnesses::{auth_mounts, ingest_sources, worker_kind, worker_names};
use taskrunner::paths::state_paths;

// ---- workers: pluggable, from config alone ---------------------------------

#[test]
fn always_provides_the_built_in_codex_and_claude_workers() {
    let config = parse_config("").unwrap();
    assert_eq!(worker_names(&config), vec!["codex", "claude"]);
    assert_eq!(worker_kind(&config, "codex"), Some(HarnessKind::Codex));
    assert_eq!(worker_kind(&config, "claude"), Some(HarnessKind::Claude));
    assert_eq!(worker_config(&config, "codex").image.as_deref(), Some("taskrunner/codex-worker"));
    assert_eq!(
        worker_config(&config, "claude").auth_volume.as_deref(),
        Some("taskrunner-claude-home")
    );
}

#[test]
fn brings_a_custom_worker_to_life_from_config_alone() {
    let config = parse_config(
        r#"
        [worker.qwen]
        harness = "codex"
        model = "qwen2.5-coder:32b"
        provider = "ollama"
        allowed_domains = ["host.docker.internal:11434"]
        "#,
    )
    .unwrap();
    assert_eq!(worker_kind(&config, "qwen"), Some(HarnessKind::Codex));
    // The catch-all keeps custom worker sections intact.
    let cfg = worker_config(&config, "qwen");
    assert_eq!(cfg.model.as_deref(), Some("qwen2.5-coder:32b"));
    assert_eq!(cfg.provider, Some(Provider::Ollama));
    assert_eq!(cfg.allowed_domains, vec!["host.docker.internal:11434"]);
    assert_eq!(cfg.auth_volume, None);
}

#[test]
fn skips_workers_whose_harness_kind_does_not_exist() {
    let config = parse_config("[worker.mystery]\nmodel = \"x\"\n").unwrap();
    assert_eq!(worker_kind(&config, "mystery"), None);
    assert!(worker_names(&config).contains(&"mystery".to_string()));
}

#[test]
fn rejects_unknown_harness_kinds_at_config_parse_time() {
    assert!(parse_config("[worker.q]\nharness = \"not-a-harness\"\n").is_err());
}

#[test]
fn keeps_built_in_defaults_for_keys_a_section_leaves_out() {
    let config =
        parse_config("[worker.claude]\nmodel = \"m\"\nlimits = { memory = \"8g\" }\n").unwrap();
    let claude = worker_config(&config, "claude");
    assert_eq!(claude.model.as_deref(), Some("m"));
    assert_eq!(claude.image.as_deref(), Some("taskrunner/claude-worker"));
    assert_eq!(claude.allowed_domains.len(), 4);
    assert_eq!(claude.limits.memory, "8g");
    assert_eq!(claude.limits.cpus, 2.0);
    assert_eq!(claude.limits.pids, NonZeroU64::new(512).unwrap());
}

#[test]
fn an_empty_list_written_out_stays_empty() {
    let text = "[worker.codex]\nallowed_domains = []\n[ingest.sources.claude-code]\ndirs = []\n";
    let config = parse_config(text).unwrap();
    assert!(worker_config(&config, "codex").allowed_domains.is_empty());
    assert!(config.ingest.sources["claude-code"].dirs.is_empty());
    assert_eq!(config.ingest.sources["codex"].dirs, vec!["~/.codex/sessions"]);
}

// ---- ingest sources ----------------------------------------------------------

#[test]
fn a_stray_key_on_an_ingest_source_is_refused_not_dropped() {
    assert!(parse_config("[ingest.sources.codex]\nvolume = \"x\"\n").is_err());
    assert!(parse_config("[ingest.sources.hermes]\ndirs = [\"/x\"]\n").is_err()); // no format
}

#[test]
fn derives_one_volume_source_per_worker_with_an_auth_volume() {
    let config = parse_config(
        r#"
        [worker.qwen]
        harness = "codex"
        auth_volume = "taskrunner-codex-home"

        [ingest.sources.hermes]
        format = "codex"
        dirs = ["~/.hermes"]
        "#,
    )
    .unwrap();
    let sources = ingest_sources(&config);
    let hosts: Vec<(&str, &[String])> = sources
        .iter()
        .filter(|s| s.volume.is_none())
        .map(|s| (s.format.as_str(), s.dirs.as_slice()))
        .collect();
    assert_eq!(hosts.len(), 3);
    assert_eq!(hosts[0], ("claude-code", &["~/.claude/projects".to_string()][..]));
    assert_eq!(hosts[2], ("codex", &["~/.hermes".to_string()][..]));
    // codex and qwen share a volume: ingested once, with the worker's layout.
    let volumes: Vec<(&str, &str, &str, &str)> = sources
        .iter()
        .filter_map(|s| {
            Some((
                s.format.as_str(),
                s.volume.as_deref()?,
                s.subdir.as_deref()?,
                s.image.as_deref()?,
            ))
        })
        .collect();
    assert_eq!(
        volumes,
        vec![
            ("codex", "taskrunner-codex-home", "sessions", "taskrunner/codex-worker"),
            (
                "claude-code",
                "taskrunner-claude-home",
                ".claude/projects",
                "taskrunner/claude-worker"
            ),
        ]
    );
}

#[test]
fn mounts_only_the_auth_paths_of_each_harness() {
    let claude: Vec<_> =
        auth_mounts(HarnessKind::Claude).iter().map(|m| m.container_path).collect();
    assert_eq!(claude, vec!["/home/worker/.claude", "/home/worker/.claude.json"]);
    assert_eq!(auth_mounts(HarnessKind::Codex)[0].subpath, None);
}

// ---- files -------------------------------------------------------------------

#[test]
fn a_missing_config_file_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(&dir.path().join("config.toml")).unwrap();
    assert_eq!(config, Config::default_loaded());
}

#[test]
fn state_paths_lay_out_the_root() {
    let paths = state_paths(Path::new("/s"));
    assert_eq!(paths.events_log, Path::new("/s/events.jsonl"));
    assert_eq!(paths.socket_path, Path::new("/s/runtime/daemon.sock"));
    assert_eq!(paths.lock_file, Path::new("/s/runtime/daemon.lock"));
    assert_eq!(paths.ingest_staging_dir, Path::new("/s/ingest-staging"));
    assert_eq!(paths.skills_dir, Path::new("/s/skills"));
}

// ---- hosts: harnesses that run taskrunner ----------------------------------

#[test]
fn reads_host_settings_and_defaults_delegation_to_suggest() {
    let config = parse_config(
        r#"
        [host.claude]
        connected = true

        [host.hermes]
        connected = true
        delegation = "on-request"
        "#,
    )
    .unwrap();
    assert!(config.host(HostKind::Claude).unwrap().connected);
    assert_eq!(config.delegation(Some(HostKind::Claude)), Delegation::Suggest);
    assert_eq!(config.delegation(Some(HostKind::Hermes)), Delegation::OnRequest);
    // A harness sync never met, and a session that didn't name its harness.
    assert_eq!(config.delegation(Some(HostKind::Codex)), Delegation::Suggest);
    assert_eq!(config.delegation(None), Delegation::Suggest);
}

#[test]
fn refuses_an_unknown_harness_and_a_stray_host_key() {
    assert!(parse_config("[host.cursor]\nconnected = true\n").is_err());
    assert!(parse_config("[host.claude]\nconected = true\n").is_err());
    assert!(parse_config("[host.claude]\ndelegation = \"always\"\n").is_err());
}
