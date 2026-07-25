# Conversation archive

Taskrunner keeps a permanent, searchable record of AI conversations — both the tasks
it runs and the sessions of your own host coding agents. It all lives locally under
`~/.taskrunner/`.

## What gets recorded

- **Your host agents.** Taskrunner periodically sweeps the transcripts that Claude
  Code and Codex write on your machine into its own archive.
- **The workers it runs.** The full interior of every delegated turn — every
  intermediate tool call and message, not just the final answer — is captured and
  tied to its task.

Why this matters: coding tools don't keep their history forever (Claude Code, for
example, deletes transcripts after 30 days by default). Once a conversation is in
Taskrunner's archive, it stays — so you can go back to it months later.

> **Tip:** to make sure nothing is lost before the first sweep, raise
> `cleanupPeriodDays` in `~/.claude/settings.json` so Claude Code holds its
> transcripts longer.

The archive is **read-only toward your files**: it copies transcripts, never moves or
edits them, so your agents' own session-resume keeps working.

## Reading it back

Two ways, both through your agent:

- **One task's interior** — ask `lookup-task` to include the **transcript**. You get
  the tool calls, reasoning, and messages that ran inside that task's container.
- **Search everything** — `search-transcripts` runs a full-text search across the
  whole archive (worker turns *and* your host sessions) and returns matching messages
  with a snippet. When a match came from a delegated task, it tells you which one.

## Where logins and transcripts are stored (worth understanding)

There are two separate storage locations, and knowing the difference explains some
behavior you might otherwise find surprising.

- **The state root** (`~/.taskrunner`) is a daemon's private **notebook**: its task
  list, its event log, its search index.
- **A worker's storage** (for example `taskrunner-codex-home`) is that worker's shared
  **filing cabinet**. It holds the worker's login *and* the session transcripts it
  writes as it works. There's exactly one per worker, by design — so you sign in once
  and every task reuses that login.

The consequence: anything a worker does is written into that one shared cabinet, and
**any** Taskrunner daemon watching it will archive those transcripts. So if you ever
run a task under a different, throwaway state root (for testing, say), its transcripts
still land in the shared cabinet — and your main `~/.taskrunner` daemon will pick them
up on its next sweep.

This never causes a conflict:

- Separate notebooks don't collide — different tasks, different logs, kept apart.
- The cabinet is only *read* during a sweep, and recording is idempotent, so two
  daemons reading the same cabinet simply archive the same lines once each.
- A picked-up transcript from another state root shows up as **un-attributed** text
  (searchable, but tied to no task), because that task's record lived only in the
  other notebook. It never creates a phantom task in your list.

In short: **notebooks are private per daemon; a worker's login and logs are shared, so
whatever a worker does gets archived by whoever is watching that shared cabinet.**

The internal format of the archive — how records are de-duplicated, indexed for
search, and copied out of worker storage — is in the
[implementation notes](archive/implementation-notes.md#conversation-archive).
