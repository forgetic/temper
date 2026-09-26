use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_protocol_activity::{
    AgentActivityChildRecordV1, AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1,
};

use super::{MAX_CHILD_ACTIVITY_FRAME_BYTES, MAX_CHILD_ACTIVITY_RECORD_BYTES, TraceRun};

mod reader;
use reader::ActivityRecordReader;

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const FRAME_READ_TIMEOUT: Duration = Duration::from_secs(2);
// Bound even a continuously writing peer after shutdown. This wire budget is
// independent of accepted-event quotas (duplicates and whitespace cost bytes).
const MAX_TERMINAL_DRAIN_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
#[path = "endpoint_completion_tests.rs"]
mod completion_tests;
#[cfg(test)]
#[path = "endpoint_drain_tests.rs"]
mod drain_tests;

/// A per-run loopback endpoint. Each accepted connection is a persistent,
/// newline-delimited stream of independently bounded bare frames or
/// attachment-bearing child records. The stream may remain idle while the run
/// is active.
#[derive(Clone, Default)]
struct ActivityEndpointState {
    stopping: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    stream_finished: Arc<AtomicBool>,
    main_scope_started: Arc<AtomicBool>,
    main_scope_finished: Arc<AtomicBool>,
}

pub struct ActivityEndpoint {
    address: String,
    state: ActivityEndpointState,
    thread: Option<JoinHandle<()>>,
}

impl TraceRun {
    /// Binds a known first-party producer whose successful capture requires
    /// both durable main-scope boundaries, even if no child record arrives.
    /// Synthetic and third-party traces may use `bind_endpoint` instead.
    pub fn bind_endpoint_requiring_main_scope(&self) -> io::Result<ActivityEndpoint> {
        self.inner
            .state
            .lock()
            .expect("trace run state lock")
            .main_scope
            .require();
        self.bind_endpoint()
    }
}

impl ActivityEndpoint {
    pub(super) fn bind(run: TraceRun) -> io::Result<Self> {
        Self::bind_with_read_poll(run, FRAME_READ_TIMEOUT)
    }

    /// Binds an endpoint with a short shutdown/read poll for deterministic
    /// stream receiver tests without changing the production endpoint API.
    #[cfg(test)]
    pub(crate) fn bind_with_read_timeout(
        run: TraceRun,
        read_poll_duration: Duration,
    ) -> io::Result<Self> {
        Self::bind_with_read_poll(run, read_poll_duration)
    }

    fn bind_with_read_poll(run: TraceRun, read_poll_duration: Duration) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?.to_string();
        let state = ActivityEndpointState::default();
        let thread_state = state.clone();
        let thread_address = address.clone();
        let thread = thread::Builder::new()
            .name(format!("trace-{}", run.run_id()))
            .spawn(move || {
                serve(
                    listener,
                    run,
                    thread_state,
                    &thread_address,
                    read_poll_duration,
                )
            })?;
        Ok(Self {
            address,
            state,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// Joins the endpoint after a finite drain of buffered and socket bytes.
    /// Socket waiting has one read-poll budget, independent of durable append
    /// latency; a wire-byte cap also bounds continuously writing peers.
    pub fn stop(mut self) -> bool {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> bool {
        // A main-scope terminus is the final child record for the run. Give an
        // already-connected producer a short bounded opportunity to make that
        // record durable before closing the endpoint; otherwise the host's
        // outer terminal event could race ahead and reject the typed child
        // reason as `AlreadyTerminal`.
        for _ in 0..10 {
            if self.state.connected.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        if self.state.connected.load(Ordering::Acquire) {
            // `connect` can win just before the receiver ingests scope.started.
            // Let it identify a real main-scope stream before deciding whether
            // the terminal drain applies.
            for _ in 0..10 {
                if self.state.main_scope_started.load(Ordering::Acquire)
                    || self.state.stream_finished.load(Ordering::Acquire)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        if self.state.main_scope_started.load(Ordering::Acquire) {
            for _ in 0..250 {
                if self.state.main_scope_finished.load(Ordering::Acquire)
                    || self.state.stream_finished.load(Ordering::Acquire)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.state.stopping.store(true, Ordering::Release);
        self.thread
            .take()
            .is_none_or(|thread| thread.join().is_ok())
    }
}

impl Drop for ActivityEndpoint {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

fn serve(
    listener: TcpListener,
    run: TraceRun,
    state: ActivityEndpointState,
    address: &str,
    read_poll_duration: Duration,
) {
    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                // A producer may already be queued when stop is requested.
                // Accept and drain that connection before leaving the listener.
                state.connected.store(true, Ordering::Release);
                state.stream_finished.store(false, Ordering::Release);
                let outcome = receive_activity_stream(
                    stream,
                    &run,
                    &state,
                    read_poll_duration,
                    MAX_TERMINAL_DRAIN_BYTES,
                );
                state.stream_finished.store(true, Ordering::Release);
                if let Err(error) = outcome {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.activity.record_rejected",
                        run_id = run.run_id(),
                        peer = %peer,
                        %error,
                        "worker rejected an agent activity record"
                    );
                }
                if state.stopping.load(Ordering::Acquire) {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if state.stopping.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.accept_failed",
                    run_id = run.run_id(),
                    endpoint = address,
                    %error,
                    "worker activity endpoint stopped after an accept failure"
                );
                break;
            }
        }
    }
}

/// Receives all newline-delimited activity records on one persistent stream.
/// Socket timeouts are cancellation polls: an active run keeps both the stream
/// and any partial record alive across any number of idle polls.
fn receive_activity_stream(
    stream: TcpStream,
    run: &TraceRun,
    state: &ActivityEndpointState,
    read_poll_duration: Duration,
    drain_byte_limit: u64,
) -> Result<(), String> {
    let mut reader = ActivityRecordReader::new(stream, read_poll_duration, drain_byte_limit)?;
    let mut received = false;
    while let Some(mut bytes) = reader.read_record(&state.stopping)? {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if bytes.is_empty() {
            return Err("child activity record is empty".to_string());
        }
        if bytes.len() > MAX_CHILD_ACTIVITY_RECORD_BYTES {
            return Err(format!(
                "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
            ));
        }

        let frame = serde_json::from_slice::<AgentActivityFrameV1>(&bytes);
        let record = serde_json::from_slice::<AgentActivityChildRecordV1>(&bytes);
        match (frame, record) {
            (Ok(frame), Err(_)) => {
                if bytes.len() > MAX_CHILD_ACTIVITY_FRAME_BYTES {
                    return Err(format!(
                        "child frame exceeds {MAX_CHILD_ACTIVITY_FRAME_BYTES} bytes"
                    ));
                }
                let is_main_start = is_main_scope_start(&frame);
                let is_main_terminal = is_main_scope_terminal(&frame);
                run.accept_frame(frame).map_err(|error| error.to_string())?;
                if is_main_start {
                    state.main_scope_started.store(true, Ordering::Release);
                }
                if is_main_terminal {
                    state.main_scope_finished.store(true, Ordering::Release);
                }
            }
            (Err(_), Ok(record)) => {
                let is_main_start = is_main_scope_start(&record.frame);
                let is_main_terminal = is_main_scope_terminal(&record.frame);
                run.accept_record(record)
                    .map_err(|error| error.to_string())?;
                if is_main_start {
                    state.main_scope_started.store(true, Ordering::Release);
                }
                if is_main_terminal {
                    state.main_scope_finished.store(true, Ordering::Release);
                }
            }
            (Ok(_), Ok(_)) => {
                return Err("child activity input is ambiguous".to_string());
            }
            (Err(_), Err(_)) => {
                return Err("child activity record is malformed".to_string());
            }
        }
        received = true;
        if state.main_scope_finished.load(Ordering::Acquire) {
            return Ok(());
        }
    }
    if received || state.stopping.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err("child activity record is empty".to_string())
    }
}

fn is_main_scope_start(frame: &AgentActivityFrameV1) -> bool {
    frame.scope.kind == AgentScopeKindV1::Main
        && matches!(frame.event, AgentActivityEventV1::ScopeStarted(_))
}

fn is_main_scope_terminal(frame: &AgentActivityFrameV1) -> bool {
    frame.scope.kind == AgentScopeKindV1::Main
        && matches!(frame.event, AgentActivityEventV1::ScopeFinished(_))
}
