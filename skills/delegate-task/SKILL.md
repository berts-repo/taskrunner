---
name: delegate-task
description: Hand a coding task to a taskrunner worker (codex, claude, or a local model) that works in its own isolated container and returns the result on a git branch for review. {delegation} Not the same as a harness's own subagents, such as Hermes's delegate_task.
---

# Delegate a task to a taskrunner worker

A worker is a different agent (often a different model) running in a throwaway
Docker container. It gets a clone of the project at its last commit, does the
work, and its commits come back on a branch named `taskrunner/<task-id>`. Nothing
is merged into the user's branch.

## When

{delegation}

Offering means: name the worker, the task in one line, and why it is worth handing
off. Then wait. Never start a task the user has not agreed to.

## Before assigning

1. **Pick the worker.** The `worker` argument of `assign-task` lists them; `codex`
   and `claude` are built in. A different model than the one you are is the point
   of a second opinion.
2. **Check for uncommitted work.** Run `git status --porcelain` in the project.
   The worker's clone starts from the last commit, so uncommitted and untracked
   files are not there. If there are any, list them and ask the user: commit first,
   delegate anyway, or stop. Do not commit for them. `assign-task` also reports
   these as `not included:`.
3. **Write a self-contained prompt.** The worker sees the repository, not this
   conversation. Say the goal, the files that matter, the constraints, how to check
   the work (which tests to run), and ask it to commit when done — only committed
   work comes back.
4. **Network.** A worker reaches only its own API. Extra domains (`allowDomains`)
   need the user's explicit yes first; the `assign-task` description has the rule.

## Assign

`assign-task` with the absolute project path, the worker, and the prompt. For a
short task pass `wait: true`. For a long one, leave `wait` off and tell the user it is
running. Then:

- **If you can run a shell command in the background and are told when it ends**
  (Claude Code can), run `taskrunner wait <task-id>` that way. It prints nothing
  until the task finishes, then a short result — status, branch, one-line summary —
  so you can review it without the user having to ask. Don't poll as well.
- **Otherwise**, check with `lookup-task` and the task id when the user asks.

## Review the result

1. The result shows status, the worker's summary, changed files, and
   `branch: taskrunner/<task-id>` when commits landed.
2. Read the diff: `lookup-task` with `include: ["diff"]`, or
   `git diff HEAD...taskrunner/<task-id>` in the project.
3. Tell the user what changed, anything risky or unverified (tests the worker did
   not run, files outside the task, network use — `include: ["audit"]` shows it),
   and recommend one of: merge, follow up, or discard.
4. Never merge, rebase, cherry-pick, or delete the branch yourself. That is the
   user's decision; give them the command (`git merge taskrunner/<task-id>`).

## Follow up

- `continue-task` sends another prompt; the worker keeps its session.
- `cancel-task` stops a running turn; the task and its history are kept.
- A turn that fails with a login or auth error needs the worker signed in again:
  use the `worker-login` skill.
