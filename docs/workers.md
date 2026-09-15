# Custom & local-model workers

Codex and Claude are built in, but a worker is just a bit of configuration — adding
one is never a code change. Any `[worker.<name>]` section in your `config.toml`
becomes a worker you can delegate to.

Each worker names a **harness** — the built-in loop that drives it. A new worker
usually reuses an existing harness (`codex` or `claude`) with different settings.

Custom workers inherit the same container resource limits as the built-ins
(memory 4g, cpus 2, pids 512); add a `[worker.<name>.limits]` section to override
them — see [Configuration](configuration.md).

## Another cloud model

To hand tasks to a different model on the same service, name the harness and the
model:

```toml
[worker.luna]
harness = "codex"
model = "gpt-5.6-luna"
```

It shares the built-in worker's sign-in, image and network allowlist, so there is
nothing else to set: signing `codex` in once covers both. Ask your agent to delegate
with `worker: "luna"`. Set `auth_volume` or `allowed_domains` in the section to give
it its own.

A worker picks the model, not the job. For a job you hand out often, such as a docs
librarian, write a skill (see [Your own skills](configuration.md#your-own-skills)) and
have it say which worker to delegate to. A worker sees only the project, not your
skills, so rules a delegated task must follow belong in the project's `AGENTS.md` or
`CLAUDE.md`, or in the prompt.

## A local, offline model

You can run a worker against a model on your own machine, with no login and no
internet. The container's only route out is the firewall, which forwards a single
port to the model server running on your host:

```toml
[worker.qwen]
harness = "codex"
provider = "ollama"                # or "lmstudio"
model = "qwen2.5-coder:32b"
allowed_domains = ["host.docker.internal:11434"]
```

Because it names a `provider`, it inherits nothing from the codex worker: no sign-in
and no network access beyond the port it lists.

To use it:

1. Install [Ollama](https://ollama.com) on your machine.
2. `ollama pull qwen2.5-coder:32b` (or whichever model you named).
3. Ask your agent to delegate with `worker: "qwen"`.

Trying a different model is just another `[worker.<name>]` section. And because every
turn's worker is recorded in the archive, you can always see which worker — and so
which model — produced any given result.

See also [Network access](network-access.md) for how the single-port forwarding to a
local server works, and [Configuration](configuration.md) for the full settings list.
