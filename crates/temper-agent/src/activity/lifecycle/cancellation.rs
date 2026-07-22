//! Monotonic worker-to-agent lifecycle cancellation transport.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentCancellationStage, AgentLifecycleCancellationAckV1,
    AgentLifecycleCommandV1, MAX_AGENT_LIFECYCLE_FRAME_BYTES,
};

#[derive(Default)]
struct CancellationState {
    requested: Option<AgentCancellationStage>,
    callback: Option<Arc<dyn Fn() + Send + Sync>>,
    waiters: Vec<Waker>,
    emergency_registry: Option<temper_agent_core::EmergencyTerminationRegistry>,
}

/// Race-safe bridge between lifecycle commands, in-process worker control, and
/// the core control handle that becomes available after startup.
///
/// Cancellation is monotonic. Consumers using [`Self::next_stage_after`] see
/// Graceful, ForcedTermination, and HardKill in order even if the publisher
/// advances through several stages before the consumer is next polled.
#[derive(Clone, Default)]
pub struct AgentCancellationLatch {
    state: Arc<Mutex<CancellationState>>,
}

impl AgentCancellationLatch {
    pub fn install(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(callback);
        let requested = {
            let mut state = self.lock();
            state.callback = Some(Arc::clone(&callback));
            state.requested.is_some()
        };
        if requested {
            callback();
        }
    }

    /// Installs the out-of-band process authority associated with this native
    /// run. A forced or hard stage dispatches here before callbacks or polling,
    /// so a blocked agent/MCP request cannot delay descendant termination.
    pub fn install_emergency_registry(
        &self,
        registry: temper_agent_core::EmergencyTerminationRegistry,
    ) {
        let stage = {
            let mut state = self.lock();
            state.emergency_registry = Some(registry.clone());
            state.requested
        };
        dispatch_emergency(&registry, stage);
    }

    /// Strongest validated worker cancellation stage received so far.
    pub fn requested_stage(&self) -> Option<AgentCancellationStage> {
        self.lock().requested
    }

    /// Compatibility query for callers that only distinguish cancellation from
    /// normal execution.
    pub fn worker_cancellation_requested(&self) -> bool {
        self.requested_stage().is_some()
    }

    /// Requests cooperative cancellation through the same monotonic latch used
    /// by the lifecycle command reader.
    pub fn request_cancel(&self) {
        self.request(AgentCancellationStage::Graceful);
    }

    /// Publishes a cancellation stage and immediately drives the independent
    /// process registry for forced and hard escalation.
    pub fn request(&self, requested: AgentCancellationStage) {
        let (callback, waiters, registry, advanced) = {
            let mut state = self.lock();
            if state.requested.is_some_and(|current| current >= requested) {
                return;
            }
            let first = state.requested.is_none();
            state.requested = Some(requested);
            (
                first.then(|| state.callback.clone()).flatten(),
                std::mem::take(&mut state.waiters),
                state.emergency_registry.clone(),
                true,
            )
        };
        if advanced {
            if let Some(registry) = registry.as_ref() {
                dispatch_emergency(registry, Some(requested));
            }
            for waiter in waiters {
                waiter.wake();
            }
            if let Some(callback) = callback {
                callback();
            }
        }
    }

    /// Returns the next monotonic stage after `observed`, preserving
    /// intermediate stages when escalation was coalesced by scheduling.
    pub async fn next_stage_after(
        &self,
        observed: Option<AgentCancellationStage>,
    ) -> AgentCancellationStage {
        std::future::poll_fn(|cx| self.poll_stage_after(observed, cx)).await
    }

    pub(crate) fn poll_stage_after(
        &self,
        observed: Option<AgentCancellationStage>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<AgentCancellationStage> {
        if let Some(next) = next_stage(self.requested_stage(), observed) {
            return Poll::Ready(next);
        }
        let mut state = self.lock();
        if let Some(next) = next_stage(state.requested, observed) {
            return Poll::Ready(next);
        }
        if !state
            .waiters
            .iter()
            .any(|waiter| waiter.will_wake(cx.waker()))
        {
            state.waiters.push(cx.waker().clone());
        }
        Poll::Pending
    }

    /// Compatibility wait for the first (graceful) stage.
    pub async fn cancelled(&self) {
        let _ = self.next_stage_after(None).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CancellationState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn next_stage(
    published: Option<AgentCancellationStage>,
    observed: Option<AgentCancellationStage>,
) -> Option<AgentCancellationStage> {
    let published = published?;
    match observed {
        None => Some(AgentCancellationStage::Graceful),
        Some(AgentCancellationStage::Graceful)
            if published >= AgentCancellationStage::ForcedTermination =>
        {
            Some(AgentCancellationStage::ForcedTermination)
        }
        Some(AgentCancellationStage::ForcedTermination)
            if published >= AgentCancellationStage::HardKill =>
        {
            Some(AgentCancellationStage::HardKill)
        }
        _ => None,
    }
}

fn dispatch_emergency(
    registry: &temper_agent_core::EmergencyTerminationRegistry,
    stage: Option<AgentCancellationStage>,
) {
    match stage {
        Some(AgentCancellationStage::ForcedTermination) => {
            let _ = registry.request_forced_termination();
        }
        Some(AgentCancellationStage::HardKill) => {
            let _ = registry.request_hard_kill();
        }
        Some(AgentCancellationStage::Graceful) | None => {}
    }
}

pub(super) fn spawn_command_reader(
    stream: TcpStream,
    writer: Arc<Mutex<TcpStream>>,
    cancellation: AgentCancellationLatch,
) {
    let _ = std::thread::Builder::new()
        .name("temper-agent-lifecycle-commands".to_string())
        .spawn(move || lifecycle_command_reader(stream, writer, cancellation));
}

fn lifecycle_command_reader(
    stream: TcpStream,
    writer: Arc<Mutex<TcpStream>>,
    cancellation: AgentCancellationLatch,
) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        let Ok(read) = reader.read_until(b'\n', &mut bytes) else {
            return;
        };
        if read == 0 || bytes.len() > MAX_AGENT_LIFECYCLE_FRAME_BYTES {
            return;
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        let Ok(command) = serde_json::from_slice::<AgentLifecycleCommandV1>(&bytes) else {
            return;
        };
        if command.validate().is_err() {
            return;
        }
        match command {
            AgentLifecycleCommandV1::Cancel { stage, .. } => cancellation.request(stage),
        }
        let acknowledgement = AgentLifecycleCancellationAckV1 {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
        };
        let Ok(mut bytes) = serde_json::to_vec(&acknowledgement) else {
            return;
        };
        bytes.push(b'\n');
        if writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(&bytes)
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, Write as _};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn every_stage_is_observed_in_order_and_first_stage_triggers_control() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_callback = Arc::clone(&cancelled);
        let latch = AgentCancellationLatch::default();
        latch.install(move || cancelled_for_callback.store(true, Ordering::Release));
        latch.request(AgentCancellationStage::HardKill);

        let latch_for_wait = latch.clone();
        temper_agent_io::block_on(async move {
            let graceful = latch_for_wait.next_stage_after(None).await;
            let forced = latch_for_wait.next_stage_after(Some(graceful)).await;
            let hard = latch_for_wait.next_stage_after(Some(forced)).await;
            assert_eq!(graceful, AgentCancellationStage::Graceful);
            assert_eq!(forced, AgentCancellationStage::ForcedTermination);
            assert_eq!(hard, AgentCancellationStage::HardKill);
        });
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(
            latch.requested_stage(),
            Some(AgentCancellationStage::HardKill)
        );
    }

    #[test]
    fn lifecycle_reader_accepts_all_monotonic_stages() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let writer = Arc::new(Mutex::new(server.try_clone().unwrap()));
        let latch = AgentCancellationLatch::default();
        let latch_for_reader = latch.clone();
        let reader = std::thread::spawn(move || {
            lifecycle_command_reader(server, writer, latch_for_reader);
        });

        for stage in [
            AgentCancellationStage::Graceful,
            AgentCancellationStage::ForcedTermination,
            AgentCancellationStage::HardKill,
        ] {
            serde_json::to_writer(
                &mut client,
                &AgentLifecycleCommandV1::Cancel {
                    stage,
                    reason: "worker cancellation".to_string(),
                },
            )
            .unwrap();
            client.write_all(b"\n").unwrap();
            let mut response = String::new();
            BufReader::new(client.try_clone().unwrap())
                .read_line(&mut response)
                .unwrap();
            serde_json::from_str::<AgentLifecycleCancellationAckV1>(response.trim())
                .unwrap()
                .validate()
                .unwrap();
        }
        client.shutdown(Shutdown::Write).unwrap();
        reader.join().unwrap();
        assert_eq!(
            latch.requested_stage(),
            Some(AgentCancellationStage::HardKill)
        );
    }

    #[test]
    fn invalid_command_does_not_confer_worker_cancellation_authority() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let writer = Arc::new(Mutex::new(server.try_clone().unwrap()));
        let latch = AgentCancellationLatch::default();
        let latch_for_reader = latch.clone();
        let reader = std::thread::spawn(move || {
            lifecycle_command_reader(server, writer, latch_for_reader);
        });

        serde_json::to_writer(
            &mut client,
            &AgentLifecycleCommandV1::Cancel {
                stage: AgentCancellationStage::Graceful,
                reason: String::new(),
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        reader.join().unwrap();

        assert!(!latch.worker_cancellation_requested());
    }
}
