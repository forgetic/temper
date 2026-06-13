//! Worker wiring: build the sans-IO machine + shell and drive them.
//!
//! [`run_worker`] is the worker's entry point. It constructs the pure
//! [`WorkerMachine`](crate::worker_machine::WorkerMachine), the imperative
//! [`WorkerShell`](crate::worker_shell::WorkerShell), and a completion queue,
//! then hands them to [`smith_io_engine::drive`]. It must run inside an engine
//! task (the drive loop reads the runtime clock and the shell spawns I/O), so
//! callers wrap it in [`smith_io_engine::block_on`].

use std::sync::Arc;

use smith_io_engine::{channel, drive};

use crate::client::WorkerError;
use crate::config::{WorkerConfig, WorkerParams};
use crate::executor::JobExecutor;
use crate::worker_machine::WorkerMachine;
use crate::worker_shell::WorkerShell;

/// Run the worker to (effective) completion: register, then poll/dispatch/
/// report/heartbeat forever, driven by the completion queue. Returns only if
/// the machine stops or every completion sender is dropped (no I/O can complete
/// again) — in normal operation it runs until the process is signalled.
pub async fn run_worker<E>(config: WorkerConfig, executor: Arc<E>) -> Result<(), WorkerError>
where
    E: JobExecutor + Send + Sync + 'static,
{
    let params = WorkerParams::from_config(&config);
    let (cq_tx, cq_rx) = channel();

    // The runtime handle for spawning shell I/O. Available because run_worker is
    // awaited inside an engine task (block_on).
    let handle =
        skein::runtime::Runtime::current_handle().ok_or(WorkerError::RuntimeUnavailable)?;

    let shell = WorkerShell::new(
        handle,
        cq_tx,
        &config.daemon_url,
        config.worker_id.clone(),
        executor,
    );
    let machine = WorkerMachine::new(params);

    // drive() owns the loop; it returns when the machine stops or the queue
    // closes. In steady state it never returns.
    let _ = drive(machine, &shell, cq_rx).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    // Loop behavior is unit-tested on the pure WorkerMachine in
    // `worker_machine_tests.rs` (deterministic, runtime-free); end-to-end wiring
    // against a real daemon is covered by `tests/fake_daemon.rs`.
}
