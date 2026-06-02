mod support;

use std::time::Duration;
use support::{block_on, issue, pull_request, repository, timestamp, TestRoot};
use temper_forge::{
    ChangeKind, ChangeSource, ChangeSourceEvent, CiJob, CiJobConclusion, CiJobId, CiJobStatus,
    CreatePullRequestReview, Forge, ForgeError, ReviewDecision, UpdateIssue, Version,
};

fn completed_ci_job(repo_id: &temper_forge::RepositoryId) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-job-{}-test", repo_id.as_str())),
        repo_id: repo_id.clone(),
        pull_request_id: None,
        commit_sha: "abc123".into(),
        name: "test".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Success),
        url: None,
        created_at: timestamp(10),
        started_at: Some(timestamp(11)),
        completed_at: Some(timestamp(12)),
        updated_at: timestamp(12),
    }
}

fn expect_hint(source: &mut impl ChangeSource) -> temper_forge::ChangeHint {
    match source.recv_timeout(Duration::from_millis(200)) {
        ChangeSourceEvent::Hint(hint) => hint,
        other => panic!("expected hint, got {other:?}"),
    }
}

#[test]
fn distinct_handle_issue_mutation_publishes_item_hint() {
    let root = TestRoot::new("hints-issue");
    let writer = root.forge();
    let reader = root.forge();
    let repository = block_on(writer.create_repository(repository("alice", "project"))).unwrap();
    let mut hints = reader.subscribe_hints();

    let issue = block_on(writer.create_issue(&repository.id, issue("Implement login"))).unwrap();

    let hint = expect_hint(&mut hints);
    assert_eq!(hint.repo.owner, "alice");
    assert_eq!(hint.repo.name, "project");
    assert_eq!(hint.item, Some(issue.number));
    assert_eq!(hint.kind, ChangeKind::Issue);
}

#[test]
fn failed_mutation_does_not_publish_hint() {
    let root = TestRoot::new("hints-failed");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(&repository.id, issue("Implement login"))).unwrap();
    let mut hints = root.forge().subscribe_hints();

    let error = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            expected_version: Some(Version::new(999)),
            ..UpdateIssue::default()
        },
    ))
    .expect_err("stale version is rejected");

    assert!(matches!(error, ForgeError::Conflict(_)));
    assert_eq!(
        hints.recv_timeout(Duration::from_millis(50)),
        ChangeSourceEvent::Timeout
    );
}

#[test]
fn pull_request_review_and_ci_mutations_publish_broad_hints() {
    let root = TestRoot::new("hints-pr-ci-review");
    let writer = root.forge();
    let repository = block_on(writer.create_repository(repository("alice", "project"))).unwrap();
    let mut hints = root.forge().subscribe_hints();

    let pr = block_on(writer.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Implement login"),
    ))
    .unwrap();
    let pr_hint = expect_hint(&mut hints);
    assert_eq!(pr_hint.item, Some(pr.number));
    assert_eq!(pr_hint.kind, ChangeKind::PullRequest);

    block_on(writer.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .unwrap();
    let review_hint = expect_hint(&mut hints);
    assert_eq!(review_hint.item, Some(pr.number));
    assert_eq!(review_hint.kind, ChangeKind::Review);

    writer
        .seed_ci_jobs(&repository.id, vec![completed_ci_job(&repository.id)])
        .unwrap();
    let ci_hint = expect_hint(&mut hints);
    assert_eq!(ci_hint.item, None);
    assert_eq!(ci_hint.kind, ChangeKind::Ci);
}

#[test]
fn restarted_listener_starts_at_tail_and_misses_old_hints() {
    let root = TestRoot::new("hints-restart");
    let writer = root.forge();
    let repository = block_on(writer.create_repository(repository("alice", "project"))).unwrap();

    block_on(writer.create_issue(&repository.id, issue("First"))).unwrap();
    let mut restarted = root.forge().subscribe_hints();

    assert_eq!(
        restarted.recv_timeout(Duration::from_millis(50)),
        ChangeSourceEvent::Timeout
    );

    block_on(writer.create_issue(&repository.id, issue("Second"))).unwrap();
    assert_eq!(expect_hint(&mut restarted).kind, ChangeKind::Issue);
}
