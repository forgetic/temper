//! Tests for the public runtime gate-signal reader.

mod support;

use support::{block_on, create_pr, new_repo, seed_ci, submit_review, workflow, TestRoot};
use temper_forge::{CiJobConclusion, ReviewDecision};
use temper_workflow::ArtifactSource;

#[test]
fn read_gate_signals_uses_fresh_native_ci_and_reviews() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let number = create_pr(&forge, &repo, &["implementation"], "");

    seed_ci(&forge, &repo, number, CiJobConclusion::Failure);
    submit_review(&forge, &repo, number, ReviewDecision::ChangesRequested);

    let signals = block_on(
        workflow
            .executor(&forge)
            .read_gate_signals(&repo, ArtifactSource::PullRequest { number }),
    )
    .expect("gate signals are read");

    assert!(signals.ci().is_failed());
    assert!(signals.review().has_changes_requested());
}
