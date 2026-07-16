// SPDX-License-Identifier: MPL-2.0

//! High-fidelity worker harness: the production WorkerMachine/WorkerShell loop
//! runs under LabRuntime over the reusable in-process daemon transport.

use temper_protocol_worker::{JobHeartbeatPhase, JobTimeoutReason};
use temper_sim::real_worker::{
    FOLLOW_UP_JOB_ID, HUNG_FORGE_JOB_ID, REAL_WORKER_JOB_ID, run_hung_forge_watchdog_once,
    run_success_stub_worker_once,
};

#[test]
fn real_worker_shell_stub_registers_polls_executes_releases_and_applies() {
    let outcome = run_success_stub_worker_once(42);

    outcome.model.assert_exactly_once();
    assert!(
        outcome.model.outstanding.is_empty(),
        "release should clear the model's outstanding assignment: {:?}",
        outcome.model.outstanding
    );

    let trace = outcome.trace;
    assert!(trace.registers > 0, "worker must register: {trace:?}");
    assert!(trace.polls > 0, "worker must poll: {trace:?}");
    assert!(
        trace.assigned(REAL_WORKER_JOB_ID),
        "worker must receive the queued job: {trace:?}"
    );
    assert!(
        trace.submitted_success(REAL_WORKER_JOB_ID),
        "StubExecutor success must be submitted as a worker result: {trace:?}"
    );
    assert!(
        trace.accepted_release(REAL_WORKER_JOB_ID),
        "daemon must accept/release the real worker result: {trace:?}"
    );
    assert!(
        trace.transport_errors.is_empty(),
        "in-process transport should be fault-free: {trace:?}"
    );
}

#[test]
fn real_worker_shell_stub_world_is_deterministic_per_seed() {
    let first = run_success_stub_worker_once(7);
    let second = run_success_stub_worker_once(7);

    assert_eq!(first, second);
}

#[test]
fn hung_forge_future_times_out_releases_capacity_and_late_completion_is_fenced() {
    let outcome = run_hung_forge_watchdog_once(340);
    outcome.model.assert_exactly_once();
    assert!(
        outcome.model.outstanding.is_empty(),
        "both durable claims must converge: {:?}",
        outcome.model.outstanding
    );

    let before = &outcome.trace_before_late_completion;
    assert_eq!(
        before.assignments,
        [HUNG_FORGE_JOB_ID.to_string(), FOLLOW_UP_JOB_ID.to_string()],
        "capacity one must dispatch the unrelated job only after timeout release"
    );
    assert!(
        before
            .liveness
            .iter()
            .filter(|(job_id, phase, timeout)| {
                job_id == HUNG_FORGE_JOB_ID
                    && *phase == JobHeartbeatPhase::Running
                    && timeout.is_none()
            })
            .count()
            >= 3,
        "lease heartbeats must continue without becoming agent progress: {before:?}"
    );
    assert!(before.liveness.iter().any(|(job_id, phase, timeout)| {
        job_id == HUNG_FORGE_JOB_ID
            && *phase == JobHeartbeatPhase::CancelRequested
            && *timeout == Some(JobTimeoutReason::NoProgress)
    }));
    assert_eq!(before.result_count(HUNG_FORGE_JOB_ID), 1);
    assert_eq!(before.release_count(HUNG_FORGE_JOB_ID), 1);
    assert!(before.submitted_transient(HUNG_FORGE_JOB_ID));
    assert_eq!(
        before.durable_result_sends,
        [HUNG_FORGE_JOB_ID.to_string(), FOLLOW_UP_JOB_ID.to_string()],
        "each payload must exist in the restart-readable outbox before transport"
    );
    assert_eq!(before.result_count(FOLLOW_UP_JOB_ID), 1);
    assert_eq!(before.release_count(FOLLOW_UP_JOB_ID), 1);
    assert!(before.submitted_success(FOLLOW_UP_JOB_ID));
    assert!(before.transport_errors.is_empty(), "{before:?}");

    let executor_before = &outcome.executor_before_late_completion;
    assert_eq!(executor_before.cancellations, [HUNG_FORGE_JOB_ID]);
    assert_eq!(executor_before.completions, [FOLLOW_UP_JOB_ID]);
    assert!(executor_before.result_file_acceptances.is_empty());
    assert!(executor_before.validations.is_empty());
    assert!(executor_before.pushes.is_empty());
    assert!(executor_before.forge_mutations.is_empty());

    assert!(!outcome.late_progress_accepted);
    let after = &outcome.trace_after_late_completion;
    assert_eq!(after.assignments, before.assignments);
    assert_eq!(after.results, before.results);
    assert_eq!(after.result_failure_classes, before.result_failure_classes);
    assert_eq!(after.durable_result_sends, before.durable_result_sends);
    assert_eq!(after.releases, before.releases);
    assert_eq!(after.heartbeats, before.heartbeats);
    assert_eq!(after.liveness, before.liveness);
    assert_eq!(after.transport_errors, before.transport_errors);

    let executor_after = &outcome.executor_after_late_completion;
    assert_eq!(executor_after.starts, executor_before.starts);
    assert_eq!(executor_after.completions, executor_before.completions);
    assert_eq!(executor_after.cancellations, executor_before.cancellations);
    assert_eq!(
        executor_after.result_file_acceptances,
        executor_before.result_file_acceptances
    );
    assert_eq!(executor_after.validations, executor_before.validations);
    assert_eq!(executor_after.pushes, executor_before.pushes);
    assert_eq!(
        executor_after.forge_mutations,
        executor_before.forge_mutations
    );
    assert_eq!(executor_after.forge_future_resolutions, [HUNG_FORGE_JOB_ID]);
    assert_eq!(executor_after.late_progress_attempts, [HUNG_FORGE_JOB_ID]);
    assert!(executor_after.accepted_late_progress.is_empty());
}
