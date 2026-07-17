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

## Project Documents

- [PLAN.md](./docs/specs/PLAN.md): system design and decision record.
- [NAMING.md](./docs/specs/NAMING.md): approved and retired naming register.

## Status

The product is feature-complete: the on-demand daemon, the stdio MCP shim
with auto-start, the four core MCP tools with the async delegation contract,
the JSONL event log with a rebuildable SQLite index, trace-capable lookup,
and Docker workers (codex and claude) in task-local clones behind a
filtering egress proxy. New workers land as config entries via the pluggable
`harness` mechanism. Cut scope is recorded in `docs/specs/PLAN.md` § Cut
scope.

## Quick Start

```sh
npm install
npm run build
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
