//! Bounded loopback request/response servers owned by one agent run.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use temper_protocol_agent::{
    ForgeContextRequest, ForgeContextResponse, PROTOCOL_VERSION, SubmitForPrRequest,
    SubmitForPrResponse, WorkspaceContext,
};

use super::{AttemptFence, SubmitForPrHandler};
use crate::agent_runner::{AcceptedSubmitProofStore, handle_submit_for_pr_with_proof};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;
const ATTEMPT_UNAVAILABLE: &str = "agent attempt is no longer available";

pub(super) struct ForgeSideChannelRequest {
    pub(super) operation: temper_protocol_agent::ForgeContextOperation,
    pub(super) response: std::sync::mpsc::SyncSender<ForgeContextResponse>,
}

/// One listener plus at most one accepted stream. Both are explicitly stopped
/// and the serving thread joined at the run's quiescence boundary.
pub(super) struct LocalServer {
    stop: Arc<AtomicBool>,
    address: String,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LocalServer {
    pub(super) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = self
            .active_stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

pub(super) fn start_submit_server(
    listener: TcpListener,
    address: String,
    handler: SubmitForPrHandler,
    accepted_submit: AcceptedSubmitProofStore,
    context: WorkspaceContext,
    cwd: PathBuf,
    fence: AttemptFence,
) -> LocalServer {
    start_server(listener, address, move |stream, stopping| {
        handle_submit_stream(
            stream,
            &handler,
            &accepted_submit,
            &context,
            &cwd,
            &fence,
            stopping,
        );
    })
}

pub(super) fn start_forge_server(
    listener: TcpListener,
    address: String,
    requests: temper_worker_io::CqSender<ForgeSideChannelRequest>,
    fence: AttemptFence,
) -> LocalServer {
    start_server(listener, address, move |stream, stopping| {
        handle_forge_stream(stream, &requests, &fence, stopping);
    })
}

fn start_server(
    listener: TcpListener,
    address: String,
    mut handle: impl FnMut(TcpStream, &AtomicBool) + Send + 'static,
) -> LocalServer {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let active_stream = Arc::new(Mutex::new(None));
    let active_for_thread = Arc::clone(&active_stream);
    let thread = thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        while !stop_for_thread.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    if stop_for_thread.load(Ordering::Acquire) {
                        break;
                    }
                    if let Ok(shutdown_stream) = stream.try_clone() {
                        *active_for_thread
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(shutdown_stream);
                    }
                    handle(stream, &stop_for_thread);
                    active_for_thread
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    LocalServer {
        stop,
        address,
        active_stream,
        thread: Some(thread),
    }
}

fn handle_forge_stream(
    mut stream: TcpStream,
    requests: &temper_worker_io::CqSender<ForgeSideChannelRequest>,
    fence: &AttemptFence,
    stopping: &AtomicBool,
) {
    set_timeouts(&stream);
    let mut request_bytes = Vec::new();
    let response = match (&mut stream)
        .take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut request_bytes)
    {
        Ok(_) if request_bytes.len() as u64 <= MAX_MESSAGE_BYTES => {
            match serde_json::from_slice::<ForgeContextRequest>(&request_bytes) {
                Ok(request) if request.protocol_version == PROTOCOL_VERSION => {
                    if !fence.is_open() || stopping.load(Ordering::Acquire) {
                        forge_unavailable()
                    } else {
                        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
                        if requests
                            .send(ForgeSideChannelRequest {
                                operation: request.operation,
                                response: response_tx,
                            })
                            .is_err()
                        {
                            forge_unavailable()
                        } else {
                            wait_for_forge_response(response_rx, fence, stopping)
                        }
                    }
                }
                _ => ForgeContextResponse::error(
                    temper_protocol_agent::ForgeContextErrorCode::InvalidRequest,
                ),
            }
        }
        _ => {
            ForgeContextResponse::error(temper_protocol_agent::ForgeContextErrorCode::LimitExceeded)
        }
    };
    write_bounded(&mut stream, &response);
}

fn wait_for_forge_response(
    response: std::sync::mpsc::Receiver<ForgeContextResponse>,
    fence: &AttemptFence,
    stopping: &AtomicBool,
) -> ForgeContextResponse {
    let started = std::time::Instant::now();
    loop {
        if !fence.is_open() || stopping.load(Ordering::Acquire) {
            return forge_unavailable();
        }
        match response.recv_timeout(RESPONSE_POLL_INTERVAL) {
            Ok(response) if fence.is_open() => return response,
            Ok(_) => return forge_unavailable(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return forge_unavailable(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if started.elapsed() < IO_TIMEOUT => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return forge_unavailable(),
        }
    }
}

fn forge_unavailable() -> ForgeContextResponse {
    ForgeContextResponse::error(temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable)
}

fn handle_submit_stream(
    mut stream: TcpStream,
    handler: &SubmitForPrHandler,
    accepted_submit: &AcceptedSubmitProofStore,
    context: &WorkspaceContext,
    cwd: &Path,
    fence: &AttemptFence,
    stopping: &AtomicBool,
) {
    set_timeouts(&stream);
    let mut request_bytes = Vec::new();
    let mut response = if !fence.is_open() || stopping.load(Ordering::Acquire) {
        SubmitForPrResponse::rejected(ATTEMPT_UNAVAILABLE)
    } else {
        match (&mut stream)
            .take(MAX_MESSAGE_BYTES + 1)
            .read_to_end(&mut request_bytes)
        {
            Ok(_) if request_bytes.len() as u64 <= MAX_MESSAGE_BYTES => {
                match serde_json::from_slice::<SubmitForPrRequest>(&request_bytes) {
                    Ok(request) if request.protocol_version == PROTOCOL_VERSION => {
                        handle_submit_for_pr_with_proof(
                            accepted_submit,
                            |request, context, cwd| handler(request, context, cwd),
                            request,
                            context,
                            cwd,
                        )
                    }
                    Ok(request) => SubmitForPrResponse::rejected(format!(
                        "submit_for_pr protocol version mismatch: got {}, expected {}",
                        request.protocol_version, PROTOCOL_VERSION
                    )),
                    Err(error) => SubmitForPrResponse::rejected(format!(
                        "invalid submit_for_pr request: {error}"
                    )),
                }
            }
            Ok(_) => SubmitForPrResponse::rejected("submit_for_pr request exceeds hard limit"),
            Err(error) => {
                SubmitForPrResponse::rejected(format!("read submit_for_pr request: {error}"))
            }
        }
    };
    if !fence.is_open() || stopping.load(Ordering::Acquire) {
        accepted_submit.clear();
        response = SubmitForPrResponse::rejected(ATTEMPT_UNAVAILABLE);
    }
    write_bounded(&mut stream, &response);
}

fn set_timeouts(stream: &TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
}

fn write_bounded(stream: &mut TcpStream, response: &impl serde::Serialize) {
    if let Ok(bytes) = serde_json::to_vec(response) {
        if bytes.len() as u64 <= MAX_MESSAGE_BYTES {
            let _ = stream.write_all(&bytes);
            let _ = stream.shutdown(Shutdown::Write);
        }
    }
}

pub(super) fn submit_for_pr_available(context: &WorkspaceContext) -> bool {
    context.work_item.role == "engineer"
        && context.repos.iter().any(|repo| repo.is_writable())
        && !matches!(
            context.checkout.as_deref(),
            Some("read_only" | "pull_request_read_only")
        )
}
