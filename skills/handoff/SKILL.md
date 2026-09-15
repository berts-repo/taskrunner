---
name: handoff
description: Write HANDOFF.md at the project root, a short note the user pastes into a fresh session to carry the work on. Use when the user asks for a handoff, is about to clear the chat or start a new session, or asks to pick up from HANDOFF.md.
---

# Write a handoff

Long sessions get worse as they grow, so the user clears the chat or opens a new
session, in any harness, and pastes this note in. The next agent has none of this
conversation: the note is all it gets. There is one note per project, and it is
replaced every time.

## Write

1. **Find the place.** The project root is `git rev-parse --show-toplevel`; outside
   git, use the working directory. Overwrite `HANDOFF.md` there. Never append, and
   don't keep old notes: a stale one is worse than none.
2. **Keep it out of git** without touching the project's shared files. If
   `git check-ignore -q HANDOFF.md` fails, add a `HANDOFF.md` line to the file
   `git rev-parse --git-path info/exclude` prints. That is git's local ignore list,
   never committed, and the command finds it in a worktree too. Don't edit
   `.gitignore`.
3. **Fill in the template.** Aim for one screen; being short is what makes it
   useful. Drop a section that has nothing in it.

        # Handoff: <project>, <the task in one line>

        Written <YYYY-MM-DD> by a <harness> session that has since been cleared.
        You have none of that conversation. Trust the code and git over this note,
        and check a claim before relying on it.

        ## Goal
        ## Where things stand
        Branch, last commits, uncommitted files, and tests: the command run and
        what it actually printed.
        ## Done and verified
        ## Not verified or still open
        ## Next step
        ## Decisions the user made
        Only ones the repository doesn't already record, each with its reason.
        ## Dead ends
        What was tried and why it failed, so it isn't tried again.
        ## More detail
        The previous session is in taskrunner's archive: `lookup-session` lists
        recent sessions (or run `taskrunner sessions --project <path>`). Read one
        exchange at a time.

4. **Only what can be checked.** State what a command confirms and mark the rest
   unverified. Point at files and commits instead of pasting code or diffs. Never
   write secrets or tokens into it.
5. **Tell the user** the path and that it is ready to paste. Don't commit it.

## Pick up

When the user pastes a handoff or asks to pick up from `HANDOFF.md`: read it, check
"Where things stand" against `git status` and `git log`, and say what has changed
since it was written. Then say the next step before acting on it.
