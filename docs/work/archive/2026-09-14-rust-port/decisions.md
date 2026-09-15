# Decisions

## Port before the redesign (2026-09-13)

The redesign is additive around a core that survives: log and index, scheduler,
runner, harnesses, parsers, MCP tools and CLI all stay; only the egress proxy is
replaced. Porting the core first means the new pieces are written once, in the final
language, on a floor already proven equal to the old system. Rust is also the
stronger tool for the hardest new piece, a TLS-terminating proxy (`rustls`, `hyper`,
`rcgen`), and ships as one binary.

## A port means the same program

Same commands, same files on disk, same behaviour; not better, not redesigned. That is
what made it checkable. Frozen during the port:

- `~/.taskrunner/events.jsonl`: the existing archive must load unchanged.
- `config.toml`: same keys, same defaults.
- MCP tool names and arguments; CLI commands and output.
- Test fixtures (sample Claude and Codex transcripts), reused as-is.
- Docker images: they run `claude` and `codex`, not taskrunner.

The shim-to-daemon transport was internal and not frozen.

## Order and scope

- Bottom up, one module per step, each green before the next: storage, ingest,
  config and paths, daemon and shim, runner and harnesses, scheduler, then the MCP
  tools and CLI.
- The Node egress proxy (`docker/egress-proxy/server.cjs`) was not ported: it runs in
  its own image and is to be replaced by the redesign's proxy.
- HTTP stays on the unix socket, and the shim stays a plain forwarder (spike, step 0).
- Hash-chaining the log was meant to start with the port. Formats were frozen, so
  the port shipped without it; see [Event log chain](../2026-09-15-event-log-chain/README.md).
