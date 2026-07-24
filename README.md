# Taskrunner MCP Server

Taskrunner is a local-first delegation broker for MCP clients. Ask your
coding agent (Claude Code, Codex, or anything else that speaks MCP) to hand
a task to a configured worker; the worker runs in an isolated Docker
container over a task-local git clone, behind a filtering egress proxy, and
every observable step lands in a durable audit trail you can query back
over MCP.

- Four core tools: `assign-task`, `continue-task`, `lookup-task`,
  `cancel-task`.
- Asynchronous multi-turn tasks: assign returns immediately, `lookup-task`
  retrieves results (including paired exchanges and end-to-end traces), and
  `continue-task` resumes the worker's native session.
- Docker-only worker isolation: task-local clone mounted at `/workspace`,
  internal network with no outside route, egress proxy allowlist, and
  narrow worker-owned auth volumes — never host credentials.
- Durable storage: append-only JSONL event log as the write path, a
  rebuildable SQLite index, and content-addressed artifacts (worker event
  streams, diffs).
- Pluggable workers: a worker is a `[worker.<name>]` config entry with a
  `harness` key — never a code change.

## Core Concepts

- **Task**: delegated unit of work, scoped to a project.
- **Turn**: one interaction within a task.
- **Worker**: execution runtime such as Codex or Claude Code.
- **Worker harness**: integration code that starts, resumes, and records
  worker activity.
- **Session**: durable record of an MCP client connection; tasks and audit
  events link back to it.
- **Artifact**: stored output such as diffs and raw worker event streams.

## How it works

The `mcp` command is a thin stdio shim: it forwards to a single daemon over
a unix socket under the state root, auto-starting it if needed, so any
number of MCP clients share one daemon. `assign-task` registers the project
on first sight and clones it into a task-local workspace. Each turn then
runs in a fresh worker container joined to an internal Docker network whose
only route out is an egress proxy sidecar that forwards allowlisted domains
and logs every decision. When the turn ends, the daemon captures the
workspace diff and the worker's raw event stream as artifacts. Every
observable step is appended to `events.jsonl` — the sole write path — and
folded into a SQLite index that `lookup-task` reads and that can always be
rebuilt from the log.

Connecting agents need no out-of-band setup: the MCP handshake delivers
server instructions generated from the live config — the configured workers
with their egress defaults, the network-approval model, and the task
lifecycle — so any client that honors instructions discovers what it can
delegate and to whom.

## Conversation archive

Beyond the turns it runs itself, the daemon periodically sweeps the native
transcripts of host coding agents (Claude Code under `~/.claude/projects`,
Codex under `~/.codex/sessions`) into the same durable event log, one
`message.recorded` event per conversation record, folded into a `messages`
table. This makes `~/.taskrunner/events.jsonl` a permanent, queryable
archive of agent conversations that outlives each tool's own retention —
Claude Code, for instance, purges its transcripts after 30 days by default.

The same sweep also reaches inside the workers: each worker's transcripts
are copied out of its Docker auth volume (via a short-lived `docker cp`, no
host mount needed) and archived too, so the full interior of every
delegated turn — including intermediate tool calls — is captured and links
back to its task through the `worker_sessions` table. Worker-volume sources
are derived automatically from each configured worker's `auth_volume`, so a
custom worker is archived with no extra config.

The archive is ingest-only: native transcript files are never moved or
modified (session resume depends on them), and re-sweeping is idempotent —
message ids are a deterministic hash of `(source, session, record)`, so a
record already archived is skipped before anything is appended. Byte offsets
in `~/.taskrunner/ingest-state.json` are only a performance cache; deleting
it forces a harmless full re-scan and the event log stays the sole source of
truth. Sources are pluggable exactly like workers — an
`[ingest.sources.<name>]` entry naming a `format` adds one, no code change.

To stop Claude Code's 30-day purge so nothing is lost before the first
sweep, raise `cleanupPeriodDays` in `~/.claude/settings.json`.

## Quick Start

Requires Node 22+ and Docker.

```sh
npm install
npm run build
npm run build:images   # worker + egress proxy images
claude mcp add --scope user taskrunner -- node /path/to/taskrunner/dist/cli.js mcp
```

The daemon starts on demand and keeps durable state under `~/.taskrunner/`.
CLI: `taskrunner up | down | status | doctor | mcp` (`--state-root <dir>`
overrides the state root). `taskrunner doctor` is a read-only preflight over
Docker, worker images and auth volumes, the egress proxy image, and
transcript-ingestion health — run it when a worker won't start.

Tests: `npm test`. Live Codex delegation check (requires
`codex login`): `TASKRUNNER_LIVE_CODEX=1 npx vitest run tests/workers/integration.test.ts`.

## Worker sign-in

Docker workers authenticate from their own named volume — never from host
credentials (`~/.codex`, `~/.claude`), which would let host and container
sessions invalidate each other's refresh tokens. Build the images first
(`npm run build:images`), then log each worker in once:

```sh
# Codex: device auth avoids the localhost callback, which cannot cross the
# container boundary (the login server binds the container's loopback).
# Mount the volume at ~/.codex — it holds only codex's own auth/session
# state, and the daemon mounts it at that same narrow path during turns.
docker run -it --rm -v taskrunner-codex-home:/home/worker/.codex \
  taskrunner/codex-worker codex login --device-auth

# Claude: the interactive login flow hands you a URL and takes a pasted
# code. Log in with the volume at the whole home; during turns the daemon
# mounts only ~/.claude and ~/.claude.json out of it, so tasks cannot plant
# shell rc files or other home-directory state for a later turn.
docker run -it --rm -v taskrunner-claude-home:/home/worker \
  taskrunner/claude-worker claude /login
```

Open the URL each flow prints, approve, and the credentials land in the
volume. Repeat only when a worker's login expires or a new machine needs
setting up. Log in at exactly the paths above (narrow `~/.codex` for codex,
whole home for claude): logging in at the wrong path buries the credentials
where the daemon's turn-time mounts can't see them, which surfaces as 401
"Missing bearer" errors against `api.openai.com`.

## Configuration

Global config lives at `<state root>/config.toml` (so by default
`~/.taskrunner/config.toml`). Every key has a working default; the file
only exists to override them. The keys, shown with their defaults (the
per-worker `harness`/`model`/`provider` keys are covered in the next
section):

```toml
[task]
turn_timeout_seconds = 1800   # per-turn wall-clock limit

[worker.codex]                # built-in; [worker.claude] is analogous
image = "taskrunner/codex-worker"
auth_volume = "taskrunner-codex-home"
allowed_domains = ["api.openai.com", "auth.openai.com", "chatgpt.com", "*.chatgpt.com"]

[egress]
proxy_image = "taskrunner/egress-proxy"

[ingest]                      # host transcript archival (see below)
interval_seconds = 300        # how often the daemon sweeps transcripts

[ingest.sources.claude-code]  # built-in; [ingest.sources.codex] is analogous
format = "claude-code"        # selects the parser
dirs = ["~/.claude/projects"] # scanned recursively for *.jsonl transcripts
```

Any other `[worker.<name>]` section defines a new worker — see below.
Any other `[ingest.sources.<name>]` section adds a transcript source; it
needs a `format` naming a built-in parser (`claude-code`, `codex`) and the
host `dirs` to scan. Unknown keys are rejected rather than ignored, so a
typo fails the config instead of silently archiving nothing.

A source is always host directories. Worker transcripts living inside
Docker auth volumes are not configured here — they are derived from each
worker's own `auth_volume` and `image`, which is what keeps the volume, the
image used to reach into it, and the per-harness transcript path from ever
disagreeing. Copies land under `~/.taskrunner/ingest-staging`, created
owner-only because an auth volume also holds that worker's credentials.

## Network access

A worker's default egress is its `allowed_domains` — normally just its own
vendor's API, so a task cannot fetch web pages or install packages out of
the box. To grant more, the delegating agent passes `allowDomains` on
`assign-task`; any value makes the task "networked", which requires your
explicit yes in conversation (relayed as `userApproved: true` and recorded
in the audit log). The value `"*"` grants the entire public internet.

Whatever the allowlist says, the proxy resolves every destination itself
and refuses loopback, LAN, and other special-use addresses — an approved
(or DNS-rebinding) domain can never reach your local network. Deliberate
local destinations must be pinned explicitly, either as an IP-literal entry
(`127.0.0.1:8080`) or a Docker host name (`host.docker.internal:11434`).
Entries without a port — including `"*"` — cover only ports 80 and 443.
Every connection attempt, allowed or refused, lands in the audit trail.

## Custom and local-model workers

Workers are pluggable: any `[worker.<name>]` config section becomes a
worker, with `harness` selecting the loop that drives it. A local model
needs no login and no internet — the container's only route is the egress
proxy, which forwards exactly one port to the model server on your machine:

```toml
[worker.qwen]
harness = "codex"
provider = "ollama"                # or "lmstudio"
model = "qwen2.5-coder:32b"
allowed_domains = ["host.docker.internal:11434"]
```

Install [Ollama](https://ollama.com) on the host, `ollama pull` the model,
and `assign-task` with `worker: "qwen"`. Trying another model is another
config section; the audit trail records which worker (and so which model)
produced every turn.
