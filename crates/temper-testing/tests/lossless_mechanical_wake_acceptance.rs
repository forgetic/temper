//! Executable traceability for issue #316's mechanical wake race evidence.
//!
//! The race suite owns only the new paused-Forge scenarios. Existing transport,
//! recovery, targeting, and staged-artifact regressions remain in their original
//! focused fixtures and are referenced here instead of being duplicated.

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
const HANDLERS: &str = include_str!("../../temper-engine/src/daemon/handlers_tests.rs");
const COORDINATOR: &str = include_str!("../../temper-engine/src/daemon/wake_coordinator_tests.rs");
const CHANGE_SOURCE: &str = include_str!("../../temper-engine/tests/change_source_wake.rs");
const MECHANICAL_BACKSTOP: &str = include_str!("../../temper-engine/tests/mechanical_backstop.rs");
const TARGETED_ROLE: &str = include_str!("../../temper-runner/tests/scan/targeted_role.rs");
const MECHANICAL_AUTOMATION: &str =
    include_str!("../../temper-runner/tests/mechanical_automation.rs");

const MATRIX: &[Criterion] = &[
    Criterion {
        name: "stale active reads cause an immediate fresh exact follow-up",
        evidence: &[Evidence {
            crate_path: "temper-testing::mechanical_wake_races",
            source: RACES,
            test: "ci_change_after_stale_active_read_lands_in_immediate_exact_follow_up",
        }],
    },
    Criterion {
        name: "heartbeat and broad bursts stay bounded without losing CI priority",
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
        name: "periodic broad work survives an in-flight targeted pass",
        evidence: &[
            Evidence {
                crate_path: "temper-testing::mechanical_wake_races",
                source: RACES,
                test: "mechanical_poll_racing_targeted_work_runs_one_immediate_broad_follow_up",
            },
            Evidence {
                crate_path: "temper-engine::change_source_wake",
                source: CHANGE_SOURCE,
                test: "poll_backstop_assigns_work_when_change_hints_are_missing",
            },
        ],
    },
    Criterion {
        name: "verified webhook acknowledgement is independent of wake execution",
        evidence: &[Evidence {
            crate_path: "temper-engine::daemon::handlers",
            source: HANDLERS,
            test: "verified_webhook_acks_before_wake_scan_finishes",
        }],
    },
    Criterion {
        name: "startup and missed-hint recovery retain broad convergence",
        evidence: &[
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
    Criterion {
        name: "targeted reads preserve exact query budgets and role signal unioning",
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
        ],
    },
    Criterion {
        name: "targeted and broad mechanical paths exclude staged artifacts",
        evidence: &[Evidence {
            crate_path: "temper-engine::mechanical_backstop",
            source: MECHANICAL_BACKSTOP,
            test: "targeted_mechanical_wake_does_not_mutate_staged_artifact",
        }],
    },
];

#[test]
fn issue_316_acceptance_matrix_names_live_deterministic_regressions() {
    assert_eq!(MATRIX.len(), 7, "every #316 evidence row stays represented");
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
