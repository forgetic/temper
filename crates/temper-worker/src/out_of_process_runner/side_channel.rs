//! Bounded loopback request/response servers owned by one agent run.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use temper_protocol_agent::{
    ForgeContextRequest, ForgeContextResponse, PROTOCOL_VERSION, SubmitForPrRequest,
    SubmitForPrResponse, WorkspaceContext,
};

use super::SubmitForPrHandler;
use crate::agent_runner::{AcceptedSubmitProofStore, handle_submit_for_pr_with_proof};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

pub(super) struct ForgeSideChannelRequest {
    pub(super) operation: temper_protocol_agent::ForgeContextOperation,
    pub(super) response: std::sync::mpsc::SyncSender<ForgeContextResponse>,
}

pub(super) struct LocalServer {
    stop: Arc<AtomicBool>,
    address: String,
    thread: Option<thread::JoinHandle<()>>,
}

impl LocalServer {
    pub(super) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn start_submit_server(
    listener: TcpListener,
    address: String,
    handler: SubmitForPrHandler,
    accepted_submit: AcceptedSubmitProofStore,
    context: WorkspaceContext,
    cwd: PathBuf,
) -> LocalServer {
    start_server(listener, address, move |stream| {
        handle_submit_stream(stream, &handler, &accepted_submit, &context, &cwd);
    })
}

pub(super) fn start_forge_server(
    listener: TcpListener,
    address: String,
    requests: temper_worker_io::CqSender<ForgeSideChannelRequest>,
) -> LocalServer {
    start_server(listener, address, move |stream| {
        handle_forge_stream(stream, &requests);
    })
}

fn start_server(
    listener: TcpListener,
    address: String,
    mut handle: impl FnMut(TcpStream) + Send + 'static,
) -> LocalServer {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        while !stop_for_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _addr)) => handle(stream),
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
        thread: Some(thread),
    }
}

fn handle_forge_stream(
    mut stream: TcpStream,
    requests: &temper_worker_io::CqSender<ForgeSideChannelRequest>,
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
                    let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
                    if requests
                        .send(ForgeSideChannelRequest {
                            operation: request.operation,
                            response: response_tx,
                        })
                        .is_err()
                    {
                        ForgeContextResponse::error(
                            temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable,
                        )
                    } else {
                        response_rx.recv_timeout(IO_TIMEOUT).unwrap_or_else(|_| {
                            ForgeContextResponse::error(
                                temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable,
                            )
                        })
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

fn handle_submit_stream(
    mut stream: TcpStream,
    handler: &SubmitForPrHandler,
    accepted_submit: &AcceptedSubmitProofStore,
    context: &WorkspaceContext,
    cwd: &Path,
) {
    set_timeouts(&stream);
    let mut request_bytes = Vec::new();
    let response = match (&mut stream)
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
                Err(error) => {
                    SubmitForPrResponse::rejected(format!("invalid submit_for_pr request: {error}"))
                }
            }
        }
        Ok(_) => SubmitForPrResponse::rejected("submit_for_pr request exceeds hard limit"),
        Err(error) => SubmitForPrResponse::rejected(format!("read submit_for_pr request: {error}")),
    };
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
