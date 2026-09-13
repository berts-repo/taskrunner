# Network access

Every task runs behind a firewall. A worker can only reach the domains on its
allowlist — nothing else gets out, and every attempt (allowed or blocked) is recorded.

## The default: locked down

Out of the box a worker's only reach is its own vendor's API — codex can talk to
OpenAI, claude to Anthropic, and that's it. A task **cannot** browse the web, install
packages, or call other services unless you allow it. This is deliberate: delegated
work shouldn't be able to phone home or pull in arbitrary code by default.

## Granting more reach

To let a task reach further, your agent adds domains when it delegates. Because that
widens what the task can do, it becomes a **networked** task, which needs your
explicit go-ahead in the conversation — your agent asks, you say yes, and that
approval is recorded in the archive.

- Add specific domains (for example a package registry) to allow just those.
- Use `"*"` to grant the whole public internet.

## What can never be reached

No matter what the allowlist says, the firewall resolves every destination itself and
**refuses your local network** — your machine's loopback, your LAN, and other private
addresses stay blocked. An allowed domain can't be used as a backdoor to something
local.

If you genuinely need a task to reach something local (say a model server on your own
machine), you pin it explicitly — either by IP and port (`127.0.0.1:8080`) or by
Docker host name (`host.docker.internal:11434`). Entries without a port — including
`"*"` — cover only the standard web ports 80 and 443.

Every connection attempt lands in the audit trail, so you can always see exactly what
a task tried to reach. The mechanism behind the local-network refusal is described in
the [implementation notes](internals.md#network-firewall).
