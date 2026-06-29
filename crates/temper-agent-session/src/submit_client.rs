// SPDX-License-Identifier: MPL-2.0

//! Local client for the worker-owned `submit_for_pr` side channel.
//!
//! The out-of-process agent receives the address as a non-secret CLI flag. Each
//! tool call opens one loopback TCP connection, writes a JSON
//! [`SubmitForPrRequest`], half-closes the write side, then reads the JSON
//! [`SubmitForPrResponse`]. Transport failures are returned to the model as a
//! host rejection so the live run remains intact.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use temper_agent::SubmitForPrHost;
use temper_protocol_agent::{SubmitForPrRequest, SubmitForPrResponse};

const SUBMIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SUBMIT_REQUEST_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn host_for_address(address: String) -> SubmitForPrHost {
    Arc::new(move |request, _context, _cwd| submit_once(&address, request))
}

fn submit_once(address: &str, request: SubmitForPrRequest) -> SubmitForPrResponse {
    match submit_once_result(address, &request) {
        Ok(response) => response,
        Err(error) => {
            SubmitForPrResponse::rejected(format!("submit_for_pr host channel failed: {error}"))
        }
    }
}

fn submit_once_result(
    address: &str,
    request: &SubmitForPrRequest,
) -> std::io::Result<SubmitForPrResponse> {
    let mut stream = connect_submit_channel(address)?;
    stream.set_write_timeout(Some(SUBMIT_REQUEST_WRITE_TIMEOUT))?;
    // The host owns pre-push execution and command timeouts; a client-side
    // response read timeout would turn a legitimate long-running gate into a
    // transport error before the worker can return structured gate data.
    stream.set_read_timeout(None)?;
    let bytes = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn connect_submit_channel(address: &str) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for socket_address in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket_address, SUBMIT_CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("submit_for_pr host address `{address}` resolved to no socket addresses"),
        )
    }))
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
        assert!(
            started.elapsed() >= response_delay,
            "client returned before the delayed worker response was written"
        );
        let expected = server.join().expect("submit response thread");
        assert_eq!(response, expected);
    }
}
