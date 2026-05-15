# Taskrunner MCP Server

Local-first MCP server for Claude Code, Codex, and later other MCP-compatible clients.

The first version is intentionally narrow: prove local coding-agent delegation, task
continuity, basic audit, and artifact capture before expanding into a broader
orchestration platform.

It is designed to act as both:
- An MCP toolbox that exposes useful tools to agents and users
- A local delegation and audit broker for installed workers

## Goals

- Delegate work to one installed coding-agent worker first
- Keep active client context small through project-scoped task records
- Maintain a basic audit trail for delegated work
- Store and retrieve core artifacts safely
- Leave room for stronger security boundaries and multi-agent support in v2

## Core Features

### Delegation

The system is designed to let MCP clients such as Claude Code and Codex:

- Call Taskrunner as a shared control plane
- Delegate work to installed workers
- Continue multi-turn delegated tasks
- Exchange artifacts, summaries, and structured results

The current v1 direction is to support one local worker first, chosen from:

- Claude Code
- Codex

The second coding-agent backend is planned for v2.

### Isolation

Delegated execution is intended to move toward stronger isolation. The planned v2
model is:

- Coding workers run in Docker containers
- Each delegated coding task gets its own git worktree
- The same worktree and container are reused across turns within a task
- Coding-session containers have network disabled by default
- Research or fetch-oriented tasks are deferred until after the core coding
  delegation loop is working

### Memory

Rich memory is deferred until v2. The v1 server will maintain project-scoped
task records that include:

- Delegated task summary
- Worker and worker-native session ID
- Status
- Changed files
- Logs and artifacts
- Timestamps

V2 may add structured project memory for decisions, facts, and open tasks.

### Task Records

The server will maintain project-scoped task records so different agents can reference prior work for the same project directory.

Task recording is planned as a hybrid model:

- Lightweight automatic summaries and records by default
- Explicit deeper saves when requested

### Lookup Commands

Initial tool surface:

- `assign-task`
- `continue-task`
- `lookup-task`

V2 may add separate `memory.lookup`, `rules.lookup`, and `tasks.lookup` tools once
those concepts exist as real stored data.

### Context Control

The system is intended to reduce context-window pressure in MCP clients by:

- Returning compact summaries by default
- Expanding only when explicitly requested
- Using project-aware retrieval instead of loading broad history
- Exploring reference-rule files and retrieval-on-demand

### Audit

The v1 server will keep a basic operational audit trail, including:

- Delegation requests
- Tool calls
- Prompts
- Responses
- Artifacts
- Approvals
- Project and task links

V2 should expand this into a full audit and retention policy:

- Keep records for X days
- Enforce a maximum storage capacity
- Apply tiered retention by record type

### Artifacts

The system should store delegated-task artifacts such as:

- Result summaries
- Patches and diffs
- Changed-file lists
- Logs
- Retrieved supporting material

Artifacts should have separate retention treatment from core history and session summaries.

### Naming

User-facing names require explicit user approval before implementation. This
includes MCP tool names, command names, config keys, project-local directory names,
database concepts that appear in logs or docs, risk tier names, and terminology for
tasks, sessions, workers, artifacts, rules, and memory.

A dedicated naming document should track approved terms and retired terms before
implementation begins.

### Security

Security is a first-class part of the design.

Planned security properties:

- Store everything by default, but allow redaction rules
- Global security defaults with optional per-project overrides
- Config-file-first policy management
- `TOML` configuration
- Secrets stored via environment variables, including optional local `.env` support
- Retrieved or imported content treated as untrusted informational content
- Consequential actions require approval

## Runtime and Storage

- Runtime: TypeScript / Node.js
- Primary database: SQLite
- Execution model: single local Taskrunner process
- Database export support: required
- Durable state: hybrid global database plus project-local state

Vector or semantic search support may still be useful later, but it is not part of
the v1 product axis.

## Project Status

This repository is currently in the planning phase.

See [PLAN.md](./PLAN.md) for the interview-driven requirements and design decisions collected so far.
