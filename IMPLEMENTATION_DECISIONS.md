# Implementation Decisions

This file tracks build-facing decisions for Taskrunner. It is the reference file
to use when implementation starts.

## Settled Baseline

- Product name: Taskrunner
- Initial shape: local personal MCP server
- Product spine: always-on sessions, always-on audit, explicit delegation, and
  controlled automatic memory.
- Primary interaction model: normal client behavior stays native, while
  participating clients route observable prompt/response activity into
  Taskrunner sessions and audit. Human users may explicitly ask their active CLI
  client to delegate to Codex, Gemini, Claude, or another worker; clients use the
  MCP toolkit as the consistent cross-client protocol.
- Runtime: TypeScript / Node.js
- Database: SQLite
- First worker: Codex
- Initial Codex control surface: `codex exec` / `codex exec resume`
- Initial MCP tools: `assign-task`, `continue-task`, `lookup-task`
- Project-local control directory: `.taskrunner/`
- Delegated work object: task
- Per-task interaction: turn
- Execution runtime: worker
- Worker integration code: worker harness
- Stored output: artifact
- Coding workspace model: task-specific git worktree
- Multi-turn model: reuse the same task worktree across turns
- Durable state model: hybrid global database plus project-local state
- Config format: TOML
- Secret handling: environment variables, with optional local `.env` support
- Integration model: clients/workers are configured capabilities. Workstation
  setup must allow Taskrunner core without requiring Codex, Claude, or Gemini to
  all be installed, authenticated, or enabled.

## Project-Wide Implementation Principles

### Always-On Sessions

- Status: decided
- Decision: Every participating client interaction should belong to a durable
  Taskrunner session, including normal client work and explicit delegated work.
- Implementation guidance:
  - Sessions should continue across turns automatically.
  - Sessions should be project-scoped when a project directory is known.
  - Sessions should link normal foreground client activity, Taskrunner tool use,
    delegated tasks, worker-native session IDs, artifacts, and memory extraction.
  - Low-level identifiers should be stable and available for lookup/audit, but
    should not dominate the normal human workflow.

### Always-On Audit

- Status: decided
- Decision: Taskrunner's audit goal is every prompt and every response for all
  participating clients, not only delegated tasks.
- Implementation guidance:
  - Audit should record observed prompts, responses, tool calls, worker events,
    artifacts, approvals, file changes, memory writes, lookups, errors,
    timestamps, project links, session links, task links, and client/worker
    identity.
  - Audit is automatic for everything Taskrunner can observe.
  - Full coverage requires client launch wrappers, client hooks/plugins, MCP
    integration, terminal capture, native log import, or another capture path.
  - The first implementation may start with the reliable capture surfaces
    available, but the product requirement remains complete prompt/response
    audit for participating clients.

### Explicit Delegation

- Status: decided
- Decision: Taskrunner should not silently route normal work to other workers.
  Delegation happens when the user or active client explicitly invokes a
  Taskrunner command, skill, prompt pattern, or MCP tool.
- Implementation guidance:
  - Normal Codex, Claude, or Gemini behavior should stay native unless
    Taskrunner is invoked.
  - Human-facing behavior should support requests such as "delegate this to
    Codex", "ask Claude to review this", or "look up what happened with that
    task".
  - MCP tools are the shared protocol behind those workflows, not necessarily the
    literal commands humans type.

### Controlled Automatic Memory

- Status: decided
- Decision: Memory should be extracted lightly and automatically from the audit
  stream, while retrieval/injection should be controlled to avoid context bloat.
- Implementation guidance:
  - Automatically extract decisions, facts, follow-up tasks, and compact session
    summaries where practical.
  - Prefer compact lookup results by default.
  - Expand memory or audit details only when the user, agent, or workflow asks
    for them.

### Optional Integrations

- Status: decided
- Decision: Taskrunner core must not assume Codex, Claude, and Gemini are all
  present on a workstation.
- Implementation guidance:
  - Setup should allow Taskrunner core only.
  - Workers/clients should be enabled as configured capabilities.
  - The first implementation may ship one working worker integration, currently
    Codex, while representing other integrations as unavailable/not configured.
  - A request for an unavailable worker should fail clearly with configured
    alternatives.
  - Polished add/remove commands and auto-detection for every client can come
    later if needed.

### Concurrency and Serialization

- Status: decided
- Decision: The first implementation should guarantee one worker per assigned
  task while allowing many tasks to exist concurrently.
- Implementation guidance:
  - Persist all session, task, turn, prompt, response, approval, artifact, error,
    and worker event state with durable ordering.
  - Turns within the same task should run serially.
  - Different tasks may run concurrently when they are isolated.
  - Each delegated coding task should get its own worktree.
  - Two workers should not write to the same task worktree at the same time.
  - Multi-agent fanout should be modeled later as parent and child tasks, not as
    a required first-build feature.

## Open Build Decisions

### 1. Initial Implementation Priority

- Status: decided
- User-facing question: When Taskrunner's first build hits a tradeoff, what should
  it protect most?
- Options:
  - Security and isolation first
  - Rich multi-turn delegation first
  - Minimal runnable implementation first
  - Balanced first build
- Recommendation: Balanced first build. Ship multi-turn Codex delegation with
  task-specific worktrees, keep Docker behind a runtime boundary, and avoid
  features that would force a broad security/policy engine before the core loop
  works.
- Answer: Balanced first build. Ship multi-turn Codex delegation with
  task-specific git worktrees and durable records, but keep Docker behind a
  runtime boundary if it would slow down the first working loop.

### 2. Network and Package Install Policy

- Status: decided
- User-facing question: When a delegated worker needs the internet or package
  installation, what should happen by default?
- Options:
  - Disabled by default
  - User approval per task
  - Domain allowlist
  - Open network with logging
- Recommendation: User approval per task, with network disabled until approved.
- Answer: User approval per task. Network and package installation start
  disabled; when a delegated worker needs them, Taskrunner asks for approval for
  that task.

### 3. Worker Session Durability

- Status: decided
- User-facing question: Should Taskrunner copy worker-native session state out of
  the worker environment after every turn?
- Options:
  - Yes, after every turn
  - Only when a task completes
  - No, keep worker-native session state inside the worker environment
- Recommendation: Yes, after every turn.
- Answer: Yes, after every turn. Taskrunner should copy out enough
  worker-native session state after each turn to support durable continuation and
  audit, even if the worker environment is later removed or reset.

### 4. Delegated Turn Result Contract

- Status: decided
- User-facing question: What should every delegated turn report back in a
  predictable format?
- Options:
  - Minimal result only
  - Practical build result
  - Full normalized result
- Recommendation: Practical build result: final answer, status, worker-native
  session ID, changed files, patch/diff when available, log/artifact references,
  and error details.
- Answer: Practical build result for the initial implementation, with an explicit
  path to full normalized results later. Every delegated turn should return final
  answer, status, worker-native session ID, changed files, patch/diff when
  available, log/artifact references, and error details. Taskrunner should also
  store raw worker events/logs so a full normalized event/result contract can be
  added after the first Codex loop works end to end.

### 5. Taskrunner Host Powers

- Status: decided
- User-facing question: Should Taskrunner itself be allowed to run direct
  shell/file actions, or should that be limited to workers?
- Options:
  - Taskrunner orchestration only
  - Taskrunner may run narrow host operations
  - Taskrunner may run broad host operations
- Recommendation: Taskrunner may run narrow host operations needed for
  orchestration, storage, policy checks, git worktree setup, and cleanup.
  Project edits and arbitrary commands should run through workers.
- Answer: Taskrunner may run narrow host operations needed for orchestration,
  storage, policy checks, git worktree setup, artifact/session-state copy-out,
  worker lifecycle, and cleanup. Arbitrary project edits and broad shell
  commands for delegated work should run through workers. This does not restrict
  the foreground client/orchestrator the user is actively typing into; Codex or
  Claude Code may keep its normal host-command workflow under its own existing
  sandbox and approval rules.

### 6. Worker Credential Model

- Status: decided
- User-facing question: What credentials should worker containers receive by
  default?
- Options:
  - No credentials by default
  - Short-lived credentials only
  - Long-lived credentials allowed by config
- Recommendation: No credentials by default, with explicit opt-in per worker or
  project.
- Answer: No credentials by default, except credentials explicitly configured for
  a worker to operate, such as a narrow Codex or Claude auth volume. Delegated
  workers should not automatically inherit developer/project secrets such as SSH
  keys, GitHub tokens, cloud credentials, package publish tokens, host `.env`
  files, shell profiles, browser sessions, or broad host home directories.
  Additional credentials require explicit worker or project configuration.

### 7. Session UX

- Status: decided
- User-facing question: When looking at delegated work, should users mostly see
  Taskrunner tasks, native worker sessions, or both?
- Options:
  - Taskrunner tasks only
  - Native worker sessions only
  - Both, with Taskrunner tasks as the main view
- Recommendation: Both, with Taskrunner tasks as the main view and worker-native
  session IDs available as metadata.
- Answer: Both, with Taskrunner tasks as the main model, optimized primarily for
  agent use rather than frequent manual user commands. Calling agents should use
  Taskrunner task handles when continuing or looking up delegated work. Human
  users will usually encounter this through prompted lookup, audit, or
  troubleshooting views. Worker-native session IDs should be stored and exposed
  in detailed/debug/audit output, but not treated as the normal user-facing
  handle.

### 8. Claude Next-Worker Strategy

- Status: pending
- User-facing question: When Claude support is added after Codex, should it start
  with the CLI, the SDK, or a bridge that can move from CLI to SDK?
- Options:
  - Claude CLI first
  - Claude Agent SDK first
  - CLI first with SDK upgrade path
- Recommendation: CLI first with SDK upgrade path, after Docker file-edit and
  resume retests pass.
- Answer:
