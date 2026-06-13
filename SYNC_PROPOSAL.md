# Cross-Workstation Sync (Proposal)

Status: possible addition, not adopted. This is a design writeup for review,
not a committed part of the baseline plan in `PLAN.md`.

## Why this exists

Two pressures point at the same feature:

1. We want one user to work on a project across multiple workstations (laptop,
   desktop, VM) with durable sessions, memory, and resumable delegated tasks
   following them.
2. The current baseline makes `.taskrunner/sessions/` git-backed portable
   history committed and pushed with the project (`PLAN.md:420`, `:437-440`).
   Captured prompts, responses, and summaries riding the project's shared git
   remote is a disclosure risk that redaction cannot fully close, and git
   history makes it effectively unrevocable.

This proposal replaces "session history rides the project repo" with a
user-owned sync channel, which solves the portability goal and removes the leak
vector at the same time.

## Scope and simplifying assumption

Single user, multiple workstations. One owner, one trust domain, one credential.

This is much simpler than multi-user collaboration:

- No redaction-before-share is required; the user already has access to all of
  their own data.
- No inter-person access control is needed.
- The only adversary is accidental leakage into a shared or public location.

Multi-user collaboration is explicitly out of scope here and would need a
separate design (curated, redacted export rather than full-fidelity sync).

## What the DB provides (and what is actually sensitive)

The durable schema (`PLAN.md:464-503`) is mostly operational and structural, not
prompt/response content:

- Project identity and resolution: `projects`, `project_aliases`.
- Delegation orchestration and worker continuity: `tasks`, `turns`,
  `worker_sessions` (including the worker-native session IDs needed to resume a
  task).
- Memory and recall: `memory_records`.
- Artifact store and graph: `artifacts`, `artifact_links`.
- Reproducibility: `instruction_packages`, `instruction_snapshots`.
- Governance ledger: `policy_sets`, `approval_records`.
- Per-workstation capability registry: `capabilities`.

Only a thin slice is privacy-sensitive: the prompt/response bodies inside
`audit_events` payloads and `turns` content. The decision is therefore not
"should we keep the audit DB" but "where do the sensitive payloads live and do
they ever leave the machine."

## Core design

### 1. Append-only event log is the synced source of truth

SQLite does not merge across machines. Instead:

- The durable, synced artifact is a per-session / per-task append-only event log
  (JSONL is already an export format, `PLAN.md:661`).
- The local SQLite database becomes a derived index that can be rebuilt from the
  log at any time. This matches the existing append-oriented stance (`:509`) and
  the "delete-and-rebuild is allowed for derived state" rule (`:461`).

Append logs merge cleanly across machines, which removes almost all conflict
complexity. SQLite stops being a thing we sync and becomes a cache.

### 2. A dedicated, encrypted, user-owned state remote

Separate from the project's code repo. Proposed v1 form: an encrypted private
git "state repo".

- Git push/pull semantics already match the baseline sync rules: explicit
  push/pull (`PLAN.md:457`), one active writer per session (`:458`),
  stash-and-rebuild over clever merging (`:459-460`). Those rules were correct;
  they were only aimed at the wrong remote.
- "Sign-in" is an existing GitHub/GitLab credential the user already has.
- Sensitive payloads are encrypted with a user-held key before they leave the
  machine. Single user means a single key (OS keychain or passphrase-derived).
  This is what makes off-machine sync safe without redaction: a misconfigured
  backend sees only ciphertext.

The project's code repo stays completely clean of session data.

### 3. Workstation registry and active-writer safety

- Each machine gets a stable workstation ID so the event log can attribute
  writes and enforce one active writer per session.
- A soft "this session is checked out on <machine>" indicator prevents the
  two-machines-at-once footgun.

### 4. Content-addressed artifact sync, lazy by default

- `artifacts` already carry hashes (`PLAN.md:476`). Sync large blobs by
  reference and pull on demand by hash rather than eagerly.
- Large patch bundles and raw event streams stay on their existing expirable
  retention tier (`:640`).

### 5. Explicit local-vs-synced boundary

Reuses the existing `.taskrunner/local/` split (`PLAN.md:424-433`).

- Synced: the event log, memory, instruction snapshots, policy, artifact
  references.
- Never synced: runtime, caches, locks, sockets, worktrees, per-machine config,
  and the per-machine `capabilities` set.

### 6. New-machine bootstrap

`taskrunner clone` (or auto-discovery from a user-global project to state-remote
map): pull the event log, rebuild SQLite, decrypt with the user's key, ready.
This satisfies the "project moves to a VM or another workstation" goal (`:449`)
without touching the project's code repo.

## Ease-of-use additions (single machine)

These make the local experience zero-config so the sync layer has something
frictionless to extend.

- `taskrunner up`: start the server, detect installed clients/workers on PATH,
  write their MCP configs, and populate `capabilities` automatically. Clients
  without native MCP get the portable wrapper (`PLAN.md:41`, `:60`).
- No explicit project creation: resolve the project from the git root / cwd via
  `projects` and `project_aliases` (`:468-469`); worktrees resolve back to their
  origin project.
- Working defaults for risk tier, retention, and redaction (`:329`, `:346`,
  `:624`); config-file-first but optional.
- `taskrunner status`: report exactly what is captured vs. not, reusing the
  existing partial-capture message (`:705-707`).

## Resulting UX

- Machine A: work normally. Taskrunner captures locally and syncs encrypted to
  the private state remote at session/task boundaries (not continuous).
- Machine B: sign in once with an existing git credential, open the project, and
  full history, memory, and resumable delegated tasks are already present because
  worker-native session IDs traveled in the log (`:473`).

One sign-in, every project follows the user.

## Deltas to `PLAN.md` if adopted

- Demote `.taskrunner/sessions/` from "committed to the project remote" to a
  local derived cache rebuilt from the synced event log; it stops being a
  portable git artifact.
- Add the state remote as a first-class concept (encrypted, user-owned, separate
  from project git).
- Add payload encryption and key management (currently the plan only has
  pattern-based redaction, `:624`, which cannot make sharing safe on its own).
- Add a `workstations` table and active-writer / check-out mechanics.
- Reframe the "Sync and recovery rule" section (`:455-462`) to target the state
  remote, keeping its existing rules nearly verbatim.

## Open questions

- State remote granularity: one state repo per project, or one per user holding
  all projects? Per-user is the simplest sign-in story; per-project is easier to
  scope and discard.
- Where the project to state-remote pointer lives: user-global config (keeps the
  project repo clean) vs. an opt-in non-secret pointer committed in
  `project.toml`.
- Key management UX: keychain-backed vs. passphrase-derived, and how a new
  machine obtains the key.
- Bring-your-own-remote (git or S3-compatible) first, with a hosted sync service
  as a possible later option that would only ever hold encrypted blobs.
- Sync cadence: session/task boundaries vs. an explicit `taskrunner sync`, and
  whether a background watcher is worth the added churn.
