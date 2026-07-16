# Taskrunner Holistic Plan

## Goal

Taskrunner is a local-first MCP server for durable sessions, prompt/response
audit, project memory, artifact capture, and delegated worker execution across
Claude Code, Codex, Gemini, and other MCP-compatible clients.

The plan describes the target system as one coherent architecture. Work can be
implemented incrementally, but the design should stay oriented around the full
portable workstation experience rather than disconnected scope slices.

## Product Shape

- Local personal MCP server with database export support.
- MCP toolbox plus orchestration layer.
- TypeScript/Node.js runtime.
- Append-only JSONL event log as the durable write path, with SQLite as the
  derived, rebuildable index.
- Single local Taskrunner daemon shared by all participating clients through
  thin connection shims.
- Project directory as a first-class scope key.
- Durable sessions for participating clients.
- Always-on audit for every prompt and response Taskrunner can observe.
- Explicit delegation to configured workers.
- Project-scoped task records and continuation.
- Lightweight automatic memory extraction from the audit stream.
- Artifact capture for normal sessions and delegated work.
- Task-specific isolated git workspaces as the code-state boundary for
  delegated coding: worktrees for host-run workers, task-local clones for
  Docker workers.
- Docker containers as the preferred worker isolation model.
- Global defaults plus persistent project policy.
- TOML configuration.
- Secrets via environment variables, including optional local `.env` support.
- Portable setup on any workstation, with clients and workers enabled as
  optional configured capabilities.

## User Experience

From the user's perspective:

- Install Taskrunner on a workstation.
- Enable whichever clients and workers are available on that machine.
- Launch participating clients through portable wrapper commands when native
  capture is not available.
- Keep normal client behavior native unless Taskrunner delegation, lookup,
  memory, or audit workflows are invoked.
- Ask the active client to delegate work to a configured worker.
- Continue delegated tasks across turns.
- Look up prior task records, summaries, artifacts, and decisions for the same
  project.
- Move between workstations without requiring every client or worker integration
  to be installed everywhere.

## Client Capture Model

Taskrunner uses a portability-first hybrid capture model.

Required baseline:

- Wrapper commands such as `taskrunner codex`, `taskrunner claude`, and
  `taskrunner gemini`, acting as thin shims that record launch metadata and a
  durable session boundary.
- Full PTY transcript capture is out of the baseline: parsing raw terminal
  output from TUI clients is high effort and low fidelity. It stays a possible
  later enhancement only if a concrete need proves it out.
- Structured prompt/response capture comes from native hooks and from delegated
  work, where Taskrunner observes everything directly as the broker.

Enhanced capture:

- Native hooks/plugins where a client supports them cleanly.
- MCP prompt/response boundary calls where clients support them.

Recovery capture:

- Native log/session import when available and stable enough.

Manual MCP calls:

- Used for explicit workflows such as delegation, lookup, memory, and audit
  commands.
- Not the primary mechanism for always-on audit.

## Sessions And Audit

Every participating client interaction belongs to a durable Taskrunner session
when Taskrunner can observe it.

Audit records should include:

- Observed user prompts.
- Observed client and worker responses.
- Delegation requests.
- Tool calls and worker events where available.
- Approvals.
- Errors.
- File changes.
- Artifact references.
- Memory writes.
- Lookups.
- Timestamps.
- Project, session, task, turn, client, and worker links.

Audit is automatic for observable activity. Coverage depends on the capture path
available for each client.

## Delegation Model

Taskrunner is the broker between clients and workers.

- Claude Code, Codex, Gemini, and other clients call Taskrunner.
- Taskrunner delegates to configured workers such as Codex, Claude Code, Gemini,
  or provider/research workers.
- Delegation is explicit. Taskrunner should not silently route ordinary client
  work to another worker.
- Multi-turn delegated tasks are required.
- Taskrunner owns the canonical task/turn graph.
- Worker-native session IDs are stored for continuation, audit, and debugging.
- Users and agents should normally refer to Taskrunner task handles, while
  worker-native IDs remain available in detailed output.

## Worker Harness

All workers should fit behind a shared worker harness interface.

The harness should support:

- Starting a task turn.
- Resuming a task turn by worker-native session ID.
- Streaming structured events where available.
- Capturing the final response.
- Capturing process exit status and structured errors.
- Capturing worker-native session IDs.
- Detecting changed files from worker events or git fallback.
- Capturing logs and raw worker events.
- Exporting enough worker-native session state after each turn for durable
  continuation.
- Running behind a runtime boundary that supports Docker execution and a host
  execution fallback when necessary.

Codex is the current worker starting point because the backend spike proved `codex exec` and
`codex exec resume` can start, resume, emit JSONL events, expose a `thread_id`,
and report file changes.

The Codex worker control surface is:

- `codex exec`
- `codex exec resume`

Claude Code is a strong additional worker candidate once Docker file-edit and resume
tests are completed.

## Workspace And Isolation

Delegated coding workers should run in isolated workspaces.

- Each delegated coding task gets a task-specific isolated git workspace.
- The workspace mechanism depends on the runtime boundary:
  - Host-run workers use a task-specific git worktree.
  - Docker workers use a task-local clone (`git clone --no-hardlinks` from
    the host repository), because a worktree's `.git` file points back into
    the main repository. Mounting only the worktree breaks git inside the
    container, and mounting the main repository's `.git` writable would let a
    worker plant hooks or config that execute on the host. `--no-hardlinks`
    matters: a default local clone hardlinks object files, which share inodes
    with the host repository — a worker writing through one could corrupt
    host history.
- Docker task clones are fully self-contained and disposable. Completed work
  lands by fetching the task branch from the clone on the host side, followed
  by review; the worker never writes to the real repository.
- The same workspace is reused across turns for the same task.
- One worker owns a task at a time.
- Turns within the same task run serially.
- Different tasks may run concurrently when isolated.
- Multi-agent fanout should be modeled as parent and child tasks.

Container posture:

- Coding workers run in Docker containers by default.
- Research/fetch workers run in separate containers.
- Containers are execution sandboxes, not the system of record.
- Taskrunner database, audit log, artifact store, session graph, and extracted
  memory stay outside worker containers.
- Containers should run as non-root where practical.
- Workspace mounts should be narrow: only the task-local clone directory, and
  never the main repository or its `.git`.
- Broad host secret mounts are avoided.
- Coding containers have network disabled by default.
- Package installation and network access require task-level approval.
- Egress enforcement uses an egress proxy: worker containers sit on an
  internal Docker network with no outside route; a proxy container spans the
  internal and external networks and forwards only connections to domains on
  that worker's `allowed_domains` list (plus per-task approved additions).
  Refused attempts are logged as audit events. Cloud workers default to their
  own API domains; a local-model worker defaults to only the local model port
  on the host, keeping it internet-free.

Taskrunner itself may run narrow host operations needed for orchestration,
storage, policy checks, git workspace setup (worktrees and task clones),
task-branch fetch from completed clones, artifact/session-state copy-out,
worker lifecycle, and cleanup. Arbitrary delegated edits and broad shell commands
belong in workers.

## Credentials

Workers receive no credentials by default except credentials explicitly
configured for that worker to operate.

Allowed by explicit configuration:

- Narrow Codex auth volume.
- Narrow Claude auth volume.
- Other worker-specific auth material.

Worker auth volumes hold a separate worker login (e.g. `codex login` run once
into the volume), never a mount of the host's own auth file: host and
container refreshing the same token invalidates the account
(`refresh_token_reused`), and a worker compromise must not expose host
credentials.

Not inherited automatically:

- SSH keys.
- GitHub tokens.
- Cloud credentials.
- Package publish tokens.
- Host `.env` files.
- Shell profiles.
- Browser sessions.
- Broad host home directories.

## Storage Model

Durable state uses a hybrid global plus project-local model.

Global durable state:

- SQLite database.
- Audit records.
- Session graph.
- Task/turn graph.
- Artifact metadata.
- Extracted memory.
- Instruction snapshots.
- Worker-native session references.
- Policy and approval records.

Project-local `.taskrunner/` state:

- Shared project config.
- Instruction package files.
- Portable project metadata.
- Optional local override config.
- Local runtime/state directories for machine-specific data.

Project-local state should split portable core files from machine-local runtime
state so projects can move across workstations cleanly.

## Memory And Context Control

Memory starts as lightweight automatic extraction from the audit stream.

Memory records follow the instruction-package pattern: markdown files under
`.taskrunner/memory/` are the human-readable, human-editable source of truth,
and the database keeps snapshots and indexes over them. Editing the file is
editing the memory. This keeps the knowledge layer legible and directly usable
by external tools such as Obsidian, while the machine-generated audit trail
stays in structured storage.

Memory records include:

- Decisions.
- Facts.
- Follow-up tasks.
- Compact session summaries.
- Delegated task summaries.

Retrieval behavior:

- Project-directory first.
- Optional global lookup.
- Compact by default.
- Expand only when the user, agent, or workflow asks for detail.
- Avoid loading broad history into active client context.

Semantic/vector search is part of the target memory strategy when it improves
retrieval quality, but plain structured/text retrieval should remain available
and predictable.

## Instructions

Shared prompts and skills are provider-neutral instruction packages.

Authoring format:

- Instruction packages live under `.taskrunner/`.
- Each package contains `instruction.toml` metadata and a `body.md` Markdown
  body.
- Filesystem packages are the source of truth for editing, git history, review,
  and sharing.

Audit format:

- Taskrunner snapshots loaded or used instructions into SQLite.
- Snapshots include metadata, body text, source path, content hash, and
  timestamps.
- Sessions, tasks, turns, artifacts, and audit records link to the exact
  instruction snapshot used at the time.
- Provider and worker integrations compile the same snapshot into their native
  prompt/message/tool format.

An instruction registry can provide database-backed browsing and editing while
preserving the filesystem package plus snapshot model.

## MCP Tool Surface

Core tools:

- `assign-task`
- `continue-task`
- `lookup-task`
- `cancel-task`

Specialized tools can be added when workflows need separate contracts:

- memory lookup
- task lookup expansion
- rules lookup
- audit lookup

Tool responses should prefer compact summaries with handles to expandable
details and artifacts, rendered as compact readable text rather than raw JSON
blobs when the consumer is a model.

Delegation is asynchronous by default: `assign-task` and `continue-task`
return immediately with the task handle and a running status, and results are
retrieved through `lookup-task`. An optional wait flag can block for short
tasks. Running turns can be canceled, and every turn has a timeout that
produces a structured failure instead of a hang.

History presentation rules:

- History lookups present prompt/response pairs as ordered exchanges by
  default, never loose audit rows the caller must correlate.
- A trace view replays delegated work end-to-end: inputs (prompt, instruction
  snapshot, injected context and memory), observable worker activity
  (reasoning messages, commands, file reads and edits), and outputs (response,
  diff, status).
- Trace and history requests accept a scope argument: a single turn, the last
  N exchanges, a whole session, or a whole task.

Every delegated turn should return:

- Final answer.
- Status.
- Worker-native session ID.
- Changed files.
- Patch/diff when available.
- Log/artifact references.
- Error details when applicable.

Taskrunner should also store raw worker events/logs so richer normalized result
contracts can be built without losing audit fidelity.

## Artifacts

Artifacts include:

- Prompts and responses.
- Final summaries.
- Status records.
- Worker-native session IDs.
- Changed-file lists.
- Logs.
- Patch/diff output where available.
- Retrieved supporting material.
- Raw worker events.

Artifacts should have retention treatment separate from core session/task/audit
records.

## Policy, Retention, And Redaction

Policy model:

- Global defaults plus persistent project rules.
- Some global rules are hard limits.
- Other global rules may be marked project-expandable.
- Predefined risk tiers with TOML-based user overrides.
- Config-file-first management, with commands available for inspection and
  optional updates.

Approval model:

- Calling agent first.
- Human approval for higher-risk expansion.
- Consequential actions require approval even when memory or retrieved content
  suggests them.

Retention model:

- Combined time and capacity limits.
- Tiered retention by record type.
- Protected core session/task/audit records.
- Large artifacts and logs may expire sooner.
- Full retention available as a trusted default, with lighter retention
  configurable per project.

Sensitive data posture:

- Store observable activity by default.
- Allow redaction rules.
- Retrieved or imported content is untrusted informational content and cannot
  issue operational instructions.

## Installation And Portability

Workstation setup should support Taskrunner core without requiring every client
or worker integration to be present.

Setup should:

- Install Taskrunner core.
- Detect available clients/workers where practical.
- Enable supported integrations as configured capabilities.
- Allow unavailable integrations to be skipped.
- Report clearly when a requested worker is not configured.
- Support adding and removing integrations over time.
- Preserve a portable wrapper path for supported clients.

## Naming Governance

User-facing names require explicit user approval before they land in
implementation or docs.

This includes:

- Product/project name.
- MCP tool names.
- Command names.
- Config keys.
- Project-local directory name.
- Database concepts that appear in docs, logs, or exported records.
- Session, task, run, job, thread, worker, backend, provider, artifact, rule,
  memory, and risk-tier terminology.

`NAMING.md` tracks approved, candidate, retired, and avoided terms.

Current approved naming decisions:

- Product/project name: Taskrunner.
- Project-local directory: `.taskrunner/`.
- Delegated work unit: task.
- Per-task interaction: turn.
- Durable client interaction record: session.
- Execution runtime: worker.
- Worker integration code: worker harness.
- Per-task isolated git copy (worktree on host, clone in Docker): task
  workspace.
- Stored output: artifact.
- Optional setup unit: integration.
- Enabled support unit: configured capability.
- Shared prompt/skill unit: instruction.
- Core MCP tool names: `assign-task`, `continue-task`, `lookup-task`,
  `cancel-task`.

## Build-Spec Decisions

These decisions close the main planning gaps and define the default shape to
build against first. They can evolve later, but implementation should treat
them as the current baseline rather than reopen them opportunistically.

### 1. Project-local file layout

Portable shared paths under `.taskrunner/`:

- `project.toml`: project identity and shared defaults.
- `instructions/`: instruction packages, each with `instruction.toml` and
  `body.md`.
- `memory/`: extracted memory records as markdown files, the editable source of
  truth indexed and snapshotted by the database.
- `policy/`: shared project policy overlays and allowlists.
- `imports/`: optional checked-in imported reference material intended to move
  with the project.
- `sessions/`: git-backed portable project session history in a compact,
  append-friendly format.
- `.gitignore`: ignores machine-local runtime state.

Machine-local paths under `.taskrunner/local/`:

- `config.toml`: workstation-local overrides such as enabled capabilities.
- `cache/`: derived lookup caches and temporary import products.
- `logs/`: local operational logs not intended for sync.
- `locks/`: process and task locks.
- `runtime/`: sockets, PID files, and other ephemeral process state.
- `worker-sessions/`: copied worker-native continuation state when it is stored
  project-locally instead of in the global state root.
- `stash/`: stashed local history fragments kept during drift recovery.

Git posture:

- Commit portable files in `.taskrunner/` except machine-local runtime state.
- Ignore `.taskrunner/local/` wholesale by default.
- Treat `.taskrunner/sessions/` as portable project history that normally moves
  with project pushes and pulls.
- Do not place task workspaces (worktrees or task clones) inside
  `.taskrunner/`; Taskrunner may reference them from local runtime state, but
  workspace directories themselves should live in a Taskrunner-managed local
  runtime root outside the project tree.

Portability rule:

- Files under `.taskrunner/` outside `local/` are the portable project contract.
- Files under `.taskrunner/local/` are per-machine and disposable.
- Session continuity is part of portable project state. If a project moves to a
  VM or another workstation, its compact session history should move with it.
- Portable session history should include compact session/task/turn records and
  related summaries or references, not every large log or raw worker event by
  default.

Sync and recovery rule:

- Session portability uses explicit git push/pull rather than background sync.
- Taskrunner should assume one active writer per session at a time.
- If portable session history is stale or divergent, Taskrunner should detect
  drift and prefer stash-and-rebuild recovery over clever merging.
- Delete-and-rebuild is an allowed fast-path recovery option for disposable
  local derived state.

### 2. Database schema

Initial durable tables:

- `projects`: canonical project records keyed by normalized project root.
- `project_aliases`: additional observed paths that resolve to the same project.
- `sessions`: durable participating-client sessions.
- `tasks`: delegated work records, optionally linked to an originating session.
- `turns`: per-task delegated interactions.
- `worker_sessions`: worker-native session identifiers and copied continuation
  state metadata.
- `audit_events`: append-only observable events across sessions and tasks.
- `artifacts`: stored blobs or file references with hashes, sizes, and media
  types.
- `artifact_links`: joins between artifacts and sessions, tasks, turns, or
  audit events.
- `memory_records`: extracted decisions, facts, follow-ups, and summaries.
- `instruction_packages`: logical filesystem instruction identities.
- `instruction_snapshots`: point-in-time bodies and metadata used by sessions,
  tasks, or turns.
- `approval_records`: user, agent, or policy approvals and denials.
- `policy_sets`: global and project policy documents plus effective hashes.
- `capabilities`: enabled client and worker capabilities for a workstation.

Core relationships:

- A `project` has many `sessions`, `tasks`, `memory_records`, and project-level
  `policy_sets`.
- A `session` may create many `tasks` and many `audit_events`.
- A `task` belongs to one `project`, may reference one originating `session`,
  has many `turns`, and may have many `worker_sessions` over time.
- A `turn` belongs to one `task`, may reference one active `worker_session`,
  and has many `audit_events`, `artifacts`, `approval_records`, and
  `instruction_snapshots`.
- `audit_events` are the common event spine and may link to a `session`, `task`,
  `turn`, `artifact`, `approval_record`, or `memory_record`.
- `artifacts` are immutable once stored; relationships live in
  `artifact_links`.
- `instruction_snapshots` are immutable and referenced by the exact session,
  task, or turn that used them.

Implementation rules:

- Use opaque stable IDs for every top-level record.
- Keep foreign-key integrity on by default.
- Treat `audit_events` and `turns` as append-oriented records; corrections
  should be new events, not destructive edits.

### 3. MCP tool schemas

Tool argument names use camelCase (`taskId`, `allowDomains`): they are JSON
keys written by agents and read by code, so they follow JSON convention.
Kebab-case stays on things typed in a shell (tool names, CLI commands and
flags); snake_case stays in TOML config keys and stored record fields.

`assign-task` request:

- `project`: absolute project path or known project handle.
- `worker`: requested worker capability such as `codex`.
- `prompt`: the delegated instruction text for the first turn.
- `instructions`: optional instruction package refs or inline snapshots.
- `context`: optional task/session/artifact refs to preload.
- `policy`: optional requested expansions such as network or package install.
- `metadata`: optional caller identity and correlation data.
- `wait`: optional; block until the turn completes instead of returning
  immediately.

`assign-task` response:

- `task_id`
- `turn_id`
- `status`
- `worker`
- `worker_session_id`
- `summary`: compact final answer or current blocked state.
- `changed_files`
- `artifacts`
- `error`

Without `wait`, the response returns immediately with a running status and the
result fields populate as the turn progresses; `lookup-task` retrieves them.
With `wait`, the response carries the completed turn.

`continue-task` request:

- `taskId`
- `prompt`
- `instructions`
- `context`
- `policy`
- `metadata`

`continue-task` response:

- Same shape as `assign-task`, with the current `task_id` and new `turn_id`.

`lookup-task` request:

- One of `taskId`, `sessionId`, or a constrained project-scoped query.
- Optional `include` fields such as `turns`, `artifacts`, `audit`,
  `approvals`, `memory`, `diff`, or `trace`.
- Optional `scope` for history and trace expansion: a single `turnId`, the
  last N exchanges, a whole session, or a whole task.
- Optional pagination controls for expanded results.

`lookup-task` response:

- Compact task or task-list summaries by default.
- Conversation history returned as paired prompt/response exchanges in turn
  order.
- `trace` expansions replay each in-scope turn end-to-end: inputs, observable
  worker activity, and outputs, per the history presentation rules.
- Expansion blocks only for requested `include` fields.
- Artifact refs returned as handles, not inline large payloads.

`cancel-task` request:

- `taskId`
- `reason`: optional caller-provided note recorded in the audit trail.

`cancel-task` response:

- `task_id`
- `turn_id`: the turn that was running, when one was.
- `status`: canceled, or the task's current status when nothing was running.

Artifact handle shape:

- `artifact_id`
- `kind`
- `label`
- `media_type`
- `size_bytes`
- `sha256`
- `locator`

Error codes:

- `invalid_request`
- `not_found`
- `not_configured`
- `approval_required`
- `policy_denied`
- `capture_unavailable`
- `worker_unavailable`
- `worker_failed`
- `conflict`
- `internal_error`

### 4. Risk tiers and default policy

Default risk tiers:

- `read-only`: lookup, audit, memory, and other non-mutating operations.
- `workspace-write`: isolated delegated edits inside the task workspace with no
  network.
- `networked`: delegated work that needs outbound network or package install.
- `privileged`: host-level operations, broad mounts, or exceptional credential
  exposure.

Approval defaults:

- `read-only` may run without extra approval when the caller is already inside a
  participating session.
- `workspace-write` requires explicit delegation but not an extra step beyond
  the delegation action itself.
- `networked` requires task-level approval.
- `privileged` always requires human approval.

How approval is given (mixed by risk):

- `networked`: approved in-conversation; the calling agent relays the user's
  yes to Taskrunner, and the approval record shows it came through that agent.
- `privileged`: approved only by the human running `taskrunner approve
  <task_id>` (or `taskrunner deny <task_id>`) directly; agent-relayed
  approval is not accepted for this tier.
- Host-run (non-Docker) worker execution is classified `privileged`.

Global hard limits:

- No silent delegation of ordinary client work.
- No broad host home, shell profile, or secret inheritance into workers.
- No concurrent writers in the same task workspace.
- Network and package installation start disabled.
- Taskrunner host operations stay limited to orchestration, storage, policy,
  git/workspace management, artifact/session-state copy-out, worker lifecycle,
  and cleanup.

Project-expandable settings:

- Allowed workers and client integrations.
- Default instruction packages.
- Project-specific redaction additions.
- Retention reductions or tighter storage caps.
- Preapproved network domains or package registries, if global policy allows
  that class of expansion.

Default redaction coverage:

- Authorization and bearer headers.
- API keys and access tokens.
- Cookie and session-token values.
- SSH private keys.
- Common `.env` secret values.
- Worker auth material copied into logs or transcripts.

### 5. Retention defaults and export format

Default retention:

- Core records are retained indefinitely by default:
  projects, sessions, tasks, turns, approvals, policy sets, instruction
  snapshots, memory records, and compact audit metadata.
- Expirable large artifacts are retained for 90 days by default:
  raw worker event streams, verbose logs, imported supporting material, and
  large patch bundles. Reasoning, command, and file-change events are extracted
  from raw streams into protected audit records, so stream expiry does not
  hollow out trace views.
- Project-local machine caches under `.taskrunner/local/` are best-effort and
  may be deleted at any time.

Protected records:

- Sessions, tasks, turns, approvals, instruction snapshots, memory records, and
  the minimal audit/event records required to reconstruct history are protected
  from automatic cleanup.
- Prompt/response bodies and worker reasoning events are protected so trace
  views stay complete for the life of the record; only bulky raw event streams
  and verbose logs are expirable.

Capacity policy:

- Apply storage caps only to expirable artifact classes.
- Evict oldest expirable artifacts first; never evict protected records to
  satisfy a cap.

Export formats:

- Full SQLite export for lossless local backup.
- JSONL export for sessions, tasks, turns, audit events, approvals, policies,
  and memory records.
- Artifact directory export with a manifest file.

Artifact references in exports:

- Structured records reference artifacts by `artifact_id`, content hash, and
  relative export-manifest path.
- Exports should remain readable even when large artifacts are omitted by
  policy, with omission recorded explicitly in the manifest.

### 6. Client-specific capture behavior

Wrapper capture baseline for supported clients:

- Launch command, arguments, timestamp, cwd, detected project root, and exit
  status.
- No raw terminal transcript capture in the baseline; wrappers stay thin
  session-boundary shims per the client capture model.
- Explicit Taskrunner tool calls and delegation requests.
- A durable Taskrunner session boundary even when structured prompt/response
  parsing is unavailable.

Current client expectations:

- Codex:
  wrapper capture is available now; delegated worker execution has strong
  structured capture via `codex exec --json` and `codex exec resume --json`.
- Claude Code:
  wrapper capture is available now; delegated worker capture should use
  `--output-format json` or `stream-json` once authenticated Docker retests are
  stable.
- Gemini:
  wrapper capture is the baseline target; native capture and import behavior are
  not yet assumed reliable.

Native or import capture posture:

- Use native hooks/plugins only where they provide cleaner, stable structured
  prompt/response capture than wrappers.
- Use log/session import only as recovery capture, not as the primary always-on
  path.

Visible warning for uncaptured work:

- When Taskrunner can provide tools but cannot capture the active session,
  surface: `Session not under Taskrunner capture; delegation and lookup remain
  available, but prompt/response audit for this interaction will be incomplete.`

### 7. Claude worker strategy

Initial strategy:

- Claude support should start CLI first, with an SDK upgrade path rather than an
  SDK-first design.
- Taskrunner should remain the central policy and approval broker; Claude-native
  prompts for expanded permissions should be translated into Taskrunner approval
  events where possible instead of becoming a separate policy system.

Docker auth posture:

- Use a narrow Claude auth volume mounted into the worker home.
- Run as a non-root user.
- Avoid host home mounts, Docker socket mounts, and privileged containers.
- Treat login persistence as viable based on the Docker auth spike.

Resume and event-stream posture:

- Keep Claude as the additional worker candidate after Codex, not the worker
  starting point.
- Assume resume and file-edit/event-stream support are provisional until the
  authenticated Docker retest confirms successful start, resume, and structured
  event output during edits.
- If Claude CLI proves stable, keep the harness contract aligned with Codex:
  start turn, resume turn, capture worker-native session ID, stream events,
  capture final response, and detect changed files.

### 8. Process model

- One local Taskrunner daemon per workstation owns the database, event log,
  locks, policy checks, and worker lifecycle.
- Clients connect through thin shims: a stdio MCP shim per client process that
  proxies to the daemon, or a local streamable HTTP endpoint where the client
  supports it.
- The daemon starting on demand (first shim connection or `taskrunner up`) is
  preferred over requiring a manually managed service.
- Single-writer guarantees are enforced by construction: only the daemon writes
  durable state.

### 9. Storage write path

- Every durable record is appended to the JSONL event log first; SQLite is a
  derived index rebuilt from the log at any time.
- Delete-and-rebuild of the index is the universal recovery path.
- This keeps the future cross-workstation sync design (see
  `SYNC_PROPOSAL.md`) an additive feature rather than a storage migration.
- Streamed worker events are appended to the audit log as they arrive during a
  turn, not buffered until turn completion; only bulky artifacts and
  worker-native session state wait for end-of-turn copy-out. A crashed turn
  keeps its partial audit trail and is recorded as failed.

### 10. Async delegation contract

- `assign-task` and `continue-task` return immediately by default with a
  running status; `wait` blocks for short tasks.
- Every turn has a configurable timeout that terminates the worker and records
  `worker_failed` with partial audit retained.
- A running turn can be canceled with `cancel-task`; cancellation records a
  canceled status and preserves the audit trail and workspace state.
- A `continue-task` call against a task with a running turn returns `conflict`.

### 11. Implementation phases

Build order treats the delegation broker as the product core and adds layers
around it:

- Phase 1, usable broker: daemon, the four core MCP tools with the async
  contract, Codex worker on the host in a task worktree, JSONL log plus SQLite
  index,
  paired-exchange lookup, and trace view. No Docker, no wrappers, no memory
  extraction.
- Phase 2, safety: Docker workers with task clones, egress proxy with
  per-worker `allowed_domains`, enforced risk tiers and approvals (mixed by
  risk: agent-relayed for `networked`, human `taskrunner approve` for
  `privileged`), Claude worker after its authenticated retest. Host-run mode
  is retained behind config as `privileged`. Stretch goal: local-model
  worker (internet-free, local model port only). Sequencing starts with the
  Claude authenticated Docker retest.
- Phase 3, knowledge: memory extraction, markdown memory files, compact
  summaries, Obsidian-compatible memory views.
- Phase 4, reach: encrypted state-remote sync, wrapper shims, Gemini,
  research workers, optional vector search.

## Supporting Documents

- `README.md`: short project overview.
- `NAMING.md`: approved and candidate naming register.
- `BACKEND_SPIKE.md`: Codex and Claude worker spike results.
