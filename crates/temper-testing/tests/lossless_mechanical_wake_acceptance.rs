//! Executable traceability for all eight issue #316 acceptance criteria.
//!
//! The matrix names focused deterministic evidence rather than duplicating race,
//! transport, targeting, staged-artifact, convergence, or tracing fixtures.

use std::collections::BTreeSet;

struct Evidence {
    crate_path: &'static str,
    source: &'static str,
    test: &'static str,
}

struct Criterion {
    name: &'static str,
    evidence: &'static [Evidence],
}

const RACES: &str = include_str!("mechanical_wake_races.rs");
const OBSERVABILITY: &str = include_str!("mechanical_observability.rs");
const HANDLERS: &str = include_str!("../../temper-engine/src/daemon/handlers_tests.rs");
const MACHINE: &str = include_str!("../../temper-engine/src/daemon/machine_tests.rs");
const COORDINATOR: &str = include_str!("../../temper-engine/src/daemon/wake_coordinator_tests.rs");
const CHANGE_SOURCE: &str = include_str!("../../temper-engine/tests/change_source_wake.rs");
const MECHANICAL_BACKSTOP: &str = include_str!("../../temper-engine/tests/mechanical_backstop.rs");
const TARGETED_ROLE: &str = include_str!("../../temper-runner/tests/scan/targeted_role.rs");
const MECHANICAL_AUTOMATION: &str =
    include_str!("../../temper-runner/tests/mechanical_automation.rs");

const MATRIX: &[Criterion] = &[
    Criterion {
        name: "1: stale active reads cause an immediate fresh exact follow-up",
        evidence: &[Evidence {
            crate_path: "temper-testing::mechanical_wake_races",
            source: RACES,
            test: "ci_change_after_stale_active_read_lands_in_immediate_exact_follow_up",
        }],
    },
    Criterion {
        name: "2: concurrent hints are bounded and unioned without a delivery queue",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "duplicate_targets_dedupe_and_the_thirty_third_promotes_to_broad",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "heartbeat_burst_keeps_ci_target_bounded_and_ahead_of_broad_work",
            },
        ],
    },
    Criterion {
        name: "3: CI and PR hints use exact targeting without staged-child mutation",
        evidence: &[
            Evidence {
                crate_path: "temper-runner::mechanical_automation",
                source: MECHANICAL_AUTOMATION,
                test: "targeted_ci_wake_lands_pr_without_terminal_list_queries",
            },
            Evidence {
                crate_path: "temper-runner::scan::targeted_role",
                source: TARGETED_ROLE,
                test: "targeted_pr_unions_signal_needs_once_and_emits_subscribers_deterministically",
            },
            Evidence {
                crate_path: "temper-engine::mechanical_backstop",
                source: MECHANICAL_BACKSTOP,
                test: "targeted_mechanical_wake_does_not_mutate_staged_artifact",
            },
        ],
    },
    Criterion {
        name: "4: heartbeat and broad traffic cannot starve an exact CI reaction",
        evidence: &[
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "heartbeat_burst_keeps_ci_target_bounded_and_ahead_of_broad_work",
            },
            Evidence {
                crate_path: "temper-engine::daemon::handlers",
                source: HANDLERS,
                test: "proven_heartbeat_is_acknowledged_before_suppression_accounting",
            },
        ],
    },
    Criterion {
        name: "5: a periodic broad tick survives an in-flight targeted pass",
        evidence: &[Evidence {
            crate_path: "temper-testing::mechanical_wake_races",
            source: RACES,
            test: "mechanical_poll_racing_targeted_work_runs_one_immediate_broad_follow_up",
        }],
    },
    Criterion {
        name: "6: deterministic paused-Forge tests cover stale, burst, and poll races",
        evidence: &[
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "ci_change_after_stale_active_read_lands_in_immediate_exact_follow_up",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "heartbeat_burst_keeps_ci_target_bounded_and_ahead_of_broad_work",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "mechanical_poll_racing_targeted_work_runs_one_immediate_broad_follow_up",
            },
        ],
    },
    Criterion {
        name: "7: traces distinguish wake lifecycle, phases, gate reads, and landing attempts",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::machine",
                source: MACHINE,
                test: "wake_measurements_carry_stable_run_id_scope_counts_and_latencies",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_observability",
                source: OBSERVABILITY,
                test: "broad_phase_measurements_include_provider_deltas_and_non_merge_has_no_attempt",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_observability",
                source: OBSERVABILITY,
                test: "targeted_phases_and_repeated_gate_observations_keep_wake_correlation",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_observability",
                source: OBSERVABILITY,
                test: "landing_attempt_pairs_started_with_applied_terminal_outcome",
            },
            Evidence {
                crate_path: "temper-testing::mechanical_observability",
                source: OBSERVABILITY,
                test: "failed_targeted_scan_emits_terminal_duration_provider_delta_and_wake_id",
            },
        ],
    },
    Criterion {
        name: "8: immediate acknowledgement and startup/poll backstops retain convergence",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::handlers",
                source: HANDLERS,
                test: "verified_webhook_acks_before_wake_scan_finishes",
            },
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "repository_unknown_push_recovery_poll_and_startup_requests_are_broad",
            },
            Evidence {
                crate_path: "temper-engine::change_source_wake",
                source: CHANGE_SOURCE,
                test: "poll_backstop_assigns_work_when_change_hints_are_missing",
            },
        ],
    },
];

#[test]
fn issue_316_acceptance_matrix_names_live_deterministic_regressions() {
    assert_eq!(
        MATRIX.len(),
        8,
        "every #316 acceptance criterion stays represented"
    );
    let mut criteria = BTreeSet::new();
    for criterion in MATRIX {
        assert!(
            criteria.insert(criterion.name),
            "duplicate criterion: {}",
            criterion.name
        );
        assert!(
            !criterion.evidence.is_empty(),
            "{} has no evidence",
            criterion.name
        );
        for evidence in criterion.evidence {
            let ordinary = format!("fn {}(", evidence.test);
            let asynchronous = format!("async fn {}(", evidence.test);
            assert!(
                evidence.source.contains(&ordinary) || evidence.source.contains(&asynchronous),
                "{} no longer exposes deterministic test `{}` for `{}`",
                evidence.crate_path,
                evidence.test,
                criterion.name
            );
        }
    }
}
