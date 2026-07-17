// Risk tiers and default policy.
// Workers always run in Docker; a delegated task's tier follows from what it
// asks for:
//   workspace-write — isolated edits in the task workspace; the delegation
//                     action itself is the approval.
//   networked       — extra outbound domains beyond the worker's own API;
//                     task-level approval relayed by the calling agent.
// read-only covers non-mutating operations (lookup/audit), which never
// create tasks and so never reach this resolution.

export type RiskTier = "read-only" | "workspace-write" | "networked";

export function resolveTier(allowDomains: string[]): RiskTier {
  return allowDomains.length > 0 ? "networked" : "workspace-write";
}
