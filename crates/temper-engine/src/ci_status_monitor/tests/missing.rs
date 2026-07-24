// SPDX-License-Identifier: MPL-2.0

use std::sync::Mutex;

use super::*;

fn missing_observation(number: u64, head_sha: &str) -> CiStatusObservation {
    CiStatusObservation {
        pull_request_number: ItemNumber::new(number),
        head_sha: head_sha.to_string(),
        current_head_ci_present: false,
        state: CiState::Pending,
        completed_at: None,
        terminal_evidence: Vec::new(),
    }
}

fn controlled_monitor(now: &str, grace: Duration) -> (CiStatusMonitor, Arc<Mutex<DateTime<Utc>>>) {
    let now = Arc::new(Mutex::new(timestamp(now)));
    let clock_now = Arc::clone(&now);
    let clock: WallClock = Arc::new(move || *clock_now.lock().expect("test clock"));
    (CiStatusMonitor::new(grace, clock), now)
}

fn missing_transition(transition: &CiStatusTransition) -> &CiMissingCurrentHeadTransition {
    let CiStatusTransition::MissingCurrentHead(transition) = transition else {
        panic!("expected missing-current-head transition, got {transition:?}");
    };
    transition
}

#[test]
fn missing_current_head_reemits_after_grace_until_observation_changes() {
    let repository = target("repo-1", "acme", "service");
    let (mut monitor, now) = controlled_monitor("2026-07-21T10:00:00Z", Duration::from_secs(300));

    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-missing")],)
            .is_empty()
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:04:59Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-missing")],)
            .is_empty()
    );

    *now.lock().expect("test clock") = timestamp("2026-07-21T10:05:00Z");
    let expired = monitor
        .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-missing")]);
    assert_eq!(expired.len(), 1);
    let expired = missing_transition(&expired[0]);
    assert_eq!(
        expired.hint,
        ChangeHint::pull_request(repository.path.clone(), ItemNumber::new(7), ChangeKind::Ci,)
    );
    assert_eq!(expired.head_sha, "head-missing");
    assert_eq!(expired.first_observed_at, timestamp("2026-07-21T10:00:00Z"));

    *now.lock().expect("test clock") = timestamp("2026-07-21T11:00:00Z");
    let retry = monitor
        .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-missing")]);
    assert_eq!(retry.len(), 1, "expired missing intervals remain retryable");
    assert_eq!(
        missing_transition(&retry[0]).first_observed_at,
        timestamp("2026-07-21T10:00:00Z"),
        "retries retain the original uninterrupted observation time"
    );
}

#[test]
fn present_pending_work_cancels_recovery_and_later_missing_starts_a_new_window() {
    let repository = target("repo-1", "acme", "service");
    let (mut monitor, now) = controlled_monitor("2026-07-21T10:00:00Z", Duration::from_secs(300));
    monitor.observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")]);

    *now.lock().expect("test clock") = timestamp("2026-07-21T10:04:00Z");
    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(7, "head-1", CiState::Pending, None)],
            )
            .is_empty()
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:06:00Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")],)
            .is_empty(),
        "the second interval starts when jobs disappear again"
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:10:59Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")],)
            .is_empty()
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:11:00Z");
    assert_eq!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")],)
            .len(),
        1
    );
}

#[test]
fn leaving_a_successful_snapshot_clears_the_missing_window() {
    let repository = target("repo-1", "acme", "service");
    let (mut monitor, now) = controlled_monitor("2026-07-21T10:00:00Z", Duration::from_secs(300));
    monitor.observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")]);

    *now.lock().expect("test clock") = timestamp("2026-07-21T10:04:00Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, Vec::new())
            .is_empty()
    );
    assert!(monitor.observations.is_empty());

    *now.lock().expect("test clock") = timestamp("2026-07-21T10:06:00Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")])
            .is_empty(),
        "returning to the snapshot starts a fresh interval"
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:11:00Z");
    let transitions =
        monitor.observe_repository_snapshot(&repository, vec![missing_observation(7, "head-1")]);
    assert_eq!(transitions.len(), 1);
    assert_eq!(
        missing_transition(&transitions[0]).first_observed_at,
        timestamp("2026-07-21T10:06:00Z")
    );
}

#[test]
fn head_change_resets_missing_window_and_transition_names_the_new_head() {
    let repository = target("repo-1", "acme", "service");
    let (mut monitor, now) = controlled_monitor("2026-07-21T10:00:00Z", Duration::from_secs(300));
    monitor.observe_repository_snapshot(&repository, vec![missing_observation(7, "head-old")]);

    *now.lock().expect("test clock") = timestamp("2026-07-21T10:05:00Z");
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![missing_observation(7, "head-new")],)
            .is_empty(),
        "the old head's elapsed window must not apply to the new head"
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T10:10:00Z");
    let transitions =
        monitor.observe_repository_snapshot(&repository, vec![missing_observation(7, "head-new")]);
    assert_eq!(transitions.len(), 1);
    let transition = missing_transition(&transitions[0]);
    assert_eq!(transition.head_sha, "head-new");
    assert_eq!(
        transition.first_observed_at,
        timestamp("2026-07-21T10:05:00Z")
    );
    assert_eq!(monitor.observations.len(), 1);
}

#[test]
fn queued_current_head_run_without_jobs_never_ages_as_missing() {
    let forge = MemoryForge::new();
    let repository = create_repository(&forge, "queued-run");
    let pull_request = create_pull_request(&forge, &repository, "head-queued");
    forge.seed_ci_run(&repository.id, Some(&pull_request.id), "head-queued");
    let repositories = RepositorySet::new(vec![repository]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let (mut monitor, now) = controlled_monitor("2026-07-21T12:00:00Z", Duration::from_secs(300));

    assert!(
        block_on(run_ci_status_monitor_tick(
            &mut monitor,
            &forge,
            &repositories,
            &workflow,
            &compiled,
        ))
        .is_empty()
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T13:00:00Z");
    assert!(
        block_on(run_ci_status_monitor_tick(
            &mut monitor,
            &forge,
            &repositories,
            &workflow,
            &compiled,
        ))
        .is_empty(),
        "registered current-head CI remains pending beyond the missing grace"
    );
    assert!(matches!(
        monitor.observations.values().next(),
        Some(RecordedObservation::Present {
            state: CiState::Pending,
            ..
        })
    ));
}

#[test]
fn failed_repository_read_does_not_advance_missing_grace() {
    let forge = MemoryForge::new();
    let repository = create_repository(&forge, "missing");
    let pull_request = create_pull_request(&forge, &repository, "head-missing");
    let repositories = RepositorySet::new(vec![repository.clone()]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let (mut monitor, now) = controlled_monitor("2026-07-21T12:00:00Z", Duration::from_secs(300));

    assert!(
        block_on(run_ci_status_monitor_tick(
            &mut monitor,
            &forge,
            &repositories,
            &workflow,
            &compiled,
        ))
        .is_empty()
    );
    *now.lock().expect("test clock") = timestamp("2026-07-21T12:05:00Z");
    forge.fail_next(
        FaultOp::ListPullRequests,
        "repository temporarily unavailable",
    );
    assert!(
        block_on(run_ci_status_monitor_tick(
            &mut monitor,
            &forge,
            &repositories,
            &workflow,
            &compiled,
        ))
        .is_empty(),
        "a failed read cannot emit expiry from retained evidence"
    );

    let recovered = block_on(run_ci_status_monitor_tick(
        &mut monitor,
        &forge,
        &repositories,
        &workflow,
        &compiled,
    ));
    assert_eq!(recovered.len(), 1);
    let recovered = missing_transition(&recovered[0]);
    assert_eq!(recovered.head_sha, "head-missing");
    assert_eq!(
        recovered.hint,
        ChangeHint::pull_request(repository.path.clone(), pull_request.number, ChangeKind::Ci,)
    );
}
