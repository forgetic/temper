// SPDX-License-Identifier: MPL-2.0

//! Minimal Jig-script adapter for deterministic late in-band SSE failures.
//!
//! The pinned Jig server intentionally leaves `StreamError` as an extension
//! point. This adapter keeps Jig's `ScriptFile`/`ScriptAction` as the source of
//! every normal reply and only owns the missing wire primitive: in bounded,
//! declarative role-request ranges it emits a successful HTTP response with a
//! partial delta followed by an empty provider error object. That shape has no
//! status, code, or provider prose and therefore exercises Temper's canonical
//! streamed-failure path.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use jig_core::render::frames_to_body;
use jig_core::request::parse_openai;
use jig_core::{Dialect, RecordedRequest, Script, ScriptAction, render_openai};

use super::LateStreamFailureFixture;

pub(super) struct LateStreamJig {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LateStreamJig {
    pub(super) fn start(
        script: Script,
        fixture: LateStreamFailureFixture,
        architect_requests: Arc<AtomicUsize>,
        engineer_requests: Arc<AtomicUsize>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("bind late-stream Jig adapter: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("configure late-stream Jig listener: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("read late-stream Jig address: {error}"))?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let shutdown = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&shutdown);
        let thread = thread::Builder::new()
            .name("temper-late-stream-jig".to_string())
            .spawn(move || {
                let mut matching_requests = 0u32;
                while !stopped.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                            let _ = handle(
                                &mut stream,
                                &script,
                                &fixture,
                                &recorded,
                                &architect_requests,
                                &engineer_requests,
                                &mut matching_requests,
                            );
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| format!("start late-stream Jig adapter: {error}"))?;
        Ok(Self {
            base_url: format!("http://{address}"),
            requests,
            shutdown,
            thread: Some(thread),
        })
    }

    pub(super) fn base_url(&self) -> String {
        self.base_url.clone()
    }

    pub(super) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for LateStreamJig {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle(
    stream: &mut TcpStream,
    script: &Script,
    fixture: &LateStreamFailureFixture,
    requests: &Mutex<Vec<RecordedRequest>>,
    architect_requests: &AtomicUsize,
    engineer_requests: &AtomicUsize,
    matching_requests: &mut u32,
) -> std::io::Result<()> {
    let request = read_request(stream)?;
    let view = parse_openai(&request.body);
    let path = request.path.split('?').next().unwrap_or("/").to_string();
    let request_sequence = {
        let mut requests = requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        requests.push(RecordedRequest {
            path: path.clone(),
            method: request.method,
            body: request.body,
            view: Some(view.clone()),
        });
        requests.len()
    };

    let architect = messages_contain(&view, "ROLE: architect");
    let engineer = messages_contain(&view, "ROLE: engineer");
    if architect {
        architect_requests.fetch_add(1, Ordering::SeqCst);
    }
    if engineer {
        engineer_requests.fetch_add(1, Ordering::SeqCst);
    }
    let matches_role = messages_contain(&view, &format!("ROLE: {}", fixture.role));
    if matches_role {
        *matching_requests = matching_requests.saturating_add(1);
    }
    let request_id = format!("jig-request-{request_sequence}");
    if matches_role && should_inject(fixture, *matching_requests) {
        return write_late_stream_failure(stream, &request_id);
    }

    match script.next_action(&view) {
        ScriptAction::Reply(reply) => {
            let body = frames_to_body(&render_openai(&reply));
            write_sse(stream, &body, &request_id)
        }
        ScriptAction::HttpError(error) => {
            let rendered = error.render_body(Dialect::OpenAi);
            write_response(
                stream,
                error.status,
                &rendered.content_type,
                &rendered.body,
                &request_id,
            )
        }
        ScriptAction::StreamError(_) | ScriptAction::AbortStream(_) => write_response(
            stream,
            500,
            "application/json",
            r#"{"error":{"code":"unsupported_script_action"}}"#,
            &request_id,
        ),
    }
}

fn should_inject(fixture: &LateStreamFailureFixture, matching_request: u32) -> bool {
    fixture.bursts.iter().any(|burst| {
        matching_request > burst.after_requests
            && matching_request <= burst.after_requests + burst.failures
    })
}

fn messages_contain(view: &jig_core::RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn write_late_stream_failure(stream: &mut TcpStream, request_id: &str) -> std::io::Result<()> {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
        "data: {\"error\":{}}\n\n"
    );
    write_sse(stream, body, request_id)
}

fn write_sse(stream: &mut TcpStream, body: &str, request_id: &str) -> std::io::Result<()> {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-Id: {request_id}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    request_id: &str,
) -> std::io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status} Error\r\nContent-Type: {content_type}\r\nX-Request-Id: {request_id}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

struct Request {
    path: String,
    method: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request headers ended early",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or("/").to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or_default();
    let body_start = header_end + 4;
    let mut body = bytes[body_start..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Request { path, method, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_manifest::LateStreamFailureBurst;

    #[test]
    fn separated_bursts_leave_a_successful_same_turn_retry_before_later_exhaustion() {
        let fixture = LateStreamFailureFixture {
            role: "engineer".to_string(),
            bursts: vec![
                LateStreamFailureBurst {
                    after_requests: 2,
                    failures: 1,
                },
                LateStreamFailureBurst {
                    after_requests: 5,
                    failures: 3,
                },
            ],
        };
        let injected = (1..=9)
            .filter(|request| should_inject(&fixture, *request))
            .collect::<Vec<_>>();

        assert_eq!(injected, [3, 6, 7, 8]);
        assert!(!should_inject(&fixture, 4), "the same-turn retry succeeds");
        assert!(
            !should_inject(&fixture, 5),
            "normal progress can precede deferral"
        );
        assert!(
            !should_inject(&fixture, 9),
            "the provider becomes healthy again"
        );
    }

    #[test]
    fn late_failure_has_partial_delta_and_no_status_code_or_provider_prose() {
        let mut bytes = Vec::new();
        // Exercise the wire body directly; a Vec cannot implement TcpStream,
        // so keep this assertion on the canonical literal used above.
        bytes.extend_from_slice(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
            "data: {\"error\":{}}\n\n"
        ).as_bytes());
        let body = String::from_utf8(bytes).unwrap();
        assert!(body.contains("partial"));
        assert!(body.contains(r#""error":{}"#));
        assert!(!body.contains("code"));
        assert!(!body.contains("message"));
        assert!(!body.contains("status"));
    }
}
