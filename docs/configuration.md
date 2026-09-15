# Configuration

Taskrunner works out of the box — **every setting has a default**. Configuration is
optional and only exists to override those defaults.

Config lives at `<state root>/config.toml`, which by default is
`~/.taskrunner/config.toml`. Create the file only if you want to change something.

Here are the settings, shown with their defaults:

```toml
[task]
turn_timeout_seconds = 1800   # how long a single turn may run before it's stopped

[worker.codex]                # built-in; [worker.claude] is the same shape
image = "taskrunner/codex-worker"
auth_volume = "taskrunner-codex-home"
allowed_domains = ["api.openai.com", "auth.openai.com", "chatgpt.com", "*.chatgpt.com"]

[worker.codex.limits]         # resource ceilings for this worker's container
memory = "4g"                 # Docker kills the container if it exceeds this
cpus = 2                      # fractional allowed, e.g. 1.5
pids = 512                    # cap on processes/threads (guards fork bombs)

[egress]
proxy_image = "taskrunner/egress-proxy"

[skills]
dirs = []                     # folders of your own agent skills — see below

[ingest]                      # the conversation archive — see docs/transcripts.md
interval_seconds = 300        # how often Taskrunner sweeps in new transcripts

[ingest.sources.claude-code]  # built-in; [ingest.sources.codex] is the same shape
format = "claude-code"        # which parser reads this source
dirs = ["~/.claude/projects"] # folders scanned for transcript files
```

The built-in claude worker defaults to
`["api.anthropic.com", "*.anthropic.com", "claude.ai", "platform.claude.com"]`, and
the built-in codex ingest source scans `["~/.codex/sessions"]`.

## Adding your own

- **A new worker** is any other `[worker.<name>]` section — see
  [Custom & local-model workers](workers.md). One that calls a cloud model inherits
  its harness's sign-in, image and API domains; one with a local `provider` inherits
  none of them.
- **Your own skills** live in folders listed under `[skills]` — see
  [Your own skills](#your-own-skills).
- **A new transcript source** is any other `[ingest.sources.<name>]` section; it needs
  a `format` naming a built-in parser (`claude-code` or `codex`) and the host `dirs`
  to scan.

## Harnesses

The agents you run Taskrunner *from* — Claude Code, Codex and Hermes — each get a
`[host.<name>]` section. `taskrunner sync` writes it the first time it meets that
agent; after that it's yours to edit:

```toml
[host.claude]
connected = true           # false: sync leaves this agent alone and stops asking
delegation = "suggest"     # "suggest": offer to delegate when a task fits, then wait
                           # for your yes. "on-request": only when you ask.
```

The names are `claude`, `codex` and `hermes`. After changing `delegation`, run
`taskrunner sync` to update that agent's skills; skills an agent fetches over MCP pick
the change up when the daemon restarts (`taskrunner down`).

## Your own skills

A skill you write yourself is a folder in the
[Agent Skills](https://agentskills.io/specification) format: a `SKILL.md`, plus any
`references/`, `scripts/` or `assets/`. Put your skill folders in one or more folders
and list them:

```toml
[skills]
dirs = ["~/skills"]           # each subfolder with a SKILL.md is one skill
```

`taskrunner sync` links each skill into every connected agent, next to Taskrunner's
own, and removes the link once the folder is gone. It skips a skill, and says why,
when its `name` doesn't match its folder, or the name is already taken by one of
Taskrunner's skills or a skill in an earlier folder: with two skills of one name, the
agent would be left to pick. Paths must be absolute or start with `~/`.

Your skills are linked, never served over MCP, and they don't reach workers. A worker
sees only the project, so put rules a delegated task must follow in the project's
`AGENTS.md` or `CLAUDE.md`, or in the prompt.

## Good to know

- **Every worker has resource limits.** The `[worker.<name>.limits]` ceilings
  (memory 4g, cpus 2, pids 512 by default) apply to built-in and custom workers
  alike, and to the container that reads a finished turn's work back. They bound a runaway turn — Docker stops the container at the limit
  instead of your machine grinding to a halt. Raise them for heavy builds, lower
  them for tighter isolation.
- **Typos fail loudly.** Unknown keys are rejected, not ignored — so a misspelled
  setting stops the config from loading instead of silently doing nothing.
- **Worker transcripts aren't configured here.** A source is always a set of *host*
  folders. The transcripts a worker writes inside its own storage are picked up
  automatically from that worker's settings, so there's nothing to wire up (and
  nothing that can fall out of sync). Details in the
  [conversation archive](transcripts.md).
