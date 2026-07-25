# Getting started

## Requirements

- **Node 22+**
- **Docker** (running)

## Install and build

```sh
npm install
npm run build           # compile Taskrunner
npm run build:images    # build the worker and firewall Docker images
```

## Register with your agent

Point your MCP client at Taskrunner's `mcp` command. For Claude Code:

```sh
claude mcp add --scope user taskrunner -- node /path/to/taskrunner/dist/cli.js mcp
```

Taskrunner runs as a background daemon that starts on demand and keeps all its state
under `~/.taskrunner/`. You don't start it yourself — your agent's first request
launches it.

## Sign the workers in

A worker signs in **once**, into its own private storage — never from your personal
`~/.codex` or `~/.claude` (sharing those would make your host and the container fight
over the same login). Build the images first, then run each login:

```sh
# Codex — approve the device code it prints.
docker run -it --rm -v taskrunner-codex-home:/home/worker/.codex \
  taskrunner/codex-worker codex login --device-auth

# Claude — open the URL it prints and paste the code back.
docker run -it --rm -v taskrunner-claude-home:/home/worker \
  taskrunner/claude-worker claude /login
```

Approve in the browser and the credentials land in the worker's storage. You only
repeat this when a login expires or you set up a new machine.

**Use exactly the volume paths shown above** (`~/.codex` for codex, the whole home
for claude). Logging in at a different path hides the credentials where the worker
can't find them at run time, which shows up later as `401 Missing bearer` errors.
The reason for these specific paths is in the
[implementation notes](archive/implementation-notes.md#worker-sign-in).

## Check everything is ready

```sh
taskrunner doctor
```

`doctor` is a read-only preflight: it checks Docker, the worker images and their
logins, the firewall image, and the archive's health — and tells you exactly what's
missing. Run it whenever a worker won't start.

Other commands: `taskrunner up | down | status | doctor | mcp`. Add
`--state-root <dir>` to point at a different state directory.

## Run the tests (optional)

```sh
npm test
```

There's also a live check that delegates a real task to codex (needs a codex login):

```sh
TASKRUNNER_LIVE_CODEX=1 npx vitest run tests/workers/integration.test.ts
```

Next: [Using Taskrunner](tools.md).
