//! Risk tiers and default policy.
//!
//! Workers always run in Docker; a delegated task's tier follows from what it
//! asks for:
//!   workspace-write — isolated edits in the task workspace; the delegation
//!                     action itself is the approval.
//!   networked       — extra outbound domains beyond the worker's own API;
//!                     task-level approval relayed by the calling agent.
//! read-only covers non-mutating operations (lookup/audit), which never create
//! tasks and so never reach this resolution.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    ReadOnly,
    WorkspaceWrite,
    Networked,
}

impl RiskTier {
    pub fn as_str(self) -> &'static str {
        match self {
            RiskTier::ReadOnly => "read-only",
            RiskTier::WorkspaceWrite => "workspace-write",
            RiskTier::Networked => "networked",
        }
    }
}

pub fn resolve_tier(allow_domains: &[String]) -> RiskTier {
    if allow_domains.is_empty() { RiskTier::WorkspaceWrite } else { RiskTier::Networked }
}
