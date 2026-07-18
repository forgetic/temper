//! Temper-owned HTTP provider for cancellation/loop regressions.
//!
//! Unlike Jig's general-purpose request oracle, this fixture never stores a
//! complete request body or append-only request history. It incrementally
//! drains each body, retains a small tail for a fixed number of recent
//! requests, and serves one deterministic looping tool response.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_process_containment::{BoundedCapture, CaptureMode};

pub const MAX_PROVIDER_REQUEST_BYTES: usize = 1024 * 1024;
pub const RETAINED_PROVIDER_REQUESTS: usize = 4;
pub const PROVIDER_REQUEST_TAIL_BYTES: usize = 8 * 1024;
pub const PROVIDER_HISTORY_BYTES: usize = RETAINED_PROVIDER_REQUESTS * PROVIDER_REQUEST_TAIL_BYTES;
const MAX_PROVIDER_HEADER_BYTES: usize = 32 * 1024;
const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(5);
const PROVIDER_IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoundedProviderStats {
    pub request_count: usize,
    pub retained_request_count: usize,
    pub retained_history_bytes: usize,
    pub dropped_history_bytes: u64,
    pub largest_request_bytes: u64,
    pub oversized_request_count: usize,
}

#[derive(Default)]
struct ProviderState {
    stats: BoundedProviderStats,
    retained: VecDeque<Vec<u8>>,
}

/// A fixed-response OpenAI-compatible provider with bounded request history.
pub struct BoundedFixedProvider {
    address: SocketAddr,
    shutdown: Arc<AtomicBool>,
    state: Arc<Mutex<ProviderState>>,
    thread: Option<JoinHandle<()>>,
}

impl BoundedFixedProvider {
    pub fn start_looping_tool_response(command: impl Into<String>) -> io::Result<Self> {
        let command = command.into();
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ProviderState::default()));
        let owner_shutdown = Arc::clone(&shutdown);
        let owner_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("temper-bounded-fixed-provider".to_string())
            .spawn(move || serve(listener, &owner_shutdown, &owner_state, &command))?;
        Ok(Self {
            address,
            shutdown,
            state,
            thread: Some(thread),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn stats(&self) -> BoundedProviderStats {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stats
            .clone()
    }
}

impl Drop for BoundedFixedProvider {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Wake a nonblocking accept loop promptly even on platforms where its
        // sleep granularity is coarse.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    shutdown: &AtomicBool,
    state: &Mutex<ProviderState>,
    command: &str,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle_connection(stream, state, command);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(PROVIDER_POLL_INTERVAL);
            }
            Err(_) => return,
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    state: &Mutex<ProviderState>,
    command: &str,
) -> io::Result<()> {
    stream.set_read_timeout(Some(PROVIDER_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(PROVIDER_IO_TIMEOUT))?;
    let header = read_header(&mut stream)?;
    let content_length = content_length(&header)?;

    let mut capture = BoundedCapture::new(CaptureMode::Tail, PROVIDER_REQUEST_TAIL_BYTES);
    let mut remaining = content_length;
    let mut buffer = [0_u8; 8 * 1024];
    while remaining > 0 {
        let want = remaining.min(buffer.len());
        let read = stream.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "provider request body ended early",
            ));
        }
        capture.push(&buffer[..read]);
        remaining -= read;
    }
    let captured = capture.finish().expect("tail capture cannot overflow");
    record_request(state, content_length, captured.into_bytes());

    if content_length > MAX_PROVIDER_REQUEST_BYTES {
        return write_response(
            &mut stream,
            "413 Payload Too Large",
            "application/json",
            br#"{"error":{"message":"fixture request exceeded byte limit"}}"#,
        );
    }
    write_response(
        &mut stream,
        "200 OK",
        "text/event-stream",
        looping_tool_response(command).as_bytes(),
    )
}

fn read_header(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while header.len() < MAX_PROVIDER_HEADER_BYTES {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "provider request ended before headers",
            ));
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(header);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "provider request headers exceeded byte limit",
    ))
}

fn content_length(header: &[u8]) -> io::Result<usize> {
    let text = String::from_utf8_lossy(header);
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if !name.trim().eq_ignore_ascii_case("content-length") {
                continue;
            }
            return value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid provider content-length: {error}"),
                )
            });
        }
    }
    Ok(0)
}

fn record_request(state: &Mutex<ProviderState>, content_length: usize, tail: Vec<u8>) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.stats.request_count = state.stats.request_count.saturating_add(1);
    let observed = u64::try_from(content_length).unwrap_or(u64::MAX);
    state.stats.largest_request_bytes = state.stats.largest_request_bytes.max(observed);
    if content_length > MAX_PROVIDER_REQUEST_BYTES {
        state.stats.oversized_request_count = state.stats.oversized_request_count.saturating_add(1);
    }
    state.stats.dropped_history_bytes = state
        .stats
        .dropped_history_bytes
        .saturating_add(observed.saturating_sub(u64::try_from(tail.len()).unwrap_or(u64::MAX)));

    while state.retained.len() >= RETAINED_PROVIDER_REQUESTS {
        if let Some(evicted) = state.retained.pop_front() {
            state.stats.dropped_history_bytes = state
                .stats
                .dropped_history_bytes
                .saturating_add(u64::try_from(evicted.len()).unwrap_or(u64::MAX));
        }
    }
    state.retained.push_back(tail);
    while state.retained.iter().map(Vec::len).sum::<usize>() > PROVIDER_HISTORY_BYTES {
        if let Some(evicted) = state.retained.pop_front() {
            state.stats.dropped_history_bytes = state
                .stats
                .dropped_history_bytes
                .saturating_add(u64::try_from(evicted.len()).unwrap_or(u64::MAX));
        }
    }
    state.stats.retained_request_count = state.retained.len();
    state.stats.retained_history_bytes = state.retained.iter().map(Vec::len).sum();
}

fn looping_tool_response(command: &str) -> String {
    let frames = [
        serde_json::json!({
            "choices": [{"delta": {"role": "assistant"}, "finish_reason": null}]
        })
        .to_string(),
        serde_json::json!({
            "choices": [{
                "delta": {"content": "{\"verdict\":\"needs_architect\",\"summary\":\"must not become a result\"}"},
                "finish_reason": null
            }]
        })
        .to_string(),
        serde_json::json!({
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "abort-loop",
                    "type": "function",
                    "function": {"name": "bash", "arguments": ""}
                }]},
                "finish_reason": null
            }]
        })
        .to_string(),
        serde_json::json!({
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"arguments": serde_json::json!({"command": command}).to_string()}
                }]},
                "finish_reason": null
            }]
        })
        .to_string(),
        serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        })
        .to_string(),
        "[DONE]".to_string(),
    ];
    frames
        .into_iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect()
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}
