# Next work

1. **Hermes parser** — Read Hermes's `~/.hermes/state.db` messages into the
   taskrunner archive and deduplicate them by deterministic message id; see the
   [complete-audit proposal](proposals/complete-audit.md).
   **In scope:** the read-only SQLite/FTS5 `messages`-table ingest slice.
   **Behind:** wire capture, redaction, per-host settings, and retention.
2. **Session handles** — Let `taskrunner session <prefix>` resolve an unambiguous
   short session id and support the `last` word; see the
   [session-handles proposal](proposals/session-handles.md).
   **In scope:** terminal session lookup while preserving the existing views and
   prompt selection. **Behind:** numbered list positions and MCP tool-schema changes.
3. **Fix the README § Worker sign-in pointers** — Update the stale references in
   `src/doctor.rs`, `src/workers/runner.rs`, and `src/harnesses.rs` to point to
   [Sign the workers in](../guide/getting-started.md#sign-the-workers-in); see
   [REVISIT.md](REVISIT.md).
   **In scope:** those documentation pointers only. **Behind:** changes to worker
   authentication or sign-in behavior.
