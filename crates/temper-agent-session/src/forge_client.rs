//! Cancellation-aware agent-side client for the worker Forge read channel.

use std::future::Future;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use temper_agent::ForgeContextHost;
use temper_protocol_agent::{
    ForgeContextErrorCode, ForgeContextOperation, ForgeContextRequest, ForgeContextResponse,
    ForgeContextToolOutcome, PROTOCOL_VERSION,
};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_CANCELLATION_SLICE: Duration = Duration::from_millis(100);
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

type ForgeResult = Result<temper_protocol_agent::ForgeContextResult, ForgeContextErrorCode>;

pub(crate) fn host_for_address(address: String) -> ForgeContextHost {
    Arc::new(move |operation| {
        let address = address.clone();
        Box::pin(async move { ForgeIoTask::spawn(address, operation).await })
    })
}

fn fetch_once_cancellable(
    address: &str,
    operation: ForgeContextOperation,
    cancelled: &AtomicBool,
    active_stream: &Mutex<Option<TcpStream>>,
) -> ForgeResult {
    let request = ForgeContextRequest {
        protocol_version: PROTOCOL_VERSION,
        operation,
    };
    let response = fetch_once_io(address, &request, cancelled, active_stream)
        .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(ForgeContextErrorCode::InvalidRequest);
    }
    match response.outcome {
        ForgeContextToolOutcome::Success { result } => Ok(result),
        ForgeContextToolOutcome::Error { code } => Err(code),
    }
}

fn fetch_once_io(
    address: &str,
    request: &ForgeContextRequest,
    cancelled: &AtomicBool,
    active_stream: &Mutex<Option<TcpStream>>,
) -> std::io::Result<ForgeContextResponse> {
    let mut stream = connect(address, cancelled)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    *active_stream
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stream.try_clone()?);
    if cancelled.load(Ordering::Acquire) {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Forge request cancelled",
        ));
    }

    let bytes = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Forge request exceeds hard limit",
        ));
    }
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    let read = (&mut stream)
        .take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut response);
    active_stream
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    read?;
    if response.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Forge response exceeds hard limit",
        ));
    }
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn connect(address: &str, cancelled: &AtomicBool) -> std::io::Result<TcpStream> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut last_error = None;
    for socket_address in address.to_socket_addrs()? {
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Forge connection cancelled",
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
            "Forge host address resolved to no socket addresses",
        )
    }))
}

struct ForgeTaskState {
    result: Option<ForgeResult>,
    waker: Option<Waker>,
}

/// Joined blocking socket operation. Its Drop is the cancellation path used by
/// the generic tool deadline and nested-agent cancellation.
struct ForgeIoTask {
    state: Arc<Mutex<ForgeTaskState>>,
    cancelled: Arc<AtomicBool>,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl ForgeIoTask {
    fn spawn(address: String, operation: ForgeContextOperation) -> Self {
        let state = Arc::new(Mutex::new(ForgeTaskState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let active_stream = Arc::new(Mutex::new(None));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_stream = Arc::clone(&active_stream);
        let thread = match thread::Builder::new()
            .name("forge-side-channel-client".to_string())
            .spawn(move || {
                let result =
                    fetch_once_cancellable(&address, operation, &thread_cancelled, &thread_stream);
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(result);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }) {
            Ok(thread) => Some(thread),
            Err(_) => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .result = Some(Err(ForgeContextErrorCode::ForgeUnavailable));
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

impl Future for ForgeIoTask {
    type Output = ForgeResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
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
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ForgeIoTask {
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
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn dropping_a_hung_forge_call_closes_the_socket_and_joins() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("request");
            thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(b"{}");
        });
        let operation =
            ForgeContextOperation::ForgeGetItem(temper_protocol_agent::ForgeGetItemOperation {
                repo: "ai/temper".to_string(),
                number: 1,
                artifact_type: None,
                include_comments: false,
            });
        let outcome = temper_agent_io::block_on(async move {
            temper_agent_io::timeout(
                Duration::from_millis(100),
                ForgeIoTask::spawn(address, operation),
            )
            .await
        });
        assert!(outcome.is_err());
        server.join().expect("server observed socket close");
    }
}
