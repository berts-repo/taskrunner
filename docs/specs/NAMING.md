# Naming

User-facing names require explicit user approval before they land in implementation
or docs.

## Naming Rule

Any name exposed to users, agents, config files, logs, exports, or MCP clients must
be reviewed before implementation.

## Names Requiring Approval

- Product/project name
- MCP tool names
- Command names
- Config keys
- Project-local directory name
- Database concepts that appear in docs, logs, or exported records
- Session/task/run/job/thread terminology
- Worker/backend/provider/agent terminology
- Client/integration/configured capability terminology
- Artifact/rule/memory terminology
- Risk tier names

## Approved Terms

- Product/project name: Taskrunner
- Project-local directory: `.taskrunner/`
- Delegated work unit: task
- Per-task interaction: turn
- Durable client interaction record: session
- Execution runtime: worker
- Worker integration code: worker harness
- Per-task isolated git copy (worktree on host, clone in Docker): task
  workspace
- Stored output: artifact
- Optional client or worker setup unit: integration
- Enabled client or worker support: configured capability
- Reusable provider-neutral prompt or skill unit: instruction
- Direct reusable prompt text: prompt
- Reusable behavior or workflow guidance: skill
- Folder containing instruction metadata and Markdown content: instruction package
- Instruction package metadata file: `instruction.toml`
- Instruction package Markdown content file: `body.md`
- Code that reads, validates, and snapshots instruction packages: instruction loader
- Point-in-time database copy used for audit/history: instruction snapshot
- Database-backed instruction management layer: instruction registry
- Core MCP tool names:
  - `assign-task`
  - `continue-task`
  - `lookup-task`
  - `cancel-task`
- MCP tool argument casing: camelCase (`taskId`, `turnId`, `allowDomains`,
  `userApproved`) — approved 2026-07-16. Arguments are JSON keys written by
  agents, so they follow JSON convention; kebab-case stays on tool names and
  CLI commands/flags, snake_case stays in TOML config keys and stored record
  fields
- Global state root: `~/.taskrunner/`
- Global state root layout:
  - Append-only event log (source of truth): `events.jsonl`
  - Derived SQLite index (rebuildable): `index.db`
  - Content-addressed artifact store: `artifacts/`
  - Task workspaces: `workspaces/<task_id>/`
  - Ephemeral process state (`daemon.sock`, `daemon.pid`, `daemon.lock`):
    `runtime/`
  - Daemon operational logs: `logs/`
  - Global TOML config: `config.toml`
- CLI commands: `taskrunner up`, `taskrunner down`, `taskrunner status`,
  `taskrunner mcp`
- Privileged-approval CLI commands: `taskrunner approve <task_id>`,
  `taskrunner deny <task_id>`
- Risk tier names: `read-only`, `workspace-write`, `networked`, `privileged`
- Egress-filtering component: egress proxy
- Egress config section: `[egress]`
- Per-worker egress allowlist key: `allowed_domains`
- Task workspace git branch: `taskrunner/<task_id>`
- Record ID shape: prefixed lowercase ULIDs (`task_…`, `turn_…`, `sess_…`,
  `art_…`, `evt_…`, `proj_…`)
- Initial global config keys: `[daemon]`, `[task] turn_timeout_seconds`,
  `[worker.codex] command`
- Planning-docs directory: `docs/specs/`
- State root override flag: `--state-root <dir>`
- State root override env var: `TASKRUNNER_STATE_ROOT`
- Turn statuses (in tool output, index, exports): `running`, `completed`,
  `failed`, `canceled`; failure cause is carried separately in `error_code`
  (e.g. `worker_failed`), never baked into the status word
- Task status before its first turn: `created`
- Artifact kinds: `worker-events`, `diff`
- Audit event kinds: `tool.<tool-name>`, `worker.<event-kind>`,
  `worker.unparsed_output`. The segment after `worker.` is worker-native
  vocabulary quoted verbatim, not a Taskrunner-owned name
- Phase 2 batch (approved 2026-07-16; `-worker` explicitly chosen over
  `-runner` to keep `runner` for the internal placement layer):
  - Docker image names: `taskrunner/codex-worker`, `taskrunner/claude-worker`,
    `taskrunner/egress-proxy`
  - Worker auth volume names: `taskrunner-codex-home`, `taskrunner-claude-home`
  - Per-turn Docker resource name prefixes: network
    `taskrunner-egress-<turn_id>`, containers `taskrunner-proxy-<turn_id>`
    and `taskrunner-worker-<turn_id>`
  - Config keys under `[worker.<name>]`: `runtime` (`"docker"` | `"host"`),
    `image`, `auth_volume`; worker section `[worker.claude]`; egress key
    `[egress] proxy_image`
  - `assign-task` tool arguments: `allowDomains`, `userApproved`, `runtime`
  - Second worker name: `claude`
  - Audit event kinds for egress decisions: `egress.allowed`, `egress.refused`
  - Approval state values (in tool output and index): `none`, `pending`,
    `approved`, `denied`
  - Approval record ID prefix: `appr_`
  - Approval origin values (`via` on approval records): `agent`, `human`
  - npm script for building the Docker images: `build:images`

## Candidate Terms

### Product or Project

- Local Agent Broker
- Agent Relay
- Task Broker
- Universal Orchestrator

### Delegated Work Unit

- run
- job
- thread

### Execution Backend

- backend
- runner
- agent
- provider
- connector
- driver
- adapter
- bridge
- harness

### Optional Setup Unit

- connector
- plugin
- provider

### Shared Prompt and Skill Unit

- prompt package
- skill package
- instruction catalog

### Project-Local Directory

- `.orchestrator/`
- `.agent-broker/`
- `.uo/`
- `.omagion/`
- `.looking-glass/`
- `.glass/`

## Retired Or Avoided Terms

- Product/project names:
  - Looking Glass
  - Local Agent Broker
  - Universal Orchestrator
- Integration-code terms:
  - adapter
- Cancellation tool name:
  - `stop-task`
- Tool naming shapes:
  - `task.start`, `task.continue`, `task.lookup`
  - `dispatch.start`, `dispatch.continue`, `dispatch.lookup`
  - underscore-separated tool names such as `assign_task`

## Notes

- Tool names use hyphens because MCP permits hyphens and they read better in
  user-facing tool lists.
- `task` is the durable user-facing delegated work object.
- `session` is the durable record for normal participating client interaction.
- `job` remains available for internal queue/execution semantics if needed.
- `integration` describes optional setup for clients or workers such as Codex,
  Claude, and Gemini. `configured capability` describes the same idea from a
  policy/runtime perspective.
