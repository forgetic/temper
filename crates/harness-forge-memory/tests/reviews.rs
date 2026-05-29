use harness_forge::{
    BranchRef, CreatePullRequest, CreatePullRequestReview, CreateRepository, Forge, ForgeError,
    PullRequestReviewStatus, RepositoryId, RequestReviewers, ReviewDecision, User, UserId,
};
use harness_forge_memory::MemoryForge;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park"),
    }
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository created")
    .id
}

fn user(id: &str, handle: &str) -> User {
    User {
        id: UserId::new(id),
        handle: handle.into(),
        display_name: None,
        email: None,
    }
}

fn pr_input(repo: &RepositoryId) -> CreatePullRequest {
    CreatePullRequest {
        title: "implementation".into(),
        body: String::new(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: vec!["implementation".into()],
        assignees: Vec::new(),
    }
}

#[test]
fn reviews_are_requested_recorded_ordered_and_aggregated() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let pr = block_on(forge.create_pull_request(&repo, pr_input(&repo))).unwrap();
    let reviewer = UserId::new("user-1");

    let requested = block_on(forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![reviewer.clone(), reviewer.clone()],
        },
    ))
    .unwrap();
    assert_eq!(requested.requested_reviewers, vec![reviewer.clone()]);
    assert_eq!(requested.version, pr.version.next());

    let duplicate = block_on(forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![reviewer.clone()],
        },
    ))
    .unwrap();
    assert_eq!(duplicate.version, requested.version);

    block_on(forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Commented,
            body: Some("looks reasonable".into()),
        },
    ))
    .unwrap();
    block_on(forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::ChangesRequested,
            body: None,
        },
    ))
    .unwrap();
    block_on(forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .unwrap();

    let reviews = block_on(forge.list_pull_request_reviews(&pr.id)).unwrap();
    assert_eq!(reviews.len(), 3);
    assert_eq!(
        reviews[0].id.as_str(),
        format!("review-{}-0000000000000001", pr.id)
    );
    assert!(reviews[0].submitted_at < reviews[1].submitted_at);
    assert!(reviews[1].submitted_at < reviews[2].submitted_at);

    let status = PullRequestReviewStatus::from_reviews(&requested.requested_reviewers, &reviews);
    assert!(status.is_approved());
    assert!(!status.has_changes_requested());

    let widened = block_on(forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![UserId::new("user-2")],
        },
    ))
    .unwrap();
    let status = PullRequestReviewStatus::from_reviews(&widened.requested_reviewers, &reviews);
    assert!(
        !status.is_approved(),
        "all requested reviewers must approve"
    );
}

#[test]
fn handles_over_one_store_can_act_as_different_users() {
    let base = MemoryForge::new();
    let alice = user("user-alice", "alice");
    let bob = user("user-bob", "bob");
    let alice_forge = base.as_user(alice.clone());
    let bob_forge = base.as_user(bob.clone());
    let bob_clone = bob_forge.clone();

    assert_eq!(block_on(alice_forge.current_user()).unwrap(), alice);
    assert_eq!(block_on(bob_forge.current_user()).unwrap(), bob);
    assert_eq!(block_on(bob_clone.current_user()).unwrap(), bob);

    let repo = new_repo(&alice_forge);
    let pr = block_on(alice_forge.create_pull_request(&repo, pr_input(&repo))).unwrap();
    block_on(alice_forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![bob.id.clone()],
        },
    ))
    .unwrap();

    let review = block_on(bob_forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .unwrap();
    assert_eq!(review.reviewer_id, bob.id);

    let reviews = block_on(alice_forge.list_pull_request_reviews(&pr.id)).unwrap();
    assert_eq!(reviews, vec![review]);
}

#[test]
fn review_operations_report_missing_pull_requests() {
    let forge = MemoryForge::new();
    let missing = harness_forge::PullRequestId::new("pull-request-missing");
    assert!(matches!(
        block_on(forge.list_pull_request_reviews(&missing)),
        Err(ForgeError::NotFound(_))
    ));
}
