//! Deterministic boundary around process-global termination signal receivers.

use std::time::Instant;

/// The monotonic instant at which a process-global termination signal was
/// observed by the installed receiver.
pub(super) struct SignalReceipt {
    pub(super) received_at: Instant,
}

/// Waits for either service termination signal and captures the deadline origin
/// in the same poll that observes readiness.
pub(super) async fn wait() -> Result<SignalReceipt, String> {
    let mut sigint = skein::signal::sigint()
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    let mut sigterm = skein::signal::sigterm()
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;
    let receipt = std::future::poll_fn(|task_cx| {
        if sigint.poll_recv(task_cx).is_ready() || sigterm.poll_recv(task_cx).is_ready() {
            std::task::Poll::Ready(SignalReceipt {
                received_at: Instant::now(),
            })
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    Ok(receipt)
}
