# Taskrunner

Taskrunner lets your coding agent hand work to another AI worker that runs in a
safe, isolated sandbox — and keeps a durable, searchable record of everything it
did.

You ask your agent (Claude Code, Codex, or anything that speaks MCP) to delegate a
task. Taskrunner runs that task in a throwaway Docker container over a private copy
of your project, behind a network firewall that blocks everything by default. When
it's done you get the result, the file changes, and a full transcript of what the
worker actually did — all stored locally on your machine.

## Why use it

- **Delegate without watching.** Kick off a task and get back to your own work;
  check the result whenever you like.
- **Safe by default.** Each task runs in its own container over a private git clone,
  with no network access beyond the worker's own vendor API unless you approve more.
- **Nothing is lost.** Every task, file change, and worker conversation is saved to a
  permanent local archive you can search later — even after the original tool would
  have deleted its own history.
- **Bring your own workers.** Codex and Claude are built in; adding another (including
  a fully offline local model) is a few lines of config, not code.

## Quick start

Requires a Rust toolchain ([rustup](https://rustup.rs)) and Docker.

```sh
cargo build --release
sh scripts/build-images.sh
ln -sfn "$PWD/target/release/taskrunner" ~/.local/bin/taskrunner
taskrunner sync
```

`taskrunner sync` connects the agents on your machine — Claude Code, Codex, Hermes —
to Taskrunner and gives them its skills, asking once about each. Then sign each worker
in once (see [Getting started](docs/guide/getting-started.md)) and ask your agent to
delegate something. Full walkthrough, including the one-time worker
login, is in the getting-started guide.

## Documentation

- **[Getting started](docs/guide/getting-started.md)** — install, build the worker
  images, connect your agents, sign workers in, and run the health check.
- **[Using Taskrunner](docs/guide/tools.md)** — the tools your agent uses to delegate,
  follow up, inspect, cancel, browse sessions, and search.
- **[Configuration](docs/guide/configuration.md)** — the optional `config.toml` and
  every setting with its default.
- **[Network access](docs/guide/network-access.md)** — how the firewall works and how
  to grant a task more reach.
- **[Security](docs/security/overview.md)** — what isolates a task, what it can still
  do, and what gets recorded.
- **[Conversation archive](docs/guide/transcripts.md)** — what Taskrunner records, how
  to read it back, and how worker logins are stored.
- **[Proving the record hasn't changed](docs/guide/log-integrity.md)** — how the record
  is chained and anchored, and how to check it with `taskrunner verify`.
- **[Custom & local-model workers](docs/guide/workers.md)** — add your own worker,
  including an offline local model.

Security investigations, such as what a real Codex task tried to reach on the
network, are in [docs/security/](docs/security/).

For maintainers: start at [CONTEXT.md](CONTEXT.md). How Taskrunner works is in
[docs/reference/internals.md](docs/reference/internals.md); work in flight, the
queue, design directions not built yet, and past work are under
[docs/work/](docs/work/).
