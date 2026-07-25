# Custom & local-model workers

Codex and Claude are built in, but a worker is just a bit of configuration — adding
one is never a code change. Any `[worker.<name>]` section in your `config.toml`
becomes a worker you can delegate to.

Each worker names a **harness** — the built-in loop that drives it. A new worker
usually reuses an existing harness (`codex` or `claude`) with different settings.

Custom workers inherit the same container resource limits as the built-ins
(memory 4g, cpus 2, pids 512); add a `[worker.<name>.limits]` section to override
them — see [Configuration](configuration.md).

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

To use it:

1. Install [Ollama](https://ollama.com) on your machine.
2. `ollama pull qwen2.5-coder:32b` (or whichever model you named).
3. Ask your agent to delegate with `worker: "qwen"`.

Trying a different model is just another `[worker.<name>]` section. And because every
turn's worker is recorded in the archive, you can always see which worker — and so
which model — produced any given result.

See also [Network access](network-access.md) for how the single-port forwarding to a
local server works, and [Configuration](configuration.md) for the full settings list.
