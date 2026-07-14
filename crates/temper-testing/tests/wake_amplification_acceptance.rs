//! Executable traceability matrix for issue #317's aggregate acceptance criteria.
//!
//! The prerequisite issues own the deterministic behavior tests. This suite is
//! intentionally non-ignored and fails if one of those named regressions is
//! removed or renamed without updating the aggregate acceptance map.

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

const COORDINATOR: &str = include_str!("../../temper-engine/src/daemon/wake_coordinator_tests.rs");
const MACHINE: &str = include_str!("../../temper-engine/src/daemon/machine_tests.rs");
const HANDLERS: &str = include_str!("../../temper-engine/src/daemon/handlers_tests.rs");
const WEBHOOK: &str = include_str!("../../temper-engine/src/webhook/tests.rs");
const CHANGE_SOURCE: &str = include_str!("../../temper-engine/tests/change_source_wake.rs");
const TARGETED_ROLE: &str = include_str!("../../temper-runner/tests/scan/targeted_role.rs");
const ROLE_SCAN: &str = include_str!("../../temper-runner/tests/scan/basic.rs");
const MECHANICAL: &str = include_str!("../../temper-runner/tests/mechanical_automation.rs");
const FAN_OUT: &str = include_str!("../../temper-workflow/tests/create_intent_recovery.rs");
const FORGEJO_BUDGET: &str = include_str!("forgejo_fanout_budget.rs");

const MATRIX: &[Criterion] = &[
    Criterion {
        name: "100 webhooks during apply defer to one repository follow-up",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "one_hundred_hints_during_nested_applies_defer_to_one_repository_generation",
            },
            Evidence {
                crate_path: "temper-engine::daemon::machine",
                source: MACHINE,
                test: "nested_applies_release_one_deferred_repository_generation_only_after_final_completion",
            },
        ],
    },
    Criterion {
        name: "duplicate and distinct in-flight hints stay bounded and promote targets",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "duplicate_targets_dedupe_and_the_thirty_third_promotes_to_broad",
            },
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "hints_during_a_failed_run_make_one_lane_specific_dirty_follow_up",
            },
        ],
    },
    Criterion {
        name: "repositories progress independently and roles share broad discovery",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::wake_coordinator",
                source: COORDINATOR,
                test: "global_cap_and_btree_drain_order_are_deterministic",
            },
            Evidence {
                crate_path: "temper-runner::scan",
                source: ROLE_SCAN,
                test: "broad_multi_role_wake_shares_one_candidate_query_plan",
            },
        ],
    },
    Criterion {
        name: "only proven heartbeats suppress and ambiguous edits broad-fallback",
        evidence: &[Evidence {
            crate_path: "temper-engine::webhook",
            source: WEBHOOK,
            test: "suppresses_only_proven_heartbeat_body_delta",
        }],
    },
    Criterion {
        name: "startup and poll recover lossy pending state",
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
        name: "issue PR review and CI targeting avoids unrelated terminal history",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::webhook",
                source: WEBHOOK,
                test: "payload_families_preserve_artifact_namespace",
            },
            Evidence {
                crate_path: "temper-runner::scan::targeted_role",
                source: TARGETED_ROLE,
                test: "targeted_issue_reads_only_the_selected_namespace_and_filters_roles",
            },
            Evidence {
                crate_path: "temper-runner::scan::targeted_role",
                source: TARGETED_ROLE,
                test: "targeted_pr_unions_signal_needs_once_and_emits_subscribers_deterministically",
            },
            Evidence {
                crate_path: "temper-runner::mechanical_automation",
                source: MECHANICAL,
                test: "targeted_ci_wake_lands_pr_without_terminal_list_queries",
            },
        ],
    },
    Criterion {
        name: "ten-child fan-out budgets and every uncertain checkpoint converge",
        evidence: &[
            Evidence {
                crate_path: "temper-workflow::create_intent_recovery",
                source: FAN_OUT,
                test: "known_first_core_operations_follow_child_and_dependent_child_formula",
            },
            Evidence {
                crate_path: "temper-workflow::create_intent_recovery",
                source: FAN_OUT,
                test: "ten_child_dag_converges_across_every_uncertain_pass_mutation",
            },
            Evidence {
                crate_path: "temper-testing::forgejo_fanout_budget",
                source: FORGEJO_BUDGET,
                test: "known_first_ten_child_fanout_stays_within_forgejo_http_budget",
            },
            Evidence {
                crate_path: "temper-testing::forgejo_fanout_budget (ignored local evidence)",
                source: FORGEJO_BUDGET,
                test: "local_forgejo_ten_child_fanout_meets_budget_and_crash_converges",
            },
        ],
    },
    Criterion {
        name: "verified transport acknowledges 202 before work and staged children stay excluded",
        evidence: &[
            Evidence {
                crate_path: "temper-engine::daemon::handlers",
                source: HANDLERS,
                test: "verified_webhook_acks_before_wake_scan_finishes",
            },
            Evidence {
                crate_path: "temper-runner::scan",
                source: ROLE_SCAN,
                test: "staged_issue_is_never_returned_by_role_scans_despite_ready_labels",
            },
        ],
    },
];

#[test]
fn issue_317_acceptance_matrix_names_live_deterministic_regressions() {
    assert_eq!(
        MATRIX.len(),
        8,
        "every #317 acceptance row stays represented"
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
