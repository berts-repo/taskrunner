# Naming

User-facing names require explicit user approval before they land in implementation
or docs.

## Naming Rule

Any name exposed to users, agents, config files, logs, exports, or MCP clients must
be reviewed before implementation.

## Names Requiring Approval

- Product/project name
- MCP tool names
- Command names
- Config keys
- Project-local directory name
- Database concepts that appear in docs, logs, or exported records
- Session/task/run/job/thread terminology
- Worker/backend/provider/agent terminology
- Artifact/rule/memory terminology
- Risk tier names

## Approved Terms

- Product/project name: Taskrunner
- Project-local directory: `.taskrunner/`
- Delegated work unit: task
- Per-task interaction: turn
- Execution runtime: worker
- Worker integration code: worker harness
- Stored output: artifact
- Initial MCP tool names:
  - `assign-task`
  - `continue-task`
  - `lookup-task`

## Candidate Terms

### Product or Project

- Local Agent Broker
- Agent Relay
- Task Broker
- Universal Orchestrator

### Delegated Work Unit

- run
- job
- thread
- session

### Execution Backend

- backend
- runner
- agent
- provider
- connector
- driver
- adapter
- bridge
- harness

### Project-Local Directory

- `.orchestrator/`
- `.agent-broker/`
- `.uo/`
- `.omagion/`
- `.looking-glass/`
- `.glass/`

## Retired Or Avoided Terms

- Product/project names:
  - Looking Glass
  - Local Agent Broker
  - Universal Orchestrator
- Integration-code terms:
  - adapter
- Tool naming shapes:
  - `task.start`, `task.continue`, `task.lookup`
  - `dispatch.start`, `dispatch.continue`, `dispatch.lookup`
  - underscore-separated tool names such as `assign_task`

## Notes

- Tool names use hyphens because MCP permits hyphens and they read better in
  user-facing tool lists.
- `task` is the durable user-facing delegated work object.
- `job` remains available later for internal queue/execution semantics if needed.
