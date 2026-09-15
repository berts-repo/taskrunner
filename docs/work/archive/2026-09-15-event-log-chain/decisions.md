# Decisions

## The log is hash-chained (2026-09-13)

Append-only was a promise the code kept, not something the file proved. Each event
carries a SHA-256 fingerprint of what came before it, so editing, deleting or
reordering any past line breaks the chain where it happened, and one pass finds it.
The same mechanism as git commit parents and certificate-transparency logs. Cost: one
field per line, under 5% of the log.

The chain proves a record has not changed since it was written, not that it was true
when written.

## A chain needs anchors

On its own, a chain doesn't stop someone recomputing every link after an edit, or
cutting lines off the end. Both need the current fingerprint kept somewhere the log
writer can't reach: an anchor. Taskrunner writes automatic anchors; stronger ones (a
line saved off the machine, a friend's copy) are the user's choice.

## History written before chaining

The proposal left this open: chaining started after the Rust port, so the log already
held unchained lines, and re-hashing them later is exactly the act a chain exists to
make suspicious.

Resolved in `d86781f`: the first chained line's fingerprint already depends on every
line before it, so earlier history is covered without being rewritten.

## A damaged log stops the daemon (2026-09-15)

Opening the log used to cut it at the first line that didn't parse, which silently
discarded a damaged line in the middle and every event after it. Now a complete line
that isn't a valid event is an error naming the line, and the file is left untouched.
Only an unterminated last line, which never was a whole event, is still removed.
