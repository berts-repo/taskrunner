# Decisions

## First plan: global skills, one copy (2026-09-13, replaced)

Claude Code, Codex and Hermes all read the same `SKILL.md` format, so one copy could
serve every harness. The plan was a global `~/.agents/skills/`, taskrunner's own
skills alongside it, and `[host.<name>].skills` for per-host extras, all symlinked by
`taskrunner sync`. What shipped kept the symlinking but not the global folder or the
per-host extras; see below.

## Taskrunner's own skills (2026-09-14)

Built into the binary, so a skill always describes the tools of the version serving
it. Served over MCP (SEP-2640) to a harness that asks, and written under
`skills/<host>/` in the state root and linked for a harness that can't ask yet.

## Agents: no format of taskrunner's own (2026-09-15)

There is no agent standard. Claude Code agents are markdown files under
`~/.claude/agents/`; Codex has only `AGENTS.md` instructions; Hermes subagents are
defined by the `delegate_task` call.

- Per-harness, native format, taskrunner managing only which are active per host —
  ✅ full fidelity. ❌ an agent wanted everywhere is written up to three times.
- Taskrunner-owned definitions rendered into each harness's shape — ✅ one source.
  ❌ lossy: `tools` and `model` don't translate.
- Roles written as Agent Skills, the format all three harnesses already read — ✅ one
  source, no new format, and MCP can serve it. ❌ no skill format names a model or a
  tool list.

**Decision:** a role such as a librarian or a doc writer ships as an Agent Skill. The
user's skills live in folders listed under `[skills] dirs`, and `taskrunner sync`
links them into every connected harness. They are linked, not served over MCP: a
served skill is untrusted input whose scripts need approval, and only one harness
fetches skills that way. The model that does the work is a worker
(`[worker.<name>] model`), since no skill can name one; a skill that wants a model
says which worker to delegate to. Agents needing harness-specific tools stay native.
An earlier idea, a small *portable agent* format rendered per harness, was dropped: a
skill already is one.

This matches common practice: a shared skills or prompt repository synced into every
tool, with tool-specific agent configuration kept next to each tool.
