// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_workflow::RoleId;

fn t(nanos: u64) -> EngineTime {
    EngineTime::from_nanos(nanos)
}

fn repo(owner: &str, name: &str) -> RepositoryPath {
    RepositoryPath::new(owner, name)
}

fn role(name: &str) -> WakeLane {
    WakeLane::Role(RoleId::new(name))
}

fn configured(
    debounce: Duration,
    cap: usize,
    repository: &RepositoryPath,
    lanes: impl IntoIterator<Item = WakeLane>,
) -> WakeCoordinator {
    let mut coordinator = WakeCoordinator::new(debounce, cap);
    coordinator.configure_repository(repository.clone(), lanes);
    coordinator
}

fn timer(decisions: &[WakeDecision]) -> Option<u64> {
    decisions.iter().find_map(|decision| match decision {
        WakeDecision::StartTimer { generation, .. } => Some(*generation),
        _ => None,
    })
}

fn started(decisions: &[WakeDecision]) -> Option<WakeWork> {
    decisions.iter().find_map(|decision| match decision {
        WakeDecision::Started { work } => Some(work.clone()),
        _ => None,
    })
}

fn targeted(
    repository: &RepositoryPath,
    lanes: impl IntoIterator<Item = WakeLane>,
    number: u64,
) -> WakeRequest {
    WakeRequest::targeted_for_lanes(
        repository.clone(),
        lanes,
        HintArtifactKind::Issue,
        ItemNumber::new(number),
        ChangeKind::Label,
    )
}

#[test]
fn duplicate_targets_dedupe_and_the_thirty_third_promotes_to_broad() {
    let repository = repo("ai", "temper");
    let lane = role("engineer");
    let mut coordinator = configured(Duration::from_millis(5), 2, &repository, [lane.clone()]);

    let first = coordinator.schedule(t(1), targeted(&repository, [lane.clone()], 1), false);
    let first_generation = timer(&first).expect("first hint arms timer");
    let duplicate = coordinator.schedule(t(2), targeted(&repository, [lane.clone()], 1), false);
    assert!(matches!(duplicate[0], WakeDecision::Coalesced { .. }));
    assert!(
        timer(&duplicate).is_none(),
        "duplicates do not postpone the leading edge"
    );

    for number in 2..=32 {
        let decisions = coordinator.schedule(
            t(number),
            targeted(&repository, [lane.clone()], number),
            false,
        );
        assert!(timer(&decisions).is_none());
    }
    let state = coordinator.repository_state(&repository).unwrap();
    assert_eq!(state.pending.scope(&lane).unwrap().target_count(), 32);
    assert_eq!(state.timer_generation(), Some(first_generation));

    let overflow = coordinator.schedule(t(33), targeted(&repository, [lane.clone()], 33), false);
    assert!(overflow.iter().any(|decision| matches!(
        decision,
        WakeDecision::BroadPromoted {
            mode: BroadMode::Overflow,
            ..
        }
    )));
    let scope = coordinator
        .repository_state(&repository)
        .unwrap()
        .pending
        .scope(&lane)
        .unwrap();
    assert_eq!(scope.broad_mode(), Some(BroadMode::Overflow));
    assert_eq!(scope.target_count(), 0);
}

#[test]
fn repository_unknown_push_recovery_poll_and_startup_requests_are_broad() {
    let repository = repo("ai", "temper");
    let lane = role("engineer");
    let modes = [
        BroadMode::Repository,
        BroadMode::Unknown,
        BroadMode::Push,
        BroadMode::Recovery,
        BroadMode::Poll,
        BroadMode::Startup,
    ];
    for mode in modes {
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [lane.clone()]);
        coordinator.schedule(
            t(0),
            WakeRequest::broad_for_lanes(repository.clone(), [lane.clone()], mode),
            false,
        );
        assert!(matches!(
            coordinator
                .repository_state(&repository)
                .unwrap()
                .pending
                .scope(&lane),
            Some(WakeScope::Broad { .. })
        ));
    }
}

#[test]
fn one_hundred_hints_during_nested_applies_defer_to_one_repository_generation() {
    let repository = repo("ai", "temper");
    let lanes = [role("architect"), role("engineer"), WakeLane::Mechanical];
    let mut coordinator = configured(Duration::from_millis(10), 2, &repository, lanes.clone());

    coordinator.begin_apply();
    for number in 0..100 {
        let decisions = coordinator.schedule(
            t(number),
            targeted(&repository, lanes.clone(), (number % 4) + 1),
            true,
        );
        assert!(timer(&decisions).is_none());
        assert!(started(&decisions).is_none());
        assert!(
            decisions
                .iter()
                .any(|decision| matches!(decision, WakeDecision::Deferred { .. }))
        );
    }
    // A nested apply completion is represented by the machine retaining a
    // non-empty applying set, so promotion is intentionally not called.
    assert!(
        coordinator
            .repository_state(&repository)
            .unwrap()
            .pending
            .is_empty()
    );

    let promoted = coordinator.promote_apply_deferred();
    assert_eq!(
        promoted
            .iter()
            .filter(|decision| matches!(decision, WakeDecision::Promoted { .. }))
            .count(),
        1
    );
    assert_eq!(
        promoted
            .iter()
            .filter(|decision| matches!(decision, WakeDecision::StartTimer { .. }))
            .count(),
        1
    );
    assert_eq!(
        coordinator
            .repository_state(&repository)
            .unwrap()
            .pending
            .len(),
        3
    );
}

#[test]
fn apply_start_invalidates_an_armed_generation_and_stale_timer_is_ignored() {
    let repository = repo("ai", "temper");
    let lane = role("engineer");
    let mut coordinator = configured(Duration::from_millis(10), 2, &repository, [lane.clone()]);
    let old_generation =
        timer(&coordinator.schedule(t(0), targeted(&repository, [lane], 1), false)).unwrap();

    coordinator.begin_apply();
    let stale = coordinator.timer_elapsed(t(10), repository.clone(), old_generation, true);
    assert!(matches!(stale[0], WakeDecision::IgnoredStaleTimer { .. }));
    let promoted = coordinator.promote_apply_deferred();
    let new_generation = timer(&promoted).unwrap();
    assert_ne!(old_generation, new_generation);
}

#[test]
fn hints_during_a_failed_run_make_one_lane_specific_dirty_follow_up() {
    let repository = repo("ai", "temper");
    let engineer = role("engineer");
    let reviewer = role("reviewer");
    let mut coordinator = configured(
        Duration::from_millis(10),
        2,
        &repository,
        [engineer.clone(), reviewer.clone()],
    );
    let generation =
        timer(&coordinator.schedule(t(0), targeted(&repository, [engineer.clone()], 1), false))
            .unwrap();
    let work =
        started(&coordinator.timer_elapsed(t(10), repository.clone(), generation, false)).unwrap();

    for _ in 0..20 {
        coordinator.schedule(t(11), targeted(&repository, [engineer.clone()], 2), false);
    }
    coordinator.schedule(t(12), targeted(&repository, [reviewer.clone()], 3), false);
    let finished = coordinator.finish(
        t(20),
        &work,
        WakeOutcome::Failed {
            reason: "forge unavailable".to_string(),
        },
        false,
    );
    let follow_up_generation = timer(&finished).expect("dirty work gets one timer");
    let lanes = finished
        .iter()
        .find_map(|decision| match decision {
            WakeDecision::DirtyFollowUp { lanes, .. } => Some(lanes),
            _ => None,
        })
        .unwrap();
    assert_eq!(lanes, &BTreeSet::from([engineer, reviewer]));

    let follow_up =
        started(&coordinator.timer_elapsed(t(30), repository, follow_up_generation, false))
            .unwrap();
    let no_retry = coordinator.finish(
        t(40),
        &follow_up,
        WakeOutcome::Failed {
            reason: "still unavailable".to_string(),
        },
        false,
    );
    assert!(timer(&no_retry).is_none(), "failure does not self-retry");
}

#[test]
fn final_apply_promotion_merges_into_dirty_when_repository_is_in_flight() {
    let repository = repo("ai", "temper");
    let lane = role("engineer");
    let mut coordinator = configured(Duration::ZERO, 2, &repository, [lane.clone()]);
    let generation =
        timer(&coordinator.schedule(t(0), targeted(&repository, [lane.clone()], 1), false))
            .unwrap();
    let work =
        started(&coordinator.timer_elapsed(t(1), repository.clone(), generation, false)).unwrap();

    coordinator.schedule(t(2), targeted(&repository, [lane.clone()], 2), true);
    let promoted = coordinator.promote_apply_deferred();
    assert!(promoted.iter().any(|decision| matches!(
        decision,
        WakeDecision::Promoted {
            destination: WakePromotion::Dirty,
            ..
        }
    )));
    assert!(timer(&promoted).is_none());

    let finished = coordinator.finish(t(3), &work, WakeOutcome::Succeeded, false);
    assert_eq!(
        finished
            .iter()
            .filter(|decision| matches!(decision, WakeDecision::DirtyFollowUp { .. }))
            .count(),
        1
    );
}

#[test]
fn global_cap_and_btree_drain_order_are_deterministic() {
    let repo_a = repo("acme", "a");
    let repo_b = repo("acme", "b");
    let repo_c = repo("acme", "c");
    let lane = role("engineer");
    let mut coordinator = WakeCoordinator::new(Duration::ZERO, 1);
    for repository in [&repo_a, &repo_b, &repo_c] {
        coordinator.configure_repository(repository.clone(), [lane.clone()]);
    }

    // C occupies the only permit. A and B then become ready while the cap
    // is full, despite their timer completions arriving in reverse order.
    let gen_c =
        timer(&coordinator.schedule(t(0), targeted(&repo_c, [lane.clone()], 1), false)).unwrap();
    let work_c = started(&coordinator.timer_elapsed(t(1), repo_c, gen_c, false)).unwrap();
    let gen_b =
        timer(&coordinator.schedule(t(2), targeted(&repo_b, [lane.clone()], 1), false)).unwrap();
    assert!(started(&coordinator.timer_elapsed(t(3), repo_b.clone(), gen_b, false)).is_none());
    let gen_a = timer(&coordinator.schedule(t(4), targeted(&repo_a, [lane], 1), false)).unwrap();
    assert!(started(&coordinator.timer_elapsed(t(5), repo_a.clone(), gen_a, false)).is_none());
    assert_eq!(coordinator.in_flight_repositories(), 1);

    let after_c = coordinator.finish(t(6), &work_c, WakeOutcome::Succeeded, false);
    let work_a = started(&after_c).expect("lexically first ready repository starts");
    assert_eq!(work_a.repo, repo_a);
    let after_a = coordinator.finish(t(7), &work_a, WakeOutcome::Succeeded, false);
    assert_eq!(started(&after_a).unwrap().repo, repo_b);
}

#[test]
fn opaque_repository_catalog_admits_only_its_configured_number_of_paths() {
    let lane = role("engineer");
    let mut coordinator = WakeCoordinator::new(Duration::ZERO, 2);
    coordinator.configure_unresolved_repositories([lane.clone()], 2);

    for (index, repository) in [repo("acme", "one"), repo("acme", "two")]
        .into_iter()
        .enumerate()
    {
        let decisions = coordinator.schedule(
            t(index as u64),
            targeted(&repository, [lane.clone()], 1),
            false,
        );
        assert!(timer(&decisions).is_some());
    }
    let rejected = coordinator.schedule(
        t(3),
        targeted(&repo("acme", "not-configured"), [lane], 1),
        false,
    );
    assert!(matches!(
        rejected[0],
        WakeDecision::IgnoredUnknownRepository { .. }
    ));
    assert_eq!(coordinator.repositories.len(), 2);
}

#[test]
fn unknown_repositories_cannot_grow_coordinator_state() {
    let known = repo("ai", "temper");
    let lane = role("engineer");
    let mut coordinator = configured(Duration::ZERO, 2, &known, [lane.clone()]);
    for number in 0..100 {
        let unknown = repo("unconfigured", &format!("repo-{number}"));
        let decisions = coordinator.schedule(
            t(number),
            targeted(&unknown, [lane.clone()], number + 1),
            false,
        );
        assert!(matches!(
            decisions[0],
            WakeDecision::IgnoredUnknownRepository { .. }
        ));
    }
    assert_eq!(coordinator.repositories.len(), 1);
}
