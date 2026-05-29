//! Optimistic-concurrency (compare-and-swap) tests for the filesystem backend.
//!
//! These verify the portable conditional-write primitive from ADR 0013: each
//! artifact carries a monotonic `Version`, `update_issue` / `update_pull_request`
//! apply only when `expected_version` matches the stored version, and a stale
//! token is rejected with `ForgeError::Conflict` without mutating the store.

mod support;

use harness_forge::{Forge, ForgeError, UpdateIssue, UpdatePullRequest, Version};
use support::{block_on, issue, pull_request, repository, TestRoot};

#[test]
fn conditional_issue_update_rejects_a_stale_version() {
    let root = TestRoot::new("optimistic-concurrency");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let created = block_on(forge.create_issue(&repository.id, issue("Versioned"))).unwrap();
    // A freshly created artifact starts at the initial version.
    assert_eq!(created.version, Version::INITIAL);

    // A compare-and-swap against the captured version succeeds and advances it.
    let updated = block_on(forge.update_issue(
        &created.id,
        UpdateIssue {
            title: Some("Renamed".into()),
            expected_version: Some(created.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.version, created.version.next());

    // A second update against the now-stale captured version is rejected
    // without mutating anything or advancing the logical clock.
    let conflict = block_on(forge.update_issue(
        &created.id,
        UpdateIssue {
            title: Some("Stale".into()),
            expected_version: Some(created.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(conflict, ForgeError::Conflict(_)));

    let current = block_on(forge.get_issue(&created.id)).unwrap().unwrap();
    assert_eq!(current.title, "Renamed", "a rejected CAS changes nothing");
    assert_eq!(current.version, updated.version);
    assert_eq!(
        current.updated_at, updated.updated_at,
        "a rejected CAS does not advance the logical clock"
    );

    // An unconditional update still applies and advances the version.
    let unconditional = block_on(forge.update_issue(
        &created.id,
        UpdateIssue {
            title: Some("Final".into()),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(unconditional.version, updated.version.next());
    assert_eq!(unconditional.title, "Final");
}

#[test]
fn conditional_pull_request_update_rejects_a_stale_version() {
    let root = TestRoot::new("optimistic-concurrency");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let created = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Implement login"),
    ))
    .unwrap();
    assert_eq!(created.version, Version::INITIAL);

    let updated = block_on(forge.update_pull_request(
        &created.id,
        UpdatePullRequest {
            add_labels: vec!["needs-review".into()],
            expected_version: Some(created.version),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.version, created.version.next());

    let conflict = block_on(forge.update_pull_request(
        &created.id,
        UpdatePullRequest {
            add_labels: vec!["needs-testing".into()],
            expected_version: Some(created.version),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(conflict, ForgeError::Conflict(_)));

    let current = block_on(forge.get_pull_request(&created.id))
        .unwrap()
        .unwrap();
    assert_eq!(current.version, updated.version);
    assert!(!current.labels.contains(&"needs-testing".to_string()));
}
