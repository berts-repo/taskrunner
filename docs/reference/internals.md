# Implementation notes

Design rationale and internals, kept out of the user-facing docs. This is a
maintainer reference — the "why it's built this way" behind the features described in
[the docs](./). Nothing here is needed to *use* Taskrunner.

## Storage model

- **The event log is the only write path.** Every durable fact is appended as one
  JSON line to `~/.taskrunner/events.jsonl`. Nothing else is authoritative.
- **The SQLite index is derived and disposable.** It's a fold of the event log built
  for fast reads (`lookup-task`, search). Delete-and-rebuild from the log is the
  universal recovery path, so the reducer is deterministic (it uses event timestamps
  only, never the wall clock) and idempotent (id-keyed `INSERT OR IGNORE`, natural-key
  updates).
- **The daemon rebuilds the index from the log on every boot.** This is why a schema
  change is safe with no migration step: bump `SCHEMA_VERSION` and the next start
  repopulates a fresh index from `events.jsonl`. (Most recently, bumping 6 → 7 to add
  the per-message tool facts and prompt index needed no migration and no re-sweep for
  exactly this reason — the log already held the content they are derived from.)
- **The log is hash-chained and anchored.** Each line carries `prev`, the fingerprint of
  every line before it, and the daemon appends the current fingerprint to
  `anchors.jsonl` on open, every 100 durable events and on stop. `storage/chain.rs`
  holds the fingerprint, the walk and `verify`; the log writer only adds `prev` and
  anchors. Format, rules and the pre-chain history are in
  [Proving the record hasn't changed](../guide/log-integrity.md#technical-details).
- **Artifacts are content-addressed.** Diffs and raw worker event streams are stored
  by hash, referenced from the index.

## Daemon, shim, and the socket

- The `mcp` command is a thin stdio shim. It pumps the client's stdio byte for byte
  to one daemon's `runtime/mcp.sock`, auto-starting the daemon if needed, so any
  number of MCP clients share one daemon.
- **A connection can name its harness.** `taskrunner mcp --host <name>` makes the shim
  send one line, `taskrunner-host <name>`, before any JSON-RPC. The daemon reads it (only a first byte of `t`
  starts one) and records `host` on `session.started`
  (index schema 8). It labels the session, for rendering skills and for the audit
  trail; the owner-only socket is still the only access control.
- **A connection is a session.** Each connection gets its own MCP service and its own
  `session.started`/`session.ended` records. MCP protocol `2026-07-28` removes
  protocol-level sessions, so the session record can't lean on the SDK's HTTP session
  manager.
- A second socket, `runtime/daemon.sock`, serves HTTP: `/status` and the read-only
  query routes the CLI uses.
- **The tool contract is data.** `src/daemon/tools.json` holds the six tools' names,
  descriptions and input schemas, served verbatim and used to validate every call.
  Invalid arguments are a protocol error: the tool never runs and nothing is audited.
- **`/wait-task` long-polls.** It answers when the task's running turn ends (the same
  watch `wait: true` uses) or its `timeout` passes, as JSON `{status, text}` so
  `taskrunner wait` takes its exit code from a field. The CLI asks in 5-minute rounds.
- **Skills are compiled in and leave two ways.** `skills/<name>/SKILL.md` is embedded
  in the binary (`src/skills.rs`), and `delegate-task`'s description is rendered from
  the host's `delegation` setting. Over MCP the daemon declares the resources
  capability and the `io.modelcontextprotocol/skills` extension (SEP-2640), answers
  `skills/list` and `skills/get` as custom methods, and serves each file as
  `skill://<name>/SKILL.md`, rendered for the session's host. Entries carry the SEP's
  per-file `resources` digests plus a top-level `digest`, which Claude Code 2.1's
  pre-final client checks. For harnesses that can't fetch skills yet, `taskrunner sync`
  writes the same rendering under `skills/<host>/` as read-only files and links it into
  the harness; the daemon rewrites those files on boot, so an upgrade needs no sync.
  Skills requests are audited (`skills.list`, `skills.get`,
  `resources.list`, `resource.read`); a
  `skills.list` from a host within the last week is how sync knows to drop that host's
  links. A week rather than the latest session, because `claude mcp get` health-checks
  the server with a session that fetches nothing.
- **`sun_path` limit.** A unix socket path is capped around 104 bytes on macOS. State
  roots with long paths fail fast with a clear error rather than a confusing bind
  failure; tests and any throwaway state roots must use short paths.
- Turns left `running` by a crash are recovered on the next boot (failed with their
  audit retained), before the socket starts accepting requests.

## Network firewall

The egress proxy is a sidecar on an internal Docker network; the worker container's
only route out is through it.

- **DNS-rebinding defense.** The proxy resolves every destination itself at connection
  time and refuses loopback, LAN, and other special-use addresses — an approved (or
  maliciously rebinding) domain can never resolve to something on the local network.
- **Explicit local pinning.** Deliberate local destinations must be named as an
  IP-literal (`127.0.0.1:8080`) or a Docker host name
  (`host.docker.internal:11434`); this is how a local-model worker reaches its server.
- **Port rules.** Entries without a port — including `"*"` — cover only ports 80 and
  443.
- Every connection attempt, allowed or refused, is audit-logged. `networked` tasks
  (any `allowDomains`) require agent-relayed user approval, recorded as an approval
  event.

## Webpost drafting skill

`draft-webpost` is a built-in skill registered in `src/skills.rs`, distributed
through the same MCP and sync paths as the other built-in skills. It uses the
current session, project files, and scoped archive queries to draft short portfolio
posts for `~/Projects/helloto/`. Draft MDX and private review notes live under
`.webpost-drafts/<slug>/`, the canonical draft location outside published content.
Discovery includes hidden and ignored drafts, project-local writeups, and archive
exchanges when a referenced draft cannot be found locally. Continued older drafts
record their source path in the review note. It revises matching drafts,
keeps proposed updates separate from published articles, and publishes only after
an explicit user request for the reviewed article. Publishing checks the site's
production branch and build, commits only approved content, uses the established
deployment workflow, and records commit and live-URL verification in the private
review note. Articles include a verified public GitHub repository link when
available; the review note records the URL and visibility evidence or its omission.
The skill checks the site's loader and article route for formatting; the current
page supplies the H1, so draft bodies begin with prose. Review notes identify the
intended post path and route without writing to published content.
Editorial instructions belong in `skills/draft-webpost/SKILL.md`;
generated harness copies are not the source.

## Conversation archive

The sweeper periodically ingests transcripts into the event log as `message.recorded`
events, folded into the `messages` table and mirrored into the `messages_fts` FTS5
table for search.

- **Two source kinds.** Host directories (Claude Code under `~/.claude/projects`,
  Codex under `~/.codex/sessions`) are configured. Worker-volume sources are **not**
  configured — they are derived from each worker's `auth_volume` + `image` + a
  per-harness transcript subpath, so the volume name, the image used to reach into it,
  and the subpath can never drift apart. A stray hand-written `volume` key is rejected
  by the strict source schema.
- **Reaching into worker volumes.** Transcripts inside a worker's Docker auth volume
  are copied out with a short-lived `docker cp` (create a container with the volume
  mounted read-only, copy the subtree, remove it) — no host mount, and the image needs
  no `tar`/shell. Copies land under `~/.taskrunner/ingest-staging`, created owner-only
  because an auth volume also holds that worker's credentials. A volume source with no
  subdir is refused rather than defaulting to the volume root (which would copy the
  credentials).
- **Task linkage.** Worker transcripts join to their task at query time via
  `worker_sessions.native_session_id = messages.native_session_id`. Host transcripts
  have no such link and surface as un-attributed in search.
- **Idempotent ingest.** `message_id` is a deterministic hash of
  `(source, native_session_id, native_record_id)`, so re-sweeping a record re-emits
  the same event and `INSERT OR IGNORE` keeps `messages` clean. **The FTS table has no
  such guard**, so the fold inserts into `messages_fts` only when the `messages`
  insert actually changed a row (the insert's changed-row count) — otherwise a rebuild would
  double-index every re-swept message.
- **Unknown record types are reported, not silently dropped.** A parser counts every
  line it skips only because it does not know the record type, and the sweeper logs
  the counts per file (`ingest: codex: skipped records it does not recognise in …:
  response_item/foo ×3`). Types skipped on purpose are listed in the parser and not
  counted. This exists because Codex 0.154 moved most tool calls to
  `custom_tool_call`, and for a while they reached the archive as nothing at all.
  Only newly read lines are parsed, so each line is reported once.
- **Byte offsets are a cache only.** `~/.taskrunner/ingest-state.json` records how far
  each source was read to make resumption incremental; deleting it forces a harmless
  full re-scan, and the event log stays the sole source of truth.
- **Session aggregate.** A `transcript_sessions` table holds one row per distinct
  `(source, native_session_id)` — project, first/last timestamp, message count —
  maintained by the same `message.recorded` fold, **inside the same inserted-a-row
  guard** as the FTS insert so a re-swept message never double-counts. It exists so
  listing sessions by recency is O(sessions) rather than a `GROUP BY` over all of
  `messages`; recency orders by `COALESCE(last_ts, last_recorded_at)` (formats without
  a per-record timestamp still order by ingest time). Like everything here it is
  derived — a delete-and-rebuild replays the log and reconstructs it exactly.

### Message facts (schema v7)

`storage/facts.rs` promotes a handful of objective facts out of each message's
content blob at projection time — `tool_use_id`, `tool_name`, `tool_target`,
`is_error`, and the `prompt_idx` an exchange is addressed by. This is what turns
"every Edit under `src/daemon`" into a query instead of a grep over JSON.

- **Only unambiguous facts get promoted.** They are re-derived on every rebuild, so a
  wrong call here costs a delete-and-rebuild rather than a migration — but it also
  means anything needing a judgement about conversation structure (parenting,
  sidechains) is deliberately left out. `parent_message_id` / a message tree was
  considered and dropped; the outline has since been used against real sessions
  without wanting it.
- **Structural, not source-switched.** `source` is free-form and new harnesses appear,
  so each fact is read from whichever known field spelling is present (claude-code's
  `id`/`tool_use_id`/`input`, codex's `call_id`/`arguments`, which also arrives as a
  nested JSON *string*). An unrecognized shape yields nulls, never a throw. Codex's
  `exec` passes a JavaScript program as `input`; it is not an object, so the call
  keeps its id and name but no `tool_target`, and the program is shown whole.
- **A target is an identifier, not a sentence.** `TARGET_KEYS` is an ordered list,
  most specific first; prose keys (`description`, `prompt`, `explanation`) are excluded
  on purpose, which is why prose-only tools legitimately have no `tool_target`. Codex
  passes argv as an array, so a target is flattened to one line, whitespace collapsed,
  and capped at 500 chars — the full text stays searchable through FTS.
- **Failure is never guessed.** A harness-recorded `is_error` wins; otherwise the exit
  status codex prints ahead of exec output is parsed. Records stating neither
  (rejections, aborts, MCP payloads) stay NULL rather than being folded into a false
  "succeeded".
- **`prompt_idx` counts *real* prompts.** Sessions are full of user records the harness
  wrote on the user's behalf, and counting those destroys the numbering as an address —
  a session picks up more on every slash command, interruption and reminder. The
  `HARNESS_PREFIXES` exclusion list was derived by surveying the whole live corpus, not
  guessed, and matches as a prefix of the trimmed content. A real session measured 52
  prompts against 68 raw `user/message` records.
- **Everything before a session's first real prompt stays at 0**, which is why the
  outline has a `[0] (before the first prompt)` group at all.
- **The counter is read before the insert and committed only if the insert changed a
  row** — inside the same inserted-a-row guard as the FTS insert and the session
  aggregate, so a re-swept duplicate can never advance the numbering.

### Query surface

- **`lookup-session`** lists sessions from `transcript_sessions` (recency, optional
  project filter, task link via a correlated `worker_sessions` subquery so a
  multi-task session stays one row), or reads one session's messages straight from
  `messages` keyed on `(source, native_session_id)` — so a host session no task links
  is still readable. A bare id matching several sources lists candidates rather than
  guessing.
- **Scoped `search-transcripts`.** `search_messages` builds its `WHERE` dynamically over
  `messages_fts` (still `messages_fts MATCH ?` even when aliased) joined 1:1 to
  `messages` on the unique `message_id` — the join is what carries `project_path` into
  a hit and lets `project` filter. `sessions`/`lastSessions` add `native_session_id IN
  (…)` (the latter resolved through `list_sessions`); `role`/`kind`/`since`/`until`
  filter the FTS row's UNINDEXED columns; `sort:"recent"` orders by `native_ts` instead
  of `rank`.
- **Filter-only search.** With no `query` the same function drops the FTS
  join and scans `messages` directly on the v7 fact columns, ordered by recency — so
  `tool`/`target`/`failed` are a search in their own right, not just a narrowing of a
  text hit. `target` matches `LIKE %…%` (paths are searched by fragment); `tool` is
  exact. The filter names on both wires are `tool`/`target`/`failed`, not the column
  spellings.
- **A call and its result are one thing.** `ERROR_STATE` coalesces a row's own
  `is_error` with its pair's, so `failed` composes with `tool`/`target` — which only
  exist on the *call* record. Because both halves then match, a filter-only search adds
  `kind = 'tool_use'` (the useful half: it names the tool and the target), or one
  failure would be reported twice. Deliberately **not** applied when there is a text
  query: there the caller asked for whichever record their words appear in. Results
  stating no outcome are NULL and so fall out of `failed:true` *and* `failed:false`.
- **On-demand host sweep.** Session-recency queries call `sweep` with `host_only`
  first — it skips volume sources (no `docker cp`), so the newest host session reflects
  the live conversation without paying for a worker-volume copy-out. Coalescing is
  first-caller-wins; the interval sweep still reaches the volumes.
- **CLI parity.** The daemon serves read-only routes (`/lookup-session`,
  `/search-transcripts`, `/lookup-task`) over the control socket that call the *same*
  renderers as the tools, so `taskrunner sessions|session|search|task|tasks` print
  byte-identical output without opening an MCP session.

### Rendering: the three views

`view/transcript.rs` owns all three renderers plus the compact-line helpers, which
`view/lookup.rs` also uses for audit/trace rows — so the dependency runs lookup →
transcript, never back.

- **`outline`** — the session as an index: a counts line, then one block per exchange
  headed by its `[N]` address, one line per reply and per tool call. Every call gets a
  line in the order it was made, because a collapsed or capped list stops the outline
  being a *complete* index of what happened.
- **`compact`** — one truncated line per message, every message, one scan.
- **`timeline`** — the audit view: bodies printed unindented and unmodified so code and
  diffs stay copy-pasteable. The prompt index is stamped on the first message of each
  exchange only; that is the address `--prompt N` takes, and repeating it every line is
  noise.

**The clipping rule.** What the conversation *said* is never clipped — user prompts,
assistant replies, reasoning. Everything else is harness furniture and shares the
`--tool-lines` budget (default 20, `0` = all): tool inputs, tool results, and
developer/system preambles, the last of which matters because a codex worker session
opens with ~15k characters of them. Tool inputs render the target on line 1 and the
remaining input keys after it, so a Write/Edit payload stays in the audit trail.

**The outline never loads a body.** It is fed by its own query, not the message path:
prose is `substr(content, 1, 400)` in SQL, `tool_use` rows select NULL, and
`tool_result` rows are excluded outright (their failure state is already folded onto
the call). Reasoning, developer preambles and harness-written user records are left
out — a user record that does not *open* a prompt group is by definition one the
harness wrote. Measured on a 240-message session: outline 13k bytes, compact 47k,
timeline 123k. The `[0]` heading is deferred until something files under it, and must
be cleared when a new group opens or it leaks into the next one.

**View resolution is what made the default flip safe.** `resolve_view`: an explicit
`view` wins; else `prompt N` → `timeline` (drilling into one exchange means reading
it); else `last` → `compact` (a bounded message read, exactly as before); else
`outline`. Without those two carve-outs, changing the default would have silently
changed every existing caller. The drill-down selector is `prompt` on the MCP wire and
`--prompt N` on the CLI, mapped to `prompt_idx` internally (in
`daemon/tools.rs`) because `prompt` already means the worker instruction on `assign-task`.

**Defaults are split by surface on purpose.** The CLI defaults to `timeline` with no
message cap (`limit: null` → `LIMIT -1`) and always sends `view` explicitly; the MCP
tools and daemon routes resolve to `outline`, and the 500-message cap still applies
whenever they fall to a message view. (The outline itself is not capped — it is an
index, and a partial one would be a lie.) A person at a shell wants to read; an agent
scanning pays for every line it takes in — so the two surfaces never have to agree. An
explicit `last` always wins.

Two incidental hazards worth keeping: the CLI has no boolean flags, because a bare
`--failed` would swallow the next argv entry as the query (`--failed true|false` takes
a value); and `read_query` in `src/cli.rs` writes output itself, ignoring a broken
pipe, because `print!` panics when a timeline-sized result is piped to `head` and the
reader exits early. Only the CLI does this — the daemon shares the binary, and must
never be taken down by a client hanging up.

### Sweeper invariants (load-bearing)

These were paid for in real bugs; a "simplification" reintroduces them.

- **The sweep never runs where requests are served.** A first sweep once blocked the
  daemon ~375s on a real corpus, so it never became ready within the shim's 10s
  window. Sweeping starts only after the sockets are listening, and each sweep —
  `docker cp` copy-out included — runs on a blocking thread (`spawn_blocking` in
  `daemon/sweep_gate.rs`), so a backfill of any size can't delay a request.
- **`flush()` immediately before `save_state()` in `Sweeper::sweep`.** Ingest opts out of
  per-record fsync (fsync cost ~28.5ms/record dominated the backfill); offsets must
  never advance past durably-logged records, or a lost tail plus an advanced offset
  means dedupe never re-reads those messages. Do not reorder them.
  Lifecycle events stay fsynced because they are authoritative and not reconstructible;
  `message.recorded` is derived from transcripts ingest never modifies, so a lost tail
  is re-readable work rather than data loss.
- **Full copy-out each sweep (known cost).** `docker cp` has no incremental mode, so
  the whole worker subtree is re-copied every sweep. Parsing stays incremental via the
  byte offsets; the copy does not. Acceptable while worker corpora are small; revisit
  against a real one.

## Worker sign-in

Docker workers authenticate from their own named volume, never from host credentials
(`~/.codex`, `~/.claude`) — sharing those lets host and container sessions invalidate
each other's refresh tokens (the `refresh_token_reused` failure). Signing in via a
browser or IDE does **not** rewrite `~/.codex/auth.json`; only the CLI login flow does.

- **Codex uses `--device-auth`** because the normal localhost OAuth callback can't
  cross the container boundary — the login server would bind the container's loopback,
  unreachable from your browser. The volume mounts at `~/.codex`, which holds only
  codex's own auth/session state; the daemon mounts it at that same narrow path during
  turns.
- **Claude logs in with the volume at the whole home**, but during turns the daemon
  mounts only `~/.claude` and `~/.claude.json` out of it — so a task can't plant shell
  rc files or other home-directory state for a later turn. `platform.claude.com` is on
  claude's egress allowlist because it serves the OAuth token refresh; blocking it
  strands the worker with 401s once its access token ages out.
- **Log in at exactly those paths.** The wrong path buries the credentials where the
  daemon's turn-time mounts can't see them, surfacing as `401 Missing bearer` against
  `api.openai.com`.

## Restarting the daemon

After a rebuild, run `taskrunner down` so a stale daemon isn't left running old code.
A mid-session restart drops the MCP tools until the client reconnects (`/mcp` in
Claude Code), because the shim's connection to the old daemon is gone. A restart is
also how a changed `[host.<name>] delegation` reaches skills served over MCP (the
daemon reads config at boot); `taskrunner sync` rewrites the skills on disk at once.
