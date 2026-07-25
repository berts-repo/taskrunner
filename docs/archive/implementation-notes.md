# Implementation notes

Design rationale and internals, kept out of the user-facing docs. This is a
maintainer reference — the "why it's built this way" behind the features described in
[the docs](../). Nothing here is needed to *use* Taskrunner.

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
  repopulates a fresh index from `events.jsonl`. (Most recently, bumping 5 → 6 to add
  the `transcript_sessions` aggregate needed no migration for exactly this reason.)
- **Artifacts are content-addressed.** Diffs and raw worker event streams are stored
  by hash, referenced from the index.

## Daemon, shim, and the socket

- The `mcp` command is a thin stdio shim. It forwards to a single daemon over a unix
  socket under the state root, auto-starting the daemon if needed, so any number of
  MCP clients share one daemon.
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
  insert actually changed a row (`res.changes > 0`) — otherwise a rebuild would
  double-index every re-swept message.
- **Byte offsets are a cache only.** `~/.taskrunner/ingest-state.json` records how far
  each source was read to make resumption incremental; deleting it forces a harmless
  full re-scan, and the event log stays the sole source of truth.
- **Session aggregate.** A `transcript_sessions` table holds one row per distinct
  `(source, native_session_id)` — project, first/last timestamp, message count —
  maintained by the same `message.recorded` fold, **inside the same `res.changes > 0`
  guard** as the FTS insert so a re-swept message never double-counts. It exists so
  listing sessions by recency is O(sessions) rather than a `GROUP BY` over all of
  `messages`; recency orders by `COALESCE(last_ts, last_recorded_at)` (formats without
  a per-record timestamp still order by ingest time). Like everything here it is
  derived — a delete-and-rebuild replays the log and reconstructs it exactly.

### Query surface

- **`lookup-session`** lists sessions from `transcript_sessions` (recency, optional
  project filter, task link via a correlated `worker_sessions` subquery so a
  multi-task session stays one row), or reads one session's messages straight from
  `messages` keyed on `(source, native_session_id)` — so a host session no task links
  is still readable. A bare id matching several sources lists candidates rather than
  guessing.
- **Scoped `search-transcripts`.** `searchMessages` builds its `WHERE` dynamically over
  `messages_fts` (still `messages_fts MATCH ?` even when aliased) joined 1:1 to
  `messages` on the unique `message_id` — the join is what carries `project_path` into
  a hit and lets `project` filter. `sessions`/`lastSessions` add `native_session_id IN
  (…)` (the latter resolved through `listSessions`); `role`/`kind`/`since`/`until`
  filter the FTS row's UNINDEXED columns; `sort:"recent"` orders by `native_ts` instead
  of `rank`.
- **On-demand host sweep.** Session-recency queries call `sweep({ hostOnly: true })`
  first — it skips volume sources (no `docker cp`), so the newest host session reflects
  the live conversation without paying for a worker-volume copy-out. Coalescing is
  first-caller-wins; the interval sweep still reaches the volumes.
- **CLI parity.** The daemon serves read-only routes (`/lookup-session`,
  `/search-transcripts`, `/lookup-task`) over the control socket that call the *same*
  renderers as the tools, so `taskrunner sessions|session|search|task|tasks` print
  byte-identical output without opening an MCP session.

### Sweeper invariants (load-bearing)

These were paid for in real bugs; a "simplification" reintroduces them.

- **Everything the sweeper calls is async and yields.** A synchronous first sweep once
  blocked the event loop ~375s on a real corpus, so the daemon never became ready
  within the shim's 10s window. `startSweeping()` runs *after* `server.listen()`, and
  `runSweep`/`sweepFile` yield via `setImmediate` on a **50ms time budget** — a time
  budget, not a line count, because a single Codex rollout record can carry an entire
  tool payload. Phase 2's synchronous `docker cp` reintroduced the same stall through
  a different door and had to be made async too.
- **`flush()` immediately before `saveState()` in `runSweep`.** Ingest opts out of
  per-record fsync (fsync cost ~28.5ms/record dominated the backfill); offsets must
  never advance past durably-logged records, or a lost tail plus an advanced offset
  means dedupe never re-reads those messages. Do not reorder those two lines.
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
Claude Code), because the shim's connection to the old daemon is gone.
