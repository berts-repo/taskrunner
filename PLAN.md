# Taskrunner MCP Server Plan

## Goal
Design an MCP server that can run inside Claude Code, Codex, and later other MCP-compatible clients, with:
- Delegated local worker execution via installed coding agents such as Claude Code and Codex
- Basic auditing and artifact capture
- Project-scoped session continuity

The v1 scope is intentionally narrower than the full long-term platform: prove
local coding-agent delegation first, then add richer orchestration, isolation,
memory, and policy features in v2.

## Interview Status
- Mode: requirements interview
- Decision style: one question at a time

## Agreed Scope Direction

### V1 alpha product shape
- Build a local MCP delegation broker for coding agents.
- Use Codex as the first worker, based on the current backend spike.
- Use TypeScript/Node.js and SQLite.
- Keep project directory as a first-class scope key.
- Support project-scoped delegated task records and continuation.
- Store basic audit records and artifacts for delegated work.
- Keep the initial MCP tool surface small.
- Use a hybrid runtime approach: design the worker runtime interface for future
  Docker/worktree isolation, but implement the first v1 alpha loop with the
  simplest local Codex execution path.

### V1 alpha includes
- MCP server.
- One local coding-agent worker: Codex.
- SQLite session/audit store.
- Project-scoped task continuation.
- Initial MCP tools:
  - `assign-task`
  - `continue-task`
  - `lookup-task`
- Basic artifacts:
  - final summary
  - status
  - worker-native session ID
  - changed files
  - logs
  - patch/diff where available

### Deferred from v1
- Optional research/provider workers.
- Broad provider abstraction.
- Semantic/vector memory.
- Multiple specialized lookup tools.
- Complex retention tiers.
- Research containers.
- Full Docker/worktree isolation, though the v1 alpha interfaces should leave a
  clean path to add it next.

### V2 candidate additions
- Second coding-agent worker.
- Structured artifact/result contract shared across workers.
- Docker/worktree isolation:
  - per-session git worktree
  - worker container
  - reused worktree/container across turns
  - network disabled by default
  - explicit escalation for package install or network access
- Structured project memory:
  - decisions
  - facts
  - open tasks
  - prior session summaries
- Retention and redaction policies:
  - max days
  - max storage
  - tiered retention by record type
  - global hard limits
  - project overrides
- Project-local control directory.
- Expanded lookup tools:
  - task lookup expansion
  - `memory.lookup`
  - `tasks.lookup`
  - `rules.lookup`
- Optional narrow research/provider feature:
  - fetch/research task
  - summary with cited excerpts
  - artifact handoff to coding worker

### V2 priority order
1. Second worker
2. Structured artifact contract
3. Docker/worktree isolation
4. Project memory
5. Retention/redaction policy
6. Project control directory
7. Expanded lookup tools
8. Optional research provider

## Backend Spike

Decision: run a short spike comparing Claude Code and Codex before choosing the v1
worker.

Initial result: Codex passed the start/resume spike and is the current
recommendation for the v1 worker if implementation starts immediately. Claude
Code has the right CLI surface. Host login is not visible inside the Codex
sandbox, but login-based Claude auth was proven viable inside a non-root Docker
worker with a persistent `/home/worker` volume. Claude's remaining file-edit and
resume tests are blocked by account usage availability, not container auth.

The spike should test each backend for:
- Starting a delegated task non-interactively.
- Resuming the same delegated task.
- Capturing final output.
- Capturing or reconstructing the worker-native session ID.
- Detecting changed files.
- Capturing errors and exit status.
- Understanding approval behavior.
- Understanding whether the interface is stable enough for v1.

The output of the spike should be a recommendation for:
- Which worker should ship first in v1.
- Which worker should be added second in v2.
- What worker harness interface is actually needed.
- What task/artifact fields are required for the first schema.

## Naming Governance

The user wants to be directly involved in naming. Any user-facing name requires
explicit approval before it lands in implementation or docs.

This includes:
- Product/project name.
- MCP tool names.
- Command names.
- Config keys.
- Project-local directory name.
- Database concepts that appear in docs, logs, or exported records.
- Session, task, run, job, thread, worker, backend, provider, artifact, rule, memory, and risk-tier terminology.

Before implementation, create a `NAMING.md` file to track:
- Approved terms.
- Candidate terms.
- Retired or banned terms.
- Notes explaining why important names were chosen.

Current approved naming decisions:
- Product/project name: Taskrunner
- Project-local directory: `.taskrunner/`
- Delegated work unit: task
- Per-task interaction: turn
- Execution runtime: worker
- Worker integration code: worker harness
- Stored output: artifact
- Initial MCP tool names: `assign-task`, `continue-task`, `lookup-task`

## Decisions

### 1. Primary usage mode
- Status: pending
- Status: decided
- Question: Who is this mainly for in day-to-day use?
- Options:
  - Manual tool for the user
  - Autonomous tool for agents
  - Both
- Recommendation: Both, unless you want to keep the first version narrow
- Answer: Both

### 2. Deployment scope
- Status: pending
- Status: decided
- Question: Should the first version be personal/local, or built as a multi-user service from the start?
- Options:
  - Local personal server
  - Single-user remote server
  - Multi-user service
- Recommendation: Local personal server first
- Answer: Local personal server, with an option to export the database

### 3. Server role
- Status: pending
- Status: decided
- Question: Is this mainly a toolbox exposed over MCP, or should it also act as an orchestrator that routes work to providers like Gemini?
- Options:
  - MCP toolbox only
  - MCP toolbox plus orchestration
  - Full orchestrator first
- Recommendation: MCP toolbox plus orchestration
- Answer: MCP toolbox plus orchestration

### 4. Provider strategy
- Status: pending
- Status: decided
- Question: Should optional non-coding providers be hardcoded for one backend at first, or abstracted from day one?
- Options:
  - One provider only
  - One provider first, provider abstraction in the interface
  - Multi-provider from day one
- Recommendation: One provider first, provider abstraction in the interface
- Answer: Provider adapters should be abstracted in the interface from day one, even if only one optional provider is added first

### 7. Context minimization strategy
- Status: pending
- Question: How should the server help keep client context windows small while still preserving rules, memory, and traceability?
- Notes:
  - User wants the system to avoid bloating MCP/client context
  - Possible approaches include reference-rule files, retrieval-on-demand, explicit "check memory" commands, and compact handles instead of full transcripts
- Recommendation: Explore this explicitly before locking the storage and tool design
- Answer:

### 8. Project task records
- Status: pending
- Question: How should the server store and expose per-project task records that different agents can reference later?
- Notes:
  - User wants task records scoped to a project directory
  - Different agents should be able to reference prior sessions for that directory
  - This likely overlaps with memory retrieval, audit storage, and context minimization
- Recommendation: Treat project directory as a first-class scope in the data model
- Answer:

### 9. Audit retention policy
- Status: decided
- Question: How should audit retention work by default?
- Options:
  - Keep for X days
  - Keep within a max storage capacity
  - Tiered retention by record type
  - Combined policy
- Recommendation: Combined policy
- Answer: Combined policy with X-day retention, max-capacity cutoff, and tiered retention by record type

### 10. Audit contents
- Status: pending
- Status: decided
- Question: What should be recorded in the audit trail by default?
- Options:
  - Tool calls only
  - Tool calls plus prompts and responses
  - Full operational record including tool calls, prompts, responses, artifacts, costs, latency, and approvals
- Recommendation: Full operational record, with configurable redaction if needed
- Answer: Full operational record

### 11. Memory behavior
- Status: pending
- Status: decided
- Question: What should "memory" mean in the first version from the user's perspective?
- Options:
  - Searchable history only
  - Searchable history plus extracted facts/decisions
  - Searchable history plus extracted facts/decisions/tasks
- Recommendation: Searchable history plus extracted facts and decisions
- Answer: Searchable history plus extracted facts, decisions, and tasks

### 12. Memory scope
- Status: pending
- Status: decided
- Question: How should memory and task records be scoped for retrieval?
- Options:
  - Global only
  - Project-directory first, with optional global lookup
  - Fully separate silos by client
- Recommendation: Project-directory first, with optional global lookup
- Answer: Project-directory first, with optional global lookup

### 13. Context minimization behavior
- Status: pending
- Status: decided
- Question: How should agents and users access rules, memory, and prior sessions without bloating active context?
- Options:
  - Explicit commands only
  - Automatic retrieval based on project and task
  - Hybrid: compact defaults with explicit expansion
- Recommendation: Hybrid
- Answer: Hybrid, with further exploration needed later on the retrieval policy

### 14. Memory access commands
- Status: pending
- Status: decided
- Question: How should users and agents ask the server to inspect memory, sessions, rules, or prior decisions?
- Options:
  - One generic memory lookup command
  - Separate commands for memory, sessions, and rules
  - Mixed model with simple defaults and specialized commands
- Recommendation: Mixed model with simple defaults and specialized commands
- Answer: Mixed model, with tasks included as a first-class retrieval command

### 15. Runtime language
- Status: pending
- Status: decided
- Question: What implementation language should we target for the MCP server?
- Options:
  - TypeScript/Node.js
  - Python
  - Decide based on fit after requirements
- Recommendation: TypeScript/Node.js unless you have a strong Python preference
- Answer: TypeScript/Node.js

### 16. Primary database
- Status: pending
- Status: decided
- Question: What should the main local database be?
- Options:
  - SQLite
  - PostgreSQL
  - Start with SQLite, leave room for PostgreSQL later
- Recommendation: Start with SQLite, leave room for PostgreSQL later
- Answer: SQLite

### 17. Semantic search storage
- Status: deferred
- Question: How should semantic memory search be implemented initially?
- Options:
  - SQLite-based vector approach
  - Separate local vector database
  - Text and structured retrieval first, vector search later
- Recommendation: Prefer SQLite-based local vector support if mature enough; otherwise defer vector search until after MVP
- Answer: Deferred for further exploration during project planning
- Notes:
  - User wants to revisit this once the project plan is more concrete

### 18. Runtime shape
- Status: pending
- Status: decided
- Question: How should the local system run operationally?
- Options:
  - Single local process
  - Dockerized local stack
  - Support both, but optimize for one
- Recommendation: Single local process first
- Answer: Single local process

### 19. Session recording policy
- Status: pending
- Status: decided
- Question: How aggressively should the system create and update project task records and extracted tasks during normal use?
- Options:
  - Manual only
  - Automatic by default
  - Hybrid with automatic lightweight summaries and explicit deeper saves
- Recommendation: Hybrid with automatic lightweight summaries and explicit deeper saves
- Answer: Hybrid

### 20. Sensitive data storage policy
- Status: decided
- Question: How should the system handle sensitive content by default?
- Options:
  - Store everything
  - Store everything but allow redaction rules
  - Minimize by default
- Recommendation: Store everything but allow redaction rules
- Answer: Store everything but allow redaction rules

### 21. Untrusted content policy
- Status: pending
- Status: decided
- Question: How should the system treat instructions or guidance found inside web pages and other retrieved artifacts?
- Options:
  - Treat as normal content
  - Treat as untrusted content that can inform answers but not issue instructions
  - Require explicit approval before retrieved instructions can influence actions
- Recommendation: Treat as untrusted content that can inform answers but not issue instructions
- Answer: Treat as untrusted content that can inform answers but not issue instructions

### 22. Action approval boundary
- Status: pending
- Status: decided
- Question: When memory, rules, or retrieved artifacts suggest an action, how strict should approval be before the system treats that suggestion as operational guidance?
- Options:
  - Use automatically when confidence is high
  - Use for suggestions, but require approval for consequential actions
  - Always require approval before operational use
- Recommendation: Use for suggestions, but require approval for consequential actions
- Answer: Use for suggestions, but require approval for consequential actions

### 23. Security configuration UX
- Status: pending
- Status: decided
- Question: How should users configure security policies like redaction, retention, trust boundaries, and approval behavior?
- Options:
  - Config file only
  - Commands only
  - Config file plus commands
- Recommendation: Config file plus commands
- Answer: Config file first, with commands available for inspection and optional updates
- Notes:
  - User wants security settings to be easy to configure

### 24. Security policy scope
- Status: decided
- Question: Where should security settings live?
- Options:
  - Global only
  - Per-project only
  - Both
- Recommendation: Both
- Answer: Both, with easy editable files and no UI required for the first version

### 25. Config format
- Status: decided
- Question: Which config format should user-editable settings use?
- Options:
  - TOML
  - YAML
  - JSON
- Recommendation: TOML
- Answer: TOML

### 26. Secret handling
- Status: pending
- Status: decided
- Question: How should secrets like API keys be referenced by the system?
- Options:
  - Store directly in config files
  - Environment variables or secret references only
  - Support both, but discourage storing directly in config
- Recommendation: Environment variables or secret references only
- Answer: Environment variables only, including optional local `.env` support

### 27. Delegation topology
- Status: decided
- Question: How should Claude Code and Codex participate in the system?
- Options:
  - Direct peer-to-peer delegation between coding agents
  - Both agents call Taskrunner, which can delegate to installed workers
  - Orchestrator-only internal workers
- Recommendation: Use Taskrunner as the broker
- Answer: Claude Code and Codex should both call Taskrunner, and Taskrunner should be able to delegate to installed workers

### 28. Delegation interaction model
- Status: decided
- Question: Should delegated coding work support follow-up turns?
- Options:
  - Single-shot tasks only
  - Multi-turn delegated tasks
- Recommendation: Multi-turn delegated tasks
- Answer: Multi-turn delegated tasks are required

### 29. Coding worker isolation
- Status: decided
- Question: Where should delegated coding workers run?
- Options:
  - Native host processes
  - Docker containers by default
  - Mixed, depending on task
- Recommendation: Docker containers by default
- Answer: Delegated coding workers should run inside Docker containers by default

### 30. Research execution isolation
- Status: decided
- Question: Where should web search and other fetch-oriented provider execution run?
- Options:
  - In the same coding worker containers
  - On the host
  - In separate research containers
- Recommendation: Separate research containers
- Answer: Web search and research execution should run in separate containers

### 31. Coding workspace model
- Status: decided
- Question: What filesystem view should a delegated coding session operate on?
- Options:
  - The real host checkout
  - A task-specific git worktree
  - A copied snapshot
- Recommendation: A task-specific git worktree
- Answer: Each delegated coding session should use its own git worktree

### 32. Coding session runtime reuse
- Status: decided
- Question: For multi-turn delegated coding, should runtime state be reused?
- Options:
  - New container and workspace every turn
  - Reuse the same worktree and container across turns within a session
  - Hybrid
- Recommendation: Reuse the same worktree and container within a session
- Answer: One container and one worktree should be reused across turns for the same delegated session

### 33. Coding container network default
- Status: decided
- Question: What should the default network policy be for coding-session containers?
- Options:
  - No network by default
  - Domain allowlist by default
  - Open network by default with logging
- Recommendation: No network by default
- Answer: Coding-session containers should have network disabled by default

### 34. Research-to-coding handoff
- Status: decided
- Question: What should a coding worker receive from an optional provider/fetch task by default?
- Options:
  - Summary only
  - Summary plus selected cited excerpts/artifacts
  - Full raw content bundle
- Recommendation: Summary plus selected cited excerpts/artifacts
- Answer: Coding workers should receive a compact summary plus selected cited excerpts/artifacts

### 35. Approval escalation path
- Status: decided
- Question: When a delegated worker needs more capability, who should be asked?
- Options:
  - The calling agent only
  - The human user only
  - The calling agent first, with human approval for higher-risk cases
- Recommendation: Calling agent first, with human approval for higher-risk cases
- Answer: Taskrunner should escalate through the calling agent first, with human approval required for higher-risk expansion

### 36. Risk tier policy model
- Status: decided
- Question: How should approval/risk tiers be configured?
- Options:
  - Predefined tiers only
  - Fully user-defined tiers only
  - Predefined defaults with user overrides in config
- Recommendation: Predefined defaults with user overrides
- Answer: Use predefined default risk tiers with user overrides in TOML configuration

### 37. Policy scope and persistence
- Status: decided
- Question: How should project policies behave?
- Options:
  - Global only
  - Project only
  - Global defaults plus persistent project rules
- Recommendation: Global defaults plus persistent project rules
- Answer: Project rules should persist and be auto-applied for that project, with global defaults also in effect

### 38. Global versus project policy limits
- Status: decided
- Question: Can project policy be looser than global policy?
- Options:
  - Yes, always
  - No, never
  - Only for settings explicitly marked as project-expandable
- Recommendation: Allow only explicitly expandable settings
- Answer: Some global rules are hard limits, while others may be marked as project-expandable

### 39. Project-local control directory
- Status: decided
- Question: Should each project have a hidden local control directory?
- Options:
  - No
  - Yes, local-only
  - Yes, with some commit-worthy files and some local-only files
- Recommendation: Yes, with mixed shared and local contents
- Answer: Each project should have a hidden local control directory named `.taskrunner/`, and some files should be commit-worthy while others stay local-only

### 40. Storage topology
- Status: decided
- Question: How should durable state be split between global and project-local storage?
- Options:
  - Global only
  - Project-local only
  - Hybrid global plus project-local
- Recommendation: Hybrid
- Answer: Use a hybrid model with a global durable database/audit layer plus project-local state

### 41. Project-local portability
- Status: decided
- Question: Should project-local state be portable across machines?
- Options:
  - Yes, fully portable
  - No, machine-specific is fine
  - Mixed portable core plus machine-local runtime state
- Recommendation: Mixed
- Answer: Project-local state should be split into portable core files and machine-local runtime state

### 42. Global retention posture
- Status: decided
- Question: How should the global DB/audit store retain project content by default?
- Options:
  - Full copies by default
  - References/metadata by default
  - Tiered retention with full retention as a configurable trusted default
- Recommendation: Tiered retention
- Answer: Use a tiered retention model, with full retention available as the trusted default but lighter retention configurable per project

### 43. Project config file layout
- Status: decided
- Question: How should project-local configuration files be structured?
- Options:
  - One combined file
  - Separate shared and local files
  - One main shared file plus optional local override file
- Recommendation: One main shared file plus optional local override file
- Answer: Use one main shared project file plus an optional local override file, alongside local runtime/state directories

### 44. V1 alpha runtime priority
- Status: decided
- Question: Should v1 alpha prioritize minimal working delegation, Docker/worktree isolation from day one, or a hybrid?
- Options:
  - Minimal working delegation first
  - Docker/worktree isolation from day one
  - Hybrid interface-first approach
- Recommendation: Hybrid interface-first approach
- Answer: Hybrid. Build interfaces assuming isolation, but implement host-run Codex first behind a worker runtime boundary so Docker/worktree isolation can be added next without changing the MCP tool contract.

### 45. Product/project name
- Status: decided
- Question: What should the product/project be called?
- Options explored:
  - Universal Orchestrator
  - Local Agent Broker
  - Agent Relay
  - Looking Glass
  - Taskrunner
- Recommendation: Taskrunner
- Answer: Taskrunner

### 46. Project-local directory name
- Status: decided
- Question: What should the project-local control directory be named?
- Options:
  - `.taskrunner/`
  - `.tr/`
  - `.taskrunner-local/`
- Recommendation: `.taskrunner/`
- Answer: `.taskrunner/`

### 47. Delegated work terminology
- Status: decided
- Question: What should the core delegated work object and interaction be called?
- Options:
  - task + turn
  - session + turn
  - job + run
- Recommendation: task + turn
- Answer: task + turn

### 48. Worker terminology
- Status: decided
- Question: What should the execution runtime and integration code be called?
- Options explored:
  - worker + adapter
  - worker + connector
  - worker + harness
  - runner + driver
- Recommendation: worker + worker harness
- Answer: worker + worker harness

### 49. Stored output terminology
- Status: decided
- Question: What should stored outputs such as logs, diffs, and summaries be called?
- Options:
  - artifact
  - result
  - record
  - bundle
- Recommendation: artifact
- Answer: artifact

### 50. Initial MCP tool names
- Status: decided
- Question: What should the initial MCP tools be named?
- Options explored:
  - `task.start`, `task.continue`, `task.lookup`
  - `dispatch.start`, `dispatch.continue`, `dispatch.lookup`
  - `assign_task`, `continue_task`, `lookup_task`
  - `assign-task`, `continue-task`, `lookup-task`
- Recommendation: `assign-task`, `continue-task`, `lookup-task`
- Answer: `assign-task`, `continue-task`, `lookup-task`

## Direction Update: Local Worker Delegation

The project direction has been narrowed around delegation and audit.

Taskrunner is now intended to act as a local broker that:
- Exposes MCP tools to Claude Code, Codex, and later other MCP clients
- Receives delegation requests from those clients
- Launches installed coding-agent applications as local workers
- Preserves audit, policy, task continuity, and project-scoped records across those delegated runs

Optional provider workers may still exist later, but they are no longer the primary product framing.

Current preferred architecture:
- Taskrunner runs as a separate local process
- Claude Code and Codex can both call Taskrunner as MCP clients
- Taskrunner can delegate work to installed workers such as `claude` and `codex`
- Multi-turn delegation is a first-class requirement
- Taskrunner owns the canonical task/turn graph, while also storing worker-native
  session IDs for continuation.

## Direction Update: Containerized Workers

The current preferred isolation model is:
- Run delegated coding workers inside Docker containers by default
- Run delegated fetch-oriented provider tasks inside containers as well
- Keep the primary Taskrunner database and long-term audit store outside the worker containers
- Treat containers as execution sandboxes, not as the system of record

Rationale:
- Stronger host isolation for file access, shell commands, and web access
- Cleaner policy boundaries for autonomous runs
- Better reproducibility across workstations
- Safer support for higher-autonomy worker modes

Current bias:
- Per-task worker containers
- Non-root containers
- Narrow workspace mounts, preferably task-specific worktrees
- Restricted or allowlisted network access
- No broad host secret mounts
- Container teardown after task completion, while keeping logs/artifacts outside the container or on tightly scoped volumes

## Open Questions For Interview

These are the main design questions that still need direct answers before the implementation plan is stable.

### B. Codex backend choice
- Should Codex integration start with:
  - `codex exec` / `codex exec resume`
  - `codex app-server`
  - `codex exec` first with an upgrade path to `app-server`
- How much instability from experimental Codex control surfaces is acceptable in v1?

### C. Claude backend choice
- Should Claude integration start with:
  - `claude -p` / `--resume`
  - Claude Agent SDK
  - CLI first with an upgrade path to the SDK
- Do you want Taskrunner to broker Claude approval requests centrally from day one?

### E. Network model
- Should package installation be:
  - Disabled by default
  - Allowlisted by domain
  - User-approvable per task

### F. State placement
- What data must stay outside containers:
  - SQLite database
  - Audit log
  - Session graph
  - Artifact store
  - Extracted memory
- What data may live inside containers temporarily:
  - Worker-native session files
  - Temp outputs
  - Tool caches
- Should worker-native session state be copied out after every turn for durability?

### G. Artifact and result contract
- What should every delegated turn return?
  - Final answer
  - Sources
  - Changed files
  - Patch/diff
  - Shell log
  - Structured status
- Should Taskrunner normalize worker output into one shared schema even if native workers differ?

### H. Security boundaries
- Should Taskrunner itself ever run with direct shell/file powers, or only worker containers?
- Should sensitive host paths be blocked centrally even if a worker would otherwise allow them?
- Should worker containers receive long-lived credentials, short-lived credentials, or no credentials by default?

### I. Session UX
- Should users and agents see delegated work as:
  - A single abstract Taskrunner task
  - Separate worker-native sessions linked from a Taskrunner task
  - Both
- Should agents be able to fork delegated tasks and compare branches of work?

### J. Scope control for v1
- Is v1 primarily:
  - Cross-agent coding delegation
  - Cross-agent coding delegation plus optional provider tasks
  - A more general local worker orchestration platform
- Which matters more for the first implementation:
  - Security and isolation
  - Rich multi-turn interaction
  - Minimal implementation complexity

## Current Summary
- Local personal MCP server
- MCP toolbox plus orchestration
- Local worker delegation to installed coding agents is now a first-class design goal
- Optional provider workers may be supported, but they are no longer the primary product surface
- Product/project name is Taskrunner
- Preferred topology is Claude Code and Codex as MCP clients, with Taskrunner brokering delegated worker runs
- Multi-turn delegated tasks are required
- Delegated coding workers run in Docker containers by default
- Fetch-oriented provider tasks run in separate containers
- Each delegated coding session gets its own git worktree
- Each delegated coding session reuses the same worktree and container across turns
- Coding-session containers have network disabled by default
- Optional provider output is handed to coding workers as summary plus selected cited artifacts
- Approval escalation is agent-first with human approval for higher-risk expansion
- Risk tiers use predefined defaults with TOML-based overrides
- Project rules persist and are auto-applied for that project
- Global policy provides ceilings, with some settings allowed to be project-expandable
- Each project gets a hidden local control directory named `.taskrunner/`
- Durable state uses a hybrid global-plus-project-local model
- Project-local state is split between portable core files and machine-local runtime state
- Full operational audit with time, capacity, and tiered retention
- Searchable memory with extracted facts, decisions, and tasks
- Project-directory-first retrieval with optional global lookup
- Project-scoped task records for cross-agent continuity
- Hybrid context minimization and hybrid session recording
- Mixed retrieval command model with first-class task lookup
- TypeScript/Node.js runtime
- SQLite primary database
- Single local process
- Delegated workers and web research execution are currently biased toward Docker-based isolation
- Database and durable audit/state should remain outside worker containers
- Vector search strategy deferred for deeper planning
- Security settings: global plus project overrides, config-file-first, TOML
- Secrets via environment variables, including optional local `.env`
- Retrieved web/artifact content treated as untrusted informational content
- Consequential actions require approval even when memory or retrieved content suggests them
