# Outcome

## The threat (kept because it is the reason for the design)

Found by a Codex bug-check pass and reproduced before fixing:

1. A task's clone, `.git` included, sits in the worker container's writable mount.
2. Git obeys configuration stored in a repository, and several settings name a
   program for git to run: `diff.external`, `core.fsmonitor`,
   `uploadpack.packObjectsHook`, filter drivers reached through `.gitattributes`.
3. After a turn, taskrunner ran `git status`, `git diff HEAD` and `git rev-parse` in
   that clone, and fetched from it, on the host as the user.
4. A worker that wrote one line of `.git/config` could have a program run on the host,
   outside the container, the egress firewall and every other boundary a turn has.
   Confirmed with `diff.external` on `git diff` and `core.fsmonitor` on `git status`.

The fix (`aea1960`): all post-turn git is one script run in a throwaway container with
no network, holding only the clone and an output folder. The host reads only the
files it writes (status, diff, a tip accepted only as a hex object id, and a bundle)
as untrusted data. Commits land by fetching the bundle, a file of objects rather than
a repository, with `transfer.fsckObjects` on. The base commit sits beside the clone,
where a worker cannot edit it. Running the same script on the host is test-only.

## What happened after

- `2530897`: the workspace tests use the container inspection, so they need Docker.
- `aa81b99`: an inspection whose `git status`, `git diff` or branch step fails stops
  with a fixed message, and git's own stderr, which comes from a `.git` the worker
  controlled, is never passed on.
- `22c9d2f`: handoff deleted as done. The branch it described
  (`fix-worker-git-escape`) is merged into `main`.
- The commit messages do not record the live test output itself.

## Open issues the handoff listed, all since shipped

- Codex `custom_tool_call` records were missing from the archive: `b0ccc3f`.
- Search hits still said "assistant" instead of the harness: `22c9d2f`.
- `taskrunner --help` exited 1: `22c9d2f`.
- The install link failed when re-run (`ln -s` instead of `ln -sfn`): `22c9d2f`.
- The event log was not hash-chained: `d86781f`; see
  [Event log chain](../2026-09-15-event-log-chain/README.md).

## Working notes from the handoff not recorded elsewhere

- Tests fail first: every fix had a test run against the unfixed code and seen to
  fail. Two tests passed against unfixed code at first, so the red run matters.
- Only one agent edits the folder at a time; a second agent uses its own worktree.
  Two sessions working in it at once had one's uncommitted edits swept into the
  other's commit.
