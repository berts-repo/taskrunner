# Cross-Workstation Sync (Proposal)

Status: shelved (2026-07-16). Cross-workstation sync was cut from scope along
with the knowledge layer; see `PLAN.md` § Implementation phases. This writeup
is kept for the record and would need revision before any revival — it
predates the scope cut and still references memory records, which Taskrunner
no longer keeps. One piece was adopted into the baseline earlier: the
log-first write path (event log as source of truth, SQLite as derived index)
now lives in `PLAN.md` build-spec § Storage write path.

## Why this exists

Two pressures point at the same feature:

1. We want one user to work on a project across multiple workstations (laptop,
   desktop, VM) with durable sessions, memory, and resumable delegated tasks
   following them.
2. The current baseline makes `.taskrunner/sessions/` git-backed portable
   history committed and pushed with the project (`PLAN.md` § Project-local
   file layout, sessions path and git posture).
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

The durable schema (`PLAN.md` § Database schema) is mostly operational and
structural, not
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
  (JSONL is already an export format, `PLAN.md` § Retention defaults and export
  format).
- The local SQLite database becomes a derived index that can be rebuilt from the
  log at any time. This matches the existing append-oriented stance
  (§ Database schema, implementation rules) and the "delete-and-rebuild is
  allowed for derived state" rule (§ Project-local file layout, sync and
  recovery rule).

Append logs merge cleanly across machines, which removes almost all conflict
complexity. SQLite stops being a thing we sync and becomes a cache.

### 2. A dedicated, encrypted, user-owned state remote

Separate from the project's code repo. Proposed v1 form: an encrypted private
git "state repo".

- Git push/pull semantics already match the baseline sync rules (`PLAN.md`
  § Project-local file layout, sync and recovery rule): explicit push/pull,
  one active writer per session, stash-and-rebuild over clever merging. Those
  rules were correct; they were only aimed at the wrong remote.
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

- `artifacts` already carry hashes (`PLAN.md` § Database schema). Sync large
  blobs by reference and pull on demand by hash rather than eagerly.
- Large patch bundles and raw event streams stay on their existing expirable
  retention tier (§ Retention defaults and export format).

### 5. Explicit local-vs-synced boundary

Reuses the existing `.taskrunner/local/` split (`PLAN.md` § Project-local file
layout).

- Synced: the event log, memory, instruction snapshots, policy, artifact
  references.
- Never synced: runtime, caches, locks, sockets, task workspaces
  (worktrees/clones), per-machine config,
  and the per-machine `capabilities` set.

### 6. New-machine bootstrap

`taskrunner clone` (or auto-discovery from a user-global project to state-remote
map): pull the event log, rebuild SQLite, decrypt with the user's key, ready.
This satisfies the "project moves to a VM or another workstation" goal
(§ Project-local file layout, portability rule) without touching the project's
code repo.

## Ease-of-use additions (single machine)

These make the local experience zero-config so the sync layer has something
frictionless to extend.

- `taskrunner up`: start the server, detect installed clients/workers on PATH,
  write their MCP configs, and populate `capabilities` automatically. Clients
  without native MCP get the portable wrapper (`PLAN.md` § Client Capture
  Model).
- No explicit project creation: resolve the project from the git root / cwd via
  `projects` and `project_aliases` (§ Database schema); task workspaces resolve
  back to their origin project.
- Working defaults for risk tier, retention, and redaction (§ Policy,
  Retention, And Redaction; § Risk tiers and default policy); config-file-first
  but optional.
- `taskrunner status`: report exactly what is captured vs. not, reusing the
  existing partial-capture message (§ Client-specific capture behavior).

## Resulting UX

- Machine A: work normally. Taskrunner captures locally and syncs encrypted to
  the private state remote at session/task boundaries (not continuous).
- Machine B: sign in once with an existing git credential, open the project, and
  full history, memory, and resumable delegated tasks are already present because
  worker-native session IDs traveled in the log (§ Database schema,
  `worker_sessions`).

One sign-in, every project follows the user.

## Deltas to `PLAN.md` if adopted

- Demote `.taskrunner/sessions/` from "committed to the project remote" to a
  local derived cache rebuilt from the synced event log; it stops being a
  portable git artifact.
- Add the state remote as a first-class concept (encrypted, user-owned, separate
  from project git).
- Add payload encryption and key management (currently the plan only has
  pattern-based redaction, § Risk tiers and default policy, which cannot make
  sharing safe on its own).
- Add a `workstations` table and active-writer / check-out mechanics.
- Reframe the sync and recovery rule (§ Project-local file layout) to target
  the state remote, keeping its existing rules nearly verbatim.

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
