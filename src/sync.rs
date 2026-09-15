//! `taskrunner sync`: sets up the harnesses on this machine to use taskrunner,
//! once each, and keeps their skills current. For each harness it:
//!
//! 1. asks once whether to connect it (or takes `--connect` / `--skip`) and
//!    records the answer under `[host.<name>]` in config.toml;
//! 2. checks it is signed in, and hands the user the login command if not;
//! 3. registers taskrunner with it as `taskrunner mcp --host <name>`;
//! 4. writes taskrunner's skills under `skills/<host>/` in the state root, next
//!    to a link to each of the user's own skills (`[skills] dirs`), and links
//!    them all where the harness looks. A harness that already gets
//!    taskrunner's skills over MCP loses those links; the user's stay, since
//!    they are never served.
//!
//! Every step is something the user can also do by hand, and `taskrunner
//! doctor` reports the same state without changing it. Taskrunner never edits
//! a harness's own config file: it registers through the harness's CLI, and
//! for Hermes, whose CLI can't do it without prompting, it prints the lines
//! to add.

use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context;
use serde_json::Value;

use crate::cli::say;
use crate::config::{Config, Delegation, HostConfig, HostKind, load_config};
use crate::ingest::sweep::expand_home;
use crate::paths::{StatePaths, default_root};
use crate::skills::{self, UserSkill};

fn label(host: HostKind) -> &'static str {
    match host {
        HostKind::Claude => "Claude Code",
        HostKind::Codex => "Codex",
        HostKind::Hermes => "Hermes",
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Where a harness keeps its state. Each honours its own override variable.
pub fn harness_home(host: HostKind) -> PathBuf {
    let (var, dir) = match host {
        HostKind::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
        HostKind::Codex => ("CODEX_HOME", ".codex"),
        HostKind::Hermes => ("HERMES_HOME", ".hermes"),
    };
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(dir))
}

/// The folder a harness reads user skills from. Hermes gets none: its curator
/// archives skills in its own folder, so it reads ours through
/// `skills.external_dirs`, which the curator leaves alone.
pub fn link_dir(host: HostKind) -> Option<PathBuf> {
    match host {
        HostKind::Claude | HostKind::Codex => Some(harness_home(host).join("skills")),
        HostKind::Hermes => None,
    }
}

fn host_skills_dir(paths: &StatePaths, host: HostKind) -> PathBuf {
    paths.skills_dir.join(host.as_str())
}

/// `~/...` when the path is under the home directory: how it reads in a
/// harness's config, and what a user would type.
fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if !home().as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

// ---- asking the harnesses --------------------------------------------------

struct Ran {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn says(&self, text: &str) -> bool {
        self.stdout.contains(text) || self.stderr.contains(text)
    }
}

fn run(program: &str, args: &[&str]) -> Option<Ran> {
    let output = Command::new(program).args(args).stdin(Stdio::null()).output().ok()?;
    Some(Ran {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(program)).find(|path| {
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Each harness's command has the harness's own name.
pub fn installed(host: HostKind) -> bool {
    on_path(host.as_str()).is_some()
}

pub enum SignIn {
    Yes,
    No { login: String },
    Unknown(String),
}

pub fn sign_in(host: HostKind) -> SignIn {
    match host {
        HostKind::Claude => {
            let logged_in = run("claude", &["auth", "status", "--json"])
                .and_then(|ran| serde_json::from_str::<Value>(&ran.stdout).ok())
                .and_then(|status| status["loggedIn"].as_bool());
            match logged_in {
                Some(true) => SignIn::Yes,
                Some(false) => SignIn::No { login: "claude auth login".into() },
                None => SignIn::Unknown("`claude auth status` gave no answer".into()),
            }
        }
        HostKind::Codex => match run("codex", &["login", "status"]) {
            Some(ran) if ran.ok && ran.says("Logged in") => SignIn::Yes,
            Some(_) => SignIn::No { login: "codex login".into() },
            None => SignIn::Unknown("`codex login status` did not run".into()),
        },
        HostKind::Hermes => {
            let Some(provider) = hermes_provider() else {
                return SignIn::Unknown(
                    "Hermes has no model provider chosen yet; run `hermes model` first".into(),
                );
            };
            match run("hermes", &["auth", "status", &provider]) {
                Some(ran) if ran.says(&format!("{provider}: logged in")) => SignIn::Yes,
                Some(_) => SignIn::No { login: format!("hermes auth add {provider}") },
                None => SignIn::Unknown("`hermes auth status` did not run".into()),
            }
        }
    }
}

pub enum Registration {
    Current,
    Missing,
    /// Registered, but with other arguments (typically from before `--host`)
    /// or a taskrunner that no longer exists. Keeps the registration as it
    /// is, to put back if replacing it fails.
    Outdated {
        command: Option<String>,
        args: Vec<String>,
    },
}

/// Whether a registered command can still start: an existing path, or a bare
/// name found on PATH.
fn command_exists(command: &str) -> bool {
    if command.contains('/') { Path::new(command).exists() } else { on_path(command).is_some() }
}

/// What taskrunner is registered to run: `mcp --host <name>`, plus the state
/// root when it isn't the default one.
pub fn mcp_args(paths: &StatePaths, host: HostKind) -> Vec<String> {
    let mut args = vec!["mcp".to_string(), "--host".to_string(), host.as_str().to_string()];
    if paths.root != default_root() {
        args.extend(["--state-root".to_string(), paths.root.display().to_string()]);
    }
    args
}

pub fn registration(paths: &StatePaths, host: HostKind) -> Registration {
    let expected = mcp_args(paths, host);
    match host {
        // `claude mcp get` has no JSON form; its listing names each field.
        HostKind::Claude => {
            let Some(ran) = run("claude", &["mcp", "get", "taskrunner"]).filter(|r| r.ok) else {
                return Registration::Missing;
            };
            let field = |name: &str| {
                ran.stdout.lines().find_map(|line| line.trim().strip_prefix(name)).map(str::trim)
            };
            let command = field("Command:").map(str::to_string);
            let args: Vec<String> =
                field("Args:").unwrap_or_default().split_whitespace().map(str::to_string).collect();
            if args == expected && command.as_deref().is_some_and(command_exists) {
                Registration::Current
            } else {
                Registration::Outdated { command, args }
            }
        }
        HostKind::Codex => {
            let Some(ran) = run("codex", &["mcp", "get", "taskrunner", "--json"]).filter(|r| r.ok)
            else {
                return Registration::Missing;
            };
            let server: Value = serde_json::from_str(&ran.stdout).unwrap_or_default();
            let transport = &server["transport"];
            let command = transport["command"].as_str().map(str::to_string);
            let args: Vec<String> = transport["args"]
                .as_array()
                .map(|args| args.iter().filter_map(|a| a.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if args == expected && command.as_deref().is_some_and(command_exists) {
                Registration::Current
            } else {
                Registration::Outdated { command, args }
            }
        }
        HostKind::Hermes => {
            let Some(block) = hermes_config_block(&["mcp_servers", "taskrunner"]) else {
                return Registration::Missing;
            };
            let command = hermes_config_block(&["mcp_servers", "taskrunner", "command"])
                .and_then(|value| value.lines().next().map(|line| line.trim().to_string()));
            if expected.iter().all(|arg| block.contains(arg.as_str()))
                && command.as_deref().is_some_and(command_exists)
            {
                Registration::Current
            } else {
                Registration::Outdated { command, args: vec![] }
            }
        }
    }
}

/// The taskrunner to register: the one already registered if it still
/// exists, else the one on PATH, else this binary.
fn taskrunner_command(registered: Option<String>) -> String {
    registered
        .filter(|command| Path::new(command).exists())
        .or_else(|| on_path("taskrunner").map(|p| tidy(&p).display().to_string()))
        .or_else(|| std::env::current_exe().ok().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "taskrunner".into())
}

/// `path` without `.` and `..` segments, when that still names the same file.
/// PATH entries like `~/.local/share/../bin` are common, and a registration a
/// person reads later should say `~/.local/bin`. Symlinks stay as they are: a
/// symlinked `taskrunner` is how a rebuild reaches every registration.
fn tidy(path: &Path) -> PathBuf {
    let mut tidied = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                tidied.pop();
            }
            other => tidied.push(other),
        }
    }
    let real = |p: &Path| fs::canonicalize(p).ok();
    if real(&tidied).is_some() && real(&tidied) == real(path) { tidied } else { path.to_path_buf() }
}

/// Whether the host fetched skills over MCP (SEP-2640) in the last week. Read
/// from the index the daemon keeps; a missing index, or one built before
/// sessions carried a host, reads as no. A week, not the latest session:
/// sync's own `claude mcp get` health check opens a session that fetches
/// nothing, and should not undo the answer.
pub fn gets_skills_over_mcp(paths: &StatePaths, host: HostKind) -> bool {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let Ok(db) = rusqlite::Connection::open_with_flags(&paths.index_db, flags) else {
        return false;
    };
    db.query_row(
        "SELECT EXISTS (
           SELECT 1 FROM audit_events a JOIN mcp_sessions s ON s.id = a.session_id
           WHERE a.kind = 'skills.list' AND s.host = ?1
             AND a.ts >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-7 days'))",
        [host.as_str()],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

// ---- Hermes's config, read but never written --------------------------------

fn hermes_config_file() -> PathBuf {
    harness_home(HostKind::Hermes).join("config.yaml")
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn significant(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && !line.starts_with('#')
}

/// The lines under a key path in block-style YAML, with the key's own inline
/// value first — enough to check a few known entries without a YAML parser.
/// Anything cleverer (flow maps, anchors) reads as missing, which only means
/// sync prints the lines to add.
fn yaml_block<'a>(lines: &[&'a str], path: &[&str]) -> Option<Vec<&'a str>> {
    let Some((key, rest)) = path.split_first() else { return Some(lines.to_vec()) };
    let level = lines.iter().filter(|l| significant(l)).map(|l| indent_of(l)).min()?;
    let at = lines.iter().position(|line| {
        significant(line)
            && indent_of(line) == level
            && line.trim_start().strip_prefix(key).is_some_and(|after| after.starts_with(':'))
    })?;
    let inline = lines[at].trim_start()[key.len() + 1..].trim();
    // A list may sit at its key's own indent (`key:` then `- item`).
    let children: Vec<&str> = lines[at + 1..]
        .iter()
        .take_while(|l| !significant(l) || indent_of(l) > level || l.trim_start().starts_with("- "))
        .copied()
        .collect();
    if !rest.is_empty() {
        return yaml_block(&children, rest);
    }
    Some([inline].into_iter().chain(children).filter(|l| significant(l)).collect())
}

fn hermes_config_block(path: &[&str]) -> Option<String> {
    let text = fs::read_to_string(hermes_config_file()).ok()?;
    yaml_block(&text.lines().collect::<Vec<_>>(), path).map(|lines| lines.join("\n"))
}

fn hermes_provider() -> Option<String> {
    let block = hermes_config_block(&["model", "provider"])?;
    let provider = block.lines().next()?.trim().trim_matches(['"', '\'']).to_string();
    (!provider.is_empty()).then_some(provider)
}

/// Whether Hermes's `skills.external_dirs` includes taskrunner's Hermes skills.
pub fn hermes_reads_skills(paths: &StatePaths) -> bool {
    let dir = host_skills_dir(paths, HostKind::Hermes);
    hermes_config_block(&["skills", "external_dirs"]).is_some_and(|block| {
        block.contains(&dir.display().to_string()) || block.contains(&tilde(&dir))
    })
}

// ---- skills on disk ----------------------------------------------------------

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    changed: bool,
}

impl Report {
    fn change(&mut self, host: HostKind, what: impl AsRef<str>) {
        self.changed = true;
        self.lines.push(format!("{}: {}", host.as_str(), what.as_ref()));
    }

    fn note(&mut self, host: HostKind, what: impl AsRef<str>) {
        self.lines.push(format!("{}: {}", host.as_str(), what.as_ref()));
    }
}

/// Writes one host's rendered skills, touching only files that changed, and
/// links each of the user's skills beside them. The files are read-only: an
/// agent that curates skills (Hermes's does) edits whatever it can write.
/// Harness links point here rather than at the user's folders, so a link
/// pointing into the state root is taskrunner's whoever wrote the skill, and
/// Hermes reads both kinds through its one `external_dirs` entry.
fn write_host_skills(
    paths: &StatePaths,
    host: HostKind,
    delegation: Delegation,
    user: &[UserSkill],
    report: &mut Report,
) -> anyhow::Result<()> {
    let dir = host_skills_dir(paths, host);
    let rendered = skills::render(delegation);
    for skill in &rendered {
        let skill_dir = dir.join(&skill.name);
        let file = skill_dir.join("SKILL.md");
        if fs::read_to_string(&file).is_ok_and(|text| text == skill.text) {
            continue;
        }
        fs::create_dir_all(&skill_dir)?;
        if file.exists() {
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
        }
        fs::write(&file, &skill.text).with_context(|| format!("writing {}", file.display()))?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444))?;
        report.change(host, format!("wrote skill {}", skill.name));
    }
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let current = rendered.iter().any(|skill| skill.name == name)
            || user.iter().any(|skill| {
                skill.name == name && fs::read_link(&path).is_ok_and(|to| to == skill.dir)
            });
        if current {
            continue;
        }
        if path.is_symlink() {
            fs::remove_file(&path)?;
        } else if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            continue;
        }
        report.change(host, format!("removed skill {name}"));
    }
    for skill in user {
        let link = dir.join(&skill.name);
        if !link.is_symlink() {
            symlink(&skill.dir, &link).with_context(|| format!("linking {}", link.display()))?;
            report.change(host, format!("added your skill {}", skill.name));
        }
    }
    Ok(())
}

/// Rewrites the skills of every connected host. The daemon calls this when it
/// starts, so an upgraded taskrunner's skills reach harnesses without a sync.
pub fn write_skills(paths: &StatePaths, config: &Config) -> anyhow::Result<()> {
    let mut report = Report::default();
    // Skipped skills are sync's to report; the daemon has nobody to tell.
    let (user, _) = user_skills(config);
    for host in HostKind::ALL {
        if config.host(host).is_some_and(|settings| settings.connected) {
            write_host_skills(paths, host, config.delegation(Some(host)), &user, &mut report)?;
        }
    }
    Ok(())
}

/// The user's own skills from `[skills] dirs`, and why any were left out.
pub fn user_skills(config: &Config) -> (Vec<UserSkill>, Vec<String>) {
    let dirs: Vec<PathBuf> = config.skills.dirs.iter().map(|dir| expand_home(dir)).collect();
    skills::user_skills(&dirs)
}

/// The skills a harness's folder should link: taskrunner's unless the harness
/// gets them over MCP, and the user's always, because those are never served.
fn linked_names(over_mcp: bool, user: &[UserSkill]) -> Vec<String> {
    let own = if over_mcp { vec![] } else { skills::render(Delegation::default()) };
    own.into_iter().map(|skill| skill.name).chain(user.iter().map(|s| s.name.clone())).collect()
}

/// A link is taskrunner's when it points into the skills folder of the state root.
fn is_ours(paths: &StatePaths, link: &Path) -> bool {
    fs::read_link(link).is_ok_and(|target| target.starts_with(&paths.skills_dir))
}

#[derive(Default)]
pub struct LinkState {
    pub missing: Vec<String>,
    /// Skills of the same name that taskrunner didn't put there.
    pub foreign: Vec<String>,
}

pub fn link_state(
    paths: &StatePaths,
    host: HostKind,
    dir: &Path,
    over_mcp: bool,
    user: &[UserSkill],
) -> LinkState {
    let mut state = LinkState::default();
    for name in linked_names(over_mcp, user) {
        let link = dir.join(&name);
        if fs::symlink_metadata(&link).is_err() {
            state.missing.push(name);
        } else if fs::read_link(&link).ok() != Some(host_skills_dir(paths, host).join(&name)) {
            state.foreign.push(name);
        }
    }
    state
}

fn sync_links(
    paths: &StatePaths,
    host: HostKind,
    dir: &Path,
    over_mcp: bool,
    user: &[UserSkill],
    report: &mut Report,
) -> anyhow::Result<()> {
    let wanted = linked_names(over_mcp, user);
    let own = skills::render(Delegation::default());
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let link = entry?.path();
            if !is_ours(paths, &link) {
                continue;
            }
            let name = link.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let target = host_skills_dir(paths, host).join(&name);
            if !wanted.contains(&name) || fs::read_link(&link).ok() != Some(target) {
                fs::remove_file(&link)?;
                let served = over_mcp && own.iter().any(|skill| skill.name == name);
                let why = if served { " (it gets skills over MCP now)" } else { "" };
                report.change(host, format!("unlinked {name}{why}"));
            }
        }
    }
    for name in &wanted {
        let link = dir.join(name);
        if is_ours(paths, &link) {
            continue;
        }
        if fs::symlink_metadata(&link).is_ok() {
            report
                .note(host, format!("{} is not taskrunner's skill; left it alone", link.display()));
            continue;
        }
        fs::create_dir_all(dir)?;
        symlink(host_skills_dir(paths, host).join(name), &link)
            .with_context(|| format!("linking {}", link.display()))?;
        report.change(host, format!("linked {name}"));
    }
    Ok(())
}

// ---- setting a host up -------------------------------------------------------

pub struct SyncOptions {
    pub connect: Vec<HostKind>,
    pub skip: Vec<HostKind>,
    pub delegation: Option<Delegation>,
}

impl SyncOptions {
    pub fn parse(
        connect: Option<String>,
        skip: Option<String>,
        delegation: Option<String>,
    ) -> anyhow::Result<SyncOptions> {
        let hosts = |list: Option<String>| -> anyhow::Result<Vec<HostKind>> {
            list.unwrap_or_default()
                .split(',')
                .filter(|name| !name.is_empty())
                .map(|name| {
                    HostKind::parse(name).with_context(|| {
                        format!(
                            "'{name}' is not a harness taskrunner knows (claude, codex, hermes)"
                        )
                    })
                })
                .collect()
        };
        let delegation = match delegation {
            None => None,
            Some(value) => Some(Delegation::parse(&value).with_context(|| {
                format!("--delegation must be suggest or on-request, not '{value}'")
            })?),
        };
        Ok(SyncOptions { connect: hosts(connect)?, skip: hosts(skip)?, delegation })
    }
}

/// Reads one answer from the terminal; `None` when there is no terminal to
/// ask, as when an agent runs sync.
fn ask(question: &str) -> Option<String> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    say(question).ok()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).ok()?;
    Some(answer.trim().to_lowercase())
}

/// Asks until `read` understands the answer. Every reader takes an empty
/// answer as the default, so a closed stdin ends the loop too.
fn ask_until<T>(question: &str, hint: &str, read: impl Fn(&str) -> Option<T>) -> Option<T> {
    loop {
        if let Some(value) = read(&ask(question)?) {
            return Some(value);
        }
        let _ = say(&format!("  Please answer {hint}.\n"));
    }
}

fn read_yes_no(answer: &str) -> Option<bool> {
    match answer {
        "" | "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

/// The menu's number, or the choice in words.
fn read_delegation(answer: &str) -> Option<Delegation> {
    match answer {
        "" | "1" | "s" | "suggest" => Some(Delegation::Suggest),
        "2" | "r" | "request" | "on request" | "on-request" => Some(Delegation::OnRequest),
        _ => None,
    }
}

/// Appends a `[host.<name>]` section. Appending, not rewriting, keeps every
/// comment and setting already in the file as the user wrote it.
fn record_host(paths: &StatePaths, host: HostKind, settings: &HostConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.root)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.config_file)
        .with_context(|| format!("opening {}", paths.config_file.display()))?;
    write!(
        file,
        "\n[host.{}]\nconnected = {}\ndelegation = \"{}\"\n",
        host.as_str(),
        settings.connected,
        settings.delegation.as_str()
    )?;
    Ok(())
}

/// This host's `[host.<name>]` settings, asking for them the first time sync
/// meets it. `None` means leave it alone this run.
fn settle_host(
    paths: &StatePaths,
    config: &Config,
    host: HostKind,
    options: &SyncOptions,
    report: &mut Report,
) -> anyhow::Result<Option<HostConfig>> {
    if let Some(settings) = config.host(host) {
        if options.connect.contains(&host) && !settings.connected {
            report.note(host, "config.toml says connected = false; set it to true to connect it");
        }
        return Ok(Some(settings.clone()));
    }
    let connected = if options.connect.contains(&host) {
        true
    } else if options.skip.contains(&host) {
        false
    } else if !installed(host) {
        return Ok(None);
    } else {
        let question = format!("Connect {} to taskrunner? [Y/n] ", label(host));
        match ask_until(&question, "y or n", read_yes_no) {
            Some(connected) => connected,
            None => {
                let name = host.as_str();
                report.note(
                    host,
                    format!("found, not set up yet; run taskrunner sync --connect {name} (or --skip {name})"),
                );
                return Ok(None);
            }
        }
    };
    let delegation = match (connected, options.delegation) {
        (false, _) => Delegation::default(),
        (true, Some(delegation)) => delegation,
        (true, None) => {
            let question = format!(
                "When should {} hand a task to a worker?\n  \
                 1) suggest    — offer it when a task fits, then wait for your yes\n  \
                 2) on request — only when you ask\n\
                 Choose 1 or 2 [1]: ",
                label(host)
            );
            ask_until(&question, "1 or 2", read_delegation).unwrap_or_default()
        }
    };
    let settings = HostConfig { connected, delegation };
    record_host(paths, host, &settings)?;
    report.change(
        host,
        format!(
            "recorded in config.toml: connected = {connected}, delegation = {}",
            delegation.as_str()
        ),
    );
    Ok(Some(settings))
}

/// Registers taskrunner with Claude Code or Codex. Neither CLI replaces an
/// existing entry, so an outdated one is removed first — and put back if the
/// new one is refused, so a failed sync never leaves the harness without
/// taskrunner.
fn register(paths: &StatePaths, host: HostKind, current: &Registration, report: &mut Report) {
    let program = host.as_str();
    let scope: &[&str] = if host == HostKind::Claude { &["-s", "user"] } else { &[] };
    let add = |command: &str, args: &[String]| -> Vec<String> {
        ["mcp", "add"]
            .into_iter()
            .chain(scope.iter().copied())
            .chain(["taskrunner", "--", command])
            .map(str::to_string)
            .chain(args.iter().cloned())
            .collect()
    };
    let run_all =
        |argv: &[String]| run(program, &argv.iter().map(String::as_str).collect::<Vec<_>>());

    let previous = match current {
        Registration::Outdated { command, args } => Some((command.clone(), args.clone())),
        _ => None,
    };
    if previous.is_some() {
        let remove: Vec<String> = ["mcp", "remove"]
            .into_iter()
            .chain(scope.iter().copied())
            .chain(["taskrunner"])
            .map(str::to_string)
            .collect();
        let _ = run_all(&remove);
    }
    let command = taskrunner_command(previous.as_ref().and_then(|(command, _)| command.clone()));
    let args = mcp_args(paths, host);
    let refused = match run_all(&add(&command, &args)) {
        Some(ran) if ran.ok => {
            report.change(host, format!("registered taskrunner: {command} {}", args.join(" ")));
            return;
        }
        Some(ran) => format!("{}{}", ran.stdout, ran.stderr).trim().to_string(),
        None => format!("`{program}` did not run"),
    };
    report.note(host, format!("registering taskrunner failed: {refused}"));
    if let Some((Some(old_command), old_args)) = previous {
        let restore = add(&old_command, &old_args);
        if run_all(&restore).is_some_and(|ran| ran.ok) {
            report.note(host, "put the previous registration back");
        } else {
            report.note(
                host,
                format!(
                    "taskrunner is no longer registered; to restore it, run: {program} {}",
                    restore.join(" ")
                ),
            );
        }
    }
}

/// Hermes is registered and pointed at the skills by lines in its own config,
/// which taskrunner reads but never writes. When they are missing, the report
/// carries them for the user (or an agent) to add.
fn check_hermes(paths: &StatePaths, over_mcp: bool, user: &[UserSkill], report: &mut Report) {
    let host = HostKind::Hermes;
    let registered = matches!(registration(paths, host), Registration::Current);
    // The user's skills are never served, so they need external_dirs regardless.
    let reads_skills = (over_mcp && user.is_empty()) || hermes_reads_skills(paths);
    if registered && reads_skills {
        return;
    }
    let mut lines = vec![];
    if !registered {
        let args = mcp_args(paths, host).join(", ");
        lines.extend([
            "mcp_servers:".to_string(),
            "  taskrunner:".to_string(),
            format!("    command: {}", taskrunner_command(None)),
            format!("    args: [{args}]"),
        ]);
    }
    if !reads_skills {
        lines.extend([
            "skills:".to_string(),
            "  external_dirs:".to_string(),
            format!("    - {}", tilde(&host_skills_dir(paths, host))),
        ]);
    }
    report.note(
        host,
        format!(
            "add to {} (merge into any existing keys; taskrunner doesn't edit Hermes's config):\n{}",
            tilde(&hermes_config_file()),
            lines.iter().map(|line| format!("    {line}")).collect::<Vec<_>>().join("\n")
        ),
    );
}

fn sync_host(
    paths: &StatePaths,
    host: HostKind,
    delegation: Delegation,
    user: &[UserSkill],
    report: &mut Report,
) -> anyhow::Result<()> {
    if !installed(host) {
        report
            .note(host, format!("`{}` is not on PATH; install it, then sync again", host.as_str()));
        return Ok(());
    }
    match sign_in(host) {
        SignIn::Yes => {}
        SignIn::No { login } => {
            report.note(
                host,
                format!("not signed in; run `{login}` in your terminal, then sync again"),
            );
            return Ok(());
        }
        SignIn::Unknown(why) => {
            report.note(host, why);
            return Ok(());
        }
    }
    write_host_skills(paths, host, delegation, user, report)?;
    let over_mcp = gets_skills_over_mcp(paths, host);
    let Some(dir) = link_dir(host) else {
        check_hermes(paths, over_mcp, user, report);
        return Ok(());
    };
    match registration(paths, host) {
        Registration::Current => {}
        stale => register(paths, host, &stale, report),
    }
    sync_links(paths, host, &dir, over_mcp, user, report)
}

pub fn run_sync(paths: &StatePaths, options: &SyncOptions) -> anyhow::Result<i32> {
    let config = load_config(&paths.config_file)?;
    let mut report = Report::default();
    let (user, skipped) = user_skills(&config);
    report.lines.extend(skipped.iter().map(|why| format!("skills: {why}")));
    for host in HostKind::ALL {
        let Some(settings) = settle_host(paths, &config, host, options, &mut report)? else {
            continue;
        };
        if settings.connected {
            sync_host(paths, host, settings.delegation, &user, &mut report)?;
        }
    }
    for line in &report.lines {
        say(&format!("{line}\n"))?;
    }
    if !report.changed {
        say("taskrunner sync: nothing to change\n")?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, path: &[&str]) -> Option<String> {
        yaml_block(&text.lines().collect::<Vec<_>>(), path).map(|lines| lines.join("\n"))
    }

    const HERMES: &str = "\
# my settings
model:
  default: gpt-5.6-terra
  provider: openai-codex
skills:
  creation_nudge_interval: 15
  external_dirs:
    - ~/.taskrunner/skills/hermes
mcp_servers:
  taskrunner:
    command: /usr/bin/taskrunner
    args: [mcp, --host, hermes]
";

    #[test]
    fn reads_nested_entries_from_block_yaml() {
        assert_eq!(block(HERMES, &["model", "provider"]).as_deref(), Some("openai-codex"));
        let dirs = block(HERMES, &["skills", "external_dirs"]).unwrap();
        assert!(dirs.contains("~/.taskrunner/skills/hermes"));
        let server = block(HERMES, &["mcp_servers", "taskrunner"]).unwrap();
        assert!(server.contains("--host") && server.contains("hermes"));
        assert_eq!(block(HERMES, &["mcp_servers", "other"]), None);
    }

    #[test]
    fn answers_are_read_strictly_and_empty_means_the_default() {
        assert_eq!(read_yes_no(""), Some(true));
        assert_eq!(read_yes_no("no"), Some(false));
        assert_eq!(read_yes_no("sure"), None);
        for suggest in ["", "1", "suggest"] {
            assert_eq!(read_delegation(suggest), Some(Delegation::Suggest), "{suggest:?}");
        }
        for on_request in ["2", "request", "on request", "on-request"] {
            assert_eq!(read_delegation(on_request), Some(Delegation::OnRequest), "{on_request:?}");
        }
        assert_eq!(read_delegation("y"), None, "yes is not an answer to which of two");
    }

    #[test]
    fn tidies_dot_dot_out_of_a_path_only_when_it_names_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["share", "bin", "elsewhere/deep", "elsewhere/bin"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("bin/taskrunner"), "").unwrap();
        fs::write(root.join("elsewhere/bin/taskrunner"), "").unwrap();
        assert_eq!(tidy(&root.join("share/../bin/taskrunner")), root.join("bin/taskrunner"));

        // Through a symlinked folder, `..` means the link target's parent.
        symlink(root.join("elsewhere/deep"), root.join("link")).unwrap();
        let through_link = root.join("link/../bin/taskrunner");
        assert_eq!(tidy(&through_link), through_link);
    }

    #[test]
    fn a_list_may_sit_at_its_keys_indent() {
        let text = "skills:\n  external_dirs:\n  - /a\n  - /b\n  other: 1\n";
        let dirs = block(text, &["skills", "external_dirs"]).unwrap();
        assert!(dirs.contains("/a") && dirs.contains("/b") && !dirs.contains("other"));
    }
}
