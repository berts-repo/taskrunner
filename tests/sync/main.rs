//! `taskrunner sync` end to end: the real binary against stub harness CLIs on
//! PATH, in a throwaway HOME. The stubs keep their "registration" in a file,
//! so a second sync sees what the first one did.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use taskrunner::storage::events::{EventBody, LogEvent};
use taskrunner::storage::index::rebuild_index;

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

const CLAUDE: &str = r#"#!/bin/sh
case "$1 $2" in
  "auth status")
    if [ -f "$HOME/claude-signed-out" ]; then echo '{"loggedIn": false}'; else echo '{"loggedIn": true}'; fi ;;
  "mcp get")
    [ -f "$HOME/claude-mcp" ] || exit 1
    printf 'taskrunner:\n  Command: /opt/taskrunner\n  Args: %s\n' "$(cat "$HOME/claude-mcp")" ;;
  "mcp remove") rm -f "$HOME/claude-mcp" ;;
  "mcp add") shift 7; echo "$*" > "$HOME/claude-mcp" ;;
esac
"#;

const CODEX: &str = r#"#!/bin/sh
case "$1 $2" in
  "login status") echo "Logged in using ChatGPT" ;;
  "mcp get")
    [ -f "$HOME/codex-mcp" ] || exit 1
    args=$(sed 's/[^ ][^ ]*/"&"/g; s/" "/", "/g' "$HOME/codex-mcp")
    printf '{"transport":{"command":"/opt/taskrunner","args":[%s]}}\n' "$args" ;;
  "mcp remove") rm -f "$HOME/codex-mcp" ;;
  "mcp add") shift 5; echo "$*" > "$HOME/codex-mcp" ;;
esac
"#;

const HERMES: &str = r#"#!/bin/sh
[ "$1 $2" = "auth status" ] && echo "$3: logged in"
"#;

fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let (home, bin, state) =
        (dir.path().join("home"), dir.path().join("bin"), dir.path().join("state"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&bin).unwrap();
    Machine { _dir: dir, home, bin, state }
}

impl Machine {
    fn install(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// No terminal on stdin, as when an agent runs it.
    fn sync(&self, args: &[&str]) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_taskrunner"))
            .arg("sync")
            .args(args)
            .arg("--state-root")
            .arg(&self.state)
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("HERMES_HOME")
            .env_remove("TASKRUNNER_STATE_ROOT")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{text}{}", String::from_utf8_lossy(&output.stderr));
        text
    }

    fn claude_skill(&self, name: &str) -> PathBuf {
        self.home.join(".claude/skills").join(name)
    }

    fn config(&self) -> String {
        fs::read_to_string(self.state.join("config.toml")).unwrap_or_default()
    }
}

fn link_target(link: &Path) -> Option<PathBuf> {
    fs::read_link(link).ok()
}

#[test]
fn connects_a_harness_once_and_a_second_run_changes_nothing() {
    let m = machine();
    m.install("claude", CLAUDE);
    m.install("codex", CODEX);

    let out = m.sync(&["--connect", "claude", "--skip", "codex", "--delegation", "on-request"]);
    assert!(out.contains("claude: registered taskrunner"), "{out}");
    assert!(out.contains("claude: linked delegate-task"), "{out}");

    let config = m.config();
    assert!(config.contains("[host.claude]\nconnected = true\ndelegation = \"on-request\""));
    assert!(config.contains("[host.codex]\nconnected = false"));

    let registered = fs::read_to_string(m.home.join("claude-mcp")).unwrap();
    let expected = format!("mcp --host claude --state-root {}", m.state.display());
    assert_eq!(registered.trim(), expected);

    let skill = m.state.join("skills/claude/delegate-task");
    assert_eq!(link_target(&m.claude_skill("delegate-task")), Some(skill.clone()));
    let file = skill.join("SKILL.md");
    assert!(fs::read_to_string(&file).unwrap().contains("only when the user explicitly asks"));
    assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o222, 0, "read-only");
    // Skipped: no registration, no skills.
    assert!(!m.home.join("codex-mcp").exists());
    assert!(!m.home.join(".codex/skills").exists());

    let again = m.sync(&[]);
    assert!(again.contains("nothing to change"), "{again}");
}

#[test]
fn replaces_a_registration_made_before_hosts_existed() {
    let m = machine();
    m.install("codex", CODEX);
    fs::write(m.home.join("codex-mcp"), "mcp\n").unwrap();

    let out = m.sync(&["--connect", "codex"]);
    // The old registration's command no longer exists, so sync picks a real one.
    assert!(out.contains("codex: registered taskrunner: "), "{out}");
    assert!(out.contains(" mcp --host codex --state-root "), "{out}");
    assert!(fs::read_to_string(m.home.join("codex-mcp")).unwrap().starts_with("mcp --host codex"));
}

#[test]
fn a_signed_out_harness_gets_the_login_command_and_nothing_else() {
    let m = machine();
    m.install("claude", CLAUDE);
    fs::write(m.home.join("claude-signed-out"), "").unwrap();

    let out = m.sync(&["--connect", "claude"]);
    assert!(out.contains("run `claude auth login`"), "{out}");
    assert!(!m.home.join("claude-mcp").exists());
    assert!(!m.claude_skill("delegate-task").exists());
}

#[test]
fn a_found_harness_nobody_can_be_asked_about_is_only_mentioned() {
    let m = machine();
    m.install("codex", CODEX);

    let out = m.sync(&[]);
    assert!(out.contains("codex: found, not set up yet"), "{out}");
    assert!(!m.config().contains("[host.codex]"));
}

#[test]
fn leaves_a_skill_it_did_not_make_alone_and_removes_its_own_stale_links() {
    let m = machine();
    m.install("claude", CLAUDE);
    let skills = m.home.join(".claude/skills");
    fs::create_dir_all(skills.join("archive-search")).unwrap();
    std::os::unix::fs::symlink(
        m.state.join("skills/claude/retired-skill"),
        skills.join("retired-skill"),
    )
    .unwrap();

    let out = m.sync(&["--connect", "claude"]);
    assert!(out.contains("archive-search is not taskrunner's skill; left it alone"), "{out}");
    assert!(fs::symlink_metadata(skills.join("archive-search")).unwrap().is_dir());
    assert!(out.contains("claude: unlinked retired-skill"), "{out}");
    assert!(fs::symlink_metadata(skills.join("retired-skill")).is_err());
}

#[test]
fn hermes_gets_the_lines_to_add_and_its_config_is_never_edited() {
    let m = machine();
    m.install("hermes", HERMES);
    let config = m.home.join(".hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original =
        "# mine\nmodel:\n  provider: openai-codex\nskills:\n  creation_nudge_interval: 15\n";
    fs::write(&config, original).unwrap();

    let out = m.sync(&["--connect", "hermes"]);
    assert!(out.contains("mcp_servers:") && out.contains("args: [mcp, --host, hermes"), "{out}");
    assert!(out.contains("external_dirs:"), "{out}");
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(m.state.join("skills/hermes/delegate-task/SKILL.md").exists());

    let skills = m.state.join("skills/hermes");
    fs::write(
        &config,
        format!(
            "{original}  external_dirs:\n    - {}\nmcp_servers:\n  taskrunner:\n    command: /opt/taskrunner\n    args: [mcp, --host, hermes, --state-root, {}]\n",
            skills.display(),
            m.state.display()
        ),
    )
    .unwrap();
    let again = m.sync(&[]);
    assert!(again.contains("nothing to change"), "{again}");
}

#[test]
fn a_harness_that_gets_skills_over_mcp_loses_its_links() {
    let m = machine();
    m.install("claude", CLAUDE);
    m.sync(&["--connect", "claude"]);
    assert!(m.claude_skill("delegate-task").exists());

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let event = |id: &str, body| LogEvent { id: id.into(), ts: now.clone(), body };
    let events = [
        event(
            "evt_1",
            EventBody::SessionStarted {
                session_id: "sess_1".into(),
                project_id: None,
                client: Some("claude-code".into()),
                host: Some("claude".into()),
            },
        ),
        event(
            "evt_2",
            EventBody::AuditRecorded {
                session_id: Some("sess_1".into()),
                task_id: None,
                turn_id: None,
                kind: "skills.list".into(),
                payload: serde_json::json!({}),
            },
        ),
    ];
    rebuild_index(&m.state.join("index.db").to_string_lossy(), &events).unwrap();

    let out = m.sync(&[]);
    assert!(out.contains("unlinked delegate-task (it gets skills over MCP now)"), "{out}");
    assert!(fs::symlink_metadata(m.claude_skill("delegate-task")).is_err());
}
