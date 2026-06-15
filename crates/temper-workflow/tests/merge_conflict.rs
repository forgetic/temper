//! Merge rejection classification at the executor boundary.

mod support;

use support::crash::{CrashForge, Fault, ForgeOp};
use support::{
    TestRoot, block_on, create_pr, new_repo, pr_labels, pr_state, seed_ci, submit_review, workflow,
};
use temper_forge_model::{
    CiJobConclusion, Forge, PullRequestState, PullRequestUpdateState, ReviewDecision,
    UpdatePullRequest,
};
use temper_workflow::{ArtifactSource, ExecutionError, Executor, RoleId, TransitionId};

fn gated_pr(
    forge: &temper_forge_memory::MemoryForge,
    repo: &temper_forge_model::RepositoryId,
) -> temper_forge_model::ItemNumber {
    let number = create_pr(forge, repo, &["implementation"], "");
    submit_review(forge, repo, number, ReviewDecision::Approved);
    seed_ci(forge, repo, number, CiJobConclusion::Success);
    number
}

fn close_pr(
    forge: &temper_forge_memory::MemoryForge,
    repo: &temper_forge_model::RepositoryId,
    number: temper_forge_model::ItemNumber,
) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .expect("pull request closes");
}

#[test]
fn merge_rejection_is_typed_when_pull_request_remains_open() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = gated_pr(&forge, &repo);
    let crash = CrashForge::new(
        forge,
        vec![Fault::conflict_before(ForgeOp::MergePullRequest, 1)],
    );
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let target = ArtifactSource::PullRequest { number };

    let error = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("an open unmerged conflict is workflow-routable");

    assert!(matches!(
        error,
        ExecutionError::MergeConflict {
            target: found,
            message: _
        } if found == target
    ));
    assert_eq!(
        pr_state(crash.inner(), &repo, number),
        PullRequestState::Open
    );
    assert!(!pr_labels(crash.inner(), &repo, number).contains(&"landed".to_string()));
}

#[test]
fn closed_pull_request_rejection_is_stale_not_routable() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = gated_pr(&forge, &repo);
    close_pr(&forge, &repo, number);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &forge);
    let target = ArtifactSource::PullRequest { number };

    let error = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("closed target is stale, not a conflict route");

    assert!(matches!(
        error,
        ExecutionError::TargetStale {
            target: found,
            message: _
        } if found == target
    ));
}

#[test]
fn conflict_response_after_success_still_projects_post_merge_labels() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = gated_pr(&forge, &repo);
    let crash = CrashForge::new(
        forge,
        vec![Fault::conflict_after(ForgeOp::MergePullRequest, 1)],
    );
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let target = ArtifactSource::PullRequest { number };

    block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect("a conflict response is re-read before routing");

    assert_eq!(crash.count(ForgeOp::MergePullRequest), 1);
    assert_eq!(
        pr_state(crash.inner(), &repo, number),
        PullRequestState::Merged
    );
    let labels = pr_labels(crash.inner(), &repo, number);
    assert!(labels.contains(&"landed".to_string()));
    assert!(labels.contains(&"alignment".to_string()));
}
