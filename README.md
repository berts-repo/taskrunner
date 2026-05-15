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
- Use task-specific git worktrees and worker isolation for delegated coding.
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
- Database: SQLite.
- Execution model: single local Taskrunner process.
- Durable state: hybrid global database plus project-local `.taskrunner/` state.
- Client capture: portability-first hybrid model.
  - Wrapper commands are the baseline.
  - Native hooks/plugins are enhanced integrations where reliable.
  - Log/session import is a recovery path.
- Delegation: explicit, multi-turn, project-scoped tasks.
- Worker isolation: Docker containers by default, task-specific git worktrees,
  network disabled unless approved.
- Secrets: environment variables and explicitly configured worker credentials,
  without broad host secret inheritance.
- Configuration: TOML, with global defaults and project overrides.

## Project Documents

- [PLAN.md](./PLAN.md): holistic product and architecture plan.
- [NAMING.md](./NAMING.md): approved and candidate naming register.
- [BACKEND_SPIKE.md](./BACKEND_SPIKE.md): Codex and Claude worker spike results.

## Status

This repository is in planning and design cleanup. The core build-facing
decisions now live in `PLAN.md`; the next step is implementing the
`.taskrunner/` layout, SQLite schema, MCP tool contracts, and initial Codex
worker loop.
