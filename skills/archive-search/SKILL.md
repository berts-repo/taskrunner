---
name: archive-search
description: Look up what was done before in the taskrunner archive of past agent sessions and delegated worker turns. Use when the user asks what happened earlier, which session touched a file, where a command failed, or to recall a previous conversation.
---

# Search the taskrunner archive

Taskrunner archives every delegated worker turn and every host agent session
(Claude Code, Codex) on this machine. Sessions are long, so never read one whole
first. Work in two steps: **find**, then **drill**.

## 1. Find

- **By text:** `search-transcripts` with `query` (SQLite FTS5: bare words are
  ANDed, `"quoted text"` matches a phrase).
- **By what a tool did:** `tool` (which tool ran), `target` (the file path or
  command it acted on), `failed` (only errors, or only successes). This answers
  "which sessions touched this file" and "where did this command fail", which text
  search answers badly. Text and structured filters combine.
- **Narrow it:** `project`, `sessions`, `lastSessions`, `since` / `until`, and
  `sort` (`rank` or `recent`).
- **Or browse:** `lookup-session` with no id lists recent sessions, newest first;
  with a `sessionId` it returns an outline — one line per prompt, reply and tool
  call, each exchange numbered `[N]`.

## 2. Drill

Every search hit and outline line carries its exchange number. Pass it as
`prompt: N` to `lookup-session` (or `lookup-task` for a delegated task) to read that
one exchange in full.

Use `view: "timeline"` on a whole session only when you truly need all of it — it
costs a lot to read.

## Answering

Quote what the archive shows and say which session and exchange it came from, so
the user can check it. If nothing matches, say so and say what you searched for;
an empty result is not proof it never happened.
