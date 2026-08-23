# Session handles — proposal (not implemented)

Noted 2026-07-28. Nothing here is built yet; this is a design decision captured
before it gets lost.

## The problem

Reading a session from the terminal means typing its full 36-character id:

```sh
taskrunner session aa1314ca-bb13-4b91-b4b0-b225a575df3e
```

Resolution is exact-match only today — `src/storage/index.ts:472` compares
`native_session_id = ?`, so `taskrunner session aa1314ca` fails with
`error not_found`. There is no shorter way in.

This is a human-at-the-shell problem specifically. Agents over MCP copy-paste
full ids without complaint, so the fix can be CLI-shaped without touching tool
schemas — though prefix resolution belongs in the store so both paths get it.

## Considered and rejected: numbering the list

The obvious idea is to let `taskrunner sessions` print `1, 2, 3` and accept
`taskrunner session 2`. Rejected, for three reasons:

- **Unstable, and fails silently.** The list is ordered by `last_ts` desc, so
  your own in-progress session is always #1 and the order re-sorts as you work.
  A number read off an earlier listing resolves to a real session that isn't the
  one you meant, with no warning. A wrong-but-valid answer is worse than an
  error.
- **Depends on invisible state.** `sessions --project X` orders differently than
  the unfiltered list, so "2" means different things depending on which command
  you last ran. Making it reliable requires persisting the last-listed set —
  real statefulness for a convenience feature.
- **The parse itself is free.** Session ids are UUIDs and never bare integers,
  so there is no syntactic ambiguity in accepting a number. The objection is
  semantic, not technical.

The precedent worth copying is git: a stable short handle (prefix-matched SHAs)
for anything you might write down, plus explicitly *relative* refs (`HEAD~1`,
`@{1}`) whose syntax announces that they move.

## Recommended: short ids + a `last` word

**1. Short ids that just work.** The session list prints an 8-character id
instead of the full 36, and any unambiguous prefix resolves:

```
$ taskrunner sessions
  aa1314ca  today 10:17   145 msgs   ~/Git/taskrunner
  90bef95f  today 06:38     7 msgs   ~/
  9c288f22  Jul 26 10:05   38 msgs   ~/Documents/wazuh-lab

$ taskrunner session aa1314ca
```

Eight characters readable off the screen and retypable without copy-paste. A
prefix matching two sessions errors and lists both, rather than picking one — so
a short id jotted in a note still means the same session next week.

**2. The word `last` for the obvious case.** Most of the time you want the
conversation you were just in:

```sh
taskrunner session last              # the one I was just in
taskrunner session last --prompt 6   # exchange 6 of it
taskrunner session last2             # the one before that
```

`last` *reads* as relative, so it is obvious it points somewhere new tomorrow.
A bare `2` looks like a fixed address but is not — that is the trap. This keeps
the "grab one off the list" convenience for the recent handful without a number
that quietly goes stale.

Everything else is unchanged: same views, same `--prompt N`, same search.

## Cost

Small. Prefix matching is roughly one query change (`LIKE 'prefix%'` with
`LIMIT 2` to detect collisions), an ambiguity error, and tests. The `last`
keyword is argument parsing plus a reuse of the existing sessions-list query.

## Open questions (need sign-off before building)

Both are user-visible names, so they need explicit approval:

1. Is `last2` the right spelling for "the one before"? Alternatives: `last~2`,
   `prev`, or not supporting it at all and using short ids for anything older.
2. Should the sessions list keep printing full ids alongside the short ones, for
   copy-paste into other tools?
