use crate::CapabilityGrants;

#[test]
fn capability_grants_exact_match() {
    let g = CapabilityGrants::default().allow("agents.run");
    assert!(g.permits("agents.run"));
    assert!(!g.permits("agents.spawn"));
    assert!(!g.permits("agents"));
}

#[test]
fn capability_grants_glob_match() {
    let g = CapabilityGrants::default().allow("agents.*");
    assert!(g.permits("agents.run"));
    assert!(g.permits("agents.spawn"));
    assert!(g.permits("agents.parallel"));
    assert!(g.permits("agents.broadcast"));
    assert!(g.permits("agents.send"));
    assert!(g.permits("agents.wait"));
    assert!(g.permits("agents.inbox"));
    assert!(g.permits("agents.list"));
    assert!(!g.permits("other.run"));
    assert!(
        !g.permits("agents_run"),
        "underscore variant must not match"
    );
}

#[test]
fn capability_grants_names_includes_glob() {
    let g = CapabilityGrants::default().allow("agents.*");
    assert!(g.names().any(|n| n.starts_with("agents.")));
}
