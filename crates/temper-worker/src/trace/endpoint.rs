use std::io::{self, BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_protocol_activity::AgentActivityFrameV1;

use super::{MAX_CHILD_ACTIVITY_FRAME_BYTES, TraceRun};

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const FRAME_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A per-run loopback endpoint. Each accepted connection carries exactly one
/// bounded JSON frame and is closed by the worker after acceptance/rejection.
pub struct ActivityEndpoint {
    address: String,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ActivityEndpoint {
    pub(super) fn bind(run: TraceRun) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?.to_string();
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_address = address.clone();
        let thread = thread::Builder::new()
            .name(format!("trace-{}", run.run_id()))
            .spawn(move || serve(listener, run, thread_stopping, &thread_address))?;
        Ok(Self {
            address,
            stopping,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// Stops accepting frames and joins the endpoint thread. The loopback wake
    /// avoids waiting for the nonblocking poll interval in normal cleanup.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ActivityEndpoint {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn serve(listener: TcpListener, run: TraceRun, stopping: Arc<AtomicBool>, address: &str) {
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = receive_frame(stream, &run) {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.activity.frame_rejected",
                        run_id = run.run_id(),
                        peer = %peer,
                        %error,
                        "worker rejected an agent activity frame"
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

fn receive_frame(stream: TcpStream, run: &TraceRun) -> Result<(), String> {
    stream
        .set_read_timeout(Some(FRAME_READ_TIMEOUT))
        .map_err(|error| format!("set frame read timeout: {error}"))?;
    let limit = u64::try_from(MAX_CHILD_ACTIVITY_FRAME_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut reader = BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(limit)
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("read child frame: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        if bytes.len() > MAX_CHILD_ACTIVITY_FRAME_BYTES {
            return Err(format!(
                "child frame exceeds {MAX_CHILD_ACTIVITY_FRAME_BYTES} bytes"
            ));
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if bytes.is_empty() {
            return Err("child frame is empty".to_string());
        }
        let frame: AgentActivityFrameV1 = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse child frame: {error}"))?;
        run.accept_frame(frame).map_err(|error| error.to_string())?;
    }
}
