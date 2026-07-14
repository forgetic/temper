// SPDX-License-Identifier: MPL-2.0

//! Blocking, server-side client for the engine trace query API.
//!
//! The bearer token is inserted only into this process-to-process request. It is
//! never returned by a trait method, placed in a board DTO, or sent to browser
//! JavaScript.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use temper_web::config::TraceReadToken;
use temper_web::trace::{
    TraceApiClient, TraceClientError, TraceEventPage, TraceRunPage, TraceRunSummary,
};

pub struct HttpTraceClient {
    base_url: String,
    read_token: TraceReadToken,
    timeout: Duration,
}

/// A trace page is finite and engine-bounded. Keep the blocking proxy bounded
/// as well if a malformed or compromised upstream ignores those limits.
const MAX_TRACE_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

impl HttpTraceClient {
    pub fn new(base_url: impl Into<String>, read_token: TraceReadToken) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            read_token,
            timeout: Duration::from_secs(5),
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, TraceClientError> {
        let body = self.get(path)?;
        serde_json::from_slice(&body)
            .map_err(|_| TraceClientError::new("engine trace API returned malformed JSON"))
    }

    fn get(&self, path: &str) -> Result<Vec<u8>, TraceClientError> {
        let (host, port) = parse_host_port(&self.base_url)?;
        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|_| TraceClientError::new("cannot connect to engine trace API"))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|_| TraceClientError::new("cannot configure engine trace connection"))?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|_| TraceClientError::new("cannot configure engine trace connection"))?;
        let request = format!(
            "GET {path} HTTP/1.0\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
            self.read_token.expose()
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|_| TraceClientError::new("cannot write engine trace request"))?;

        let mut raw = Vec::new();
        stream
            .take(MAX_TRACE_RESPONSE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| TraceClientError::new("cannot read engine trace response"))?;
        if raw.len() as u64 > MAX_TRACE_RESPONSE_BYTES {
            return Err(TraceClientError::new(
                "engine trace API response exceeded the web proxy limit",
            ));
        }
        let split = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| TraceClientError::new("engine trace API returned malformed HTTP"))?;
        let head = std::str::from_utf8(&raw[..split])
            .map_err(|_| TraceClientError::new("engine trace API returned malformed HTTP"))?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse::<u16>().ok())
            .ok_or_else(|| TraceClientError::new("engine trace API returned malformed HTTP"))?;
        if status != 200 {
            return Err(TraceClientError::new(format!(
                "engine trace API returned HTTP {status}"
            )));
        }
        Ok(raw[split + 4..].to_vec())
    }
}

impl TraceApiClient for HttpTraceClient {
    fn list_runs(
        &self,
        artifact_ref: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TraceRunPage, TraceClientError> {
        let mut query = vec![format!("limit={limit}")];
        if let Some(artifact_ref) = artifact_ref {
            query.push(format!("artifact_ref={}", encode_component(artifact_ref)));
        }
        if let Some(cursor) = cursor {
            query.push(format!("cursor={}", encode_component(cursor)));
        }
        self.get_json(&format!("/v1/agent-runs?{}", query.join("&")))
    }

    fn run_summary(&self, run_id: &str) -> Result<TraceRunSummary, TraceClientError> {
        self.get_json(&format!("/v1/agent-runs/{}", encode_component(run_id)))
    }

    fn events(
        &self,
        run_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<TraceEventPage, TraceClientError> {
        self.get_json(&format!(
            "/v1/agent-runs/{}/events?after_seq={after_seq}&limit={limit}",
            encode_component(run_id)
        ))
    }
}

fn parse_host_port(base_url: &str) -> Result<(String, u16), TraceClientError> {
    if base_url.starts_with("https://") {
        return Err(TraceClientError::new(
            "HTTPS trace URLs are not supported by the blocking local client",
        ));
    }
    let without_scheme = base_url.strip_prefix("http://").unwrap_or(base_url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    if authority.is_empty() {
        return Err(TraceClientError::new("engine trace URL has no host"));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Ok((
            host.to_string(),
            port.parse()
                .map_err(|_| TraceClientError::new("engine trace URL has an invalid port"))?,
        )),
        Some(_) => Err(TraceClientError::new("engine trace URL has no host")),
        None => Ok((authority.to_string(), 80)),
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn one_response(body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake engine");
        let address = listener.local_addr().expect("address");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let count = stream.read(&mut buffer).expect("read request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            tx.send(String::from_utf8(bytes).expect("request UTF-8"))
                .expect("record request");
            let response = format!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("reply");
        });
        (format!("http://{address}"), rx)
    }

    #[test]
    fn forwards_token_only_to_engine_and_encodes_filters() {
        let (url, request) = one_response(r#"{"runs":[]}"#);
        let client = HttpTraceClient::new(url, TraceReadToken::new("top-secret").expect("token"));
        let page = client
            .list_runs(Some("ai/temper#312"), None, 50)
            .expect("list response");
        assert!(page.runs.is_empty());
        let request = request.recv().expect("captured");
        assert!(
            request
                .starts_with("GET /v1/agent-runs?limit=50&artifact_ref=ai%2Ftemper%23312 HTTP/1.0")
        );
        assert!(request.contains("Authorization: Bearer top-secret\r\n"));
    }

    #[test]
    fn event_reads_forward_the_resume_cursor() {
        let (url, request) =
            one_response(r#"{"run_id":"run-1","events":[],"next_after_seq":41,"has_more":false}"#);
        let client = HttpTraceClient::new(url, TraceReadToken::new("secret").expect("token"));
        let page = client.events("run-1", 41, 500).expect("event response");
        assert_eq!(page.next_after_seq, 41);
        assert!(
            request
                .recv()
                .expect("captured")
                .starts_with("GET /v1/agent-runs/run-1/events?after_seq=41&limit=500 HTTP/1.0")
        );
    }
}
