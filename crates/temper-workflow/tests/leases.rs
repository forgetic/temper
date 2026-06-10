//! Tests for lease acquisition, heartbeat, and release (Phase 7).
//!
//! The pure [`LeasePlanner`] tests assert the decision rules directly; the
//! [`LeaseManager`] tests drive those decisions through the deterministic
//! in-memory backend and confirm the lease is written into the artifact's
//! metadata block.

mod support;

use chrono::Duration;
use support::{block_on, create_issue, issue_body, new_repo, ts, TestRoot};
use temper_forge::ItemNumber;
use temper_workflow::{
    parse_metadata_block, ArtifactSource, Lease, LeaseConflict, LeaseError, LeaseManager,
    LeasePlanner, LeasePolicy, RoleId,
};

fn policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

fn held_lease(worker: &str) -> Lease {
    Lease {
        role: RoleId::new("engineer"),
        worker: worker.to_string(),
        claimed_at: ts("2026-05-29T00:00:00Z"),
        heartbeat_at: ts("2026-05-29T00:00:00Z"),
        expires_at: ts("2026-05-29T00:30:00Z"),
    }
}

#[test]
fn acquire_on_unclaimed_records_expiration_metadata() {
    let planner = LeasePlanner::new(policy());
    let now = ts("2026-05-29T01:00:00Z");

    let lease = planner
        .acquire(None, RoleId::new("engineer"), "run-1", now)
        .expect("an unclaimed artifact can be acquired");

    assert_eq!(lease.role, RoleId::new("engineer"));
    assert_eq!(lease.worker, "run-1");
    assert_eq!(lease.claimed_at, now);
    assert_eq!(lease.heartbeat_at, now);
    // Expiration is exactly the policy ttl after the claim time.
    assert_eq!(lease.expires_at, ts("2026-05-29T01:30:00Z"));
    assert!(!lease.is_expired(now));
    assert!(lease.is_expired(lease.expires_at));
}

#[test]
fn acquire_steals_an_expired_lease() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");
    // Past the original expiry, a different worker may take over.
    let now = ts("2026-05-29T02:00:00Z");

    let lease = planner
        .acquire(Some(&current), RoleId::new("engineer"), "run-2", now)
        .expect("an expired lease can be reclaimed");

    assert_eq!(lease.worker, "run-2");
    assert_eq!(lease.claimed_at, now, "a steal starts a new claim episode");
    assert_eq!(lease.expires_at, ts("2026-05-29T02:30:00Z"));
}

#[test]
fn acquire_by_holder_preserves_claim_time() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");
    let now = ts("2026-05-29T00:10:00Z");

    let lease = planner
        .acquire(Some(&current), RoleId::new("engineer"), "run-1", now)
        .expect("the holder can refresh its own unexpired lease");

    assert_eq!(
        lease.claimed_at, current.claimed_at,
        "claim start is preserved"
    );
    assert_eq!(lease.heartbeat_at, now);
    assert_eq!(lease.expires_at, ts("2026-05-29T00:40:00Z"));
}

#[test]
fn acquire_rejects_a_live_lease_held_by_another_worker() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");
    let now = ts("2026-05-29T00:10:00Z");

    let conflict = planner
        .acquire(Some(&current), RoleId::new("engineer"), "run-2", now)
        .expect_err("a live lease cannot be stolen");

    assert_eq!(
        conflict,
        LeaseConflict::HeldByOther {
            holder: "run-1".to_string(),
            worker: "run-2".to_string(),
        }
    );
}

#[test]
fn heartbeat_extends_the_lease() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");
    let now = ts("2026-05-29T00:20:00Z");

    let lease = planner
        .heartbeat(Some(&current), "run-1", now)
        .expect("the holder can heartbeat");

    assert_eq!(lease.claimed_at, current.claimed_at);
    assert_eq!(lease.heartbeat_at, now);
    assert_eq!(
        lease.expires_at,
        ts("2026-05-29T00:50:00Z"),
        "heartbeat pushes expiry to now + ttl"
    );
}

#[test]
fn heartbeat_without_a_lease_is_not_held() {
    let planner = LeasePlanner::new(policy());
    let conflict = planner
        .heartbeat(None, "run-1", ts("2026-05-29T00:20:00Z"))
        .expect_err("there is nothing to heartbeat");
    assert_eq!(
        conflict,
        LeaseConflict::NotHeld {
            worker: "run-1".to_string()
        }
    );
}

#[test]
fn heartbeat_by_another_worker_conflicts() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");
    let conflict = planner
        .heartbeat(Some(&current), "run-2", ts("2026-05-29T00:20:00Z"))
        .expect_err("a peer cannot heartbeat another worker's lease");
    assert!(matches!(conflict, LeaseConflict::HeldByOther { .. }));
}

#[test]
fn release_is_idempotent_for_the_holder_and_empty_state() {
    let planner = LeasePlanner::new(policy());
    let current = held_lease("run-1");

    assert_eq!(
        planner.release(Some(&current), "run-1"),
        Ok(None),
        "the holder clears its lease"
    );
    assert_eq!(
        planner.release(None, "run-1"),
        Ok(None),
        "releasing an already-empty lease is a no-op"
    );
    assert_eq!(
        planner.release(Some(&current), "run-2"),
        Err(LeaseConflict::HeldByOther {
            holder: "run-1".to_string(),
            worker: "run-2".to_string(),
        }),
        "a peer cannot release another worker's lease"
    );
}

#[test]
fn manager_writes_lease_into_issue_metadata() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "Implement login.");

    let manager = LeaseManager::new(&forge, policy());
    let now = ts("2026-05-29T00:00:00Z");
    let lease = block_on(manager.acquire(
        &repo,
        ArtifactSource::Issue { number },
        RoleId::new("engineer"),
        "run-1",
        now,
    ))
    .expect("the issue can be claimed");

    assert_eq!(lease.expires_at, ts("2026-05-29T00:30:00Z"));

    // The lease is persisted in the artifact's metadata block, and the human
    // prose is preserved alongside it.
    let body = issue_body(&forge, &repo, number);
    assert!(body.contains("Implement login."));
    let metadata = parse_metadata_block(&body)
        .expect("body metadata parses")
        .expect("body has a metadata block");
    assert_eq!(metadata.lease, Some(lease));
}

#[test]
fn manager_heartbeat_then_release_round_trips() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "");
    let target = ArtifactSource::Issue { number };
    let manager = LeaseManager::new(&forge, policy());

    block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-1",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("claimed");

    let beat = block_on(manager.heartbeat(&repo, target, "run-1", ts("2026-05-29T00:20:00Z")))
        .expect("the holder heartbeats");
    assert_eq!(beat.expires_at, ts("2026-05-29T00:50:00Z"));
    assert_eq!(beat.claimed_at, ts("2026-05-29T00:00:00Z"));

    block_on(manager.release(&repo, target, "run-1")).expect("the holder releases");
    let metadata = parse_metadata_block(&issue_body(&forge, &repo, number))
        .expect("body metadata parses")
        .expect("body still has a metadata block");
    assert_eq!(metadata.lease, None, "release clears the lease");
}

#[test]
fn manager_release_without_lease_does_not_append_empty_metadata() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "Authored body");
    let target = ArtifactSource::Issue { number };
    let manager = LeaseManager::new(&forge, policy());

    block_on(manager.release(&repo, target, "run-1")).expect("empty release is a no-op");

    let body = issue_body(&forge, &repo, number);
    assert_eq!(body, "Authored body");
    assert_eq!(
        parse_metadata_block(&body).expect("body metadata parses"),
        None
    );
}

#[test]
fn manager_rejects_a_conflicting_acquire() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "");
    let target = ArtifactSource::Issue { number };
    let manager = LeaseManager::new(&forge, policy());

    block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-1",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("first worker claims");

    let error = block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-2",
        ts("2026-05-29T00:10:00Z"),
    ))
    .expect_err("a second worker cannot steal a live lease");
    assert!(matches!(
        error,
        LeaseError::Conflict(LeaseConflict::HeldByOther { .. })
    ));
}

#[test]
fn interleaved_acquirers_cannot_both_win_the_same_unclaimed_issue() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "Implement login.");
    let target = ArtifactSource::Issue { number };
    let manager = LeaseManager::new(&forge, policy());

    // A and B both load the same "no lease" snapshot and plan a grant *before*
    // either writes, each capturing the artifact's load-time version. This is
    // the lost-update interleaving: A-load, B-load, A-write, B-write.
    let a = block_on(manager.prepare_acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("A plans a grant on the unclaimed issue");
    let b = block_on(manager.prepare_acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-b",
        ts("2026-05-29T00:00:01Z"),
    ))
    .expect("B plans a grant against the same unclaimed snapshot");
    assert_eq!(
        a.version(),
        b.version(),
        "both acquirers captured the same load-time version"
    );

    // A commits first and wins; the conditional write advances the version.
    let granted = block_on(manager.commit(a)).expect("A wins the compare-and-swap");
    assert_eq!(granted.worker, "run-a");

    // B commits against its now-stale captured version and loses the
    // compare-and-swap — without any hand-serialized re-read.
    let lost = block_on(manager.commit(b)).expect_err("B's stale write is refused");
    assert!(
        matches!(lost, LeaseError::Contended { target: t } if t == target),
        "the loser observes a contention conflict: {lost:?}"
    );

    // Exactly one lease is recorded, held by A.
    let metadata = parse_metadata_block(&issue_body(&forge, &repo, number))
        .expect("metadata parses")
        .expect("metadata present");
    assert_eq!(
        metadata.lease.expect("one lease recorded").worker,
        "run-a",
        "the first committer keeps the claim"
    );
}

#[test]
fn manager_reports_a_missing_target() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let manager = LeaseManager::new(&forge, policy());

    let error = block_on(manager.acquire(
        &repo,
        ArtifactSource::Issue {
            number: ItemNumber::new(999),
        },
        RoleId::new("engineer"),
        "run-1",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect_err("a missing artifact cannot be leased");
    assert!(matches!(error, LeaseError::TargetMissing { .. }));
}
