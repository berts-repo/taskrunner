# Rust port

Taskrunner moved from TypeScript to Rust as the same program, before the
complete-audit redesign started. Decided 2026-09-13, closed 2026-09-14.

Retroactive package: the plan lived as a section of the complete-audit proposal
(added in `6ea7e1e`, stepped out in `a278350`) and was retired when the port closed
in `d129513`. The full plan, with per-step findings, is in
`git show d129513^:docs/proposals/complete-audit.md`.

- [decisions.md](decisions.md) — why port first, and what "same program" froze.
- [outcome.md](outcome.md) — what shipped, how parity was checked, what it left open.

Current behaviour: [internals](../../../reference/internals.md).
