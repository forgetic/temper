use chrono::{DateTime, Utc};
use temper_forge::{
    CandidateContinuation, CandidateLabelSelection, CandidateLifecycle, CandidatePosition, IssueId,
    ItemNumber, PullRequestId, RepositoryId,
};
use temper_runner::{
    ArtifactAddress, TerminalDiscoveryBucket, TerminalDiscoveryCommitOutcome,
    TerminalDiscoveryContinuation, TerminalDiscoveryPageCommit, TerminalDiscoveryPolicy,
    TerminalDiscoveryState, TerminalDiscoveryStateError,
};

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("timestamp")
}

fn repo(name: &str) -> RepositoryId {
    RepositoryId::new(format!("forgejo:acme/{name}"))
}

fn issue_cursor(
    repository_id: &RepositoryId,
    boundary: DateTime<Utc>,
    after: DateTime<Utc>,
    number: u64,
) -> TerminalDiscoveryContinuation {
    TerminalDiscoveryContinuation::Issue(CandidateContinuation {
        repository_id: repository_id.clone(),
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
        boundary: CandidatePosition {
            updated_at: boundary,
            number: ItemNumber::new(99),
            id: IssueId::new(format!("issue:{}:99", repository_id.as_str())),
        },
        after: CandidatePosition {
            updated_at: after,
            number: ItemNumber::new(number),
            id: IssueId::new(format!("issue:{}:{number}", repository_id.as_str())),
        },
        backend_cursor: Some(format!("page-{number}")),
    })
}

fn pull_cursor(
    repository_id: &RepositoryId,
    boundary: DateTime<Utc>,
    after: DateTime<Utc>,
    number: u64,
) -> TerminalDiscoveryContinuation {
    TerminalDiscoveryContinuation::PullRequest(CandidateContinuation {
        repository_id: repository_id.clone(),
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
        boundary: CandidatePosition {
            updated_at: boundary,
            number: ItemNumber::new(99),
            id: PullRequestId::new(format!("pull:{}:99", repository_id.as_str())),
        },
        after: CandidatePosition {
            updated_at: after,
            number: ItemNumber::new(number),
            id: PullRequestId::new(format!("pull:{}:{number}", repository_id.as_str())),
        },
        backend_cursor: Some(format!("page-{number}")),
    })
}

fn buckets() -> (TerminalDiscoveryBucket, TerminalDiscoveryBucket) {
    (
        TerminalDiscoveryBucket::issues(CandidateLabelSelection::AnyOf(vec![
            "recover".to_string(),
        ]))
        .unwrap(),
        TerminalDiscoveryBucket::pull_requests(CandidateLabelSelection::AnyOf(vec![
            "recover".to_string(),
        ]))
        .unwrap(),
    )
}

#[test]
fn clones_resume_pages_and_authority_requires_every_successful_bucket() {
    let repository = repo("widgets");
    let (issues, pulls) = buckets();
    let state = TerminalDiscoveryState::default();
    let cold = state
        .begin(&repository, "workflow-v1", [issues.clone(), pulls.clone()])
        .unwrap();
    assert!(!cold.cache_reused);
    assert!(!cold.authoritative);

    let boundary = timestamp("2026-06-01T00:00:00Z");
    let first_after = timestamp("2026-05-01T00:00:00Z");
    assert_eq!(
        state
            .commit_page(
                &repository,
                "workflow-v1",
                &issues,
                TerminalDiscoveryPageCommit {
                    continuation: Some(issue_cursor(&repository, boundary, first_after, 1)),
                    exhausted: false,
                    overflow: true,
                    sweep_boundary: Some(boundary),
                    retained_targets: vec![ArtifactAddress::issue(ItemNumber::new(1))],
                },
            )
            .unwrap(),
        TerminalDiscoveryCommitOutcome::Advanced
    );

    // A reconstructed consumer shares the committed cursor. A failed follow-up
    // marks authority incomplete but never advances or discards that cursor.
    let reconstructed = state.clone();
    reconstructed
        .record_failed_page(&repository, "workflow-v1", &issues)
        .unwrap();
    let failed = reconstructed.snapshot(&repository).unwrap();
    assert!(failed.buckets[&issues].failed);
    assert_eq!(
        failed.buckets[&issues].continuation.clone(),
        Some(issue_cursor(&repository, boundary, first_after, 1))
    );

    reconstructed
        .commit_page(
            &repository,
            "workflow-v1",
            &issues,
            TerminalDiscoveryPageCommit {
                continuation: None,
                exhausted: true,
                overflow: false,
                sweep_boundary: Some(boundary),
                retained_targets: vec![],
            },
        )
        .unwrap();
    reconstructed
        .commit_page(
            &repository,
            "workflow-v1",
            &pulls,
            TerminalDiscoveryPageCommit {
                continuation: None,
                exhausted: true,
                overflow: false,
                sweep_boundary: Some(boundary),
                retained_targets: vec![ArtifactAddress::pull_request(ItemNumber::new(2))],
            },
        )
        .unwrap();
    let complete = reconstructed
        .begin(&repository, "workflow-v1", [issues.clone(), pulls.clone()])
        .unwrap();
    assert!(complete.cache_reused);
    assert!(complete.authoritative);
    assert_eq!(complete.retained_targets.len(), 2);
}

#[test]
fn nonadvancing_timestamp_restarts_only_the_affected_sweep() {
    let repository = repo("service");
    let (issues, _) = buckets();
    let state = TerminalDiscoveryState::default();
    state
        .begin(&repository, "workflow-v1", [issues.clone()])
        .unwrap();
    let boundary = timestamp("2026-06-01T00:00:00Z");
    let after = timestamp("2026-05-01T00:00:00Z");
    let page = |number| TerminalDiscoveryPageCommit {
        continuation: Some(issue_cursor(&repository, boundary, after, number)),
        exhausted: false,
        overflow: true,
        sweep_boundary: Some(boundary),
        retained_targets: vec![],
    };
    state
        .commit_page(&repository, "workflow-v1", &issues, page(1))
        .unwrap();
    assert_eq!(
        state
            .commit_page(&repository, "workflow-v1", &issues, page(2))
            .unwrap(),
        TerminalDiscoveryCommitOutcome::Advanced,
        "number/id tie-break movement is valid when provider timestamps tie"
    );
    assert_eq!(
        state
            .commit_page(&repository, "workflow-v1", &issues, page(2))
            .unwrap(),
        TerminalDiscoveryCommitOutcome::RestartedNonAdvancing
    );
    let snapshot = state.snapshot(&repository).unwrap();
    assert!(!snapshot.authoritative);
    assert!(snapshot.buckets[&issues].continuation.is_none());
    assert!(snapshot.buckets[&issues].sweep_boundary.is_none());
}

#[test]
fn workflow_restart_and_memory_limits_are_deterministic_and_repository_scoped() {
    let policy = TerminalDiscoveryPolicy::new(2, 2, 2, 32);
    let state = TerminalDiscoveryState::new(policy);
    let first_repo = repo("a");
    let second_repo = repo("b");
    let third_repo = repo("c");
    let (issues, _) = buckets();
    state
        .begin(&first_repo, "workflow-v1", [issues.clone()])
        .unwrap();
    state
        .begin(&second_repo, "workflow-v1", [issues.clone()])
        .unwrap();

    for number in [3, 1, 2] {
        state
            .retain_exact_target(&first_repo, ArtifactAddress::issue(ItemNumber::new(number)))
            .unwrap();
    }
    state
        .retain_exact_target(
            &second_repo,
            ArtifactAddress::pull_request(ItemNumber::new(9)),
        )
        .unwrap();
    let first = state.snapshot(&first_repo).unwrap();
    let second = state.snapshot(&second_repo).unwrap();
    assert_eq!(
        first
            .retained_targets
            .iter()
            .map(|target| target.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(first.retained_overflow);
    assert_eq!(second.retained_targets.len(), 1);
    assert!(matches!(
        state.begin(&third_repo, "workflow-v1", [issues.clone()]),
        Err(TerminalDiscoveryStateError::RepositoryCapacity { maximum: 2 })
    ));

    let changed = state
        .begin(&first_repo, "workflow-v2", [issues.clone()])
        .unwrap();
    assert!(!changed.cache_reused);
    assert!(!changed.authoritative);
    assert_eq!(changed.retained_targets.len(), 2);

    // Process restart creates a cold owner. It must build authority from a new
    // sweep instead of repeatedly claiming the newest page as complete.
    let restarted = TerminalDiscoveryState::new(policy);
    assert!(restarted.snapshot(&first_repo).is_none());
    let cold = restarted
        .begin(&first_repo, "workflow-v2", [issues])
        .unwrap();
    assert!(!cold.cache_reused);
    assert!(!cold.authoritative);
}

#[test]
fn complete_pages_require_a_stable_sweep_boundary() {
    let repository = repo("boundaries");
    let (issues, _) = buckets();
    let state = TerminalDiscoveryState::default();
    state
        .begin(&repository, "workflow-v1", [issues.clone()])
        .unwrap();

    let missing = state
        .commit_page(
            &repository,
            "workflow-v1",
            &issues,
            TerminalDiscoveryPageCommit {
                continuation: None,
                exhausted: true,
                overflow: false,
                sweep_boundary: None,
                retained_targets: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(missing, TerminalDiscoveryStateError::InvalidPage);
    assert!(!state.snapshot(&repository).unwrap().authoritative);

    let boundary = timestamp("2026-06-01T00:00:00Z");
    state
        .commit_page(
            &repository,
            "workflow-v1",
            &issues,
            TerminalDiscoveryPageCommit {
                continuation: Some(issue_cursor(
                    &repository,
                    boundary,
                    timestamp("2026-05-01T00:00:00Z"),
                    1,
                )),
                exhausted: false,
                overflow: true,
                sweep_boundary: Some(boundary),
                retained_targets: vec![],
            },
        )
        .unwrap();
    let changed = state
        .commit_page(
            &repository,
            "workflow-v1",
            &issues,
            TerminalDiscoveryPageCommit {
                continuation: None,
                exhausted: true,
                overflow: false,
                sweep_boundary: Some(timestamp("2026-06-02T00:00:00Z")),
                retained_targets: vec![],
            },
        )
        .unwrap();
    assert_eq!(
        changed,
        TerminalDiscoveryCommitOutcome::RestartedNonAdvancing
    );
    let restarted = state.snapshot(&repository).unwrap();
    assert!(!restarted.authoritative);
    assert!(restarted.buckets[&issues].sweep_boundary.is_none());
}

#[test]
fn continuation_scope_is_checked_before_commit() {
    let repository = repo("scope");
    let foreign = repo("foreign");
    let (_, pulls) = buckets();
    let state = TerminalDiscoveryState::default();
    state
        .begin(&repository, "workflow-v1", [pulls.clone()])
        .unwrap();
    let boundary = timestamp("2026-06-01T00:00:00Z");
    let error = state
        .commit_page(
            &repository,
            "workflow-v1",
            &pulls,
            TerminalDiscoveryPageCommit {
                continuation: Some(pull_cursor(
                    &foreign,
                    boundary,
                    timestamp("2026-05-01T00:00:00Z"),
                    1,
                )),
                exhausted: false,
                overflow: true,
                sweep_boundary: Some(boundary),
                retained_targets: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(error, TerminalDiscoveryStateError::ContinuationScope);

    let malformed = state
        .commit_page(
            &repository,
            "workflow-v1",
            &pulls,
            TerminalDiscoveryPageCommit {
                continuation: Some(pull_cursor(
                    &repository,
                    boundary,
                    timestamp("2026-07-01T00:00:00Z"),
                    100,
                )),
                exhausted: false,
                overflow: true,
                sweep_boundary: Some(boundary),
                retained_targets: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(malformed, TerminalDiscoveryStateError::ContinuationScope);
}
