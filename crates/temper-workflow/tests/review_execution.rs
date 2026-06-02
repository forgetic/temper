mod support;

use support::{block_on, create_pr, new_repo, seed_ci, workflow, TestRoot};
use temper_forge::{
    CiJobConclusion, CreatePullRequestReview, Forge, PullRequestState, RequestReviewers,
    ReviewDecision, UserId,
};
use temper_workflow::{ArtifactSource, ExecutionError, RoleId, TransitionId};

#[test]
fn native_review_signal_controls_the_merge_gate() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "");
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.request_pull_request_reviewers(
        &pull_request.id,
        RequestReviewers {
            reviewers: vec![UserId::new("user-1")],
        },
    ))
    .expect("reviewer requested");

    let executor = workflow.executor(&forge);
    let blocked = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("review gate is shut until native review approves");
    assert!(matches!(blocked, ExecutionError::Precondition { .. }));

    block_on(forge.submit_pull_request_review(
        &pull_request.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .expect("review submitted");
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect("native review approval opens the merge gate");
    let merged = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap();
    assert_eq!(merged.state, PullRequestState::Merged);
}
