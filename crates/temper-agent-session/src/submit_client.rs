// SPDX-License-Identifier: MPL-2.0

//! Cancellation-aware client for the worker-owned `submit_for_pr` channel.
//!
//! Each call owns a joined socket task. Dropping the tool future shuts down the
//! active stream and joins the blocking reader, so a hung gate cannot survive
//! the agent task-group's quiescence boundary.

use std::future::Future;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use temper_agent::SubmitForPrHost;
use temper_protocol_agent::{SubmitForPrRequest, SubmitForPrResponse};

const SUBMIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_CANCELLATION_SLICE: Duration = Duration::from_millis(100);
const SUBMIT_REQUEST_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn host_for_address(address: String) -> SubmitForPrHost {
    Arc::new(move |request, _context, _cwd| {
        let address = address.clone();
        Box::pin(async move { SubmitIoTask::spawn(address, request).await })
    })
}

#[cfg(test)]
fn submit_once_result(
    address: &str,
    request: &SubmitForPrRequest,
) -> std::io::Result<SubmitForPrResponse> {
    submit_once_cancellable(address, request, &AtomicBool::new(false), &Mutex::new(None))
}

fn submit_once_cancellable(
    address: &str,
    request: &SubmitForPrRequest,
    cancelled: &AtomicBool,
    active_stream: &Mutex<Option<TcpStream>>,
) -> std::io::Result<SubmitForPrResponse> {
    let mut stream = connect_submit_channel_cancellable(address, cancelled)?;
    stream.set_write_timeout(Some(SUBMIT_REQUEST_WRITE_TIMEOUT))?;
    stream.set_read_timeout(None)?;
    *active_stream
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stream.try_clone()?);
    if cancelled.load(Ordering::Acquire) {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "submit_for_pr request cancelled",
        ));
    }

    let bytes = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    let read = stream.read_to_end(&mut response);
    active_stream
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    read?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn connect_submit_channel_cancellable(
    address: &str,
    cancelled: &AtomicBool,
) -> std::io::Result<TcpStream> {
    let deadline = Instant::now() + SUBMIT_CONNECT_TIMEOUT;
    let mut last_error = None;
    for socket_address in address.to_socket_addrs()? {
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "submit_for_pr connection cancelled",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match TcpStream::connect_timeout(
                &socket_address,
                remaining.min(CONNECT_CANCELLATION_SLICE),
            ) {
                Ok(stream) => return Ok(stream),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    last_error = Some(error);
                }
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("submit_for_pr host address `{address}` resolved to no socket addresses"),
        )
    }))
}

struct SubmitTaskState {
    result: Option<SubmitForPrResponse>,
    waker: Option<Waker>,
}

struct SubmitIoTask {
    state: Arc<Mutex<SubmitTaskState>>,
    cancelled: Arc<AtomicBool>,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl SubmitIoTask {
    fn spawn(address: String, request: SubmitForPrRequest) -> Self {
        let state = Arc::new(Mutex::new(SubmitTaskState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let active_stream = Arc::new(Mutex::new(None));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_stream = Arc::clone(&active_stream);
        let thread = match thread::Builder::new()
            .name("submit-for-pr-client".to_string())
            .spawn(move || {
                let response = match submit_once_cancellable(
                    &address,
                    &request,
                    &thread_cancelled,
                    &thread_stream,
                ) {
                    Ok(response) => response,
                    Err(error) => SubmitForPrResponse::rejected(format!(
                        "submit_for_pr host channel failed: {error}"
                    )),
                };
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(response);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .result = Some(SubmitForPrResponse::rejected(format!(
                    "submit_for_pr host channel failed to start: {error}"
                )));
                None
            }
        };
        Self {
            state,
            cancelled,
            active_stream,
            thread,
        }
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Future for SubmitIoTask {
    type Output = SubmitForPrResponse;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let response = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.result.is_none()
                && !state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            state.result.take()
        };
        match response {
            Some(response) => {
                self.join();
                Poll::Ready(response)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for SubmitIoTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(stream) = self
            .active_stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use temper_protocol_agent::{PROTOCOL_VERSION, SubmitForPrGate};

    use super::*;

    #[test]
    fn delayed_host_response_is_returned_as_normal_submit_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind submit test listener");
        let address = listener
            .local_addr()
            .expect("read submit test listener address")
            .to_string();
        let response_delay = Duration::from_millis(200);

        let server = thread::spawn(move || {
            let (mut stream, _addr) = listener.accept().expect("accept submit request");
            let mut request_bytes = Vec::new();
            stream
                .read_to_end(&mut request_bytes)
                .expect("read submit request");
            let request: SubmitForPrRequest =
                serde_json::from_slice(&request_bytes).expect("parse submit request");
            assert_eq!(request.protocol_version, PROTOCOL_VERSION);
            assert_eq!(request.correlation_key, "pr-for-code-524");
            assert_eq!(request.summary.as_deref(), Some("ready after long checks"));

            thread::sleep(response_delay);
            let response = SubmitForPrResponse {
                accepted: false,
                message: "pre-push command timed out".to_string(),
                gates: vec![SubmitForPrGate {
                    command_id: "pre-push:slow-check".to_string(),
                    argv: vec!["sleep".to_string(), "120".to_string()],
                    cwd: "/workspace/temper".to_string(),
                    exit_status: "timeout".to_string(),
                    exit_code: None,
                    stdout_tail: String::new(),
                    stderr_tail: "command exceeded configured timeout".to_string(),
                    timed_out: true,
                    elapsed_ms: 120_000,
                }],
            };
            let response_bytes = serde_json::to_vec(&response).expect("serialize response");
            stream
                .write_all(&response_bytes)
                .expect("write delayed submit response");
            stream
                .shutdown(Shutdown::Write)
                .expect("half-close delayed submit response");
            response
        });

        let request = SubmitForPrRequest {
            protocol_version: PROTOCOL_VERSION,
            correlation_key: "pr-for-code-524".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            summary: Some("ready after long checks".to_string()),
        };

        let started = Instant::now();
        let response =
            submit_once_result(&address, &request).expect("delayed host response should be read");
        assert!(started.elapsed() >= response_delay);
        let expected = server.join().expect("submit response thread");
        assert_eq!(response, expected);
    }

    #[test]
    fn dropping_client_future_closes_a_hung_stream_and_joins() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).expect("read request");
            thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(b"{}");
        });
        let request = SubmitForPrRequest {
            protocol_version: PROTOCOL_VERSION,
            correlation_key: "cancel".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            summary: None,
        };
        let outcome = temper_agent_io::block_on(async move {
            temper_agent_io::timeout(
                Duration::from_millis(100),
                SubmitIoTask::spawn(address, request),
            )
            .await
        });
        assert!(outcome.is_err());
        server.join().expect("server observed closed stream");
    }
}
