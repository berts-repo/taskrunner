//! Global state root layout (default ~/.taskrunner/).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePaths {
    pub root: PathBuf,
    pub events_log: PathBuf,
    pub index_db: PathBuf,
    pub artifacts_dir: PathBuf,
    pub workspaces_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_file: PathBuf,
    pub ingest_state_file: PathBuf,
    pub ingest_staging_dir: PathBuf,
    /// Taskrunner's skills, rendered per host: `skills/<host>/<skill>/SKILL.md`.
    pub skills_dir: PathBuf,
    /// HTTP: `/status` and the read-only query routes.
    pub socket_path: PathBuf,
    /// One MCP session per connection, newline-delimited JSON-RPC.
    pub mcp_socket_path: PathBuf,
    pub pid_file: PathBuf,
    pub lock_file: PathBuf,
}

pub fn default_root() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".taskrunner")
}

pub fn state_paths(root: &Path) -> StatePaths {
    let runtime_dir = root.join("runtime");
    StatePaths {
        root: root.to_path_buf(),
        events_log: root.join("events.jsonl"),
        index_db: root.join("index.db"),
        artifacts_dir: root.join("artifacts"),
        workspaces_dir: root.join("workspaces"),
        logs_dir: root.join("logs"),
        config_file: root.join("config.toml"),
        ingest_state_file: root.join("ingest-state.json"),
        ingest_staging_dir: root.join("ingest-staging"),
        skills_dir: root.join("skills"),
        socket_path: runtime_dir.join("daemon.sock"),
        mcp_socket_path: runtime_dir.join("mcp.sock"),
        pid_file: runtime_dir.join("daemon.pid"),
        lock_file: runtime_dir.join("daemon.lock"),
        runtime_dir,
    }
}
