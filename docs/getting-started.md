# Getting started

## Requirements

- **Rust** (stable, via [rustup](https://rustup.rs)) — to build Taskrunner
- **Docker** (running)
- **Node 22+** — only to run the test suite

## Install and build

```sh
cargo build --release        # compile Taskrunner: one binary, target/release/taskrunner
sh scripts/build-images.sh   # build the worker and firewall Docker images
ln -s "$PWD/target/release/taskrunner" ~/.local/bin/taskrunner   # put it on your PATH
```

## Register with your agent

Point your MCP client at Taskrunner's `mcp` command. Each agent keeps its own list of
MCP servers, so register with every agent you use:

```sh
claude mcp add --scope user taskrunner -- "$HOME/.local/bin/taskrunner" mcp   # Claude Code
codex mcp add taskrunner -- "$HOME/.local/bin/taskrunner" mcp                 # Codex
```

Agents start their MCP servers when a session opens, so a session that was already
running won't see Taskrunner until you restart it.

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
[implementation notes](internals.md#worker-sign-in).

## Check everything is ready

```sh
taskrunner doctor
```

`doctor` is a read-only preflight: it checks Docker, the worker images and their
logins, the firewall image, and the archive's health — and tells you exactly what's
missing. Run it whenever a worker won't start.

Other commands: `taskrunner up | down | status | doctor | mcp`, plus the read-only
query commands `sessions | session | search | task | tasks` for reading the archive
from your shell (see [Conversation archive](transcripts.md#from-the-terminal)). Add
`--state-root <dir>` to point at a different state directory.

## Run the tests (optional)

```sh
cargo test
```

Node must be on your PATH: the stand-in codex and claude workers the tests drive, and
the firewall proxy with its tests, are Node scripts.

There's also a live check that runs a real worker container behind the real firewall
proxy (needs Docker and the images built):

```sh
TASKRUNNER_LIVE_DOCKER=1 cargo test --test workers docker
```

Next: [Using Taskrunner](tools.md).
