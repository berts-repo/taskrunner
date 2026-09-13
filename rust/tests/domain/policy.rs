use taskrunner::domain::policy::{RiskTier, resolve_tier};

#[test]
fn no_extra_domains_is_workspace_write() {
    assert_eq!(resolve_tier(&[]), RiskTier::WorkspaceWrite);
    assert_eq!(RiskTier::WorkspaceWrite.as_str(), "workspace-write");
}

#[test]
fn extra_domains_make_it_networked() {
    assert_eq!(resolve_tier(&["registry.npmjs.org".to_string()]), RiskTier::Networked);
    assert_eq!(RiskTier::Networked.as_str(), "networked");
}
