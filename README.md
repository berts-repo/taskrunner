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

## Quick Start

Requires Node 22+ and Docker.

```sh
npm install
npm run build
npm run build:images   # worker + egress proxy images
claude mcp add --scope user taskrunner -- node /path/to/taskrunner/dist/cli.js mcp
```

The daemon starts on demand and keeps durable state under `~/.taskrunner/`.
CLI: `taskrunner up | down | status | mcp` (`--state-root <dir>` overrides the
state root).

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
```

Any other `[worker.<name>]` section defines a new worker — see below.

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
