//! Cooperative worker-to-agent lifecycle cancellation transport.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleCancellationAckV1, AgentLifecycleCommandV1,
    MAX_AGENT_LIFECYCLE_FRAME_BYTES,
};

#[derive(Default)]
struct CancellationState {
    requested: bool,
    callback: Option<Arc<dyn Fn() + Send + Sync>>,
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

    fn request(&self) {
        let callback = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.requested = true;
            state.callback.clone()
        };
        if let Some(callback) = callback {
            callback();
        }
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
        AgentLifecycleCommandV1::Cancel { .. } => cancellation.request(),
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
        latch.install(move || cancelled_for_callback.store(true, Ordering::Release));
        let reader = std::thread::spawn(move || {
            lifecycle_command_reader(server, writer, latch);
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
    }
}
