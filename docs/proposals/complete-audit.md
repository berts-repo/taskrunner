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

## Install flow (sketch)

```
taskrunner setup
  Run your own agent inside Docker?          [no]
    Paths it may see besides the project:    ~/.gitconfig, ...
  Record your host sessions through the proxy?  [no]   (needs the daemon service)
  Worker capture is always on.
```

Every answer is a config key, not a lock-in. `taskrunner capture enable|disable` and
the choice of `claude` vs `taskrunner shell` flip them later.

## One search tool, everywhere

The parser copies Hermes's rows into the archive; the agent is not involved and does
no sorting. The copy on disk is harmless. The redundancy that *does* matter is in the
agent's tool list: inside Hermes the model would see two tools that answer "what did
we do about X" — Hermes's `session_search` and taskrunner's `search-transcripts` —
and has to guess which.

Recording never turns off; what is chosen is which tool the agent sees:

- Keep both — ❌ the model guesses; answers differ by which it picked.
- Hide taskrunner's search in Hermes — ✅ feels native. ❌ Hermes's tool knows only
  Hermes sessions: no Claude Code or Codex history, no inside of a delegated turn.
- Hide Hermes's `session_search` (its `disabled_toolsets`) and use taskrunner's —
  ✅ one tool, and it is the superset. ❌ loses Hermes's scroll/browse shapes unless
  taskrunner's tools grow them.

**Decision: the third.** The archive is the audit, so its search is the one the agent
reaches for in every harness. Which is the reason to copy Hermes's ergonomics into
`search-transcripts` / `lookup-session`: `role_filter` defaulting to
`user,assistant`, scroll-around-a-message, browse-recent, demote (not hide)
automation sessions in ranking.

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

### Size and retention

Measured 2026-09-13 on the author's machine: 7.9 MB of Claude Code transcripts (28
sessions, 30 days) and 11 MB of Codex — about **20 MB/month raw**, so ~250 MB/year of
log; the FTS index roughly doubles it to **~0.5 GB/year**. A heavy user with large
tool results might be 5–10× that. SQLite and FTS5 are comfortable into hundreds of
GB. Space is not the pressure; search quality (old noise crowding results) is, and
that is a ranking problem, not a deletion problem.

Pruning is the *harness's* choice for its working copy (Claude deletes at 30 days,
Hermes optionally by age). The archive is the thing that does not forget; if it
prunes too, nothing remembers. When shrinking is needed:

- **Archive and detach** — move old log segments to cold storage (JSONL compresses
  ~10×), keep the index over recent history. Nothing destroyed, just not hot.
- **Explicit, logged deletion** if truly required — append a `retention.pruned`
  event naming what was removed and why, so the archive records its own gap.
- Never silent, never age-based by default.

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

1. **Storage.** Read the log, fold the index, rebuild from scratch. *Check:* the
   SQLite the Rust build produces from the real `events.jsonl` matches the
   TypeScript one row for row. This single test proves most of the port.
2. **Ingest.** Claude and Codex parsers. *Check:* same fixtures in, same events out.
3. **Config and paths.**
4. **Daemon and socket.**
5. **Workers.** Docker runner, existing proxy sidecar (unchanged), Claude and Codex
   harnesses. *Check:* the existing container integration tests.
6. **Scheduler.** Tasks, turns, timeouts, cancel.
7. **MCP server and CLI.** *Check:* register the Rust binary with Claude Code in
   place of the Node one; nothing is missing.

One module, one PR, tests green per step. The old proxy is *not* ported — it is
replaced in the redesign.

### Crates

| Need | Crate |
|---|---|
| JSONL, config | `serde`, `serde_json`, `toml` |
| SQLite + FTS5 | `rusqlite` (bundled) |
| Async, socket, process spawn | `tokio` |
| MCP | `rmcp` (official Rust SDK) |
| CLI | `clap` |
| Ids | `ulid` |
| Tests | `cargo test`; `insta` for snapshots |

### Done when

`cargo test` is green; the Rust index built from the real log equals the TypeScript
one; the Rust binary has been the daily driver long enough to trust. Then the
TypeScript is deleted — not kept alongside — and the redesign begins.

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

- **Tamper-evidence.** If the archive must *prove* a record was not altered later,
  the append-only event log wants hash-chaining now, before there is history to
  migrate. Not decided.
- **Scope of "audit".** Egress decisions and changed files are logged already.
  Config changes, task assignments and capture on/off events are not; they should
  be, so the archive shows its own gaps.
- **Hermes beyond search.** Hermes also ships `delegate_task`, which collides with
  `assign-task` the way `session_search` collides with `search-transcripts`; same
  answer (disable Hermes's, use taskrunner's) or not? And Hermes is only a *host*
  harness in this proposal — is `[worker.hermes]` (an image, headless mode, its
  login) wanted too?
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
