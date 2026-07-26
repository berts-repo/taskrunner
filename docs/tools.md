# Using Taskrunner

You don't call Taskrunner directly — you ask your coding agent to, and it uses these
tools on your behalf. This page explains what each one does so you know what to ask
for. (You can also read the archive straight from a terminal — see
[From the terminal](transcripts.md#from-the-terminal).)

## A few terms

- **Task** — one unit of delegated work, tied to a project. A task can span several
  back-and-forth turns.
- **Turn** — a single request/response inside a task (the first prompt, then each
  follow-up).
- **Worker** — who does the work: `codex`, `claude`, or a worker you add yourself.
- **Artifact** — something a turn produced and Taskrunner saved, such as the diff of
  file changes or the worker's raw activity log.

## The tools

### `assign-task` — delegate new work

Starts a task: picks a **worker**, clones your project, and runs the first turn in a
fresh container. By default it returns right away with a running status so your agent
isn't blocked; ask it to **wait** if you want the result inline. If the work needs
more network access than the worker's default, that's requested here too (see
[Network access](network-access.md)).

### `lookup-task` — see what happened

Fetches a task. By default you get a compact summary (status, worker, how many
turns). Ask for more detail with **include**:

- **turns** — the paired prompt/response exchanges.
- **transcript** — the worker's interior: the tool calls, reasoning, and messages
  that ran *inside* the container. Like `lookup-session`, this starts as an outline
  you can drill into by exchange number. See [Conversation archive](transcripts.md).
- **trace** — an end-to-end replay of a turn: its input, everything the worker did,
  and its output.
- **audit** — the recorded events for a turn.
- **artifacts** — the saved outputs (diffs, raw activity logs).
- **diff** — the actual file changes, inline.

You can narrow a big task to a single turn or just the last few exchanges. Pass a
**project** path instead of a task to list that project's recent tasks.

### `continue-task` — follow up

Sends another prompt to an existing task. The worker picks up its previous session,
so it remembers the earlier turns.

### `cancel-task` — stop a running turn

Stops the turn that's currently running. The task, its history, and its workspace are
kept — nothing is thrown away.

### `lookup-session` — scan a conversation, then read one exchange

Works over **sessions** — a session being one conversation, whether a worker's or one
of your own host agents. With no id it **lists your recent sessions**, newest first,
so you can ask for "the last session" or "my last five". This is also the only way to
read back your own host sessions, which `lookup-task` can't reach because they aren't
tied to a task.

Given a session id it returns an **outline**: one line per exchange, showing what was
asked, how the reply opened, and every tool call with the file or command it acted on.
Each exchange is numbered, so the natural follow-up — "now show me exchange 4" — gets
that one exchange in full. The whole conversation end to end is available too, but
it's rarely what you want first, because a long session costs a lot to read. You can
also filter the list to one project. See [Conversation archive](transcripts.md).

### `search-transcripts` — find it, then read it

Searches every recorded conversation — both worker turns and your own host agent
sessions — three ways, alone or in combination:

- **by text** — the words in a message;
- **by what a tool did** — which **tool** ran and the **target** it acted on, a file
  path or a command. This is how you answer "which sessions touched this file", which
  searching the text answers badly;
- **by what failed** — only calls that errored, or only ones that worked.

Results come back with a snippet, the project they're from, and which task a match
belongs to when it came from a delegated turn. Every hit also reports its conversation
and **exchange number**, so a search leads straight into `lookup-session` for the full
exchange. Narrow further by project, by specific or recent sessions, by time window, or
by kind of message — and sort by relevance or most-recent. See
[Conversation archive](transcripts.md).

## How your agent knows all this

When your agent connects, Taskrunner hands it a short cheat-sheet built from your live
configuration — the available workers, their default network reach, the approval
rules, and the task lifecycle. So any agent that reads its server instructions already
knows what it can delegate and to whom, with no extra setup from you.
