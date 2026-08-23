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

Requires Node 22+ and Docker.

```sh
npm install
npm run build
npm run build:images
claude mcp add --scope user taskrunner -- node /path/to/taskrunner/dist/cli.js mcp
```

Then sign each worker in once (see [Getting started](docs/getting-started.md)) and
ask your agent to delegate something. Full walkthrough, including the one-time worker
login, is in the getting-started guide.

## Documentation

- **[Getting started](docs/getting-started.md)** — install, build the worker images,
  sign workers in, register with your agent, and run the health check.
- **[Using Taskrunner](docs/tools.md)** — the tools your agent uses to delegate,
  follow up, inspect, cancel, browse sessions, and search.
- **[Configuration](docs/configuration.md)** — the optional `config.toml` and every
  setting with its default.
- **[Network access](docs/network-access.md)** — how the firewall works and how to
  grant a task more reach.
- **[Security](docs/security.md)** — what isolates a task, what it can still do, and
  what gets recorded.
- **[Conversation archive](docs/transcripts.md)** — what Taskrunner records, how to
  read it back, and how worker logins are stored.
- **[Custom & local-model workers](docs/workers.md)** — add your own worker, including
  an offline local model.

For maintainers, the design rationale and internals live in
[docs/archive/implementation-notes.md](docs/archive/implementation-notes.md).
