//! Worker wiring: build the sans-IO machine + shell and drive them.
//!
//! [`run_worker`] is the worker's entry point. It constructs the pure
//! [`WorkerMachine`](crate::worker_machine::WorkerMachine), the imperative
//! [`WorkerShell`](crate::worker_shell::WorkerShell), and a completion queue,
//! then hands them to [`temper_worker_io::drive`]. It must run inside an engine
//! task (the drive loop reads the runtime clock and the shell spawns I/O), so
//! callers wrap it in [`temper_worker_io::block_on_with`] and pass the
//! [`RuntimeHandle`] it yields.

use std::sync::Arc;

use skein::runtime::RuntimeHandle;
use temper_worker_io::{channel, drive};

use crate::client::WorkerError;
use crate::config::{WorkerConfig, WorkerParams};
use crate::executor::JobExecutor;
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::WorkerMachine;
use crate::worker_shell::WorkerShell;

/// Run the worker to (effective) completion over the **HTTP** transport (the
/// split deployment): register, then poll/dispatch/report/heartbeat forever,
/// driven by the completion queue. Returns only if the machine stops or every
/// completion sender is dropped — in normal operation it runs until the process
/// is signalled.
///
/// `handle` is the runtime's spawn capability, passed explicitly from the
/// `block_on_with` entry (no ambient handle lookup — skein removed
/// `Runtime::current_handle`).
pub async fn run_worker<E>(
    handle: RuntimeHandle,
    config: WorkerConfig,
    executor: Arc<E>,
) -> Result<(), WorkerError>
where
    E: JobExecutor + Send + Sync + 'static,
{
    let transport = Arc::new(HttpTransport::new(&config.daemon_url));
    run_worker_with_transport(handle, config, executor, transport).await
}

/// [`run_worker`] over an arbitrary [`Transport`] — the seam the unified
/// single-process mode uses to point the worker at a co-resident daemon over an
/// in-memory channel instead of HTTP. The protocol and the whole machine/shell
/// loop are identical; only the carrier differs.
pub async fn run_worker_with_transport<E, T>(
    handle: RuntimeHandle,
    config: WorkerConfig,
    executor: Arc<E>,
    transport: Arc<T>,
) -> Result<(), WorkerError>
where
    E: JobExecutor + Send + Sync + 'static,
    T: Transport,
{
    let params = WorkerParams::from_config(&config);
    let (cq_tx, cq_rx) = channel();

    let shell =
        WorkerShell::with_transport(handle, cq_tx, transport, config.worker_id.clone(), executor);
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
