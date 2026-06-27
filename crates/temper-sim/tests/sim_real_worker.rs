// SPDX-License-Identifier: MPL-2.0

//! High-fidelity worker harness: the production WorkerMachine/WorkerShell loop
//! runs under LabRuntime over the reusable in-process daemon transport.

use temper_sim::real_worker::{REAL_WORKER_JOB_ID, run_success_stub_worker_once};

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
