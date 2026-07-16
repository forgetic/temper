//! Attempt-bound receiver for the correctness-critical agent lifecycle stream.

use std::io::{self, BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_protocol_agent::{
    AgentLifecycleFrameV1, AgentLifecycleHelloV1, MAX_AGENT_LIFECYCLE_FRAME_BYTES,
};

use crate::agent_runner::JobProgressReporter;

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const READ_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A loopback endpoint created for exactly one worker attempt.
pub(super) struct LifecycleEndpoint {
    address: String,
    stopping: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    stream_finished: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LifecycleEndpoint {
    pub(super) fn bind(reporter: JobProgressReporter) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?.to_string();
        let stopping = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let stream_finished = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_connected = Arc::clone(&connected);
        let thread_stream_finished = Arc::clone(&stream_finished);
        let thread_address = address.clone();
        let attempt_id = reporter.attempt_id().to_string();
        let thread = thread::Builder::new()
            .name(format!("lifecycle-{attempt_id}"))
            .spawn(move || {
                serve(
                    listener,
                    reporter,
                    thread_stopping,
                    thread_connected,
                    thread_stream_finished,
                    &thread_address,
                )
            })?;
        Ok(Self {
            address,
            stopping,
            connected,
            stream_finished,
            thread: Some(thread),
        })
    }

    pub(super) fn address(&self) -> &str {
        &self.address
    }

    pub(super) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        // The producer closes its write half after AgentFinished. Give an
        // already-connected stream a short bounded opportunity to drain before
        // signalling shutdown so the terminal boundary is not raced by child
        // exit. A child that never connects pays only the connect poll bound.
        for _ in 0..10 {
            if self.connected.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        if self.connected.load(Ordering::Acquire) {
            for _ in 0..100 {
                if self.stream_finished.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LifecycleEndpoint {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn serve(
    listener: TcpListener,
    reporter: JobProgressReporter,
    stopping: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    stream_finished: Arc<AtomicBool>,
    address: &str,
) {
    let mut expected_seq = 1_u64;
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                connected.store(true, Ordering::Release);
                stream_finished.store(false, Ordering::Release);
                let outcome =
                    receive_lifecycle_stream(stream, &reporter, &stopping, &mut expected_seq);
                stream_finished.store(true, Ordering::Release);
                if let Err(error) = outcome {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.lifecycle.frame_rejected",
                        attempt_id = reporter.attempt_id(),
                        peer = %peer,
                        %error,
                        "worker closed an invalid agent lifecycle stream"
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.lifecycle.accept_failed",
                    attempt_id = reporter.attempt_id(),
                    endpoint = address,
                    %error,
                    "worker lifecycle endpoint stopped after an accept failure"
                );
                break;
            }
        }
    }
}

fn receive_lifecycle_stream(
    stream: TcpStream,
    reporter: &JobProgressReporter,
    stopping: &AtomicBool,
    expected_seq: &mut u64,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(READ_POLL_INTERVAL))
        .map_err(|error| format!("set lifecycle read timeout: {error}"))?;
    let mut reader = BufReader::new(stream);
    let hello = read_record(&mut reader, stopping)?
        .ok_or_else(|| "lifecycle stream is empty".to_string())?;
    serde_json::from_slice::<AgentLifecycleHelloV1>(&hello)
        .map_err(|_| "lifecycle hello is malformed".to_string())?
        .validate()
        .map_err(|error| error.to_string())?;

    while let Some(bytes) = read_record(&mut reader, stopping)? {
        let frame = serde_json::from_slice::<AgentLifecycleFrameV1>(&bytes)
            .map_err(|_| "lifecycle frame is malformed".to_string())?;
        frame.validate().map_err(|error| error.to_string())?;
        if frame.seq < *expected_seq {
            // At-least-once child retry: duplicate frames never refresh worker
            // liveness or invoke the reporter twice.
            continue;
        }
        if frame.seq > *expected_seq {
            return Err(format!(
                "lifecycle sequence gap: expected {}, received {}",
                *expected_seq, frame.seq
            ));
        }
        *expected_seq = expected_seq.saturating_add(1);
        // A false return includes a stale attempt. The endpoint deliberately
        // ignores it; worker policy owns attempt currentness.
        let _ = reporter.accept_frame(frame);
    }
    Ok(())
}

fn read_record(
    reader: &mut BufReader<TcpStream>,
    stopping: &AtomicBool,
) -> Result<Option<Vec<u8>>, String> {
    let allowance = u64::try_from(MAX_AGENT_LIFECYCLE_FRAME_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(3);
    let mut bytes = Vec::new();
    loop {
        if stopping.load(Ordering::Acquire) {
            return Ok(None);
        }
        let accumulated = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let remaining = allowance.saturating_sub(accumulated);
        if remaining == 0 {
            return Err(format!(
                "lifecycle input exceeds {MAX_AGENT_LIFECYCLE_FRAME_BYTES} bytes"
            ));
        }
        let read = match reader
            .by_ref()
            .take(remaining)
            .read_until(b'\n', &mut bytes)
        {
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(format!("read lifecycle input: {error}")),
        };
        if read == 0 {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err("lifecycle input is not newline terminated".to_string());
        }
        if bytes.last() == Some(&b'\n') {
            break;
        }
        return Err(format!(
            "lifecycle input exceeds {MAX_AGENT_LIFECYCLE_FRAME_BYTES} bytes"
        ));
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err("lifecycle input is empty".to_string());
    }
    if bytes.len() > MAX_AGENT_LIFECYCLE_FRAME_BYTES {
        return Err(format!(
            "lifecycle input exceeds {MAX_AGENT_LIFECYCLE_FRAME_BYTES} bytes"
        ));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

    use temper_protocol_agent::{
        AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleEventV1, AgentLifecycleFrameV1,
        AgentLifecycleScopeV1,
    };

    use super::*;

    fn frame(seq: u64) -> AgentLifecycleFrameV1 {
        AgentLifecycleFrameV1 {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
            seq,
            scope: AgentLifecycleScopeV1 {
                id: "main".to_string(),
                parent_id: None,
            },
            event: AgentLifecycleEventV1::ModelProgress {
                call_id: "call-1".to_string(),
            },
        }
    }

    fn write_line(stream: &mut TcpStream, value: &impl serde::Serialize) {
        serde_json::to_writer(&mut *stream, value).expect("serialize lifecycle record");
        stream.write_all(b"\n").expect("write newline");
    }

    #[test]
    fn malformed_oversized_duplicates_gaps_and_stale_attempts_are_fenced() {
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let accepted_for_sink = Arc::clone(&accepted);
        let current = Arc::new(AtomicBool::new(true));
        let current_for_guard = Arc::clone(&current);
        let reporter = JobProgressReporter::with_attempt_guard(
            "attempt-a",
            move |_| current_for_guard.load(Ordering::Acquire),
            move |progress| accepted_for_sink.lock().unwrap().push(progress.frame.seq),
        );
        let endpoint = LifecycleEndpoint::bind(reporter).expect("bind lifecycle endpoint");

        let mut stream = TcpStream::connect(endpoint.address()).expect("connect lifecycle");
        write_line(&mut stream, &AgentLifecycleHelloV1::default());
        write_line(&mut stream, &frame(1));
        write_line(&mut stream, &frame(1)); // duplicate ignored
        write_line(&mut stream, &frame(2));
        write_line(&mut stream, &frame(4)); // gap closes this connection
        let _ = stream.shutdown(std::net::Shutdown::Write);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*accepted.lock().unwrap(), vec![1, 2]);

        current.store(false, Ordering::Release);
        let mut stale = TcpStream::connect(endpoint.address()).expect("reconnect lifecycle");
        write_line(&mut stale, &AgentLifecycleHelloV1::default());
        write_line(&mut stale, &frame(3));
        let _ = stale.shutdown(std::net::Shutdown::Write);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*accepted.lock().unwrap(), vec![1, 2]);

        let mut malformed = TcpStream::connect(endpoint.address()).expect("malformed connection");
        write_line(&mut malformed, &AgentLifecycleHelloV1::default());
        malformed.write_all(b"not-json\n").unwrap();
        let _ = malformed.shutdown(std::net::Shutdown::Write);
        std::thread::sleep(Duration::from_millis(30));

        let mut oversized = TcpStream::connect(endpoint.address()).expect("oversized connection");
        write_line(&mut oversized, &AgentLifecycleHelloV1::default());
        let payload = vec![b'x'; MAX_AGENT_LIFECYCLE_FRAME_BYTES + 1];
        let _ = oversized.write_all(&payload);
        let _ = oversized.write_all(b"\n");
        let _ = oversized.shutdown(std::net::Shutdown::Write);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*accepted.lock().unwrap(), vec![1, 2]);
        endpoint.stop();
    }
}
