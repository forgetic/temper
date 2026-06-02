//! Cross-process/-thread concurrency tests for the filesystem backend (ADR 0018).
//!
//! These use real OS threads plus a `Barrier` (no sleeps) to force genuine
//! parallel mutation of one shared store, proving the store-level advisory lock
//! makes ADR 0013's compare-and-swap hold across writers: no lost updates, no
//! duplicate item numbers, and exactly one compare-and-swap winner. Threads in
//! one process share the same on-disk store the way separate OS processes would,
//! and they take the same `<root>/.lock`, so this exercises the cross-process
//! critical section deterministically and stays in the default suite.

mod support;

use std::sync::{Arc, Barrier};
use std::thread;
use support::{block_on, issue, pull_request, repository, TestRoot};
use temper_forge::{Forge, ForgeError, IssueQuery, ItemNumber, UpdateIssue, Version};

const WORKERS: usize = 8;

#[test]
fn concurrent_updates_to_distinct_artifacts_lose_no_writes() {
    let root = TestRoot::new("concurrency");
    let forge = root.forge();
    let repo = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    // One issue per worker (all sharing issues.json) plus one shared pull
    // request that every worker links a distinct dependency onto.
    let mut issues = Vec::new();
    for index in 0..WORKERS {
        let created =
            block_on(forge.create_issue(&repo.id, issue(&format!("Issue {index}")))).unwrap();
        issues.push(created);
    }
    let pr =
        block_on(forge.create_pull_request(&repo.id, pull_request(&repo.id, "Feature"))).unwrap();

    let barrier = Arc::new(Barrier::new(WORKERS));
    let mut handles = Vec::new();
    for (index, created) in issues.iter().enumerate() {
        let forge = forge.clone();
        let barrier = Arc::clone(&barrier);
        let issue_id = created.id.clone();
        let target = created.number;
        let pr_id = pr.id.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            // A whole-file rewrite of issues.json...
            block_on(forge.update_issue(
                &issue_id,
                UpdateIssue {
                    add_labels: vec![format!("worker-{index}")],
                    ..UpdateIssue::default()
                },
            ))
            .unwrap();
            // ...and a read-modify-write of the one shared pull-request record.
            block_on(forge.add_pull_request_dependency(&pr_id, target)).unwrap();
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }

    // Every per-issue label survived: without the store lock, concurrent
    // whole-file rewrites would silently drop some of them.
    for (index, created) in issues.iter().enumerate() {
        let stored = block_on(forge.get_issue(&created.id)).unwrap().unwrap();
        assert!(
            stored.labels.contains(&format!("worker-{index}")),
            "issue {index} lost its label under concurrency"
        );
    }

    // Every dependency landed on the shared pull request: concurrent
    // read-modify-write of one record loses no links under the lock.
    let final_pr = block_on(forge.get_pull_request(&pr.id)).unwrap().unwrap();
    let mut deps = final_pr.dependencies.clone();
    deps.sort();
    let mut expected: Vec<ItemNumber> = issues.iter().map(|created| created.number).collect();
    expected.sort();
    assert_eq!(deps, expected, "concurrent dependency links lost updates");
}

#[test]
fn concurrent_creates_allocate_distinct_item_numbers() {
    let root = TestRoot::new("concurrency");
    let forge = root.forge();
    let repo = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    let barrier = Arc::new(Barrier::new(WORKERS));
    let mut handles = Vec::new();
    for index in 0..WORKERS {
        let forge = forge.clone();
        let barrier = Arc::clone(&barrier);
        let repo_id = repo.id.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            block_on(forge.create_issue(&repo_id, issue(&format!("Concurrent {index}"))))
                .unwrap()
                .number
        }));
    }
    let mut numbers: Vec<ItemNumber> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    numbers.sort();
    numbers.dedup();
    assert_eq!(
        numbers.len(),
        WORKERS,
        "concurrent creates produced duplicate item numbers"
    );

    // The store is internally consistent: exactly WORKERS issues are stored, and
    // re-reading them (which validates stored records) succeeds.
    let listed = block_on(forge.list_issues(&repo.id, IssueQuery::default())).unwrap();
    assert_eq!(listed.len(), WORKERS);
}

#[test]
fn two_acquirers_over_one_snapshot_yield_a_single_cas_winner() {
    let root = TestRoot::new("concurrency");
    let forge = root.forge();
    let repo = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let created = block_on(forge.create_issue(&repo.id, issue("Contended"))).unwrap();
    assert_eq!(created.version, Version::INITIAL);

    // Both workers capture the SAME "no lease"-style snapshot version and race a
    // conditional write keyed on it.
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for index in 0..2 {
        let forge = forge.clone();
        let barrier = Arc::clone(&barrier);
        let issue_id = created.id.clone();
        let snapshot = created.version;
        handles.push(thread::spawn(move || {
            barrier.wait();
            block_on(forge.update_issue(
                &issue_id,
                UpdateIssue {
                    add_labels: vec![format!("claim-{index}")],
                    expected_version: Some(snapshot),
                    ..UpdateIssue::default()
                },
            ))
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let winners = results.iter().filter(|result| result.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|result| matches!(result, Err(ForgeError::Conflict(_))))
        .count();
    assert_eq!(winners, 1, "exactly one compare-and-swap must win");
    assert_eq!(conflicts, 1, "the loser must observe a version conflict");

    // The store reflects exactly the winner's single write.
    let stored = block_on(forge.get_issue(&created.id)).unwrap().unwrap();
    assert_eq!(stored.version, created.version.next());
    assert_eq!(stored.labels.len(), 1);
}
