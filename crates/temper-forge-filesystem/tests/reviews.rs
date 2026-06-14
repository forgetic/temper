mod support;

use support::{TestRoot, block_on, pull_request, repository};
use temper_forge::{
    CreatePullRequestReview, Forge, ForgeError, PullRequestId, PullRequestReviewStatus,
    RequestReviewers, ReviewDecision, User, UserId,
};

#[test]
fn reviews_are_requested_persisted_ordered_and_aggregated() {
    let root = TestRoot::new("reviews");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pr = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Implement login"),
    ))
    .unwrap();
    let reviewer = User {
        id: UserId::new("user-reviewer"),
        handle: "reviewer".into(),
        display_name: None,
        email: None,
    };
    let reviewer_forge = forge.as_user(reviewer.clone());

    let requested = block_on(forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![reviewer.id.clone(), reviewer.id.clone()],
        },
    ))
    .unwrap();
    assert_eq!(requested.requested_reviewers, vec![reviewer.id.clone()]);

    block_on(reviewer_forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Commented,
            body: Some("nit".into()),
        },
    ))
    .unwrap();
    block_on(reviewer_forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::ChangesRequested,
            body: None,
        },
    ))
    .unwrap();
    block_on(reviewer_forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .unwrap();

    let reopened = root.forge();
    let current = block_on(reopened.get_pull_request(&pr.id))
        .unwrap()
        .expect("pull request persists");
    assert_eq!(current.requested_reviewers, vec![reviewer.id.clone()]);

    let reviews = block_on(reopened.list_pull_request_reviews(&pr.id)).unwrap();
    assert_eq!(reviews.len(), 3);
    assert_eq!(
        reviews[0].id.as_str(),
        format!("review-{}-0000000000000001", pr.id)
    );
    assert!(reviews[0].submitted_at < reviews[1].submitted_at);
    assert!(
        reviews
            .iter()
            .all(|review| review.reviewer_id == reviewer.id)
    );

    let status = PullRequestReviewStatus::from_reviews(&current.requested_reviewers, &reviews);
    assert!(status.is_approved());
    assert!(!status.has_changes_requested());
}

#[test]
fn review_operations_report_missing_pull_requests() {
    let root = TestRoot::new("reviews");
    let forge = root.forge();
    let missing = PullRequestId::new("pull-request-repo-0000000000000001-0000000000009999");
    assert!(matches!(
        block_on(
            forge.request_pull_request_reviewers(&missing, RequestReviewers { reviewers: vec![] })
        ),
        Err(ForgeError::NotFound(_))
    ));
}
