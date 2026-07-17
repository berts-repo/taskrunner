# Taskrunner Plan

Taskrunner is a local-first MCP delegation broker: MCP clients (Claude Code,
Codex, and anything else that speaks MCP) hand coding tasks to configured
workers, which run in isolated Docker containers over task-local git clones
behind a filtering egress proxy, with every observable step recorded in a
durable audit trail that the same clients can query back.

The product is feature-complete (decided 2026-07-16). Remaining work is
additive configuration — a new worker is a config entry plus a worker
Dockerfile — not new subsystems. § Cut scope records what was deliberately
dropped so it is not reopened opportunistically.

## Product Shape

- Local personal MCP server; TypeScript/Node.js runtime.
- Append-only JSONL event log as the durable write path, with SQLite as the
  derived, rebuildable index.
- Single local Taskrunner daemon shared by all participating clients through
  thin stdio shims; the daemon starts on demand.
- Project directory as a first-class scope key.
- Explicit delegation to configured workers; project-scoped task records and
  multi-turn continuation.
- Task-specific isolated git workspaces (task-local clones) as the code-state
  boundary; Docker containers as the only worker isolation model.
- Artifact capture (worker event streams, diffs) with content-addressed
  storage.
- Global TOML config at `~/.taskrunner/config.toml`; secrets via environment
  variables.

## Delegation Model

- Delegation is explicit. Taskrunner never silently routes ordinary client
  work to a worker.
- Taskrunner owns the canonical task/turn graph. Worker-native session IDs
  are stored for continuation and debugging, but users and agents refer to
  Taskrunner task handles.
- One worker owns a task at a time; turns within a task run serially;
  different tasks may run concurrently when isolated.
- Sessions record the MCP connection boundary: each connected client gets a
  durable session ID that tasks, approvals, and audit events link back to.
- Context reaches delegated workers only through the prompt; there is no
  automatic injection into worker prompts. Lookups are compact by default
  and expand only when the caller asks for detail.

## Worker Harness

**Workers are pluggable — this is a core design principle, not a feature.**
A worker is a config entry: `[worker.<name>]` with a `harness` key naming the
loop that drives it, plus whatever `model`/`provider`/`image`/
`allowed_domains` it needs. The broker (scheduler, storage, audit, tiers,
runners, workspaces) never special-cases worker names; only the harness
registry knows which harness kinds exist. The acceptance bar for any change
touching workers: a new worker can be brought to life purely from config,
with full audit/approval/egress parity. Example — a local model with no
internet:

```toml
[worker.qwen]
harness = "codex"
provider = "ollama"
model = "qwen2.5-coder:32b"
allowed_domains = ["host.docker.internal:11434"]
```

All workers sit behind a shared worker harness interface, which supports:

- Starting a task turn.
- Resuming a task turn by worker-native session ID.
- Streaming structured events.
- Capturing the final response.
- Capturing process exit status and structured errors.
- Capturing worker-native session IDs.
- Detecting changed files from worker events or git fallback.
- Running behind the Docker runtime boundary.

Shipped harness control surfaces:

- codex: `codex exec --json` / `codex exec resume --json` (JSONL events with
  `thread_id` and file-change items).
- claude: `claude --print --output-format stream-json --verbose`, plus
  `--resume <session_id>`; changed files are derived from tool_use edits
  with the workspace git fallback covering shell-command edits.

## Workspace And Isolation

Delegated coding workers run in isolated workspaces.

- Each delegated coding task gets a task-specific isolated git workspace: a
  task-local clone (`git clone --no-hardlinks` from the host repository).
  A clone rather than a worktree because a worktree's `.git` file points back
  into the main repository: mounting only the worktree breaks git inside the
  container, and mounting the main repository's `.git` writable would let a
  worker plant hooks or config that execute on the host. `--no-hardlinks`
  matters: a default local clone hardlinks object files, which share inodes
  with the host repository — a worker writing through one could corrupt
  host history.
- Task clones are fully self-contained and disposable. Completed work
  lands by fetching the task branch from the clone on the host side, followed
  by review; the worker never writes to the real repository.
- The same workspace is reused across turns for the same task.
- Multi-agent fanout should be modeled as parent and child tasks.

Container posture:

- Workers always run in Docker containers; there is no host-run mode.
- Containers are execution sandboxes, not the system of record. The event
  log, index, and artifact store stay outside worker containers.
- Containers run as a non-root user.
- Workspace mounts are narrow: only the task-local clone directory, never
  the main repository or its `.git`.
- Coding containers have no direct network route. Egress enforcement uses an
  egress proxy: worker containers sit on an internal Docker network with no
  outside route; a proxy container spans the internal and external networks
  and forwards only connections to domains on that worker's
  `allowed_domains` list (plus per-task approved additions). The proxy
  resolves allowlisted names itself and refuses special-use destination
  addresses, so DNS rebinding cannot reach loopback or the LAN. Refused
  attempts are logged as audit events. Cloud workers default to their own
  API domains; a local-model worker defaults to only the local model port on
  the host, keeping it internet-free.

Taskrunner itself may run narrow host operations needed for orchestration,
storage, policy checks, git workspace setup (task clones), task-branch fetch
from completed clones, artifact copy-out, worker lifecycle, and cleanup.
Arbitrary delegated edits and broad shell commands belong in workers.

## Credentials

Workers receive no credentials by default except credentials explicitly
configured for that worker to operate.

Allowed by explicit configuration:

- Narrow codex auth volume, mounted at `~/.codex` during turns.
- Narrow claude auth volume, mounted at `~/.claude` and `~/.claude.json`
  via volume subpaths during turns.

Worker auth volumes hold a separate worker login (e.g. `codex login` run once
into the volume), never a mount of the host's own auth file: host and
container refreshing the same token invalidates the account
(`refresh_token_reused`), and a worker compromise must not expose host
credentials.

Not inherited automatically: SSH keys, GitHub tokens, cloud credentials,
package publish tokens, host `.env` files, shell profiles, browser sessions,
broad host home directories.

## MCP Tool Surface

Core tools:

- `assign-task`
- `continue-task`
- `lookup-task`
- `cancel-task`

Tool responses prefer compact summaries with handles to expandable details
and artifacts, rendered as compact readable text rather than raw JSON blobs,
because the consumer is a model.

History presentation rules:

- History lookups present prompt/response pairs as ordered exchanges by
  default, never loose audit rows the caller must correlate.
- A trace view replays delegated work end-to-end: inputs (the prompt),
  observable worker activity (reasoning messages, commands, file reads and
  edits), and outputs (response, diff, status).
- Trace and history expansion accept a scope: a single turn or the last N
  exchanges.

Every completed delegated turn carries: final answer, status, worker-native
session ID, changed files, diff when available, log/artifact references, and
error details when applicable. Raw worker events are stored as artifacts so
richer result contracts can be built later without losing audit fidelity.

## MCP tool schemas

Tool argument names use camelCase (`taskId`, `allowDomains`): they are JSON
keys written by agents and read by code, so they follow JSON convention.
Kebab-case stays on things typed in a shell (tool names, CLI commands and
flags); snake_case stays in TOML config keys and stored record fields.

`assign-task` request:

- `project`: absolute project path.
- `worker`: configured worker capability such as `codex`.
- `prompt`: the delegated instruction text for the first turn.
- `wait`: optional; block until the turn completes instead of returning
  immediately.
- `allowDomains`: optional extra outbound domains beyond the worker's API
  defaults; makes the task `networked`.
- `userApproved`: optional; set only after the user explicitly approved the
  extra network access in-conversation. The approval record shows it was
  relayed by the calling agent.
- `metadata`: optional caller identity and correlation data.

`assign-task` response: `task_id`, `turn_id`, `status`, `worker`,
worker-native session ID, summary/final answer, changed files, artifacts,
and error details, rendered as compact text. Without `wait` the response
returns immediately with a running status; `lookup-task` retrieves results.

`continue-task` request: `taskId`, `prompt`, `wait`, `metadata`. Response:
same shape as `assign-task` with the current `task_id` and new `turn_id`.
Returns `conflict` while a turn is already running.

`lookup-task` request:

- `taskId`, or `project` to list that project's tasks.
- `include`: optional expansions — `turns`, `artifacts`, `audit`, `diff`,
  `trace`.
- `scope`: optional `{ turnId }` or `{ last: N }` narrowing for expansions.
- `limit`: optional cap when listing tasks.

`lookup-task` response: compact task or task-list summaries by default;
expansion blocks only for requested `include` fields; artifact refs returned
as handles, not inline payloads.

`cancel-task` request: `taskId`, optional `reason` recorded in the audit
trail. Response: `task_id`, the `turn_id` that was running (when one was),
and the resulting status.

Artifact handle shape: `artifact_id`, `kind`, `label`, `media_type`,
`size_bytes`, `sha256`, `locator`.

Error codes: `invalid_request`, `not_found`, `not_configured`,
`approval_required`, `policy_denied`, `capture_unavailable`,
`worker_unavailable`, `worker_failed`, `conflict`, `internal_error`.

## Database schema

SQLite tables, all derived from the event log:

- `projects`: canonical project records keyed by normalized project root.
- `project_aliases`: additional observed paths resolving to the same project.
- `sessions`: MCP client connection records.
- `tasks`: delegated work records, linked to project and originating session,
  carrying status, tier, allowed domains, and approval state.
- `turns`: per-task delegated interactions with prompt, response, status,
  error fields, and changed files.
- `worker_sessions`: worker-native session identifiers per task.
- `approvals`: recorded approval/denial decisions and how they arrived.
- `audit_events`: append-only observable events across sessions and tasks.
- `artifacts`: stored blobs with hashes, sizes, and media types, immutable
  once stored.
- `artifact_links`: joins between artifacts and sessions, tasks, turns, or
  audit events.

Implementation rules:

- Opaque stable IDs for every top-level record (prefixed lowercase ULIDs).
- Foreign-key integrity on.
- `audit_events` and `turns` are append-oriented; corrections are new
  events, not destructive edits.

## Storage write path

- Every durable record is appended to the JSONL event log first; SQLite is a
  derived index rebuilt from the log at any time.
- Delete-and-rebuild of the index is the universal recovery path, so the
  fold must be deterministic and idempotent.
- Streamed worker events are appended to the audit log as they arrive during
  a turn, not buffered until turn completion; only bulky artifacts wait for
  end-of-turn copy-out. A crashed turn keeps its partial audit trail and is
  recorded as failed.
- Legacy event shapes from the removed host-run flow still parse, so logs
  written before the cut keep refolding cleanly.

## Risk tiers and default policy

Risk tiers:

- `read-only`: lookup and other non-mutating operations.
- `workspace-write`: isolated delegated edits inside the task workspace with
  no network.
- `networked`: delegated work that needs outbound network or package
  install.

Approval defaults:

- `read-only` runs without extra approval.
- `workspace-write` requires explicit delegation but no extra step beyond
  the delegation action itself.
- `networked` requires task-level approval, given in-conversation: the
  calling agent relays the user's yes (`userApproved`), and the approval
  record shows it came through that agent.

Global hard limits:

- No silent delegation of ordinary client work.
- No broad host home, shell profile, or secret inheritance into workers.
- No concurrent writers in the same task workspace.
- Network and package installation start disabled.
- Taskrunner host operations stay limited to orchestration, storage, policy,
  git/workspace management, artifact copy-out, worker lifecycle, and
  cleanup.

## Process model

- One local Taskrunner daemon per workstation owns the event log, index,
  locks, policy checks, and worker lifecycle.
- Clients connect through thin shims: a stdio MCP shim per client process
  that proxies to the daemon over a unix socket.
- The daemon starts on demand (first shim connection or `taskrunner up`);
  no manually managed service.
- Single-writer guarantees are enforced by construction: only the daemon
  writes durable state.

## Async delegation contract

- `assign-task` and `continue-task` return immediately by default with a
  running status; `wait` blocks for short tasks.
- Every turn has a configurable timeout (`[task] turn_timeout_seconds`) that
  terminates the worker and records `worker_failed` with partial audit
  retained.
- A running turn can be canceled with `cancel-task`; cancellation records a
  canceled status and preserves the audit trail and workspace state.
- A `continue-task` call against a task with a running turn returns
  `conflict`.

## Naming Governance

User-facing names require explicit user approval before they land in
implementation or docs. `NAMING.md` is the register of approved and
retired terms.

## Cut scope

Recorded 2026-07-16 so these are not reopened opportunistically:

- Knowledge/memory layer, in both its original and slim forms: task
  summaries, memory files, text search, LLM extraction. The user runs a
  personal assistant hub (Hermes/OpenClaw) as the daily knowledge layer; it
  queries Taskrunner's audit trail through `lookup-task`, which already
  covers failure investigation and later lookup. A memory layer here would
  be a second knowledge garden. Taskrunner never pushes memory into other
  systems and never maintains its own knowledge garden.
- Cross-workstation sync, wrapper-command client capture, instruction
  packages, retention/redaction policy, research worker containers as a
  distinct feature, and vector search. The retained audit trail means any
  of these can be revisited later without rework, but none is planned.
- Host-run worker mode and its `privileged` human-approval flow: shipped
  early, removed as dead weight in pure-Docker use; legacy events still
  parse.

## Supporting Documents

- `README.md`: project overview and setup.
- `NAMING.md`: approved and retired naming register.
