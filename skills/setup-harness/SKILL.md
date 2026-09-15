---
name: setup-harness
description: Connect an agent harness on this machine (Claude Code, Codex, or Hermes) to taskrunner so it gets taskrunner's tools and skills, and walk the user through signing it in. Use when the user installs or sets up a harness, asks to connect one, or a harness is missing taskrunner's tools or skills.
---

# Set up a harness for taskrunner

All of this is `taskrunner sync` and `taskrunner doctor`, which the user can run
without you. This skill drives those commands and handles the parts a command
can't: asking the user, and handing over sign-ins.

## 1. Ask

Ask which harness (`claude`, `codex` or `hermes`), and whether its agent should
**offer** to delegate when a task fits (`suggest`) or delegate **only when asked**
(`on-request`). Don't pick for the user.

## 2. Sync

    taskrunner sync --connect <harness> --delegation <suggest|on-request>

- For a harness the user doesn't want connected: `taskrunner sync --skip <harness>`,
  so sync stops asking about it.
- Sync prints every change it makes: it registers taskrunner with the harness
  (`taskrunner mcp --host <harness>`), writes the skills, links them into the
  harness's skills folder, and records the answers under `[host.<harness>]` in
  `~/.taskrunner/config.toml`.
- To change the delegation setting later, edit that line in `config.toml` and run
  `taskrunner sync` again.

## 3. Sign-in

If sync reports the harness isn't signed in, it skips it and prints the login
command. Signing in opens a browser or asks for a code, so only the user can
finish it. Give them the exact command for their own terminal, wait for them, then
run sync again.

- Claude Code: `claude auth login`
- Codex: `codex login` (without a browser on this machine: `codex login --device-auth`)
- Hermes: `hermes auth add <provider>`, where the provider is `model.provider` in
  Hermes's `config.yaml`

Never copy credentials from one harness to another, or into a worker's volume:
two programs sharing one refresh token keep signing each other out. Docker
workers sign in separately — that is the `worker-login` skill.

## 4. Hermes's config

Taskrunner never rewrites Hermes's config file, because it holds the user's own
comments and settings. When sync says the Hermes entries are missing, it prints
them. Add them to `config.yaml` in the Hermes home (`$HERMES_HOME`, or `~/.hermes`)
with your edit tool, so the user sees the diff. Merge into an existing
`mcp_servers:` or `skills:` key instead of adding a second one — YAML keeps only
one of them. Then run sync again to confirm.

## 5. Check

Run `taskrunner doctor`. Its skills lines show each harness: connected, signed in,
registered, skills in place. Tell the user what is done and what is left. A
harness sees new skills in its next session.
