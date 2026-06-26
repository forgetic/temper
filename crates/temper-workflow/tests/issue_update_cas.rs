//! Regression coverage for issue transition commit-time CAS.

mod support;

use support::{TestRoot, block_on, create_issue, new_repo, workflow};
use temper_forge::Version;
use temper_workflow::{ArtifactSource, Executor, RoleId, TransitionId};

#[test]
fn issue_transition_update_uses_loaded_version_precondition() {
    let root = TestRoot::new();
    let base = root.forge();
    let workflow = workflow();
    let repo = new_repo(&base);
    let number = create_issue(&base, &repo, &["code", "ready"], "claim me");

    let forge = support::crash::CrashForge::new(base.clone(), Vec::new());
    let executor = Executor::new(&workflow, &forge);
    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect("engineer can claim a ready code issue");

    let updates = forge.issue_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].expected_version, Some(Version::INITIAL));
}
