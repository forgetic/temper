// SPDX-License-Identifier: MPL-2.0

//! Production-consumer coverage for paged Forgejo Actions inventory.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use temper_engine_io::Spawner;
use temper_forge::{ChangeHint, ChangeKind};
use temper_forge_forgejo::HttpMethod;
use temper_runner::{ArtifactAddress, read_ci_status_observations};
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

#[path = "paged_forgejo/support.rs"]
mod support;
use support::*;

#[test]
fn later_page_green_run_drives_observation_monitor_and_targeted_landing() {
    let (forge, client) = build_forge(InventoryMode::GreenOnLaterPage);
    let workflow = landing_workflow();
    let compiled = workflow.compile();
    let target = repository_target();

    let observations = block_on(read_ci_status_observations(
        &forge, &target.id, &workflow, &compiled,
    ))
    .expect("paged runner observation succeeds");
    assert_eq!(observations.len(), 1);
    let observation = &observations[0];
    assert!(observation.current_head_ci_present);
    assert_eq!(observation.state, CiState::Passed);
    assert_eq!(observation.head_sha, HEAD);
    assert_eq!(
        observation
            .terminal_evidence
            .iter()
            .map(|evidence| (
                evidence.job_name.as_str(),
                evidence.job_id.as_str(),
                evidence.run_id.as_deref(),
                evidence.attempt.as_deref(),
            ))
            .collect::<Vec<_>>(),
        [
            (
                "build",
                "forgejo:acme/widgets:actions:900:31:1:41",
                Some("900"),
                Some("1"),
            ),
            (
                "test",
                "forgejo:acme/widgets:actions:901:32:2:42",
                Some("901"),
                Some("2"),
            ),
        ]
    );

    let mut monitor = CiStatusMonitor::new(
        Duration::from_secs(300),
        Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
    );
    let transitions = block_on(run_ci_status_monitor_tick(
        &mut monitor,
        &forge,
        &RepositorySet::new(vec![target.clone()]),
        &workflow,
        &compiled,
    ));
    assert_eq!(transitions.len(), 1);
    let transition = terminal_transition(&transitions[0]);
    assert_eq!(transition.verdict, CiTerminalVerdict::Passed);
    assert_eq!(transition.head_sha, HEAD);
    assert_eq!(transition.terminal_evidence, observation.terminal_evidence);

    let config = crate::MechanicalBackstopConfig {
        repositories: RepositorySet::new(vec![target.clone()]),
        cadence: Duration::from_secs(3_600),
        lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
        pull_request_merge_observer: None,
    };
    let progress = block_on(crate::run_mechanical_backstop_tick(
        &forge,
        &workflow,
        timestamp("2026-07-21T12:00:02Z"),
        &config,
        &[InMemoryJournal::new()],
        &crate::MechanicalScope::Targeted(vec![(
            target.path,
            ArtifactAddress::pull_request(ItemNumber::new(7)),
            ChangeKind::Ci,
        )]),
    ))
    .expect("exact-head-green PR remains mechanically eligible");
    assert!(progress.changed);
    assert!(client.merged());

    let job_reads = client
        .requests()
        .into_iter()
        .filter(|request| request.path.ends_with("/jobs"))
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert!(!job_reads.is_empty());
    assert!(job_reads.chunks_exact(2).all(|reads| {
        reads[0].ends_with("/actions/runs/901/jobs") && reads[1].ends_with("/actions/runs/900/jobs")
    }));
    assert_inventory_provenance(&forge);
}

#[test]
fn pagination_failures_stay_repository_errors_across_consumers() {
    let workflow = landing_workflow();
    let compiled = workflow.compile();
    let target = repository_target();

    for mode in InventoryMode::FAILURES {
        let (forge, client) = build_forge(mode);
        let result = block_on(read_ci_status_observations(
            &forge, &target.id, &workflow, &compiled,
        ));
        assert!(
            matches!(result, Err(temper_runner::ScanError::Forge(_))),
            "{mode:?} must propagate as a repository read error"
        );
        assert!(
            !client
                .requests()
                .iter()
                .any(|request| request.method != HttpMethod::Get),
            "{mode:?} runner failure must remain read-only"
        );
        assert_inventory_provenance(&forge);

        let (forge, _) = build_forge(mode);
        let clock_reads = Arc::new(AtomicUsize::new(0));
        let observed_clock_reads = Arc::clone(&clock_reads);
        let mut monitor = CiStatusMonitor::new(
            Duration::ZERO,
            Arc::new(move || {
                observed_clock_reads.fetch_add(1, Ordering::Relaxed);
                timestamp("2026-07-21T13:00:00Z")
            }),
        );
        for _ in 0..2 {
            assert!(
                block_on(run_ci_status_monitor_tick(
                    &mut monitor,
                    &forge,
                    &RepositorySet::new(vec![target.clone()]),
                    &workflow,
                    &compiled,
                ))
                .is_empty(),
                "{mode:?} must not emit missing or terminal recovery"
            );
        }
        assert_eq!(
            clock_reads.load(Ordering::Relaxed),
            0,
            "{mode:?} must not age the missing-CI grace interval"
        );
        assert_inventory_provenance(&forge);

        let (forge, client) = build_forge(mode);
        let recovery = block_on(crate::interrupted_ci_recovery::recover_interrupted_ci(
            &forge,
            &repository(),
            &workflow,
            &compiled,
            timestamp("2026-07-21T13:00:00Z"),
            ArtifactAddress::pull_request(ItemNumber::new(7)),
        ));
        let crate::interrupted_ci_recovery::InterruptedCiRecoveryOutcome::Retryable { reason } =
            recovery
        else {
            panic!("{mode:?} must remain retryable instead of advancing recovery");
        };
        assert!(
            reason.contains("current_head_jobs_read_failed"),
            "{mode:?} must retain the repository-read boundary: {reason}"
        );
        assert!(
            !client
                .requests()
                .iter()
                .any(|request| request.method != HttpMethod::Get),
            "{mode:?} interrupted-CI failure must not mutate"
        );
        assert_inventory_provenance(&forge);

        let (forge, client) = build_forge(mode);
        let config = crate::MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![target.clone()]),
            cadence: Duration::from_secs(3_600),
            lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
            pull_request_merge_observer: None,
        };
        let result = block_on(crate::run_mechanical_backstop_tick(
            &forge,
            &workflow,
            timestamp("2026-07-21T13:00:00Z"),
            &config,
            &[InMemoryJournal::new()],
            &crate::MechanicalScope::Targeted(vec![(
                target.path.clone(),
                ArtifactAddress::pull_request(ItemNumber::new(7)),
                ChangeKind::Ci,
            )]),
        ));
        assert!(result.is_err(), "{mode:?} must fail gate evaluation");
        assert!(!client.merged(), "{mode:?} must not authorize a merge");
        assert!(
            !client
                .requests()
                .iter()
                .any(|request| request.method != HttpMethod::Get),
            "{mode:?} gate failure must not mutate"
        );
        assert_inventory_provenance(&forge);
    }
}

#[test]
fn pagination_failure_cannot_trigger_missing_ci_recovery_mutations() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let spawner: Arc<dyn Spawner> = Arc::new(handle);
        let workflow = Arc::new(landing_workflow());
        let compiled = Arc::new(workflow.compile());
        let target = repository_target();

        for mode in InventoryMode::FAILURES {
            let (forge, client) = build_forge(mode);
            let forge = Arc::new(forge);
            let daemon = crate::Daemon::new(Arc::clone(&spawner)).with_wake_execution(
                Arc::clone(&forge),
                Arc::clone(&workflow),
                Arc::clone(&compiled),
                vec![crate::RoleFeedTarget {
                    repo: target.id.clone(),
                    path: target.path.clone(),
                    role: RoleId::new("engineer"),
                    mode: crate::RoleFeedMode::Wake,
                }],
                Arc::new(|| timestamp("2026-07-21T13:00:00Z")),
                None,
            );
            daemon.submit_ci_poll_transition(CiStatusTransition::MissingCurrentHead(
                CiMissingCurrentHeadTransition {
                    hint: ChangeHint::pull_request(
                        target.path.clone(),
                        ItemNumber::new(7),
                        ChangeKind::Ci,
                    ),
                    head_sha: HEAD.to_string(),
                    first_observed_at: timestamp("2026-07-21T12:55:00Z"),
                },
            ));
            temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(75)).await;

            let requests = client.requests();
            assert!(
                requests
                    .iter()
                    .any(|request| request.path.ends_with("/actions/runs")),
                "{mode:?} missing-CI recovery must revalidate shared inventory"
            );
            assert!(
                !requests
                    .iter()
                    .any(|request| request.method != HttpMethod::Get),
                "{mode:?} repository failure must not park or mutate the PR"
            );
            assert!(!client.merged());
            assert!(daemon.queued_jobs().await.is_empty());
            assert_inventory_provenance(forge.as_ref());
        }
    });
}
