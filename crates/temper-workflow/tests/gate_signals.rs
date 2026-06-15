//! Tests for the public runtime gate-signal reader.

mod support;

use support::crash::{CrashForge, ForgeOp};
use support::{TestRoot, block_on, create_pr, new_repo, seed_ci, submit_review, workflow};
use temper_forge::{
    CiJobConclusion, Forge, PullRequestUpdateState, ReviewDecision, UpdatePullRequest,
};
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

#[test]
fn scanner_reads_ci_for_an_open_pr_but_skips_a_terminal_one() {
    // The scan-phase signal read (read_classified_gate_signals_with_needs) must
    // NOT read CI for a merged/closed pull request: CI status is irrelevant to a
    // terminal artifact, and reading it is the dominant idle cost because
    // historical PRs keep their workflow labels and are re-listed every
    // mechanical tick. An OPEN PR with a CI need still reads CI.
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();

    let open = create_pr(&forge, &repo, &["implementation"], "");
    let closed = create_pr(&forge, &repo, &["implementation"], "");
    seed_ci(&forge, &repo, open, CiJobConclusion::Success);
    seed_ci(&forge, &repo, closed, CiJobConclusion::Success);
    block_on(
        forge.update_pull_request(
            &block_on(forge.get_pull_request_by_number(&repo, closed))
                .expect("lookup")
                .expect("exists")
                .id,
            UpdatePullRequest {
                state: Some(PullRequestUpdateState::Closed),
                ..UpdatePullRequest::default()
            },
        ),
    )
    .expect("pull request closes");

    let needs = SignalNeeds::new(false, true, false); // ci only

    // Open PR: the scanner reads CI exactly once.
    let counting = CrashForge::new(forge.clone(), Vec::new());
    block_on(
        workflow
            .executor(&counting)
            .read_classified_gate_signals_with_needs(
                &repo,
                ArtifactSource::PullRequest { number: open },
                needs,
            ),
    )
    .expect("open pr signals read");
    assert_eq!(
        counting.count(ForgeOp::ListCiJobs),
        1,
        "an open PR with a CI need must read CI in the scan"
    );

    // Closed PR: the scanner skips the CI read entirely.
    let counting = CrashForge::new(forge.clone(), Vec::new());
    block_on(
        workflow
            .executor(&counting)
            .read_classified_gate_signals_with_needs(
                &repo,
                ArtifactSource::PullRequest { number: closed },
                needs,
            ),
    )
    .expect("closed pr signals read");
    assert_eq!(
        counting.count(ForgeOp::ListCiJobs),
        0,
        "a terminal (closed) PR must not trigger a scan-phase CI read"
    );
}
