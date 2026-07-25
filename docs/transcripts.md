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

Three ways through your agent, depending on what you're after:

- **Browse whole conversations** — `lookup-session` lists your recent sessions
  (newest first, optionally within one project), so you can ask for "the last session"
  or "look through my last five". Give it a session id and you get that entire
  conversation in order. This is the only way to read back **your own host sessions** —
  they aren't tied to a task, so `lookup-task` can't see them. The most recent
  transcripts are swept in on demand when you ask, so "the last session" reflects the
  conversation you were just in, up to its last saved line.
- **One task's interior** — ask `lookup-task` to include the **transcript**. You get
  the tool calls, reasoning, and messages that ran inside that task's container.
- **Search everything** — `search-transcripts` runs a full-text search across the
  whole archive (worker turns *and* your host sessions) and returns matching messages
  with a snippet and their project. When a match came from a delegated task, it tells
  you which one. You can scope a search to a project, to specific or recent sessions,
  to a time window, or to a kind of message — handy for "find where I discussed X in my
  last few sessions".

### From the terminal

You don't need an agent to read the archive. The same lookups are available as
commands, printing straight to your shell — useful for a quick grep without spending a
conversation:

```sh
taskrunner sessions                     # your recent sessions, newest first
taskrunner sessions --project /path      # just this project
taskrunner session <id>                  # one conversation in full
taskrunner search "flaky proxy test"     # full-text search
taskrunner search "proxy" --last-sessions 5   # …within your last 5 sessions
taskrunner task <id> --include transcript     # one task's interior
taskrunner tasks --project /path              # a project's recent tasks
```

Each command talks to the running daemon; if it isn't up yet, your agent's next
request (or `taskrunner up`) starts it.

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
