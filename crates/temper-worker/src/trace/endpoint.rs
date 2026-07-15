use std::io::{self, BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_protocol_activity::{AgentActivityChildRecordV1, AgentActivityFrameV1};

use super::{MAX_CHILD_ACTIVITY_FRAME_BYTES, MAX_CHILD_ACTIVITY_RECORD_BYTES, TraceRun};

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const FRAME_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A per-run loopback endpoint. Each accepted connection is a persistent,
/// newline-delimited stream of independently bounded bare frames or
/// attachment-bearing child records. The stream may remain idle while the run
/// is active.
pub struct ActivityEndpoint {
    address: String,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
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
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_address = address.clone();
        let thread = thread::Builder::new()
            .name(format!("trace-{}", run.run_id()))
            .spawn(move || {
                serve(
                    listener,
                    run,
                    thread_stopping,
                    &thread_address,
                    read_poll_duration,
                )
            })?;
        Ok(Self {
            address,
            stopping,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// Stops accepting records and joins the endpoint thread. The loopback
    /// wake avoids waiting for the nonblocking accept poll, while an accepted
    /// idle stream observes shutdown within its bounded read-poll duration.
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

fn serve(
    listener: TcpListener,
    run: TraceRun,
    stopping: Arc<AtomicBool>,
    address: &str,
    read_poll_duration: Duration,
) {
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) =
                    receive_activity_stream(stream, &run, &stopping, read_poll_duration)
                {
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

/// Receives all newline-delimited activity records on one persistent stream.
/// Socket timeouts are cancellation polls: an active run keeps both the stream
/// and any partial record alive across any number of idle polls.
fn receive_activity_stream(
    stream: TcpStream,
    run: &TraceRun,
    stopping: &AtomicBool,
    read_poll_duration: Duration,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(read_poll_duration))
        .map_err(|error| format!("set activity record read timeout: {error}"))?;
    // Include room for CRLF and one sentinel byte. `Take` prevents a peer from
    // growing the line buffer beyond the absolute record bound before the
    // worker has identified whether the value is a frame or wrapper. Every
    // retry subtracts bytes already accumulated from this original allowance.
    let read_allowance = u64::try_from(MAX_CHILD_ACTIVITY_RECORD_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(3);
    let mut reader = BufReader::new(stream);
    let mut received = false;
    loop {
        let mut bytes = Vec::new();
        loop {
            if stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            let accumulated = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            let remaining = read_allowance.saturating_sub(accumulated);
            if remaining == 0 {
                return Err(format!(
                    "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
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
                    if stopping.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    continue;
                }
                Err(error) => return Err(format!("read child activity record: {error}")),
            };

            if read == 0 {
                if !bytes.is_empty() {
                    return if bytes.len() > MAX_CHILD_ACTIVITY_RECORD_BYTES {
                        Err(format!(
                            "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
                        ))
                    } else {
                        Err("child activity record is not newline terminated".to_string())
                    };
                }
                return if received {
                    Ok(())
                } else {
                    Err("child activity record is empty".to_string())
                };
            }
            if bytes.last() == Some(&b'\n') {
                break;
            }
            // `read_until` can return without a delimiter only at EOF or when
            // this record's bounded `Take` allowance has been exhausted.
            return if bytes.len() > MAX_CHILD_ACTIVITY_RECORD_BYTES {
                Err(format!(
                    "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
                ))
            } else {
                Err("child activity record is not newline terminated".to_string())
            };
        }

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
                run.accept_frame(frame).map_err(|error| error.to_string())?;
            }
            (Err(_), Ok(record)) => {
                run.accept_record(record)
                    .map_err(|error| error.to_string())?;
            }
            (Ok(_), Ok(_)) => {
                return Err("child activity input is ambiguous".to_string());
            }
            (Err(_), Err(_)) => {
                return Err("child activity record is malformed".to_string());
            }
        }
        received = true;
    }
}
