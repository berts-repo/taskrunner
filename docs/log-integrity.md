# Proving the record hasn't changed

Taskrunner keeps a permanent record of what your agents and workers did. A record is
only worth something as evidence if you can show nobody has altered it since. This page
explains how Taskrunner makes any change to the record detectable, and the two commands
you use to check.

## The short version

- Every entry in the record is linked to **all the entries before it** by a
  fingerprint.
- Edit, delete or reorder anything, and the fingerprints stop matching.
- `taskrunner verify` checks them. `taskrunner anchor` gives you a fingerprint to keep
  somewhere safe, so you can prove the record later even if someone rewrote it
  carefully.

You don't have to do anything for the basic protection: it is on from the moment
Taskrunner starts.

## How it works

### Fingerprints

A fingerprint is a short code computed from some data (Taskrunner uses SHA-256). Three
properties make it useful:

- The same data always gives the same fingerprint.
- Changing even one character gives a completely different fingerprint.
- Nobody can work backwards, or craft different data that gives a chosen fingerprint.

### The chain

Every entry's fingerprint is computed from **the entry itself plus the fingerprint of
the entry before it**:

```
fingerprint 1    = fingerprint of (entry 1)
fingerprint 2    = fingerprint of (entry 2 + fingerprint 1)
fingerprint 3    = fingerprint of (entry 3 + fingerprint 2)
…
fingerprint 8612 = fingerprint of (entry 8612 + fingerprint 8611)
```

Think of numbered receipts where each receipt also prints the code of the receipt before
it. Fingerprint 8,612 depends on every entry from 1 to 8,612 — change any of them and it
changes — and on nothing written after. New work only adds entries at the end, so it
never disturbs the fingerprints of what came before.

Each entry stores the fingerprint before it. If someone edits entry 500, entry 501 no
longer points at the right fingerprint, and `taskrunner verify` reports the break.

### Why the chain needs anchors

A careful person could edit entry 500 and then **recompute every fingerprint after
it**. The chain would look perfect again.

The defence is to keep a copy of a fingerprint somewhere else — an **anchor**. The
rewritten record can't reproduce the fingerprint you kept, so the rewrite shows.

## Three layers of protection

| Layer | Who does it | What it catches |
|---|---|---|
| **The chain** | Taskrunner, on every entry | Careless edits, deletions and reordering |
| **Automatic anchors** | Taskrunner, in `~/.taskrunner/anchors.jsonl` | Bugs, crashes, accidental edits, and entries removed from the end — around the clock |
| **Your own anchors** | You, by running `taskrunner anchor` and saving the line off this machine | Everything up to the moment you saved it, even against someone with full control of this computer |

The automatic anchors live on the same computer as the record, so someone able to rewrite
one can usually rewrite the other. [Making the automatic anchors harder to
tamper with](#making-the-automatic-anchors-harder-to-tamper-with-optional) closes most
of that gap; an anchor you keep elsewhere closes all of it.

## Checking the record

```sh
taskrunner verify
```

When nothing has changed:

```
Event log: 11612 events, chained from event 8479, 37 anchors checked
Verified: the chain is intact and every anchor matches.
```

When something has:

```
Event log: 11612 events, chained from event 8479, 37 anchors checked
NOT verified:
  - the anchor for event 9100 (2026-09-17T14:02:11.408Z) no longer matches: history up to that event changed
```

On a record that no version of Taskrunner with this feature has opened yet, there is
nothing to compare against, and `verify` says so rather than claiming a check it
couldn't make:

```
Event log: 8588 events, not chained yet, 0 anchors checked
Nothing to check against yet: links and anchors start the next time the daemon opens the log.
```

`verify` exits with 0 when the record checks out and 1 when it doesn't, so it can run
from a script. It reads the files directly, so the daemon doesn't need to be running.
`taskrunner doctor` does not run it for you; run it when you want to know.

## Keeping your own anchor

1. Run:

   ```sh
   taskrunner anchor
   ```

   It prints one line:

   ```
   event 8612  2026-09-15T06:40:12.818Z  sha256:9f3c…e71a
   ```

2. Save that line somewhere off this computer: a USB stick, a password manager, an
   email to yourself, a notebook. It contains no conversation content, so it is safe to
   store anywhere.

3. Later, check the record against it:

   ```sh
   taskrunner verify --anchor "event 8612  2026-09-15T06:40:12.818Z  sha256:9f3c…e71a"
   ```

   Quote the whole line. Pasting only the `sha256:…` part also works.

### An anchor never expires

An anchor you saved on Monday still proves Monday's record months later, however much
work you do afterwards. What it doesn't cover is work done *after* you saved it:

| What happened after you saved an anchor at entry 8,612 | Caught by that anchor? |
|---|---|
| Someone edits an entry from last month | ✅ Yes |
| Someone edits an entry from earlier that same day | ✅ Yes |
| Someone edits work from two days later and recomputes the chain after it | ❌ No — that work came after your anchor |

So keep one file of anchors and add a line each time:

```
event 8612   2026-09-15  sha256:9f3c…
event 11612  2026-09-19  sha256:a41b…
```

Each line proves everything up to its date; your newest line covers the most. A good
habit is to add a line at the end of any session you might want to prove later. The
more often you do, the less recent work is left relying on this computer alone.

## Making the automatic anchors harder to tamper with (optional)

On Linux you can mark the anchors file **append-only**. After that, nothing running as
you — not the daemon, not your own shell, not a program acting as you — can edit,
shorten or delete it; it can only grow. Changing that takes `sudo`.

After Taskrunner has started once (so the file exists):

```sh
sudo chattr +a ~/.taskrunner/anchors.jsonl
lsattr ~/.taskrunner/anchors.jsonl        # shows an "a" in the flags
```

- ✅ Someone who only has your user account can no longer rewrite the record *and* its
  anchors to match.
- ❌ Deleting `~/.taskrunner` later fails with "Operation not permitted" until you run
  `sudo chattr -a ~/.taskrunner/anchors.jsonl`.
- ❌ It does not stop someone with root access, who can remove the flag. Only an anchor
  kept off the computer protects against that.

## What this does not do

- **It reveals changes; it does not prevent them.** Anyone who can write to your files
  can still change the record. They just can't do it without `verify` noticing.
- **It proves the record hasn't changed since it was written, not that it was true when
  written.** A worker that lies in its own transcript is recorded faithfully lying.
- **Someone with root access on this computer can rewrite the record and every local
  anchor.** Only an anchor you saved elsewhere catches that.

## If `verify` fails

1. **Don't repair or delete anything yet.** Copy `~/.taskrunner` somewhere safe first —
   it is the evidence.
2. Read the message: it names the first entry that no longer checks out, which tells you
   how far back the change reaches.
3. Compare with a backup, or with anchors you saved, to narrow down when it happened.

Two failures have innocent causes:

- **"line N of the anchors file is not an anchor"** can follow a crash in the instant an
  anchor was being written. Taskrunner starts a fresh line after it, so nothing later is
  affected; the torn line keeps being reported until you remove it (after `sudo chattr -a`
  if you set the flag).
- **"events were removed"** also happens if a damaged line appears in the middle of the
  record: when the daemon starts, it cuts the record at the first line it can't read. That
  is still a real loss worth investigating — the anchor is doing its job by reporting it.

---

## Technical details

### The fingerprint

Lines are hashed as the exact bytes on disk, without the trailing newline, so nothing
depends on how JSON is formatted or re-serialized.

```
F(1) = SHA-256(line 1)
F(n) = SHA-256(F(n−1) ‖ line n)        for n > 1
```

`F(n−1)` enters the hash as its text form, `sha256:` followed by 64 lowercase hex
digits — the same text stored in `prev` and in anchors.

### The `prev` field

Every line written since chaining started carries `"prev":"sha256:…"`, the fingerprint
of everything before it. The first line of an empty log has nothing before it and no
`prev`. The field describes the line's place in the file, so it is written and checked
by the log layer only: `LogEvent` does not have it, and the index never sees it.

### History written before chaining

Lines without `prev` that come before the first line with one are hashed exactly like any
other, so the first linked line's `prev` already depends on all of them. There is no
separate genesis record. When the daemon first opens such a log, it anchors it
immediately, so that history has an anchor from the start.

### The anchors file

`~/.taskrunner/anchors.jsonl`, one JSON object per line:

```json
{"event":8612,"id":"evt_…","ts":"2026-09-15T06:40:12.818Z","fingerprint":"sha256:…"}
```

`event` counts lines from 1; `id` and `ts` are that event's own. An anchor is appended:

- when the log is opened, unless its last event is already the latest anchor;
- after every 100 durable events — an fsynced `append`, or a `flush` after bulk
  `append_unsynced` writes, so an anchor never covers an event that could still be lost;
- when the daemon stops, after a final flush.

The file is opened append-only and fsynced after each anchor, so it works under
`chattr +a`. If its last line is torn, the next anchor begins on a new line. A failed
anchor write is reported on stderr and never fails the event write: the event is already
safe, and the next anchor covers it too.

### What `verify` checks

One streaming pass over the log:

1. An unterminated last line is a torn write, not an event, and is ignored.
2. Every line's `F(n)` is computed. A line whose `prev` differs from `F(n−1)` is a
   **wrong link**; a line without `prev` after the chain has started is **unlinked**.
   The first such line is reported.
3. Every anchor is compared with `F` at its event. An anchor past the end of the log
   means events were removed. Unreadable anchor lines are reported.
4. A saved anchor given with `--anchor` is compared at its event number, or, if only a
   fingerprint was given, looked for at every position.

`taskrunner anchor` and `taskrunner verify` read `events.jsonl` and `anchors.jsonl`
directly rather than asking the daemon, so they work while it is stopped.

### Costs

One `prev` field per line: 81 bytes, against lines that average about 2 KB in a real
archive (most are conversation messages), so around 4%. Opening the log and running
`verify` hash the whole file once. An 18 MB log verifies in about 0.04 seconds, so a log
of a few hundred megabytes takes around a second.

### Relation to the startup repair

`EventLog::open` truncates the log at the first line that doesn't parse, which exists to
discard a torn tail after a crash but also cuts after a corrupt line in the middle.
Chaining does not change that behaviour; it makes the loss visible, because every anchor
past the new end reports removed events.
