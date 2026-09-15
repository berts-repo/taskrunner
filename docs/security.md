# Security

Taskrunner hands your work to an AI worker that then runs commands on its own. This
page is the honest account of what contains that worker, what it can still do, and
what gets written down — so you can decide what you're comfortable delegating.

The short version: **a task is treated as untrusted code.** Everything below follows
from that.

## The boundary

A delegated turn runs inside a throwaway Docker container. Nothing about your machine
is visible to it except the one folder Taskrunner mounts.

- **A fresh container per turn.** It's created when the turn starts and destroyed when
  the turn ends, along with the private network it ran on. Nothing survives it except
  the files in the task's workspace.
- **Never as root.** The worker runs as an unprivileged user, and the container is
  started with privilege escalation switched off — so nothing the turn runs can gain
  root, even through a setuid binary. That user's numeric id rarely matches the id of
  the person running Taskrunner, so the task's clone (see below) is made writable by
  anyone before the container starts — safe because the clone is single-use, holds no
  secrets, and is discarded once the turn's changes land back in your repository.
- **Bounded resources.** Memory, CPU, and process count are capped (4 GB, 2 CPUs, 512
  processes by default; see [Configuration](configuration.md)). A runaway turn or a
  fork bomb hits a ceiling and Docker stops the container, instead of your machine
  grinding to a halt.
- **A time limit.** A single turn is stopped after 30 minutes by default.

## Your files

The worker never touches your actual project directory.

- **It gets a private clone.** Each task works in its own full copy of the repository,
  made without sharing any files with the original. That last part matters: an ordinary
  local clone shares its stored objects with the source repo, and a worker writing
  through one could corrupt your real history. Taskrunner never does that.
- **Only the clone is mounted.** The container sees `/workspace` and nothing else —
  not your home directory, not your other projects, not the real repository.
- **It starts from committed state.** A clone carries your commit history, but not your
  uncommitted edits or untracked files. Whatever hasn't been committed isn't there for
  the worker to see or wreck.
- **No route back.** The clone's link to your repository is removed, so the worker has
  nothing to push to.
- **Read back in a sandbox, never on your machine.** When the turn ends, Taskrunner
  still has to look at the clone: what changed, the diff, the new commits. Git obeys
  settings stored inside a repository, and some of those settings name a program for
  git to run — and the turn could have written them. So that inspection runs in its
  own throwaway container, with no network and nothing of your machine but the clone.
  Anything planted fires in there and dies with it; your machine only reads the plain
  files it produced.
- **Results come back for review.** Work the task commits is fetched into your
  repository from a bundle — a single file of git objects rather than a repository, so
  git reads it as data, with its integrity checks switched on. It lands on a branch
  named after the task: never merged, never rebased, never applied to your working
  tree — you look at it and decide.

Do note what this *doesn't* hide: a task can read the entire committed history of the
project you delegated. If a secret was ever committed to that repo, the worker can see
it.

## The network

Every task runs behind a firewall that denies by default. The container has no route to
the outside at all; its only way out is a proxy sidecar that forwards to allowlisted
domains and refuses everything else.

- **The default allowlist is the worker's own vendor API** — nothing more. A task can't
  browse the web, install packages, or call other services out of the box.
- **Widening it needs your yes.** When your agent asks for extra domains, that makes it
  a networked task, which Taskrunner refuses to start unless the agent confirms you
  approved it in conversation. That approval is written into the record.
- **Your local network is never reachable.** The proxy resolves every destination
  itself and refuses loopback, LAN, and other private addresses — so an allowed (or
  maliciously re-pointed) domain can't be used as a door to something on your machine.
  Deliberate local destinations have to be spelled out explicitly.
- **Every attempt is logged**, allowed or refused, with the host and port.

Full detail, including how to grant more reach, is in
[Network access](network-access.md).

## Worker logins

Each worker signs in once, into its own storage that only that worker uses.

- **Your own credentials are never mounted.** A worker authenticates from its own
  storage, not from your `~/.codex` or `~/.claude`. Beyond isolation this avoids a real
  bug: sharing those makes host and container sessions invalidate each other's tokens.
- **Only the login paths are mounted, not the whole home.** For the claude worker, that
  means exactly its two credential paths. A task therefore can't drop a shell startup
  file or other home-directory state into that storage for a later turn to pick up and
  execute.
- **Be aware:** the credentials are readable by the task while it runs, because both
  worker CLIs need to read and refresh them in place. A task can read the token it is
  running under. Scoping that away would take a credential broker, which Taskrunner
  doesn't have. Treat a worker login as something a delegated task has access to.

## The daemon on your machine

- **Nothing listens on the network.** The daemon accepts connections only over a local
  socket file — there's no port, so nothing off your machine can reach it.
- **File permissions are the access control.** That socket is an unauthenticated
  control channel, so it and the state root are owner-only. Anyone who can read your
  user account can drive the daemon; nobody else can.
- **Everything stays local.** Tasks, logs, transcripts, and the search index all live
  under `~/.taskrunner`. Taskrunner sends no telemetry and reports nothing anywhere.
- **The daemon itself is trusted code** running as you, with your access. The worker is
  the untrusted part, and it's the part in the container.
- **An agent's name on a connection is a label, not an identity.** Each agent is
  registered as `taskrunner mcp --host <name>`, so skills fit that agent and the audit
  trail says which one asked. The name is what the local registration says; it grants
  nothing.
- **Setting up an agent moves no credentials.** `taskrunner sync` asks each agent's own
  CLI whether it's signed in and to register Taskrunner, and for Hermes it only prints
  the lines to add — it never reads a login or edits another program's config file. The
  skill files it writes are read-only, so an agent that tidies its own skills can't
  quietly rewrite Taskrunner's.

## What gets written down

Taskrunner keeps a durable audit trail, which is a security feature in its own right —
after the fact, you can always establish what a task actually did.

- **Every tool call your agent makes**, with its arguments, and every request for
  Taskrunner's skills, with the agent that asked.
- **Every event inside a worker turn** — each command, edit, and message, streamed as
  it happens, so even a crashed turn keeps its partial trail.
- **Every network attempt**, allowed or refused.
- **Every network approval**, recorded as relayed by your agent.
- **The full conversation** of the turn, in the archive, along with the diff of what
  changed.

Read it back with the lookup tools or from the terminal — see
[Conversation archive](transcripts.md).

The flip side: transcripts are stored in plain text under your home directory. Whatever
appears in a conversation — including a secret a worker printed — is on disk until you
delete it.

## What this does not protect against

Worth knowing before you delegate something sensitive.

- **Inside its container, a task is unsandboxed on purpose.** The worker's own
  permission prompts are switched off, because a non-interactive turn would otherwise
  hang forever waiting on them, and the container is the boundary instead. Assume a task
  will run any command it likes *within* that container.
- **A container is a strong boundary, not a perfect one.** Container isolation depends
  on Docker and the kernel. On macOS, Docker Desktop's VM gives you a second layer; on
  Linux, the kernel is the layer.
- **An allowed domain is a trusted domain.** The firewall controls *where* a task can
  connect, not what it sends. Anything you allow — and especially `"*"` — is a path data
  can leave by. Grant the narrowest set that does the job.
- **A worker can be steered by what it reads.** If a task pulls in a web page, an issue,
  or a dependency containing instructions, the worker may follow them. This is why the
  defaults are what they are: the container and the empty allowlist bound how far a
  hijacked task can get.
- **Committed secrets are visible**, as above. So is anything reachable on an allowed
  domain with the worker's own credentials.

## Sensible practice

- Delegate from repositories that don't carry live secrets.
- Grant specific domains rather than `"*"`, and only for the task that needs them.
- Read the branch a task produces before merging it — that's what it's for.
- Check the audit trail after anything that touched the network.
