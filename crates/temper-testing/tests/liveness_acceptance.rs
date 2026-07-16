// SPDX-License-Identifier: MPL-2.0

//! Executable traceability matrix for issue #340 and the #331 compatibility
//! requirement. Prerequisite issues own focused regressions; this aggregate
//! suite fails when named evidence or its operator event disappears.

use std::collections::BTreeSet;

struct Evidence {
    crate_path: &'static str,
    source: &'static str,
    test: &'static str,
}

struct Criterion {
    id: &'static str,
    behavior: &'static str,
    operator_events: &'static [&'static str],
    evidence: &'static [Evidence],
}

const SIM_REAL_WORKER: &str = include_str!("../../temper-sim/tests/sim_real_worker.rs");
const WORKER_WATCHDOG: &str =
    include_str!("../../temper-worker/src/worker_machine_watchdog_tests.rs");
const WORKER_CANCELLATION_PROJECTION: &str =
    include_str!("../../temper-worker/src/worker_machine_cancellation_projection_tests.rs");
const OUT_OF_PROCESS: &str =
    include_str!("../../temper-worker/src/out_of_process_runner_supervisor_tests.rs");
const RESULT_OUTBOX: &str = include_str!("../../temper-worker/src/result_outbox.rs");
const STREAMING: &str = include_str!("../../temper-agent-core/src/shell/streaming_tests.rs");
const TOOL_EXECUTOR: &str = include_str!("../../temper-agent-core/src/shell/executor_tests.rs");
const TOOL_BATCHING: &str = include_str!("../../temper-agent-core/src/machine/tests/batching.rs");
const AGENT_LIFECYCLE: &str =
    include_str!("../../temper-agent-core/src/machine/tests/loop_lifecycle.rs");
const MANAGED_BASH: &str = include_str!("../../temper-agent-core/src/managed_bash.rs");
const SUBAGENT: &str = include_str!("../../temper-agent-core/src/subagent_tool.rs");
const MCP: &str = include_str!("../../temper-agent/src/mcp/tests.rs");
const ENGINE_DELIVERY: &str = include_str!("../../temper-engine/tests/apply/result_delivery.rs");
const FORGE_FAILURE: &str = include_str!("../../temper-engine/tests/forge_apply/failure.rs");
const ASSIGNMENT_CONVERGENCE: &str =
    include_str!("../../temper-runner/tests/assignment_convergence.rs");
const RESTART_ACCEPTANCE: &str = include_str!("hermetic_real_stack/restart_acceptance.rs");
const RESTART_RECOVERY: &str = include_str!("hermetic_real_stack/restart_recovery.rs");
const CONFIG_DEADLINES: &str = include_str!("../../temper-config/src/tests/deadlines.rs");
const CONFIG_SHOW: &str = include_str!("../../../tests/config_show_cli.rs");
const PROTOCOL_LIFECYCLE: &str =
    include_str!("../../temper-protocol-worker/src/tests/lifecycle.rs");
const OBSERVABILITY_TESTS: &str = include_str!("../../temper-worker/src/observability.rs");
const OBSERVABILITY_CATALOG: &str =
    include_str!("../../../docs/explanation/logging-and-observability.md");

const MATRIX: &[Criterion] = &[
    Criterion {
        id: "#340-1",
        behavior: "capacity-one hung Forge future times out, records durably, converges, and dispatches unrelated work without restart",
        operator_events: &[
            "worker.job.timeout",
            "worker.job.cancellation_requested",
            "worker.result.recorded",
            "worker.capacity.released",
            "worker.result.delivery",
        ],
        evidence: &[Evidence {
            crate_path: "temper-sim::sim_real_worker",
            source: SIM_REAL_WORKER,
            test: "hung_forge_future_times_out_releases_capacity_and_late_completion_is_fenced",
        }],
    },
    Criterion {
        id: "#340-2",
        behavior: "late Forge/progress/result completion is attempt-fenced from duplicate publication and mutation",
        operator_events: &[
            "worker.job.cancellation_completed",
            "worker.result.delivery",
        ],
        evidence: &[
            Evidence {
                crate_path: "temper-sim::sim_real_worker",
                source: SIM_REAL_WORKER,
                test: "hung_forge_future_times_out_releases_capacity_and_late_completion_is_fenced",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "forced_termination_fences_late_result_and_reports_cleanup",
            },
        ],
    },
    Criterion {
        id: "#340-3",
        behavior: "stalled model connect, first event, idle stream, retry, and timeout classification use virtual time",
        operator_events: &["worker.job.progress", "worker.job.timeout"],
        evidence: &[
            Evidence {
                crate_path: "temper-agent-core::shell::streaming",
                source: STREAMING,
                test: "stalled_provider_connect_retries_on_virtual_time",
            },
            Evidence {
                crate_path: "temper-agent-core::shell::streaming",
                source: STREAMING,
                test: "first_and_idle_stream_events_use_distinct_resolved_limits",
            },
        ],
    },
    Criterion {
        id: "#340-4",
        behavior: "hung process tools, parallel read batches, submit-for-PR, and nested agents cancel and settle once",
        operator_events: &["worker.job.progress", "worker.job.cancellation_completed"],
        evidence: &[
            Evidence {
                crate_path: "temper-agent-core::managed_bash",
                source: MANAGED_BASH,
                test: "dropping_a_hung_command_reaps_its_grandchild_and_joins",
            },
            Evidence {
                crate_path: "temper-agent-core::machine::batching",
                source: TOOL_BATCHING,
                test: "duplicate_and_stale_parallel_completions_settle_each_call_once",
            },
            Evidence {
                crate_path: "temper-agent-core::machine::lifecycle",
                source: AGENT_LIFECYCLE,
                test: "abort_during_tools_cancels_immediately_and_waits_for_quiescence",
            },
            Evidence {
                crate_path: "temper-agent-core::shell::executor",
                source: TOOL_EXECUTOR,
                test: "cancellation_reports_quiescence_only_after_every_registered_task_settles",
            },
            Evidence {
                crate_path: "temper-agent-core::shell::executor",
                source: TOOL_EXECUTOR,
                test: "external_cancellation_drops_a_hung_tool_without_advancing_time",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "hung_submit_host_is_dropped_and_joined_before_run_cancellation_returns",
            },
            Evidence {
                crate_path: "temper-agent-core::subagent_tool",
                source: SUBAGENT,
                test: "dropping_nested_run_aborts_every_control_and_joins_its_task_group",
            },
        ],
    },
    Criterion {
        id: "#340-5",
        behavior: "Unix process tests bound agent, MCP, descendant, side-channel, stderr, activity, and waiter cleanup",
        operator_events: &["worker.job.cancellation_completed"],
        evidence: &[
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "unresponsive_child_escalates_to_hard_kill_without_a_lingering_waiter",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "cancellation_kills_and_reaps_a_child_process_group_grandchild",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "cooperative_window_observes_a_graceful_child_exit_and_joins_stderr",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "forced_termination_fences_late_result_and_reports_cleanup",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "hard_kill_writes_synthetic_cancelled_terminal_activity_and_reports_cleanup",
            },
            Evidence {
                crate_path: "temper-worker::worker_machine",
                source: WORKER_CANCELLATION_PROJECTION,
                test: "real_cancellation_and_cleanup_outcomes_project_without_synthesis",
            },
            Evidence {
                crate_path: "temper-worker::out_of_process_runner",
                source: OUT_OF_PROCESS,
                test: "hung_submit_host_is_dropped_and_joined_before_run_cancellation_returns",
            },
            Evidence {
                crate_path: "temper-agent::mcp",
                source: MCP,
                test: "cancellation_wakes_a_request_mutex_waiter_and_joins_both_operations",
            },
            Evidence {
                crate_path: "temper-agent::mcp",
                source: MCP,
                test: "cancellation_reaps_the_mcp_server_grandchild_group",
            },
        ],
    },
    Criterion {
        id: "#340-6",
        behavior: "Forge outage, exact result replay, and live full-claim reconciliation converge idempotently",
        operator_events: &["worker.result.delivery", "assignment.convergence"],
        evidence: &[
            Evidence {
                crate_path: "temper-engine::result_delivery",
                source: ENGINE_DELIVERY,
                test: "lost_acknowledgement_replays_exact_result_without_double_apply",
            },
            Evidence {
                crate_path: "temper-engine::forge_apply::failure",
                source: FORGE_FAILURE,
                test: "retryable_failure_releases_claimed_source_issue_for_rescan",
            },
            Evidence {
                crate_path: "temper-runner::assignment_convergence",
                source: ASSIGNMENT_CONVERGENCE,
                test: "live_reconciliation_retries_release_after_forge_outage",
            },
            Evidence {
                crate_path: "temper-runner::assignment_convergence",
                source: ASSIGNMENT_CONVERGENCE,
                test: "live_reconciliation_converges_full_issue_assignment_once",
            },
        ],
    },
    Criterion {
        id: "#340-7",
        behavior: "worker/daemon restart phases preserve claims, exact results, dirty workspace, and coordination session reuse",
        operator_events: &[
            "worker.result.recorded",
            "worker.result.delivery",
            "assignment.convergence",
        ],
        evidence: &[
            Evidence {
                crate_path: "temper-testing::hermetic_real_stack::restart_acceptance",
                source: RESTART_ACCEPTANCE,
                test: "dirty_workspace_replays_after_target_advance_and_component_replacement",
            },
            Evidence {
                crate_path: "temper-testing::hermetic_real_stack::restart_acceptance",
                source: RESTART_ACCEPTANCE,
                test: "matching_worker_heartbeat_reattaches_exact_durable_job_once",
            },
            Evidence {
                crate_path: "temper-testing::hermetic_real_stack::restart_recovery",
                source: RESTART_RECOVERY,
                test: "daemon_loss_after_child_create_replays_wiring_and_activation_once",
            },
            Evidence {
                crate_path: "temper-worker::result_outbox",
                source: RESULT_OUTBOX,
                test: "record_is_restart_readable_private_and_compacts_idempotently",
            },
        ],
    },
    Criterion {
        id: "#340-8",
        behavior: "deadline equality, generations, normal completion races, stale delivery, and capacity greater than one are deterministic",
        operator_events: &["worker.job.timeout", "worker.capacity.released"],
        evidence: &[
            Evidence {
                crate_path: "temper-worker::worker_machine::watchdog",
                source: WORKER_WATCHDOG,
                test: "exact_boundary_progress_wins_and_stale_no_progress_timers_are_ignored",
            },
            Evidence {
                crate_path: "temper-worker::worker_machine::watchdog",
                source: WORKER_WATCHDOG,
                test: "normal_completion_beats_timeout_and_duplicate_completion_releases_once",
            },
            Evidence {
                crate_path: "temper-worker::worker_machine::watchdog",
                source: WORKER_WATCHDOG,
                test: "max_run_is_independent_of_progress_and_releasing_one_of_many_preserves_membership",
            },
        ],
    },
    Criterion {
        id: "#340-9",
        behavior: "schema, defaults, inheritance, template/show compatibility, protocol projection, and structured redaction remain stable",
        operator_events: &[
            "worker.job.progress",
            "worker.job.timeout",
            "worker.job.cancellation_requested",
            "worker.job.cancellation_completed",
            "worker.result.recorded",
            "worker.result.delivery",
            "worker.capacity.released",
            "assignment.convergence",
        ],
        evidence: &[
            Evidence {
                crate_path: "temper-config::deadlines",
                source: CONFIG_DEADLINES,
                test: "deadline_and_liveness_defaults_apply_without_new_toml",
            },
            Evidence {
                crate_path: "temper-config::deadlines",
                source: CONFIG_DEADLINES,
                test: "config_template_resolves_the_documented_liveness_contract",
            },
            Evidence {
                crate_path: "temper-config::deadlines",
                source: CONFIG_DEADLINES,
                test: "profiles_inherit_each_missing_deadline_independently",
            },
            Evidence {
                crate_path: "temper-config::deadlines",
                source: CONFIG_DEADLINES,
                test: "json_schema_marks_all_duration_seconds_as_positive",
            },
            Evidence {
                crate_path: "temper-cli::config_show",
                source: CONFIG_SHOW,
                test: "config_show_includes_target_pools_and_agent_profiles_without_secret_values",
            },
            Evidence {
                crate_path: "temper-protocol-worker::lifecycle",
                source: PROTOCOL_LIFECYCLE,
                test: "structured_liveness_round_trips_without_sensitive_content",
            },
            Evidence {
                crate_path: "temper-protocol-worker::lifecycle",
                source: PROTOCOL_LIFECYCLE,
                test: "legacy_heartbeat_without_liveness_remains_compatible",
            },
            Evidence {
                crate_path: "temper-worker::observability",
                source: OBSERVABILITY_TESTS,
                test: "liveness_catalog_emits_structured_levels_without_sensitive_fields",
            },
        ],
    },
    Criterion {
        id: "#331-compat",
        behavior: "an outage-retained full durable assignment converges live and repeated replay/reconciliation is harmless",
        operator_events: &["worker.result.delivery", "assignment.convergence"],
        evidence: &[
            Evidence {
                crate_path: "temper-engine::result_delivery",
                source: ENGINE_DELIVERY,
                test: "lost_acknowledgement_replays_exact_result_without_double_apply",
            },
            Evidence {
                crate_path: "temper-runner::assignment_convergence",
                source: ASSIGNMENT_CONVERGENCE,
                test: "live_reconciliation_retries_release_after_forge_outage",
            },
            Evidence {
                crate_path: "temper-runner::assignment_convergence",
                source: ASSIGNMENT_CONVERGENCE,
                test: "live_reconciliation_converges_full_issue_assignment_once",
            },
        ],
    },
];

#[test]
fn issue_340_and_331_acceptance_matrix_names_live_tests_and_operator_events() {
    assert_eq!(MATRIX.len(), 10, "nine #340 rows plus #331 compatibility");
    let mut ids = BTreeSet::new();
    for criterion in MATRIX {
        assert!(ids.insert(criterion.id), "duplicate row {}", criterion.id);
        assert!(
            !criterion.behavior.is_empty(),
            "{} has no behavior",
            criterion.id
        );
        assert!(
            !criterion.evidence.is_empty(),
            "{} has no deterministic evidence",
            criterion.id
        );
        assert!(
            !criterion.operator_events.is_empty(),
            "{} has no operator evidence",
            criterion.id
        );
        for event in criterion.operator_events {
            assert!(
                OBSERVABILITY_CATALOG.contains(&format!("`{event}`")),
                "{} operator event `{event}` left the documented catalog",
                criterion.id
            );
        }
        for evidence in criterion.evidence {
            let ordinary = format!("fn {}(", evidence.test);
            let asynchronous = format!("async fn {}(", evidence.test);
            assert!(
                evidence.source.contains(&ordinary) || evidence.source.contains(&asynchronous),
                "{} no longer exposes deterministic test `{}` for {}",
                evidence.crate_path,
                evidence.test,
                criterion.id
            );
        }
    }
}
