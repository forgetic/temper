# Agent-run liveness acceptance matrix

This is the aggregate traceability map for issue #340 and its #331 live
assignment-convergence compatibility requirement. The executable copy is
`temper-testing/tests/liveness_acceptance.rs`; it verifies that every named test
and structured operator event remains present.

The suites deliberately split by authority: pure machine tests own transition
races, `temper-sim` owns virtual-time worker capacity, hermetic real-stack tests
own durable Forge/workspace recovery, and short Unix-gated tests own process
cleanup. Long configured deadlines and child sleeps are interrupted; no row
waits for a multi-second wall-clock timeout.

| Requirement | Deterministic acceptance evidence | Operator evidence |
| --- | --- | --- |
| #340.1 capacity-one hung Forge/context future | `temper-sim::hung_forge_future_times_out_releases_capacity_and_late_completion_is_fenced` runs the production `WorkerMachine`/`WorkerShell`, observes continuing heartbeats, a transient outbox-backed result, accepted claim release, and unrelated follow-up dispatch. | `worker.job.timeout`, `worker.job.cancellation_requested`, `worker.result.recorded`, `worker.capacity.released`, `worker.result.delivery` |
| #340.2 late completion fence | The same sim resolves the original future and submits stale progress after release; `temper-worker::forced_termination_fences_late_result_and_reports_cleanup` covers the real result-file/process path. | `worker.job.cancellation_completed`, `worker.result.delivery` |
| #340.3 model stalls and retries | `stalled_provider_connect_retries_on_virtual_time` and `first_and_idle_stream_events_use_distinct_resolved_limits` use the agent lab clock and assert connect/first-event/idle classification plus retry count. | `worker.job.progress`, `worker.job.timeout` |
| #340.4 hung tools and nested work | `dropping_a_hung_command_reaps_its_grandchild_and_joins`, `duplicate_and_stale_parallel_completions_settle_each_call_once`, `abort_during_tools_cancels_immediately_and_waits_for_quiescence`, `cancellation_reports_quiescence_only_after_every_registered_task_settles`, `external_cancellation_drops_a_hung_tool_without_advancing_time`, `hung_submit_host_is_dropped_and_joined_before_run_cancellation_returns`, and `dropping_nested_run_aborts_every_control_on_its_dedicated_owner`. | `worker.job.progress`, `worker.job.cancellation_completed` |
| #340.5 bounded OS cleanup | Unix-gated `unresponsive_child_escalates_to_hard_kill_without_a_lingering_waiter`, `cancellation_kills_and_reaps_a_child_process_group_grandchild`, `cooperative_window_observes_a_graceful_child_exit_and_joins_stderr`, `forced_termination_fences_late_result_and_reports_cleanup`, `hard_kill_writes_synthetic_cancelled_terminal_activity_and_reports_cleanup`, `hung_submit_host_is_dropped_and_joined_before_run_cancellation_returns`, `cancellation_wakes_a_request_mutex_waiter_and_joins_both_operations`, and `cancellation_reaps_the_mcp_server_grandchild_group` prove graceful, forced, and hard-kill cleanup. `real_cancellation_and_cleanup_outcomes_project_without_synthesis` covers cleanup-failure projection. | `worker.job.cancellation_completed` with the real outcome, `forced`, and `descendant_cleanup` |
| #340.6 Forge outage and idempotence | `lost_acknowledgement_replays_exact_result_without_double_apply`, `retryable_failure_releases_claimed_source_issue_for_rescan`, `live_reconciliation_retries_release_after_forge_outage`, and `live_reconciliation_converges_full_issue_assignment_once`. | `worker.result.delivery`, `assignment.convergence` |
| #340.7 restart phases and retained work | `dirty_workspace_replays_after_target_advance_and_component_replacement`, `matching_worker_heartbeat_reattaches_exact_durable_job_once`, `daemon_loss_after_child_create_replays_wiring_and_activation_once`, and `record_is_restart_readable_private_and_compacts_idempotently` cover running, pre-record, uncertain delivery, acknowledgement/compaction, dirty tracked/untracked files, and session reuse. | `worker.result.recorded`, `worker.result.delivery`, `assignment.convergence` |
| #340.8 timer/capacity races | `exact_boundary_progress_wins_and_stale_no_progress_timers_are_ignored`, `normal_completion_beats_timeout_and_duplicate_completion_releases_once`, and `max_run_is_independent_of_progress_and_releasing_one_of_many_preserves_membership`. | `worker.job.timeout`, `worker.capacity.released` |
| #340.9 compatibility and observability | `deadline_and_liveness_defaults_apply_without_new_toml`, `config_template_resolves_the_documented_liveness_contract`, `profiles_inherit_each_missing_deadline_independently`, `json_schema_marks_all_duration_seconds_as_positive`, `config_show_includes_target_pools_and_agent_profiles_without_secret_values`, `structured_liveness_round_trips_without_sensitive_content`, `legacy_heartbeat_without_liveness_remains_compatible`, and `liveness_catalog_emits_structured_levels_without_sensitive_fields`. | Full liveness event catalog below |
| #331 compatibility | `lost_acknowledgement_replays_exact_result_without_double_apply`, `live_reconciliation_retries_release_after_forge_outage`, and `live_reconciliation_converges_full_issue_assignment_once` prove retained-result replay and complete assignment/lease/label/assignee convergence are idempotent. | `worker.result.delivery`, `assignment.convergence` |

The stable structured event catalog and field/level definitions live in
[Logging and observability](../explanation/logging-and-observability.md#job-liveness-and-convergence-events).
