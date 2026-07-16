// Risk tiers and default policy (PLAN § Risk tiers and default policy).
// A delegated task's tier follows from what it asks for:
//   workspace-write — isolated edits in the task workspace; the delegation
//                     action itself is the approval.
//   networked       — extra outbound domains beyond the worker's own API;
//                     task-level approval relayed by the calling agent.
//   privileged      — host-run execution (or other host-level exposure);
//                     only a human-run `taskrunner approve` grants it.
// read-only covers non-mutating operations (lookup/audit), which never
// create tasks and so never reach this resolution.

export type RiskTier = "read-only" | "workspace-write" | "networked" | "privileged";

export type WorkerRuntime = "docker" | "host";

export function resolveTier(runtime: WorkerRuntime, allowDomains: string[]): RiskTier {
  if (runtime === "host") return "privileged";
  if (allowDomains.length > 0) return "networked";
  return "workspace-write";
}
