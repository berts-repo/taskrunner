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

A single conversation runs to hundreds of messages, so reading the archive is two
steps: **find** something, then **read just that piece of it**.

Every conversation is numbered by exchange — one thing you asked, plus everything that
followed it. That number is the address. Search results carry it, outlines print it,
and you hand it back to ask for that one exchange in full.

### Through your agent

- **Find it** — `search-transcripts` searches the whole archive (worker turns *and*
  your host sessions) three ways, alone or in combination:
  - **by text** — the words in a message.
  - **by what a tool did** — which **tool** ran, and the **target** it acted on: a
    file path, a command. This answers what text search answers badly, like "which
    sessions touched `proxy.ts`".
  - **by what failed** — only the calls that errored, or only the ones that worked.

  Any search can be scoped to a project, to specific or recent sessions, to a time
  window, or to a kind of message. Every hit reports the conversation it came from and
  its exchange number, with a snippet and the task when the match came from a
  delegated turn.
- **Read it** — `lookup-session` with no id **lists your recent sessions**, newest
  first, so you can ask for "the last session" or "look through my last five". Given a
  session id it returns an **outline**: one line per exchange, with the reply's opening
  line and each tool call and what it touched — enough to see the shape of a
  conversation without paying for its contents. Then ask for one exchange by number and
  you get it in full. The whole conversation end to end is available too, for when you
  genuinely need all of it.

  This is also the only way to read back **your own host sessions** — they aren't tied
  to a task, so `lookup-task` can't see them. The most recent transcripts are swept in
  on demand when you ask, so "the last session" reflects the conversation you were just
  in, up to its last saved line.
- **One task's interior** — ask `lookup-task` to include the **transcript**: the tool
  calls, reasoning, and messages that ran inside that task's container, with the same
  outline / one-exchange / whole-conversation choice.

### From the terminal

You don't need an agent to read the archive. The same lookups are available as
commands, printing straight to your shell — useful for a quick look without spending a
conversation.

The terminal starts from a different place on purpose: it prints the **timeline** —
the conversation itself, your prompts and the replies in full, with long tool output
capped so it stays readable. A person at a shell wants to read; an agent scanning
wants the outline, because it pays for every line it takes in. Same archive, same
numbering, different starting point.

Reply and tool-call labels name the harness that wrote them — for example, Claude,
Codex, Hermes, or OpenClaw — using the source stored with each archived message.

```sh
taskrunner sessions                          # your recent sessions, newest first
taskrunner sessions --project /path          # just this project

taskrunner session <id>                      # the conversation, as a timeline
taskrunner session <id> --view outline       # one line per exchange and tool call
taskrunner session <id> --prompt 3           # just exchange 3, in full
taskrunner session <id> --tool-lines 0       # stop capping long tool output

taskrunner search "flaky proxy test"                      # by text
taskrunner search --tool Edit --target src/shim/proxy.ts  # by what a tool touched
taskrunner search --failed true --tool Bash               # by what went wrong
taskrunner search "proxy" --last-sessions 5               # …within your last 5 sessions

taskrunner task <id> --include transcript    # one task's interior, same views
taskrunner tasks --project /path             # a project's recent tasks
taskrunner wait <task-id>                    # wait for a task's turn to end, then a short result
```

Search prints an exchange number on every hit, so the loop is the same two steps here:
search, then `session <id> --prompt N`.

A timeline of a real conversation is long and there is no built-in pager, so pipe it
to one:

```sh
taskrunner session <id> | less
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
[implementation notes](../reference/internals.md#conversation-archive).
