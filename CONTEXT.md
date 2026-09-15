# Taskrunner context

## Purpose

Taskrunner lets a coding agent (Claude Code, Codex, Hermes, or anything that speaks
MCP) hand work to another AI worker that runs in an isolated Docker container over a
private clone of the project. It keeps a local, searchable and tamper-evident record
of everything the worker and the host agents did.

It is built for one developer on their own machine: the user runs the daemon, signs
the workers in, and decides what a task may reach on the network.

## Core workflows

- Connect the agents on the machine with `taskrunner sync`.
- Delegate a task, follow it up, review the diff on its `taskrunner/<task-id>`
  branch, and merge or discard it.
- Search and read back past sessions and delegated turns from the archive.
- Prove the record hasn't changed with `taskrunner verify` and `taskrunner anchor`.

## Hard constraints

- A worker reaches only its own API unless the user explicitly approves more;
  loopback, LAN and private addresses stay blocked regardless.
- The JSONL event log is the source of truth: append-only and hash-chained. The
  SQLite index is derived and can be rebuilt from the log.
- Worker logins live in their own Docker volumes, never the user's host credentials.
- Anything an agent or skill sets up can also be done by editing
  `~/.taskrunner/config.toml` and running a `taskrunner` command.

## Read First

1. `README.md` — what Taskrunner is, and the index of user guides.
2. `docs/reference/internals.md` — how it works now.
3. `docs/work/ACTIVE.md` — what is in flight.

## Task Read Orders

- **Changing behaviour:** `docs/reference/internals.md`, then the `docs/guide/` page
  for the feature, then `docs/work/ACTIVE.md`.
- **Isolation, network, logins or the log:** `docs/security/overview.md`, plus
  `docs/guide/network-access.md` or `docs/guide/log-integrity.md`.
- **What to work on next:** `docs/work/ACTIVE.md`, then `docs/work/NEXT.md`.
- **Design directions not built yet:** `docs/work/proposals/`.
- **Documentation cleanup:** `LIBRARIAN.md`, then `docs/work/ACTIVE.md`.

## Source Of Truth

- Current code and `docs/reference/` are authoritative for current behaviour.
- `docs/guide/` is user-facing; when it conflicts with `docs/reference/` or the
  code, those win.
- `docs/security/` holds the security model and security investigations.
- `docs/work/ACTIVE.md` is the single pointer to current work, `docs/work/NEXT.md`
  the queue, and `docs/work/REVISIT.md` the watch-list, not a build queue.
- `docs/work/proposals/` holds directions that are not ready to build.
- `docs/work/archive/` is history, not current truth; read it only when a current
  doc points there.
