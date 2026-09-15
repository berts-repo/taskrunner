# Outcome

**Shipped.**

- `13d6d01` (2026-09-14): four skills built into the binary (delegate-task,
  archive-search, worker-login, setup-harness), served over MCP with `skills/list`,
  `skills/get` and `skill://` resources, rendered per host. `taskrunner sync` connects
  each harness once, records `[host.<name>]` (`connected`, `delegation`), registers
  `taskrunner mcp --host <name>`, and writes and links the skills; Hermes gets the
  config lines to add. Links are dropped for a host seen fetching skills over MCP.
- `834bdb2` (2026-09-15): `[skills] dirs` links the user's own skills beside
  taskrunner's, refusing a name already taken, and keeps them linked for a host that
  gets taskrunner's skills over MCP. A custom worker without a local provider
  inherits its harness's image, login volume and API domains, so another cloud model
  is one `model` line.

Described in:

- [Getting started § Connect your agents](../../../guide/getting-started.md#connect-your-agents)
- [Configuration § Your own skills](../../../guide/configuration.md#your-own-skills)
- [Custom workers § Another cloud model](../../../guide/workers.md#another-cloud-model)
- [Using Taskrunner § How your agent knows all this](../../../guide/tools.md#how-your-agent-knows-all-this)

Not built: per-host skill extras, a shared `~/.agents/skills/` convention, and skills
inside worker containers (a worker sees only the project).
