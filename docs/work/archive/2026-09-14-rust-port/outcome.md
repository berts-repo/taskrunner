# Outcome

**Shipped.** Steps 0 to 7 landed on 2026-09-13 (`7fabb81` through `99ee102`). The Rust
binary was the daily driver from then on, running real delegated tasks. On 2026-09-14
`d129513` deleted the TypeScript source, its tests, the npm toolchain and the parity
scripts, and moved the crate to the repository root.

## How "same program" was checked

Each layer was compared against the TypeScript implementation over a corpus built from
real host transcripts plus tasks driven through the real scheduler:

- **Index:** both implementations folded the same log; `sqlite3` dumps of every table
  diffed empty.
- **Views:** 58 rendered views (session lists, outlines, compact and timeline reads,
  searches, task lookups) were byte-identical, 587 KB.
- **Ingest:** both sweepers over a snapshot of the host transcript folders produced
  2,560 identical events in the same order.
- **Config:** six sample files, including two that must fail, loaded identically.

## What the checks surfaced

- File order differed: `Array.sort()` compares whole path strings, `PathBuf` compares
  components. Fixed to match.
- JSON Schema drafts differ: `schemars` emits draft 2020-12 where zod emitted draft-07.
  Accepted as a known step-7 difference.
- rmcp over a unix socket needs `allowed_hosts` set, because its DNS-rebinding guard
  rejects a non-loopback `Host`.

## Kept rather than deleted

- `tests/fixtures/`, which the Rust tests read.
- The egress proxy's 18 tests, rewritten on `node:test` beside the proxy
  (`docker/egress-proxy/server.test.cjs`) and run by `cargo test`.

## Left open

- The log was not hash-chained; that shipped on 2026-09-15.
- The egress proxy is still the Node sidecar. Replacing it is part of the
  [complete-audit proposal](../../proposals/complete-audit.md).
