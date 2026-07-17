//! Aggregate executable acceptance for descendant-complete process ownership.
//!
//! Focused crates retain their detailed fault-injection tests. This capstone
//! runs the compiled nested-session fixture through production managed-bash,
//! out-of-process agent, cancellation, and pre-push boundaries, then verifies
//! that every named matrix authority remains checked in.

#[cfg(target_os = "linux")]
#[test]
fn compiled_fixture_crosses_production_completion_boundaries() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper-containment-acceptance"))
        .arg(env!("CARGO_BIN_EXE_temper-descendant-fixture"))
        .output()
        .expect("run descendant-containment acceptance driver");
    assert!(
        output.status.success(),
        "acceptance driver failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("BACKEND forced-supervisor PASS"),
        "{stdout}"
    );
    assert!(
        stdout.contains("BACKEND auto-cgroup-v2 PASS") || stdout.contains("CGROUP SKIP:"),
        "cgroup capability must produce a pass or explicit skip: {stdout}"
    );
    assert!(stdout.contains("PREPUSH auto PASS"), "{stdout}");
}

#[test]
fn issues_445_and_448_matrix_keeps_every_deterministic_authority() {
    let authorities = [
        (
            include_str!("../../temper-agent-core/src/managed_bash/tests.rs"),
            "normal_exit_waits_for_detached_session_cleanup_and_reader_join",
        ),
        (
            include_str!("../../temper-agent-core/src/managed_bash/tests.rs"),
            "explicit_tool_timeout_waits_for_cleanup_and_reader_join",
        ),
        (
            include_str!("../../temper-worker/src/worker_machine_watchdog_tests.rs"),
            "no_progress_timeout_quiesces_records_once_then_releases_capacity",
        ),
        (
            include_str!("../../temper-worker/tests/support/worker_containment.rs"),
            "run_case(\"normal\", 0, true)",
        ),
        (
            include_str!("../../temper-worker/tests/support/worker_containment.rs"),
            "run_case(\"failure\", 17, false)",
        ),
        (
            include_str!("../../temper-worker/src/run_tests.rs"),
            "shutdown_joins_active_job_without_publishing_a_cancellation_result",
        ),
        (
            include_str!("../../temper-worker/src/run_tests.rs"),
            "shutdown_applies_forced_and_hard_deadlines_before_joining",
        ),
        (
            include_str!("../../temper-worker/src/pre_push/process.rs"),
            "cancellation_joins_pre_push_before_late_workspace_mutation",
        ),
        (
            include_str!("../../temper-worker/src/managed_effect.rs"),
            "dropping_command_kills_and_joins_before_late_mutation",
        ),
        (
            include_str!("../../temper-process-containment/src/tests.rs"),
            "blocked_inspection_cannot_complete_cleanup",
        ),
        (
            include_str!("../../temper-process-containment/src/tests.rs"),
            "reports_bound_survivors_attempts_and_diagnostics",
        ),
        (
            include_str!("../../temper-process-containment/src/tests.rs"),
            "pid_reuse_is_structured_and_never_signals_the_reused_identity_as_the_old_process",
        ),
        (
            include_str!("../../temper-agent-session/tests/non_completed_stop.rs"),
            "worker_abort_exits_nonzero_without_result_and_names_stable_reason",
        ),
        (
            include_str!("../../temper-worker/src/observability/containment/tests.rs"),
            "cleanup_events_have_expected_severity_bounded_evidence_and_redaction",
        ),
        (
            include_str!("../../temper-worker/src/observability/containment/tests.rs"),
            "repeated_blocked_cleanup_is_throttled_by_root",
        ),
    ];
    for (source, authority) in authorities {
        assert!(
            source.contains(authority),
            "descendant-containment authority `{authority}` disappeared"
        );
    }

    let driver = include_str!("../src/bin/temper-containment-acceptance.rs");
    for production_case in [
        "managed_bash_success",
        "managed_bash_deadline",
        "out_of_process_agent",
        "out_of_process_cancellation",
        "run_pre_push_case",
        "CGROUP SKIP:",
    ] {
        assert!(
            driver.contains(production_case),
            "compiled fixture driver lost `{production_case}`"
        );
    }

    let fixture = include_str!("../src/bin/temper-descendant-fixture.rs");
    for fixture_contract in [
        "rust-test-shaped-parent",
        "temper-agent-shaped-child",
        "start_time",
        "current.ppid == 1",
        "late workspace mutation",
    ] {
        assert!(
            fixture.contains(fixture_contract),
            "compiled fixture lost `{fixture_contract}`"
        );
    }

    let reference = include_str!("../../../docs/reference/descendant-containment-acceptance.md");
    for requirement in [
        "#445",
        "#448",
        "Managed bash direct success",
        "Capacity-one no-progress watchdog",
        "Out-of-process agent normal completion and failure",
        "Split worker and standalone signal shutdown",
        "Submit/pre-push and worker-managed commands",
        "TERM failure, KILL escalation, survivor and inspection faults",
        "Exact bounded `non_completed_stop`",
        "worker.containment.cleanup_completed",
        "worker.containment.cleanup_blocked",
    ] {
        assert!(
            reference.contains(requirement),
            "traceability reference lost `{requirement}`"
        );
    }
}
