# Taskrunner MCP Server

Taskrunner is a local-first session, audit, memory, and delegation layer for
Claude Code, Codex, Gemini, and other MCP-compatible clients.

It is designed to act as:

- An MCP toolbox for agents and users.
- A durable session and audit layer for participating clients.
- A local delegation broker for configured workers.
- A project-scoped memory and artifact store.

## Goals

- Keep durable sessions for participating clients.
- Audit every prompt and response Taskrunner can observe.
- Delegate work to configured coding-agent workers.
- Keep active client context small through compact project-scoped records.
- Maintain a complete audit trail for sessions, delegation, worker activity,
  approvals, errors, artifacts, and file changes.
- Use task-specific isolated git workspaces (worktrees on host, task-local
  clones in Docker) and worker isolation for delegated coding.
- Remain portable across workstations with optional client and worker
  integrations.

## Core Concepts

- **Session**: durable record of participating client interaction.
- **Task**: delegated unit of work.
- **Turn**: one interaction within a task.
- **Worker**: execution runtime such as Codex or Claude Code.
- **Worker harness**: integration code that starts, resumes, and records worker
  activity.
- **Artifact**: stored output such as logs, diffs, summaries, prompts,
  responses, and raw worker events.
- **Instruction**: reusable provider-neutral prompt or skill package.

## Architecture Summary

- Runtime: TypeScript / Node.js.
- Storage: append-only JSONL event log as the write path, SQLite as the
  derived, rebuildable index.
- Execution model: single local Taskrunner daemon shared by all clients
  through thin connection shims.
- Durable state: hybrid global database plus project-local `.taskrunner/` state.
- Client capture: portability-first hybrid model.
  - Wrapper commands are the baseline.
  - Native hooks/plugins are enhanced integrations where reliable.
  - Log/session import is a recovery path.
- Delegation: explicit, multi-turn, project-scoped tasks.
- Worker isolation: Docker containers by default, task-specific git workspaces
  (worktrees on host, task-local clones in Docker), network disabled unless
  approved.
- Secrets: environment variables and explicitly configured worker credentials,
  without broad host secret inheritance.
- Configuration: TOML, with global defaults and project overrides.

## Project Documents

- [PLAN.md](./docs/specs/PLAN.md): holistic product and architecture plan.
- [NAMING.md](./docs/specs/NAMING.md): approved and candidate naming register.
- [BACKEND_SPIKE.md](./docs/specs/BACKEND_SPIKE.md): Codex and Claude worker
  spike results.
- [SYNC_PROPOSAL.md](./docs/specs/SYNC_PROPOSAL.md): cross-workstation sync
  proposal (not adopted).

## Status

Phase 1 (the usable broker) is implemented: the on-demand daemon, the stdio
MCP shim with auto-start, the four core MCP tools with the async delegation
contract, a host-run Codex worker in per-task worktrees, the JSONL event log
with a rebuildable SQLite index, and trace-capable lookup. Phase 2 (safety)
adds Docker workers in task-local clones behind a filtering egress proxy,
risk tiers, and the approval flow. Later phases (memory, sync) are specified
in `docs/specs/PLAN.md`.

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
# code. Claude spreads login state across the home directory, so its volume
# mounts at the whole home.
docker run -it --rm -v taskrunner-claude-home:/home/worker \
  taskrunner/claude-worker claude /login
```

Open the URL each flow prints, approve, and the credentials land in the
volume. Repeat only when a worker's login expires or a new machine needs
setting up. The mount paths must match what the daemon uses (narrow
`~/.codex` for codex, whole home for claude): logging in at the wrong path
buries the credentials where the worker can't see them, which surfaces as
401 "Missing bearer" errors against `api.openai.com`.

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

## Quick Start

```sh
npm install
npm run build
claude mcp add --scope user taskrunner -- node /path/to/taskrunner/dist/cli.js mcp
```

The daemon starts on demand and keeps durable state under `~/.taskrunner/`.
CLI: `taskrunner up | down | status | mcp` (`--state-root <dir>` overrides the
state root).

Tests: `npm test`. Full end-to-end check against the built CLI:
`npx tsx scripts/verify-e2e.ts`. Live Codex delegation check (requires
`codex login`): `TASKRUNNER_LIVE_CODEX=1 npx vitest run tests/workers/integration.test.ts`.
