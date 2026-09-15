# Librarian

This file defines reusable documentation-management rules for agents working in
a project repository. The role name is **Librarian**. The generic librarian process
(cleanup workflow, classification, bootstrap) lives in the librarian skill's
`SKILL.md`; this file is the documentation rule reference and wins on conflict
with that process.

## This project

- Folders of the model are created when a document first needs one. Taskrunner
  uses `docs/guide/`, `docs/reference/`, `docs/security/` and `docs/work/`.
  `docs/specs/` was removed on purpose (commit b391363); ask before bringing it back.
- `README.md` is the user-facing index. Adding, moving or removing a guide or
  security page updates its Documentation list in the same change.
- `skills/*/SKILL.md` are product code, built into the binary and served to agents.
  They are not documents to reorganise, but a change in what a skill should tell an
  agent updates the skill in the same commit.
- `HANDOFF.md` at the root, when present, is a scratch note the `handoff` skill
  rewrites for the user to paste into a fresh session. It is excluded from git and is
  not a document: leave it alone.
- Doc paths also appear in code: error messages and comments (for example
  `src/storage/events.rs`). A move searches `src/` and `tests/` as well as the docs.
- A dated one-off report, such as an investigation or a handoff, carries an ISO date
  prefix: `docs/security/2026-09-13-codex-three-blocked-connections.md`.

## Responsibility

Keep project documentation accurate, navigable, and organized around the current
documentation model:

```text
CONTEXT.md            Root project context and task read-order entrypoint.
docs/reference/      What the app currently is (agent-facing).
docs/guide/          User-facing feature documentation (what each thing does).
docs/security/       Security awareness and accepted/deferred-risk writeups.
docs/specs/          Text/spec source material for intended product surfaces.
docs/work/additions/ Live future feature notes and deferred capability sketches.
docs/work/proposals/ Owner-discussed directions not ready for active work.
docs/work/active/    What is being changed now.
docs/work/archive/   What was changed before.
```

Do not create loose root-level planning files. Root entrypoints such as
`CONTEXT.md`, `AGENTS.md`, and `LIBRARIAN.md` are allowed because they route to
the docs tree. New plans, checklists, handoffs, and outcomes belong under
`docs/work/`.

## Project Model

The project's purpose, intended users, core workflows, and hard constraints
belong in `CONTEXT.md`. Read that context before organizing or revising project
documentation, and preserve its framing. Keep project-specific facts and
decisions there or in the relevant reference docs.

Security and privacy notes should distinguish current protections and their
limits from explicitly deferred work. Preserve the project's stated threat
model and owner decisions, and tie each deferral to its specific scope.

Read orders are defined in `CONTEXT.md` ("Read First" and "Task Read Orders");
do not restate them here or in other docs.

## Active Work Rules

- `docs/work/ACTIVE.md` names the current active package, or lists multiple
  active packages when `NEXT.md` has explicitly declared them runnable in
  parallel (different code, no conflict). It reads `none` when no
  implementation package is active. Avoid more than two parallel packages —
  the limit exists to keep context navigable, not to forbid concurrency.
- `docs/work/NEXT.md` holds the prioritized queue of upcoming work. When a
  queued item is promoted to an active package, remove it from `NEXT.md` in the
  same change.
- Active packages live under `docs/work/active/YYYY-MM-DD-slug/`.
- Non-trivial active packages should include:
  - `README.md`
  - `checklist.md`
  - `handoff.md`
  - `plan.md` when there is an implementation plan
  - `decisions.md` when owner decisions are recorded
  - `outcome.md` when the work closes
- When the active project changes, update `docs/work/ACTIVE.md` in the same
  commit that creates or promotes the package.

## Archive Rules

Move completed, abandoned, or superseded work to
`docs/work/archive/YYYY-MM-DD-slug/`.

When archiving:

- Keep `README.md`, `outcome.md`, and `decisions.md` if present.
- Keep long checklists or plans only when they still explain useful context.
- Compress scratch notes into `outcome.md` before deleting or trimming them.
- Preserve user decisions and threat-model assumptions.
- Graduate any still-open "Open (later)" decisions into `docs/work/REVISIT.md`
  before archiving, so they are not buried where the read-order rules skip.

## Additions Rules

`docs/work/additions/` is an intake folder for work that has not shipped yet.
Keep it small enough that a user or agent can scan it without reading completed
history.

Use `additions/` for:

- owner-confirmed future feature notes that are not active yet
- implementation-ready specs waiting in `NEXT.md`
- explicit ideas marked `(idea)` or "future feature note"
- narrow handoffs for planned follow-up work that has not started

Do not use `additions/` as the permanent home for completed implementation
specs. When an addition is promoted and then closes:

1. Move the useful package context into `docs/work/archive/YYYY-MM-DD-slug/`.
2. Make the archive package's `README.md` and `outcome.md` the historical entry
   points.
3. Remove the full completed spec from `additions/`, or replace it with a tiny
   pointer only when stable inbound links still need a landing page.
4. Update `docs/work/additions/README.md`, `docs/work/NEXT.md`,
   `docs/work/ACTIVE.md`, and any archive read-order links in the same change.

If an addition only partially ships, keep a live additions doc only for the
remaining unshipped scope. The shipped part belongs in archive; the remaining
doc should say what is still open and link to the archived outcome.

## Reference Docs

`docs/reference/` is the canonical current-truth layer. Update it when code or
behavior changes affect:

- project features or product shape
- architecture
- backend/frontend structure
- data model
- security model
- dependencies
- runbook commands
- testing expectations

Standing maintenance obligations — recurring upkeep triggered by external events
rather than a work package, such as bumps to versions mirrored inside source
code — live in `docs/reference/maintenance.md`.

## Guide Docs

`docs/guide/` is user-facing feature documentation: what each feature is, when
a user needs it, how to use it, and its limits. It is
written for project users, not for agents. Agent-facing current truth stays
in `docs/reference/`; when the two conflict, `docs/reference/` and the code
win.

When a package changes user-visible behavior, update the affected guide page
before the package closes — the same obligation as reference docs above.

Keep `CONTEXT.md` short and stable. Update it only when project framing,
source-of-truth rules, hard constraints, or task read orders change.

Do not let implementation-history notes become the source of truth for current
behavior.

## Specs Docs

`docs/specs/` preserves original prompt/spec source material. Avoid
deleting it. If a spec is outdated, prefer adding a short note that points to
the current reference doc or archived outcome instead of rewriting the original
source intent.

Live future feature notes and deferred capability sketches belong under
`docs/work/additions/` and should be linked from `docs/work/NEXT.md` when they
are owner-confirmed follow-up work. Completed addition specs belong with their
closed work package in `docs/work/archive/`.

Owner-discussed cleanup or product directions that need more conversation before
they become active implementation packages belong under `docs/work/proposals/`.
Use this bucket for strategy-level direction documents; do not put active
package `plan.md` implementation steps there.

## Queue And Watch-list

`docs/work/ACTIVE.md` names the current active package. `docs/work/NEXT.md`
holds the prioritized queue of upcoming work — what to start next. Keep project
state out of this file.

`NEXT.md` holds live queue items only — never completed-work summaries; archive
packages are the historical entry points. If a queued item needs extra
implementation notes, keep them under `docs/work/additions/` while unshipped.

`docs/work/REVISIT.md` is the durable watch-list of things to come back to or
possibly remove — not a build queue; ready, intended work belongs in `NEXT.md`.
Keep rows short (what · why parked · trigger to act-or-delete · pointer), link
to code or a package rather than re-explaining it, and remove a row when it is
acted on. On package archival, graduate still-open "Open (later)" decisions
into it.
