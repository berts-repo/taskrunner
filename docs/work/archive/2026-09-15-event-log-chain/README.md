# Event log chain

The event log became hash-chained and anchored, with `taskrunner verify` and
`taskrunner anchor`, and the daemon stopped cutting a damaged log. Decided
2026-09-13 in the complete-audit proposal (`432b6c4`), shipped 2026-09-15 in
`d86781f` and `17e7852`.

Retroactive package: the decision was a section of the proposal, retired in `d86781f`.

- [decisions.md](decisions.md) — why a chain, why anchors, and how history written
  before chaining is covered.
- [outcome.md](outcome.md) — what shipped and where it is described.

Current behaviour: [Proving the record hasn't changed](../../../guide/log-integrity.md).
