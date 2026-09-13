# Complete audit — proposal (not implemented)

Noted 2026-09-13. Nothing here is built. This captures a design conversation so the
decisions survive; see [session-handles.md](session-handles.md) for the same kind of
note on a different topic.

## The goal

The system should be **completely auditable**: every prompt, tool call, tool result
and model response, from every harness — Claude Code, Codex, Hermes, and whatever
comes next — lands in one archive that can be queried later, with no holes.

Taskrunner's archive already does this for Claude Code and Codex (the sweeper plus
`search-transcripts` / `lookup-session` over MCP). Hermes ships its own equivalent:
every session goes to `~/.hermes/state.db` (SQLite + FTS5, `messages` table with
role/content/tool_calls/tool_name) and a `session_search` tool reads it back. That
raised the question that started this: *in Hermes, should taskrunner's archive be
switched off to avoid two databases of the same thing?*

**Decision: no.** A per-harness off switch leaves a hole exactly where that harness
is. The harness's own store is a *working copy*; the taskrunner archive is *the
audit*. Two copies is fine; two sources of truth is not. Hermes gets a parser
(`[ingest.sources.hermes]`) like Claude and Codex have, and dedupe by deterministic
message id handles the overlap.

## How everything gets in

Two ways to see a conversation:

| | Read the harness's files | Capture at the wire |
|---|---|---|
| What it is | Copy the `.jsonl` / `state.db` each harness writes | Sit between the harness and the model; record requests and responses as they pass |
| Complete? | Only what the harness chose to save; Claude deletes after 30 days | By construction — nothing reaches a model without passing through |
| Per-harness work | A parser each | None; same for every harness, present or future |
| What it needs | Nothing | A TLS-terminating proxy and a certificate the harness trusts |
| Failure mode | A file deleted before the sweep is gone | Proxy down → harness can't reach the model |

Wire capture needs TLS termination because the conversation is inside HTTPS. The
auth type is irrelevant — an OAuth bearer token forwards like an API key. Claude Code
documents this as the supported enterprise-proxy path (`HTTPS_PROXY` +
`NODE_EXTRA_CA_CERTS`); Codex honours the standard proxy variables and the system
trust store. Anthropic's terms restrict *reusing* an OAuth token elsewhere; a recorder
that forwards requests unmodified is the enterprise-proxy path, not reuse — but that
line is Anthropic's to draw, so the recorder must never modify requests and must
strip `Authorization` before anything hits the log.

**Direction: hybrid.** Wire capture where taskrunner already controls the network;
file ingestion everywhere else; both feed one archive.

### Workers (delegated turns)

Wire capture, on by default. The egress proxy already stands between every worker
container and the world; it grows from "CONNECT and forward" to "terminate, record,
re-encrypt." Taskrunner generates the CA once and bakes it into the worker images at
`npm run build:images`. Zero setup for the user, and it covers the part of the audit
only taskrunner can see — every tool call inside a delegated turn.

### Host sessions (the harness you talk to)

Three ways to run it, chosen at install and changeable any time:

1. **On the host, files only** — today's behaviour. Default. Nothing to set up;
   audit is whatever the harness saved.
2. **On the host, through the proxy** — opt-in via `taskrunner capture enable`,
   which writes the proxy and certificate settings into the harness's own config
   (`~/.claude/settings.json` `env` block; the Codex equivalent) and `capture
   disable` removes them. No system-wide certificate install. The cost is the
   failure mode: once routed, the daemon must be running or the CLI can't reach the
   model. Prerequisite: the daemon runs as a user service (starts at login) before
   this is offered.
3. **In Docker** — `taskrunner shell` (name TBD) starts the harness in a container
   with the current project mounted, behind the same egress proxy and certificate as
   the workers. Full capture and containment with nothing installed on the host.
   Costs: the agent sees only what is mounted; OAuth login is the URL-paste flow
   (`worker-login` already does this); Docker must be up to talk to your agent.

   **Not the default**, but offered at install. Install asks which host paths may be
   mounted (the project directory, plus an explicit allowlist — e.g. `~/.gitconfig`,
   an SSH key, a tools directory) and records them in config; `taskrunner shell`
   mounts only those. Everything else in the home directory stays invisible.

## Decided: what the wire capture stores

Every API call re-sends the whole conversation, so raw capture is quadratic in the
session — 50–100× the transcript file, and search full of the same message repeated
per call. Three options were weighed: store it all raw; store only what is new per
call; store what is new per call **plus a hash of every full request body**.

**Decision: the third.** Per call, the proxy keeps the messages past the common
prefix with the previous request, the response (streamed chunks reassembled), the
system prompt once by content hash, and a SHA-256 of the complete request body.
Storage is linear again, each message is searched once, and any call's exact input
to the model can still be *proven* — hash matches or it doesn't — without being
stored. Raw bodies may be kept for a short debugging window (7 days), then dropped
by a logged `retention.pruned` event, never silently.

This also settles **same message, two witnesses**: wire capture then produces
messages in transcript shape, so matching them to file-ingested ones is a
reconciliation step (content hash + position within the session), not a second
data model. One message, two sources recorded against it.

## Decided: secrets are redacted, and the redaction is the record

The audit question about a leaked secret is *what kind, when, through which tool,
in which session, to which model* — none of which needs the value. Storing values
would make the archive a secrets store: every backup, export, search hit or shared
copy can leak again, and an append-only log cannot rotate a secret out.

**Decision: redact with a marker and a fingerprint, always on.** The value is
replaced in place by `[REDACTED <kind> sha256:<prefix>]` and a `secret.redacted`
event is appended (kind, source, session, tool, position). A suspected value can be
hashed and compared later, so *which* secret can be proven without ever holding it.
Detection is pattern-based and will both miss and over-match; the visible marker is
what makes over-matching harmless. The archive stays `0700` and local — redaction
narrows the blast radius, it does not replace the boundary. Vaulting (encrypt in
place, recover with a key) was considered and rejected for now: a key on the same
machine gains little over plaintext.

## Decided: the log is hash-chained from line 1

Append-only is a promise the code keeps, not something the file proves. Each event
carries the SHA-256 of the previous event; editing, deleting or reordering any past
line breaks the chain at that point and `taskrunner verify` finds it in one pass.
Same mechanism as git commit parents and certificate-transparency logs.

What it does not do on its own: stop someone re-hashing everything after their edit,
or detect truncation at the tail. Both need an **anchor** — the current chain head
written somewhere the log writer cannot reach. Simplest: the daemon appends the head
to a second file every N events; stronger anchors (a git commit, a line in a
notebook, a friend's copy) are the user's choice. The chain proves a record has not
changed since it was written — not that it was true when written.

Starts with the Rust port, which rewrites the event writer anyway. Adding it later
would leave the old history as a permanent "trust me" zone or require re-hashing it
— exactly the act the chain exists to make suspicious. Cost: one field per line,
under 5% of the log.

## Decided: every harness is both a host and a worker

Hermes is a worker as well as a host: `[worker.hermes]` — an image, its headless
mode, its login in an auth volume — so Claude Code or Codex can delegate a task *to*
Hermes in a container the same way they delegate to Codex today. The harness table
(`HARNESS_KINDS`, `AUTH_MOUNTS`, transcript layout) grows one row.

The same applies to any harness added later — OpenClaw is the next expected one —
and it applies as a rule, not a case: a new harness is a `[host.<name>]` section, a
`[worker.<name>]` section, and a parser. Nothing about audit, capture, search scope
or delegation is written for a specific harness.

## Decided: the archive records its own gaps

Config changes, task assignments and capture on/off are logged as events alongside
egress decisions and changed files. An audit that cannot show when it was told to
stop looking is incomplete.

## Install flow (sketch)

```
taskrunner setup
  Which agents do you use?                   [claude, codex, hermes]
  For each:
    Run it inside Docker?                    [no]
      Paths it may see besides the project:  ~/.gitconfig, ...
    Record its sessions through the proxy?   [no]   (needs the daemon service)
  Worker capture is always on.
```

Every answer becomes a `[host.<name>]` key (see below), not a lock-in.
`taskrunner capture enable|disable` and the choice of `claude` vs `taskrunner shell`
flip them later.

## Hosts: one section per harness, and Hermes stays Hermes

The parser copies Hermes's rows into the archive; the agent is not involved and does
no sorting. The copy on disk is harmless. What *does* matter is the agent's tool
list: inside Hermes the model would see two tools that answer "what did we do about
X" — Hermes's `session_search` and taskrunner's `search-transcripts`.

An earlier draft resolved this by disabling Hermes's search and using taskrunner's
everywhere. **Reversed.** Taskrunner is a record you look things up in; Hermes's
memory (curated notes in every prompt, session lineage, the agent knowing you across
time) is the agent remembering — and that is the reason to run Hermes at all. Nothing
in Hermes is turned off: not `memory_enabled`, not `session_search`, not
`delegate_task`. Taskrunner ingests `state.db` silently; Hermes never notices.

**Decision: scope taskrunner's tools per host instead.** Today `[worker.<name>]`
describes what taskrunner *runs*; a `[host.<name>]` section describes what *runs
taskrunner*. Everything about a host lands in one place:

```toml
[host.claude]
capture = "files"          # "files" | "proxy" | "docker"
search  = "all"            # no memory of its own; taskrunner is its recall

[host.codex]
capture = "files"
search  = "all"

[host.hermes]
capture = "files"
search  = "on-request"     # Hermes remembers its own way; the archive is
ingest  = "~/.hermes/state.db"   # searched only when the user asks for it
```

- `capture` — how this host's sessions reach the archive (the three modes above);
  `docker` adds a `mounts` allowlist.
- `search` — when and what taskrunner's `search-transcripts` / `lookup-session`
  answer for this host: `all`, `workers` (delegated turns and *other* harnesses'
  sessions), `on-request` (tools present, described as "only when the user asks to
  search the archive" — the host's own memory does the everyday remembering), or
  `none`. Tool descriptions state the scope, so the model has no reason to guess.
- `ingest` — where this host's own transcripts are read from.

The MCP server is registered per harness anyway (`claude mcp add …`, Hermes's
config), so the registration passes `--host <name>` and the server loads that
section; nothing is inferred from the connection.

`assign-task` and Hermes's `delegate_task` both stay. In Hermes, `assign-task` is
for calling a *different model* — a worker on another harness or a local model —
and its description says exactly that; everything else is Hermes's own.

Named `host`, not `profile`: Hermes already uses "profile" for its multiple-home
feature, and two things called profile would confuse.

Hermes's search ergonomics are still worth copying into taskrunner's tools for the
hosts that rely on them: `role_filter` defaulting to `user,assistant`,
scroll-around-a-message, browse-recent, demote (not hide) automation sessions.

## Skills and agents across harnesses

**Skills: global by default.** Claude Code, Codex and Hermes all read the same
`SKILL.md` format (the agentskills.io spec), so one copy can serve every harness.
The user's machine already does this: Omarchy keeps its skills in one place and
symlinks them into `~/.claude/skills/` and `~/.codex/skills/`; Hermes reads shared
folders directly via `skills.external_dirs` and names `~/.agents/skills/` as the
convention.

```
~/.agents/skills/            global — one copy, every harness
~/.taskrunner/skills/        taskrunner's own (worker-login, …), also global
[host.<name>].skills = [...] per-host extras, linked into that harness only
```

`taskrunner sync` symlinks the global set into each host's skills directory (and
adds the `external_dirs` entry for Hermes), links per-host extras only where they
belong, and removes links it made that are no longer wanted. Nothing is copied;
editing a global skill changes it everywhere.

**Agents: per-harness by default.** There is no standard. Claude Code agents are
markdown files under `~/.claude/agents/` (name, description, tools, model, system
prompt); Codex has no agent files, only `AGENTS.md` instructions; Hermes subagents
are defined by the `delegate_task` call itself (goal + context), not by files.

- Per-harness, native format, taskrunner manages only *which* are active per host —
  ✅ full fidelity. ❌ an agent wanted everywhere is written up to three times.
- Taskrunner-owned definitions rendered into each harness's shape — ✅ one source.
  ❌ lossy: `tools` and `model` do not translate; lowest common denominator.

**Decision:** native per-harness agents by default, managed per `[host.<name>]`.
A small opt-in *portable agent* format (description + instructions + suggested
tools) for the agents that are really a skill with a role attached — most are —
rendered by `taskrunner sync` into a Claude agent file, a Hermes skill that says how
to delegate that role, or an `AGENTS.md` section. Anything needing harness-specific
tools or models stays native and is not made portable.

This is how companies do it too: a shared skills/prompt repo synced into every
tool, tool-specific agent configs kept next to the tool. Nobody has a working
universal agent format yet.

## Storage: what to borrow from Hermes, what not to

Both designs end in SQLite with FTS5 over a `messages` table. The difference is what
is the truth.

| | Hermes | Taskrunner |
|---|---|---|
| Source of truth | The SQLite DB | Append-only JSONL event log; SQLite is a derived index |
| If the DB breaks | Repair procedure; FTS detach/rebuild | Delete `index.db`, replay the log |
| Index kept in sync by | Insert triggers | The event fold |
| Edits/deletes | Possible (`journey delete`, prune by age) | Only by appending an event |

**Keep the log as truth.** Append-only is the audit property: a JSONL line cannot be
quietly rewritten, a SQLite row can. The DB is disposable, so schema changes and
parser bugs cost a replay, not a migration. Plain text outlives any SQLite version.
This is event sourcing — the ledger pattern banks and payment systems use: never edit
a transaction, append a correcting one, recompute the balance.

**Borrow these columns** (each additive — an event field plus a column, no rewrite):

1. On sessions: `model`, `parent_session_id`, `end_reason` — lineage dedupe ("one
   conversation split by compaction shows up once") and "which model said that."
2. On sessions: `git_branch` at start. Cheap, useful.
3. On messages: reasoning as its own field where the harness exposes it.
4. In FTS: index `tool_name` / `tool_target` alongside content, so text search finds
   tool activity without a separate structured query.

Taskrunner already has what Hermes lacks: exchange numbering (`prompt_idx`),
structured tool facts (`tool_use_id`, `tool_target`, `is_error`), task linkage
(`turn_id`), and a `source` that is a parser name rather than a platform — the thing
that makes one archive across harnesses possible. Keep all of it.

**Do not borrow:** mutable DB as truth, insert triggers, silent age-based pruning.

### Size and retention — decided

**What the wire actually carries.** Every model call re-sends the system prompt
(~60 KB), the tool definitions (~50 KB) and the whole conversation so far. For the
author's month of Claude Code — 8.5 MB of transcript, ~2,500 calls — that is roughly
275 MB of repeated headers plus ~190 MB of re-sent conversation: **~0.5 GB seen,
about 60× the transcript**. Under the storage decision above (new messages per call,
constants once by hash, one hash per call) what is *kept* is ~15–20 MB/month — about
2× the transcript, because system prompts, tool schemas and side traffic the
transcript hides are now held too. Hash-chaining adds one hash per line, under 5%.

**Nobody else keeps this.** Claude Code deletes its local transcripts after 30 days
by default; Anthropic retains consumer sessions for 30 days (5 years if "improve the
model" is on) and only for its own use; Codex and Hermes are their own working
copies. Taskrunner is the only complete, searchable record the user holds.

**Decision: keep the log, search a window.** Three tiers:

| Tier | What | Default |
|---|---|---|
| Hot | Recent history in the SQLite + FTS index | 30 days |
| Cold | Older log segments, compressed (~10×) on disk, hash chain intact | keep forever |
| Raw | Wire-capture bodies for debugging | 7 days, then deleted by a logged event |

The audit is never deleted by default; only the *index* is bounded. A month hot is
~40 MB at the author's rate; a year cold is a few tens of MB compressed.

```toml
[retention]
hot_days = 30        # indexed and searchable; change any time
cold     = "keep"    # or "180d", "2y" — deletion is explicit and logged
raw_days = 7         # wire-capture bodies
```

Commands: `taskrunner thaw <range>` / `freeze <range>` move segments between hot
and cold; `export <range> --out archive.tar.zst` writes a portable, compressed,
chained slice and `import` restores one (or loads a friend's); `prune <range>` is
the only thing that deletes and always appends `retention.pruned` first. Asking
search for something older than the hot window says so and names the thaw command
rather than silently returning nothing.

**Levers if size ever matters** (it is a search-quality problem long before a disk
one): don't full-text index thinking blocks (the largest single item and the least
searchable); store large tool results once by content hash; raise `hot_days` only
as far as search stays useful.

## Rust port — before the redesign

Decided 2026-09-13: taskrunner moves to Rust, and the port happens *before* the
redesign above. Reason: the redesign is additive around a core that survives — log
and index, scheduler, runner, harnesses, parsers, MCP tools, CLI all stay; only the
egress proxy is replaced and the rest is new. Porting the core first means the new
pieces are written once, in the final language, on a floor already proven equal to
today's system. Rust is also the stronger tool for the hardest new piece (the
TLS-terminating proxy: `rustls`, `hyper`, `rcgen`), and it ships as one binary — the
"download one file" install a non-technical user needs.

**Port means the same program.** Same commands, same files on disk, same behaviour.
Not better, not redesigned. That is what makes it checkable.

### Frozen during the port

- `~/.taskrunner/events.jsonl` — the existing archive must load unchanged.
- `config.toml` — same keys, same defaults.
- MCP tool names and arguments; CLI commands and output.
- Test fixtures (sample Claude/Codex transcripts) — reused as-is.
- Docker images — they run `claude`/`codex`, not taskrunner. Untouched.

### Order — bottom up, each layer green before the next

One module, one PR, tests green per step. The old egress proxy
(`docker/egress-proxy/server.cjs`) is *not* ported — it runs in its own image and is
replaced in the redesign. Rust lives in `rust/` (one crate, `lib.rs` + `main.rs`,
modules named after the TypeScript directories, tests under `rust/tests/`) until the
TypeScript is deleted, then moves to the root. Each step ticks its line here when it
lands.

**What the code says that this list originally missed.** There was no
`events.jsonl` on the author's machine — the corpus has to be built first. The
transcript fixtures are inline strings in the tests, not files. There are no
container integration tests: `tests/workers/integration.test.ts` drives a fake
`codex` script through `LocalRunner`, and the Docker runner is tested only at the
argv level. The query and rendering side (`domain/tasks`, `daemon/lookup`,
`daemon/transcript-view`, `render` — ~1,500 lines, ~1,000 lines of exact-string
tests) sits on the index and is part of step 1. The shim↔daemon transport is
internal, not frozen.

0. **Spike and corpus.** `rustup`; a `rust/` skeleton; confirm `rmcp` serves
   Streamable HTTP on a unix socket (it ships `transport-io`,
   `transport-streamable-http-server`, `transport-streamable-http-client-unix-socket`
   and `schemars`; `libsqlite3-sys` bundled enables FTS5). Build the corpus: run the
   TypeScript daemon once so it sweeps `~/.claude/projects`, and drive one task
   (assign, continue, cancel) through the real scheduler so the log holds every
   event kind; freeze the log outside git. `scripts/parity-index.sh <events.jsonl>`
   refolds the log with both implementations and diffs `sqlite3` dumps of every
   table ordered by primary key (FTS shadow tables skipped) — the `sqlite3` CLI is
   the neutral witness. Decision: keep HTTP on the socket; the shim stays a dumb
   forwarder.
1. **Storage** — two PRs. *1a* `ids`, `storage/{events,facts,index,artifacts}`:
   the 16 event bodies as a `type`-tagged enum, torn-tail stop and repair, fsync
   and the bulk path, schema v7 verbatim, `apply`/`rebuild`/`turn_for`. Tests
   `events`, `index` (9), `message-facts`, `artifacts` ported 1:1. *Check:*
   `parity-index.sh` on the corpus prints nothing. Known traps: `ts` must be JS's
   `toISOString()` shape (`…T10:00:00.000Z`) because timestamps are compared as
   text in SQL; `serde_json` needs `preserve_order` or every `payload` row differs;
   JS prints `1.0` as `1`. *1b* `domain/{tasks,projects,policy}`,
   `view/{transcript,render}`, `lookup`. Tests `outline`, `timeline`, `transcript`,
   `lookup`, `policy` ported as-is — they already assert exact strings and *are* the
   golden master; `insta` is for new tests only.
2. **Ingest.** First a TypeScript PR that moves the inline sample lines into
   `tests/fixtures/{claude-code,codex}/*.jsonl` (no behaviour change). Then
   `ingest/{parser,claude_code,codex,registry,sweep,volume}`. The sweep runs on
   `spawn_blocking`, so the 50 ms yield in `sweep.ts` has no equivalent — the
   invariant it protected (daemon answers within the shim's 10 s during a backfill)
   is kept by the runtime, and the test is restated that way. `flush()` before
   `save_state()` stays. *Check:* parser tests (fixtures in, same messages out), the
   11 sweep cases, `volume`; and a fresh Rust sweep of the same host directories
   refolded and diffed against the corpus with ids and `*_recorded_at` projected
   out.
3. **Config and paths.** serde + `toml`, defaults per worker, `[worker.<name>]`
   catch-all as a flattened map, `deny_unknown_fields` on ingest sources only.
   `paths`, `expand_home`, and the harness tables moved out of `daemon.ts`
   (`HARNESS_KINDS`, `AUTH_MOUNTS`, `DEFAULT_IMAGES`, `TRANSCRIPT_SUBDIR/FORMAT`,
   `ingestSources`, `buildHarnesses`). *Check:* there is no `config.test.ts`, so a
   five-line TypeScript script prints `JSON.stringify(loadConfig(f))` for a handful
   of sample files (empty, custom worker, oss worker, extra source, bad key) and the
   Rust binary prints the same; diff. Plus `harnesses.test` and a strict-rejection
   test.
4. **Daemon and socket.** Lock via `hard_link`, the boot order (repair → rebuild →
   recover crashed turns → listen → chmod 0600 → reap copy-out containers → sweep),
   `/status`, the read routes, MCP sessions recording `session.started/ended` with
   **no tools yet**, sweep timer, `stop()` in today's order. The shim and
   `up`/`down`/`status` land here — the daemon is untestable from outside without
   them. *Check:* `daemon` cases except the config-only-worker one, the shim race
   test, and `claude mcp add` against the Rust binary showing zero tools.
5. **Workers** — two PRs. *5a* `workers/runner` (docker argv, network, proxy
   sidecar, egress log → `on_egress`, `dispose`), `workspace/{git,clone}`; tests
   `runner`, `clone`. *5b* `workers/{harness,claude,codex}`; tests `claude`, `codex`
   against the fake binaries, extracted to `tests/fixtures/fake-{claude,codex}.js`
   and run with `node` (a test-only dependency). *Check:* those, plus a new
   env-gated `TASKRUNNER_LIVE_DOCKER=1` test that runs `echo` in a worker image
   behind the real proxy — the runner has no automated exercise today.
6. **Scheduler.** Assign/continue/cancel, `wait`, one running turn per task,
   timeouts, tiers and approvals, worker-session and artifact events, `afterTurn`;
   wired into the daemon's `stop()`. *Check:* the 14 scheduler cases, `integration`
   (fake codex + clone workspaces), daemon's config-only worker, and one manual
   `TASKRUNNER_LIVE_CODEX=1` run.
7. **MCP server and CLI** — two PRs. *7a* the six tools, the `tool.<name>` audit
   wrapper, `buildInstructions` verbatim; `tools` test. *Check:* `initialize` +
   `tools/list` through both shims, JSON diffed — names and argument names must
   match exactly, and schema noise (`$schema`, `additionalProperties`) is matched
   rather than stripped, since Claude Code reads it. *7b* `sessions/session/search/
   task/tasks/doctor`, usage text, `--state-root`, EPIPE guard. *Check:*
   `scripts/parity-cli.sh` runs a fixed list of commands against the corpus with
   both binaries and diffs byte for byte. Then register the Rust binary with Claude
   Code and use it daily. `doctor` has no tests and nothing depends on it; it goes
   last.

### Crates

| Need | Crate |
|---|---|
| JSONL, config | `serde`, `serde_json` (`preserve_order`), `toml` |
| SQLite + FTS5 | `rusqlite` (bundled) |
| Async, socket, process spawn | `tokio`, `hyper` |
| MCP | `rmcp` (official Rust SDK) |
| CLI | `clap` |
| Ids | `ulid` |
| Tests | `cargo test`; `insta` for new snapshots only |

### Done when

`cargo test` is green; `parity-index`, the sweep parity, the config parity, the
`tools/list` diff and `parity-cli` are all empty; the Rust binary has been the daily
driver long enough to trust. Then one PR deletes `src/`, `tests/`, `package.json`,
`tsconfig.json`, `vitest.config.ts` and `scripts/debug-refold.ts`, moves `rust/` to
the root, and updates `README`, `getting-started` (install is one binary) and
`internals` (`tsx`/`vitest` → `cargo`; the event-loop-yield paragraph goes).
`scripts/build-images.sh` and `docker/` are untouched. Not kept alongside; then the
redesign begins.

### Cost, honestly

~6.5k lines of source and ~4k of tests, mostly mechanical. Writing Rust is slower at
first (borrow checker, async, 10–60 s compiles vs instant `tsx`); building and
testing are simpler (`cargo build`, `cargo test`, one binary). Storage is the step
that takes longest and teaches most.

## How the work is done

Clean and readable is a goal of the redesign, not a nicety after it.

- **Remove old code as it is replaced.** When a path is superseded (a per-harness
  ingestion toggle, a legacy event shape, a mount table nobody reads), delete it in
  the same change. No parallel old-and-new. Legacy event kinds stay parseable only
  because the log is append-only — mark them as such and keep the note short.
- **Simplify for a human reader.** Prefer one obvious way over a clever one. A
  function should be readable top to bottom by someone new to the project; if it
  needs a paragraph of comment to explain *what* it does, restructure it instead.
  Comments say *why*.
- **Docs move with the code.** Every change that alters behaviour updates the doc
  that describes it (`docs/security.md`, `docs/transcripts.md`, `docs/configuration.md`,
  `README.md`) in the same commit. A doc that lags the code is worse than none —
  it is confidently wrong.
- **This document is retired, not archived.** As pieces land, their sections move
  into the real docs and are deleted here. When it is empty, it goes.

## Open questions

- **Hermes parser.** `state.db` schema is versioned and migrates; the parser reads
  `sessions` + `messages` (read-only, WAL is fine while Hermes writes) and must
  tolerate drift. Lean: ingest every source including `subagent`/`kanban`, demote
  in ranking rather than hide.
- **Search ergonomics.** Hermes's `session_search` defaults `role_filter` to
  `user,assistant` (tool output is noise unless asked for) and hides automation
  sessions from discovery. Taskrunner's `search-transcripts` has the same noise
  problem with worker turns; worth copying.
- **`rmcp` spike.** The port assumes the official Rust MCP SDK covers what the
  TypeScript one does here (stdio server, tool schemas, sessions). One afternoon to
  confirm, before port step 1.
- **Daemon as a service.** systemd user unit / launchd, install and uninstall
  commands, what a stop does to a running task. Small, but it gates host capture and
  `taskrunner shell`.
- **Testing the proxy.** Needs a fake upstream that speaks the Anthropic and OpenAI
  streaming shapes. Local models (Ollama, LM Studio) are plain HTTP — capture is
  trivial there and needs no certificate.

## Not doing

- Redesigning and porting at the same time. Port the core first (see above), then
  build the new pieces in Rust only.
- Porting the egress proxy. It is replaced, not carried over.

- A per-harness "ingest off" switch. Replaced by the one-archive decision above.
- Host wire capture or Docker-hosted sessions as defaults. Both are opt-in.
- Reading, modifying, or reusing credentials in the proxy. Record, forward
  unchanged, strip from the log.
