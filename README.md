# Taskrunner MCP Server

Local-first session, audit, memory, and delegation layer for Claude Code, Codex,
Gemini, and later MCP-compatible clients.

The initial release is intentionally focused, but it should include the
foundational pieces that would be expensive to retrofit later: local coding-agent
delegation, durable sessions, git worktree isolation, full prompt/response audit,
artifact capture, and project-scoped state.

It is designed to act as both:
- An MCP toolbox that exposes useful tools to agents and users
- A local session and audit layer for participating clients
- A local delegation broker for configured workers

## Goals

- Keep durable sessions for participating clients
- Audit every prompt and response Taskrunner can observe
- Delegate work to one configured coding-agent worker first
- Keep active client context small through project-scoped task records
- Maintain a complete audit trail for sessions, delegation, and worker activity
- Store and retrieve core artifacts safely
- Keep the first implementation small while avoiding throwaway architecture

## Core Features

### Sessions and Audit

Taskrunner should keep sessions working across normal client use and explicit
delegation workflows.

The target audit model is always-on for participating clients:

- Every observed user prompt
- Every observed client or worker response
- Delegation requests
- Tool calls and worker events where available
- Artifacts, approvals, errors, and file changes
- Project, session, task, and worker links

Delegation is explicit, but audit is automatic. Normal client behavior should
stay native unless the user invokes Taskrunner delegation, lookup, memory, or
audit workflows.

### Delegation

The system is designed to let clients such as Claude Code, Codex, and Gemini:

- Call Taskrunner as a shared control plane
- Delegate work to configured workers
- Continue multi-turn delegated tasks
- Exchange artifacts, summaries, and structured results

The initial release should support one local worker first, currently Codex based
on the backend spike. Workstation setup should not assume every possible worker
is installed or authenticated. Codex, Claude, Gemini, and later workers should be
configured capabilities that can be enabled, skipped, or added later.

Additional coding-agent or research backends can be added after the first worker
path is stable.

### Isolation

Delegated execution should use the same isolation model from the initial release
where practical:

- Coding workers run in Docker containers
- Each delegated coding task gets its own git worktree
- The same worktree and container are reused across turns within a task
- Coding-session containers have network disabled by default
- Research or fetch-oriented tasks can be added later after the core coding
  delegation loop is working

### Memory

The initial release will maintain project-scoped sessions and task records that
include:

- Session summary
- Delegated task summary
- Worker and worker-native session ID
- Status
- Changed files
- Logs and artifacts
- Timestamps

Structured project memory should start as lightweight automatic extraction from
the audit stream: facts, decisions, and follow-up tasks attached to
project-scoped sessions and records. Retrieval should stay compact and controlled
so memory does not constantly bloat active client context.

### Shared Instructions

Taskrunner should support provider-neutral shared prompts and skills through
instruction packages.

The initial model is:

- Author instructions as project-local files under `.taskrunner/`.
- Store each instruction package as `instruction.toml` metadata plus a `body.md`
  Markdown body.
- Treat the filesystem package as the source of truth for editing, git history,
  review, and project sharing.
- Snapshot loaded or used instructions into SQLite with metadata, body text,
  source path, content hash, and timestamps.
- Link sessions, tasks, turns, artifacts, and audit records to the exact
  instruction snapshot used at the time.
- Let provider and worker integrations compile the same snapshot into their
  native prompt or message format.

A later release should add an instruction registry with database-backed editing
and browsing. The first implementation should still store instruction snapshots
in SQLite so that later registry work builds on the same audit trail instead of
replacing it.

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

Separate `memory.lookup`, `rules.lookup`, and `tasks.lookup` tools can wait until
those concepts need independent user-facing workflows.

### Context Control

The system is intended to reduce context-window pressure in MCP clients by:

- Returning compact summaries by default
- Expanding only when explicitly requested
- Using project-aware retrieval instead of loading broad history
- Exploring reference-rule files and retrieval-on-demand

### Retention

Retention should start simple and configurable:

- Keep records for X days
- Enforce a maximum storage capacity
- Protect core session/task/audit records while allowing large artifacts and logs
  to expire sooner

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

### Installation and Integrations

Workstation setup should support Taskrunner core without requiring every client
or worker integration to be present.

Initial setup should be able to:

- Install Taskrunner core
- Enable the first supported worker
- Skip unavailable workers
- Report clearly when a requested worker is not configured
- Leave room to add Codex, Claude, Gemini, or other integrations later

## Runtime and Storage

- Runtime: TypeScript / Node.js
- Primary database: SQLite
- Execution model: single local Taskrunner process
- Database export support: required
- Durable state: hybrid global database plus project-local state

Vector or semantic search support may still be useful later, but it is not part
of the initial product axis.

## Project Status

This repository is currently in the planning phase.

See [PLAN.md](./PLAN.md) for the interview-driven requirements and design decisions collected so far.
