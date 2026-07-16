# Backend Spike

Compare Claude Code and Codex before choosing the worker starting point.

## Goal

Identify which local coding-agent worker should be used as the starting point and what
minimum worker-harness/task contract the project needs.

## Backends

- Claude Code
- Codex

## Test Matrix

For each backend, test:

- Start a delegated task non-interactively.
- Resume the same delegated task.
- Capture final output.
- Capture or reconstruct the worker-native session ID.
- Detect changed files.
- Capture errors and exit status.
- Observe approval behavior.
- Check whether the interface is stable enough for Taskrunner's worker harness.

## Suggested Test Task

Use a tiny throwaway repo or temp directory. Ask the backend to:

- Create a small text file.
- Add one line to it in a follow-up turn.
- Report what changed.

This keeps the spike focused on orchestration behavior instead of coding quality.

## Evaluation Criteria

- CLI/API stability
- Non-interactive start support
- Resume support
- Output parseability
- Session ID visibility
- Error handling
- Changed-file detection
- Approval handling
- Implementation complexity

## Result

### Environment

- Codex CLI: `codex-cli 0.130.0`
- Claude Code: `2.1.133`

### Codex Result

Codex passed the start and resume test.

- Start command: `codex -a never -s workspace-write exec --json -C <repo> <prompt>`
- Resume command: `codex -a never -s workspace-write exec resume --json <thread_id> <prompt>`
- Session identifier surfaced as JSONL `thread_id`.
- JSONL events included:
  - `thread.started`
  - `turn.started`
  - `agent_message`
  - `command_execution`
  - `file_change`
  - `turn.completed`
- File changes were directly visible in `file_change` events.
- Exit status was available from command completion and process exit code.
- Token usage was included in the final `turn.completed` event.
- Changed files can also be detected with `git status --short`.

Observed Codex thread ID:

- `019e28f6-9f73-73d0-b601-33505b06d3f5`

### Claude Result

Claude Code is installed and has promising CLI support. In the Codex sandbox, the
host Claude login is not visible. Outside that sandbox, host auth is visible. A
secure Docker auth spike was also run.

- Non-interactive mode is available with `claude --print`.
- JSON output is available with `--output-format json`.
- Streaming JSON output is available with `--output-format stream-json`.
- Resume is exposed with `--resume <session_id>`.
- A sandboxed simple JSON test returned a structured error:
  - `Not logged in · Please run /login`
- The structured error included a `session_id`, duration fields, usage fields,
  permission denials, terminal reason, and UUID.
- Resuming the unauthenticated error session did not work because no conversation
  was persisted.

Observed Claude error session ID:

- `2030fdee-4acf-47b5-a22b-1ec8b313b394`

One attempted Claude file-edit run with `--output-format stream-json` produced no
output for several minutes and had to be terminated. That may be related to auth,
permission mode, or startup behavior; retest after login before choosing Claude as
the first backend.

### Claude Docker Auth Result

Claude login-based auth can work in Docker without an API key.

Tested image shape:

- Base image: `node:20-slim`
- Claude Code installed globally in the image
- Runtime user: non-root `worker`
- Persistent Docker volume mounted at `/home/worker`
- No Docker socket mount
- No host home mount
- No privileged container

Result:

- `claude auth login --claudeai --email bert@helloto.me` completed successfully
  inside the non-root container.
- A fresh container with the same `/home/worker` volume reported:
  - `loggedIn: true`
  - `authMethod: claude.ai`
  - `subscriptionType: pro`
- A fresh non-interactive `claude --print --output-format json` reached Claude
  successfully but returned account usage limit status:
  - API status: `429`
  - Message: `You're out of extra usage · resets 4:20am (UTC)`

This means Docker login persistence is viable. The remaining blocker is usage
availability, not container authentication.

### Recommendation

- Recommended worker starting point: Codex if implementation starts before Claude usage
  resets.
- Claude Code is viable in the intended Docker model after container auth setup.
- Recommended additional worker: Claude Code, or alternative starting point if usage is available
  and file-edit/resume tests pass in Docker.
- Worker harness interface should be shaped around:
  - start turn
  - resume turn by worker-native session ID
  - stream structured events
  - capture final response
  - capture file-change events when available
  - fall back to git status/diff for changed-file detection
  - capture process exit code and structured errors
- Minimum task record fields:
  - project path
  - backend name
  - worker-native session ID
  - status
  - prompt summary
  - final response
  - changed files
  - command events or log reference
  - artifact references
  - error message
  - started/completed timestamps

### Remaining Claude Retest

After Claude usage resets, rerun inside the authenticated Docker worker:

- Start a delegated task non-interactively.
- Resume the same delegated task.
- Confirm whether stream-json reliably emits events during file edits.
- Confirm whether the session ID from a successful run can be resumed.
- Compare Claude's edit/change events against Codex's `file_change` events.

### Claude Docker Retest Result (2026-07-16)

All retest items passed inside the authenticated container (image
`uo-claude-worker:spike`, home volume `uo_claude_docker_home2`, non-root
`worker`, workspace bind-mounted at `/workspace`):

- Non-interactive start works: `claude --print --output-format json` returned
  a structured success result with `session_id`.
- File-edit run with `--output-format stream-json --verbose --permission-mode
  acceptEdits` streamed events and created the requested file. The earlier
  spike hang did not reproduce; it was most likely the unauthenticated state
  plus a pending permission prompt.
- Event shape per line: `system` (subtype `init`, carries `session_id`),
  `assistant` messages whose content includes `thinking`, `text`, and
  `tool_use` blocks (tool name plus full input, e.g. `Write`/`Edit` with
  `file_path`), `user` tool results, occasional `rate_limit_event`, and a
  final `result` with status, `num_turns`, cost, and usage.
- Cross-container resume works: a fresh container with the same home volume
  ran `--resume <session_id>`, kept the same session ID, retained context
  ("the same file"), and applied a correct `Edit` to the existing file.
- File-change comparison: Claude emits no dedicated `file_change` event
  (unlike Codex). Changed files are derivable from `tool_use` inputs
  (`Write`, `Edit`, `MultiEdit`, `NotebookEdit` → `file_path`), with git
  status/diff in the workspace as the fallback, which the harness contract
  already requires.

Conclusion: Claude Code is confirmed viable as the additional Docker worker;
`--permission-mode acceptEdits` (or stricter Taskrunner-brokered permissions)
must always be set for non-interactive runs to avoid prompt hangs.

The spike should ultimately produce:

- Recommended worker starting point
- Recommended additional worker
- Minimum worker harness interface
- Minimum task record fields
- Known risks
