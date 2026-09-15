# Outcome

**Shipped** 2026-09-15.

- `d86781f`: every line the log writes records `prev`, the fingerprint of every line
  before it. The daemon appends automatic anchors to `anchors.jsonl` when the log
  opens, every 100 durable events and when it stops; the file is only appended to, so
  it works under `chattr +a`. `taskrunner verify [--anchor "<saved line>"]` checks the
  chain, every automatic anchor and one the user kept (exit 0 verified, 1 not), and
  says when there is nothing to check yet. `taskrunner anchor` prints the current
  fingerprint to keep off the machine.
- `17e7852`: the daemon refuses to start on a damaged log instead of cutting it.
  Before deploying, every one of the live log's 8,700 lines was checked to parse.

Described in:

- [Proving the record hasn't changed](../../../guide/log-integrity.md), including
  [If the daemon refuses to start](../../../guide/log-integrity.md#if-the-daemon-refuses-to-start)
  and [History written before chaining](../../../guide/log-integrity.md#history-written-before-chaining).
- [Security](../../../security/overview.md) and [internals](../../../reference/internals.md).

Not built: the retention tiers the same proposal section planned (hot index, cold
compressed segments, raw capture bodies); they remain in the
[complete-audit proposal](../../proposals/complete-audit.md).
