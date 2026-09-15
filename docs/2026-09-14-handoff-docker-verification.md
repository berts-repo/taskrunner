# Handoff — taskrunner: verify the Docker-gated security tests

Written 2026-09-14 by the session that made the fixes. You have fresh context, so
trust the commits and the tests, not this summary. Everything below is either
checkable with a command or flagged as unverified.

---

## 1. Your task

Sudoless Docker is now available. Two tests in this repo are gated behind Docker and
**have never been run**. Run them, report what happens, and fix anything that fails.

```sh
cd /home/me/Projects/taskrunner
export PATH="$HOME/.cargo/bin:$PATH"

# The security test: the reason this branch exists.
TASKRUNNER_LIVE_DOCKER=1 cargo test --test workspace never_runs_commands_planted -- --nocapture

# The pre-existing live runner test (worker container behind the real egress proxy).
TASKRUNNER_LIVE_DOCKER=1 cargo test --test workers docker -- --nocapture
```

Both need the worker images built: `sh scripts/build-images.sh`.

---

## 2. Where things stand

- Repo: `/home/me/Projects/taskrunner`, branch **`fix-worker-git-escape`**, 5 commits
  ahead of `main`. Working tree clean apart from untracked
  `docs/codex_artifact_telemetry.html` (the user's saved web page, not ours — leave it).
- `cargo test` is **196 passing, 0 failing**; `cargo clippy --all-targets` is clean.
- The project is Rust-only now. The TypeScript implementation was deleted on this
  branch and the crate moved to the repo root.

```
57e9dc1  Keep watching cancel while a worker exits
aea1960  Inspect a task workspace in a container, never on the host
099f4d8  Keep CLI queries from panicking when the reader closes the pipe
d129513  Delete the TypeScript version; the Rust crate moves to the root
ca118f7  Name the harness that wrote each transcript reply
```

`main` still holds the TypeScript version, so `main` is the rollback.

---

## 3. What the security test is checking

The bug (found by a Codex bug-check pass, reproduced before fixing):

1. A task runs in a container over a private clone of the user's repo. The clone's
   `.git` directory is inside that container's writable mount.
2. Git obeys configuration stored in a repository, and several of those settings name
   a program for git to run: `diff.external`, `core.fsmonitor`,
   `uploadpack.packObjectsHook`, filter drivers reached via `.gitattributes`.
3. A worker can write `.git/config` — it's an ordinary file in its own workspace.
4. After the turn, taskrunner ran `git status`, `git diff HEAD`, `git rev-parse` in
   that clone **on the host, as the user**, and fetched from it.
5. Git then ran the planted program on the host: no container, no egress firewall,
   full access to the user's home, keys and other repos.

Confirmed on the old code: a planted `diff.external` fires on `git diff`, and a
planted `core.fsmonitor` fires on `git status`.

The fix (`aea1960`):

- All post-turn git is one script, `INSPECT_SCRIPT` in `src/workspace/git.rs`, run
  through the `WorkspaceGit` trait.
- Production is `ContainerGit`: `docker run --rm --network none`, mounting only the
  clone at `/workspace` and a host output directory at `/out`. Anything a planted
  config makes git run happens in there and dies with the container.
- The host reads only the four files the script writes — `status`, `diff`, `tip`,
  `bundle` — as untrusted data: size-capped, non-regular files (planted symlinks)
  ignored, and `tip` accepted only if it is a hex object id.
- Commits land by fetching the **bundle**, a file of git objects rather than a
  repository, so git consults no worker-written config, with
  `transfer.fsckObjects=true`.
- The base commit each clone started from is recorded in
  `<workspaces_dir>/<task_id>.base`, beside the clone rather than inside it, so a
  worker cannot edit which commits count as new.
- `HostGit` runs the same script directly on the host. **Test-only.** It is what
  `ContainerGit` exists to prevent; never wire it into the daemon.

---

## 4. Reading the result

**Pass** means: no marker file was created on the host, *and* the inspection still
worked (the test asserts `README.md` came back in the changed files).

Failure modes worth separating:

- **The marker exists** → the isolation is broken. Serious; stop and report before
  anything else.
- **Changed files came back empty / the container step errored** → the inspection
  didn't run. Not dangerous (it fails safe: no diff artifact, no branch landing,
  changed files fall back to what the worker reported), but it means production
  silently loses the diff and the task branch. Likely causes: no worker image built,
  the container user cannot read the clone, or git inside the container refusing the
  repository's ownership. `-c safe.directory='*'` is already in the script.
- **Timeout** → `INSPECT_TIMEOUT` is 120s in `src/workspace/git.rs`.

Debug the container step by hand with the same shape the code uses:

```sh
docker run --rm --network none -v /tmp/some-clone:/workspace -v /tmp/out:/out \
  -w /workspace -e HOME=/tmp --entrypoint sh taskrunner/codex-worker \
  -c 'git -c safe.directory="*" status --porcelain=v1 -z --untracked-files=all'
```

---

## 5. Environment notes

- `cargo` needs `export PATH="$HOME/.cargo/bin:$PATH"`.
- **Node must be on PATH for `cargo test`**: the fake codex/claude workers and the
  egress proxy's own tests are Node scripts. `tests/workers/egress_proxy.rs` runs
  `node --test docker/egress-proxy/server.test.cjs` (18 tests).
- The taskrunner daemon on this machine runs `target/release/taskrunner`, reached via
  the symlink `~/.local/bin/taskrunner`. **It is an older build than this branch**, so
  the escape bug is live until someone rebuilds.
- MCP is registered for both Claude Code and Codex at `~/.local/bin/taskrunner mcp`.
  Restarting the daemon drops the tools in any open session until `/mcp`.
- The previous session's shell could not reach `/var/run/docker.sock`, which is why
  these tests were never run. If yours can't either, say so rather than reporting a
  skip as a pass — the test prints `skipped:` and exits 0 when the env var is unset.

---

## 6. After it passes

```sh
cargo build --release
taskrunner down
nohup taskrunner up > ~/.taskrunner/logs/up.log 2>&1 &
```

Then the user runs `/mcp` in any open Claude Code session. After that, merging
`fix-worker-git-escape` into `main` is a fast-forward.

---

## 7. Known open issues — not your task, don't scope-creep

1. **Codex tool calls are missing from the archive.** `src/ingest/codex.rs` handles
   `function_call` / `function_call_output`; Codex 0.154 writes
   `custom_tool_call` / `custom_tool_call_output`. Host Codex sessions archive their
   prompts and replies but none of their commands or edits — a real hole in a project
   whose goal is a complete audit trail. Fix: teach the parser both names with a
   fixture, delete `~/.taskrunner/ingest-state.json` to force a re-read (dedupe makes
   it safe), and count unrecognised record types so the next format change surfaces.
2. **Search results still say "assistant".** The harness-name labels (`ca118f7`)
   reached the compact and timeline views but not search hits, while
   `docs/transcripts.md` says every reply and tool-call label names the harness.
3. **`taskrunner --help` exits 1** with "--help requires a value". `taskrunner help`
   works. The flag parser expects every `--flag` to take a value.
4. **`docs/getting-started.md` install line fails when re-run**: `ln -s` needs
   `ln -sfn`.
5. **The event log still isn't hash-chained.** Recorded in
   `docs/proposals/complete-audit.md`, along with the open question of anchoring the
   history written before chaining starts.

---

## 8. How work is done here

Read `CLAUDE.md` (project) and `~/.claude/CLAUDE.md` (user) first. The short version:

- **Tests fail first.** Every fix on this branch has a test that was run against the
  unfixed code and observed to fail. Two of mine passed against unfixed code at first
  — a stale `assert` and a fake worker that didn't reproduce the condition — so the
  red run is not a formality.
- **Remove old code in the same change that replaces it.** No parallel old-and-new.
- **Docs move with the code.** `docs/security.md` was updated in `aea1960`.
- **Comments say why, not what.**
- **Verify before asserting.** The user has called this out; check a claim in their
  actual context rather than stating it from memory.
- **Explain like a teacher**: plain words first, then the mechanism, then what it
  means from the user's seat, ending in one recommendation with its trade-off.
- **Filesystem names are kebab-case**, dates ISO-prefixed.
- **Only one agent edits this folder at a time.** Earlier today two sessions worked in
  it at once and one's uncommitted edits were swept into the other's commit. Use
  `claude -w <name>` or `codex --worktree` for a second agent.

---

## 9. Verified vs unverified

**Verified here:** the escape reproduced on the old code; the cancel hang reproduced
(a fake worker closing fd 1 — `process.stdout.end()` is not enough, it leaves fd 1
open) and fixed, red-to-green; the full suite at 196 green with clippy clean; the
CLI broken-pipe fix checked against the real archive.

**Not verified:** anything requiring Docker. That is `ContainerGit` end to end — the
production path of the security fix — and the live worker-runner test. The host-side
logic around it (container argv, status parsing, object-id validation, the
created-file diff, bundle landing) is covered without Docker, but the container step
itself has never executed.
