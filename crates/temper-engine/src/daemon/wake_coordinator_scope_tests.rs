// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{CiTerminalTransition, CiTerminalVerdict};
use temper_workflow::RoleId;

fn t(nanos: u64) -> EngineTime {
    EngineTime::from_nanos(nanos)
}

fn repo(owner: &str, name: &str) -> RepositoryPath {
    RepositoryPath::new(owner, name)
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
    targeted_change(
        repository,
        lanes,
        HintArtifactKind::Issue,
        number,
        ChangeKind::Label,
    )
}

fn targeted_change(
    repository: &RepositoryPath,
    lanes: impl IntoIterator<Item = WakeLane>,
    kind: HintArtifactKind,
    number: u64,
    change: ChangeKind,
) -> WakeRequest {
    WakeRequest::targeted_for_lanes(
        repository.clone(),
        lanes,
        kind,
        ItemNumber::new(number),
        change,
    )
}

fn assert_mixed_mechanical_scope(scope: &WakeScope, number: u64) {
    assert!(scope.broad_mode().is_some(), "broad marker is retained");
    assert_eq!(
        scope
            .targets()
            .get(&(HintArtifactKind::PullRequest, ItemNumber::new(number)))
            .map(|target| target.change),
        Some(ChangeKind::Ci),
        "the exact CI target is retained beside broad work"
    );
}

#[test]
fn mechanical_broad_and_ci_merge_losslessly_in_pending_both_orders() {
    let repository = repo("ai", "temper");
    for broad_first in [true, false] {
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [WakeLane::Mechanical]);
        let broad = WakeRequest::broad_for_lanes(
            repository.clone(),
            [WakeLane::Mechanical],
            BroadMode::Poll,
        );
        let ci = targeted_change(
            &repository,
            [WakeLane::Mechanical],
            HintArtifactKind::PullRequest,
            41,
            ChangeKind::Ci,
        );
        let requests = if broad_first {
            [broad, ci]
        } else {
            [ci, broad]
        };
        for (index, request) in requests.into_iter().enumerate() {
            coordinator.schedule(t(index as u64), request, false);
        }
        assert_mixed_mechanical_scope(
            coordinator
                .repository_state(&repository)
                .unwrap()
                .pending
                .scope(&WakeLane::Mechanical)
                .unwrap(),
            41,
        );
    }
}

#[test]
fn mechanical_broad_and_ci_merge_losslessly_in_dirty_both_orders() {
    let repository = repo("ai", "temper");
    for broad_first in [true, false] {
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [WakeLane::Mechanical]);
        let generation = timer(&coordinator.schedule(
            t(0),
            targeted(&repository, [WakeLane::Mechanical], 1),
            false,
        ))
        .unwrap();
        let _in_flight =
            started(&coordinator.timer_elapsed(t(1), repository.clone(), generation, false))
                .unwrap();
        let broad = WakeRequest::broad_for_lanes(
            repository.clone(),
            [WakeLane::Mechanical],
            BroadMode::Poll,
        );
        let ci = targeted_change(
            &repository,
            [WakeLane::Mechanical],
            HintArtifactKind::PullRequest,
            42,
            ChangeKind::Ci,
        );
        let requests = if broad_first {
            [broad, ci]
        } else {
            [ci, broad]
        };
        for (index, request) in requests.into_iter().enumerate() {
            coordinator.schedule(t(index as u64 + 2), request, false);
        }
        assert_mixed_mechanical_scope(
            coordinator
                .repository_state(&repository)
                .unwrap()
                .dirty
                .scope(&WakeLane::Mechanical)
                .unwrap(),
            42,
        );
    }
}

#[test]
fn mechanical_broad_and_ci_merge_losslessly_while_apply_deferred_both_orders() {
    let repository = repo("ai", "temper");
    for broad_first in [true, false] {
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [WakeLane::Mechanical]);
        let broad = WakeRequest::broad_for_lanes(
            repository.clone(),
            [WakeLane::Mechanical],
            BroadMode::Recovery,
        );
        let ci = targeted_change(
            &repository,
            [WakeLane::Mechanical],
            HintArtifactKind::PullRequest,
            43,
            ChangeKind::Ci,
        );
        let requests = if broad_first {
            [broad, ci]
        } else {
            [ci, broad]
        };
        for (index, request) in requests.into_iter().enumerate() {
            coordinator.schedule(t(index as u64), request, true);
        }
        assert_mixed_mechanical_scope(
            coordinator
                .repository_state(&repository)
                .unwrap()
                .apply_deferred
                .scope(&WakeLane::Mechanical)
                .unwrap(),
            43,
        );
    }
}

#[test]
fn ci_target_at_capacity_evicts_lowest_priority_issue_and_marks_overflow() {
    let repository = repo("ai", "temper");
    let lane = WakeLane::Mechanical;
    let mut coordinator = configured(Duration::ZERO, 2, &repository, [lane.clone()]);
    for number in 1..=32 {
        coordinator.schedule(
            t(number),
            targeted(&repository, [lane.clone()], number),
            false,
        );
    }

    let decisions = coordinator.schedule(
        t(33),
        targeted_change(
            &repository,
            [lane.clone()],
            HintArtifactKind::PullRequest,
            99,
            ChangeKind::Ci,
        ),
        false,
    );
    assert!(decisions.iter().any(|decision| matches!(
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
    assert_eq!(scope.target_count(), MAX_TARGETED_ARTIFACTS);
    assert!(
        scope
            .targets()
            .contains_key(&(HintArtifactKind::PullRequest, ItemNumber::new(99)))
    );
    assert!(
        scope
            .targets()
            .contains_key(&(HintArtifactKind::Issue, ItemNumber::new(1)))
    );
    assert!(
        !scope
            .targets()
            .contains_key(&(HintArtifactKind::Issue, ItemNumber::new(32)))
    );
    assert_eq!(
        prioritized_targets(scope.targets())[0].0,
        (HintArtifactKind::PullRequest, ItemNumber::new(99))
    );
}

#[test]
fn duplicate_pr_changes_merge_semantically_and_keep_ci_behavior() {
    let repository = repo("ai", "temper");
    for changes in [
        [ChangeKind::Label, ChangeKind::Ci],
        [ChangeKind::Ci, ChangeKind::Edited],
    ] {
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [WakeLane::Mechanical]);
        for (index, change) in changes.into_iter().enumerate() {
            coordinator.schedule(
                t(index as u64),
                targeted_change(
                    &repository,
                    [WakeLane::Mechanical],
                    HintArtifactKind::PullRequest,
                    7,
                    change,
                ),
                false,
            );
        }
        let scope = coordinator
            .repository_state(&repository)
            .unwrap()
            .pending
            .scope(&WakeLane::Mechanical)
            .unwrap();
        assert_eq!(scope.target_count(), 1);
        assert_eq!(
            scope
                .targets()
                .get(&(HintArtifactKind::PullRequest, ItemNumber::new(7)))
                .map(|target| target.change),
            Some(ChangeKind::Ci)
        );
    }
}

#[test]
fn ci_provenance_coalesces_by_source_priority_in_both_orders() {
    let repository = repo("ai", "temper");
    for poll_first in [true, false] {
        let lane = WakeLane::Role(RoleId::new("engineer"));
        let mut coordinator = configured(Duration::ZERO, 2, &repository, [lane.clone()]);
        let hint =
            ChangeHint::pull_request(repository.clone(), ItemNumber::new(627), ChangeKind::Ci);
        let webhook = WakeRequest::from_webhook_hint(
            hint.clone(),
            Some(CiTerminalVerdict::Failed),
            Some("2026-07-21T10:00:00Z".parse().unwrap()),
        );
        let poll = WakeRequest::from_ci_poll_transition(CiTerminalTransition {
            hint,
            head_sha: "head-627".to_string(),
            verdict: CiTerminalVerdict::Passed,
            completed_at: Some("2026-07-21T10:00:01Z".parse().unwrap()),
        });
        let requests = if poll_first {
            [poll, webhook]
        } else {
            [webhook, poll]
        };
        for (index, request) in requests.into_iter().enumerate() {
            coordinator.schedule(t(index as u64), request, false);
        }

        let target = coordinator
            .repository_state(&repository)
            .unwrap()
            .pending
            .scope(&lane)
            .unwrap()
            .targets()
            .get(&(HintArtifactKind::PullRequest, ItemNumber::new(627)))
            .copied()
            .expect("CI target is retained");
        let facts = target.ci.expect("CI provenance is retained");
        assert_eq!(facts.source, CiTriggerSource::CiPoll);
        assert_eq!(facts.verdict, Some(CiTerminalVerdict::Passed));
        assert_eq!(
            facts.completed_at,
            Some("2026-07-21T10:00:01Z".parse().unwrap())
        );
    }
}

#[test]
fn role_broad_scope_retains_only_bounded_ci_provenance() {
    let repository = repo("ai", "temper");
    let lane = WakeLane::Role(RoleId::new("engineer"));
    let mut coordinator = configured(Duration::ZERO, 2, &repository, [lane.clone()]);
    coordinator.schedule(
        t(0),
        WakeRequest::broad_for_lanes(repository.clone(), [lane.clone()], BroadMode::Poll),
        false,
    );
    coordinator.schedule(
        t(1),
        WakeRequest::from_ci_poll_transition(CiTerminalTransition {
            hint: ChangeHint::pull_request(repository.clone(), ItemNumber::new(44), ChangeKind::Ci),
            head_sha: "head-44".to_string(),
            verdict: CiTerminalVerdict::Failed,
            completed_at: None,
        }),
        false,
    );

    let scope = coordinator
        .repository_state(&repository)
        .unwrap()
        .pending
        .scope(&lane)
        .unwrap();
    assert_eq!(scope.broad_mode(), Some(BroadMode::Poll));
    assert_eq!(scope.target_count(), 1);
    assert_eq!(
        scope
            .targets()
            .values()
            .next()
            .and_then(|target| target.ci)
            .map(|facts| facts.source),
        Some(CiTriggerSource::CiPoll)
    );
}

#[test]
fn ci_after_in_flight_broad_pass_creates_exact_dirty_follow_up() {
    let repository = repo("ai", "temper");
    let mut coordinator = configured(Duration::ZERO, 2, &repository, [WakeLane::Mechanical]);
    let generation = timer(&coordinator.schedule(
        t(0),
        WakeRequest::broad_for_lanes(repository.clone(), [WakeLane::Mechanical], BroadMode::Poll),
        false,
    ))
    .unwrap();
    let broad_work =
        started(&coordinator.timer_elapsed(t(1), repository.clone(), generation, false)).unwrap();
    assert_eq!(
        broad_work
            .batch
            .scope(&WakeLane::Mechanical)
            .unwrap()
            .target_count(),
        0
    );

    coordinator.schedule(
        t(2),
        targeted_change(
            &repository,
            [WakeLane::Mechanical],
            HintArtifactKind::PullRequest,
            314,
            ChangeKind::Ci,
        ),
        false,
    );
    let finished = coordinator.finish(t(3), &broad_work, WakeOutcome::Succeeded, false);
    assert_eq!(
        finished
            .iter()
            .filter(|decision| matches!(decision, WakeDecision::DirtyFollowUp { .. }))
            .count(),
        1
    );
    let follow_up_generation = timer(&finished).unwrap();
    let follow_up =
        started(&coordinator.timer_elapsed(t(4), repository, follow_up_generation, false)).unwrap();
    let scope = follow_up.batch.scope(&WakeLane::Mechanical).unwrap();
    assert_eq!(scope.broad_mode(), None);
    assert_eq!(
        scope
            .targets()
            .get(&(HintArtifactKind::PullRequest, ItemNumber::new(314)))
            .map(|target| target.change),
        Some(ChangeKind::Ci)
    );
}
