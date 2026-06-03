//! Tests for the public runtime gate-signal reader.

mod support;

use support::crash::{CrashForge, ForgeOp};
use support::{block_on, create_pr, new_repo, seed_ci, submit_review, workflow, TestRoot};
use temper_forge::{CiJobConclusion, ReviewDecision};
use temper_workflow::{ArtifactSource, RoleId, SignalNeeds, TransitionId};

#[test]
fn signal_needs_are_derived_from_queues_and_transitions() {
    let workflow = workflow();
    let compiled = workflow.compile();

    assert_eq!(
        workflow.signal_needs_for_transition(&TransitionId::new("approve_merge")),
        SignalNeeds::new(false, true, true)
    );
    assert_eq!(
        workflow.signal_needs_for_transition(&TransitionId::new("mark_code_ready")),
        SignalNeeds::new(true, false, false)
    );
    assert_eq!(
        workflow.signal_needs_for_transition(&TransitionId::new("claim_code")),
        SignalNeeds::none()
    );
    assert_eq!(
        compiled.signal_needs_for_role(&RoleId::new("engineer")),
        SignalNeeds::new(false, true, true)
    );
    assert_eq!(
        compiled.signal_needs_for_role(&RoleId::new("architect")),
        SignalNeeds::none()
    );
}

#[test]
fn transition_planning_reads_only_the_transition_required_signals() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let number = create_pr(&forge, &repo, &["implementation"], "");
    let counting = CrashForge::new(forge.clone(), Vec::new());

    block_on(workflow.executor(&counting).plan(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("request_review"),
        &RoleId::new("engineer"),
    ))
    .expect("ungated transition plans");

    assert_eq!(counting.count(ForgeOp::ListCiJobs), 0);
    assert_eq!(counting.count(ForgeOp::ListPullRequestReviews), 0);
}

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
