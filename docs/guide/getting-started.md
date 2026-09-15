# Getting started

## Requirements

- **Rust** (stable, via [rustup](https://rustup.rs)) — to build Taskrunner
- **Docker** (running)
- **Node 22+** — only to run the test suite

## Install and build

```sh
cargo build --release        # compile Taskrunner: one binary, target/release/taskrunner
sh scripts/build-images.sh   # build the worker and firewall Docker images
ln -sfn "$PWD/target/release/taskrunner" ~/.local/bin/taskrunner   # put it on your PATH
```

## Connect your agents

```sh
taskrunner sync
```

`sync` sets up each agent it finds on your machine — Claude Code, Codex and Hermes —
once:

1. **Asks** whether to connect it, and whether its agent should *offer* to delegate a
   task when one fits (`suggest`) or delegate *only when you ask* (`on-request`). The
   answers go under `[host.<name>]` in `config.toml` (see
   [Configuration](configuration.md#harnesses)), so it never asks twice.
2. **Checks it is signed in.** If it isn't, sync prints the login command
   (`claude auth login`, `codex login`, or `hermes auth add <provider>`) and skips that
   agent; run sync again once you've signed in.
3. **Registers Taskrunner** with it as `taskrunner mcp --host <name>`, through the
   agent's own `mcp add` command.
4. **Gives it Taskrunner's skills** — how to delegate a task and review what comes
   back, search the archive, sign a worker in, and set up another agent. They're written
   under `~/.taskrunner/skills/` and linked into the agent's skills folder. Skills of
   your own, from the folders listed under `[skills]`, are linked the same way (see
   [Configuration](configuration.md#your-own-skills)).

Hermes is the exception to the last two: Taskrunner never edits Hermes's config file,
so sync prints the lines to add to it instead.

Run sync again whenever you install another agent. It's safe to repeat and prints what
it changed. Where there's no terminal to ask in — an agent running it for you — pass
the answers as flags: `taskrunner sync --connect claude --delegation suggest`, or
`--skip hermes`. Once one agent is connected, it can set up the others with its
**setup-harness** skill.

Agents load their MCP servers and skills when a session opens, so a session that was
already running won't see Taskrunner until you restart it.

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
[implementation notes](../reference/internals.md#worker-sign-in).

## Check everything is ready

```sh
taskrunner doctor
```

`doctor` is a read-only preflight: it checks Docker, the worker images and their
logins, the firewall image, each connected agent (signed in, registered, skills in
place), and the archive's health — and tells you exactly what's
missing. Run it whenever a worker won't start.

Other commands: `taskrunner up | down | status | doctor | sync | wait | mcp`, plus the read-only
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
