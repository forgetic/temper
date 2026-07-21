//! Cooperative worker-to-agent lifecycle cancellation transport.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleCancellationAckV1, AgentLifecycleCommandV1,
    MAX_AGENT_LIFECYCLE_FRAME_BYTES,
};

#[derive(Default)]
struct CancellationState {
    requested: bool,
    callback: Option<Arc<dyn Fn() + Send + Sync>>,
    waiters: Vec<Waker>,
}

/// Race-safe bridge between the lifecycle command reader and the core control
/// handle, which becomes available only after the lifecycle transport starts.
#[derive(Clone, Default)]
pub struct AgentCancellationLatch {
    state: Arc<Mutex<CancellationState>>,
}

impl AgentCancellationLatch {
    pub fn install(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(callback);
        let requested = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.callback = Some(Arc::clone(&callback));
            state.requested
        };
        if requested {
            callback();
        }
    }

    /// Whether a validated worker lifecycle cancellation command has been
    /// received. The state is set before the installed callback runs, so an
    /// abort caused by that callback cannot race ahead of this query.
    pub fn worker_cancellation_requested(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .requested
    }

    /// Requests in-process cancellation through the same race-safe latch used
    /// by the lifecycle command reader.
    pub fn request_cancel(&self) {
        let (callback, waiters) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.requested = true;
            (state.callback.clone(), std::mem::take(&mut state.waiters))
        };
        for waiter in waiters {
            waiter.wake();
        }
        if let Some(callback) = callback {
            callback();
        }
    }

    /// Waits for worker cancellation even before the core model-loop control
    /// exists. Native startup owners (notably MCP and credential loading) race
    /// this future and join on drop before installing the core callback.
    pub async fn cancelled(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.requested {
                return Poll::Ready(());
            }
            if !state
                .waiters
                .iter()
                .any(|waiter| waiter.will_wake(cx.waker()))
            {
                state.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
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
    let mut bytes = Vec::new();
    if reader.read_until(b'\n', &mut bytes).is_err()
        || bytes.len() > MAX_AGENT_LIFECYCLE_FRAME_BYTES
    {
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
        AgentLifecycleCommandV1::Cancel { .. } => cancellation.request_cancel(),
    }
    let acknowledgement = AgentLifecycleCancellationAckV1 {
        version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
    };
    if let Ok(mut bytes) = serde_json::to_vec(&acknowledgement) {
        bytes.push(b'\n');
        let _ = writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn cancel_command_acknowledges_and_triggers_the_installed_control() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let writer = Arc::new(Mutex::new(server.try_clone().unwrap()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_callback = Arc::clone(&cancelled);
        let latch = AgentCancellationLatch::default();
        assert!(!latch.worker_cancellation_requested());
        let latch_for_reader = latch.clone();
        latch.install(move || cancelled_for_callback.store(true, Ordering::Release));
        let reader = std::thread::spawn(move || {
            lifecycle_command_reader(server, writer, latch_for_reader);
        });

        serde_json::to_writer(
            &mut client,
            &AgentLifecycleCommandV1::Cancel {
                reason: "worker no-progress deadline".to_string(),
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        let mut response = String::new();
        BufReader::new(client).read_line(&mut response).unwrap();
        let acknowledgement: AgentLifecycleCancellationAckV1 =
            serde_json::from_str(response.trim()).unwrap();

        reader.join().unwrap();
        acknowledgement.validate().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(latch.worker_cancellation_requested());
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
                reason: String::new(),
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        reader.join().unwrap();

        assert!(!latch.worker_cancellation_requested());
    }
}
