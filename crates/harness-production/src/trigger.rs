//! Forgejo webhook receiver for host-local worker wakes.

use crate::trigger_args::TriggerArgs;
use crate::wake::{send_wake, WakeError};
use harness_forge::{ItemNumber, RepositoryPath};
use harness_runner::{ChangeHint, ChangeKind};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

#[derive(Debug)]
pub enum TriggerError {
    Io(std::io::Error),
    Wake(WakeError),
    BadRequest(String),
}

impl fmt::Display for TriggerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerError::Io(error) => write!(formatter, "trigger I/O failed: {error}"),
            TriggerError::Wake(error) => write!(formatter, "worker wake failed: {error}"),
            TriggerError::BadRequest(message) => {
                write!(formatter, "bad webhook request: {message}")
            }
        }
    }
}

impl std::error::Error for TriggerError {}

impl From<std::io::Error> for TriggerError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WakeError> for TriggerError {
    fn from(error: WakeError) -> Self {
        Self::Wake(error)
    }
}

pub fn run(args: &TriggerArgs) -> Result<(), TriggerError> {
    let webhook_secret = read_secret(&args.webhook_secret_file)?;
    let wake_secret = args
        .wake_secret_file
        .as_ref()
        .map(|path| read_secret(path))
        .transpose()?;
    let listener = TcpListener::bind(args.bind)?;
    eprintln!("harness-trigger-forgejo: listening on {}", args.bind);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) =
                    handle_connection(stream, args, &webhook_secret, wake_secret.as_deref())
                {
                    eprintln!("harness-trigger-forgejo: {error}");
                }
            }
            Err(error) => eprintln!("harness-trigger-forgejo: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    args: &TriggerArgs,
    webhook_secret: &str,
    wake_secret: Option<&str>,
) -> Result<(), TriggerError> {
    let request = HttpRequest::read_from(&mut stream)?;
    let response = handle_request(&request, args, webhook_secret, wake_secret);
    stream.write_all(response.to_http().as_bytes())?;
    Ok(())
}

fn handle_request(
    request: &HttpRequest,
    args: &TriggerArgs,
    webhook_secret: &str,
    wake_secret: Option<&str>,
) -> HttpResponse {
    match accept_webhook(request, webhook_secret) {
        Ok(hint) => {
            let mut sent = 0_u64;
            let mut failed = 0_u64;
            for socket in wake_sockets(args) {
                match send_wake(&socket.1, wake_secret) {
                    Ok(()) => sent += 1,
                    Err(error) => {
                        failed += 1;
                        eprintln!(
                            "harness-trigger-forgejo: wake '{}' failed: {error}",
                            socket.0
                        );
                    }
                }
            }
            eprintln!(
                "harness-trigger-forgejo: accepted {:?} for {}/{} item {:?}; wake sent={sent} failed={failed}",
                hint.kind,
                hint.repo.owner,
                hint.repo.name,
                hint.item.map(ItemNumber::get)
            );
            HttpResponse::new(202, "accepted\n")
        }
        Err(error) => {
            eprintln!("harness-trigger-forgejo: rejected webhook: {error}");
            HttpResponse::new(401, "rejected\n")
        }
    }
}

fn wake_sockets(args: &TriggerArgs) -> Vec<(String, std::path::PathBuf)> {
    let mut sockets: Vec<(String, std::path::PathBuf)> = args
        .wake_sockets
        .iter()
        .map(|socket| (socket.name.clone(), socket.path.clone()))
        .collect();
    if let Some(dir) = &args.wake_dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("sock") {
                    let name = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("worker")
                        .to_string();
                    sockets.push((name, path));
                }
            }
        }
    }
    sockets
}

fn accept_webhook(request: &HttpRequest, webhook_secret: &str) -> Result<ChangeHint, TriggerError> {
    if request.method != "POST" || request.path != "/forgejo/webhook" {
        return Err(TriggerError::BadRequest(
            "expected POST /forgejo/webhook".into(),
        ));
    }
    verify_signature(&request.headers, &request.body, webhook_secret)?;
    let event = header(&request.headers, "x-forgejo-event")
        .or_else(|| header(&request.headers, "x-gitea-event"))
        .unwrap_or("unknown");
    parse_hint(&request.body, event)
}

fn parse_hint(body: &[u8], event: &str) -> Result<ChangeHint, TriggerError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| TriggerError::BadRequest(format!("invalid JSON payload: {error}")))?;
    let repo = parse_repo(&value)?;
    let item = value
        .pointer("/pull_request/number")
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/issue/number").and_then(Value::as_u64))
        .map(ItemNumber::new);
    let kind = match event {
        "issues" | "issue" => ChangeKind::Issue,
        "pull_request" | "pull_request_sync" => ChangeKind::PullRequest,
        "issue_comment" | "pull_request_comment" | "comment" => ChangeKind::Comment,
        "pull_request_review"
        | "pull_request_review_approved"
        | "pull_request_review_rejected"
        | "pull_request_review_comment"
        | "review" => ChangeKind::Review,
        "push" => ChangeKind::Push,
        "status" | "check_run" | "workflow_run" | "workflow_job" => ChangeKind::Ci,
        _ => ChangeKind::Unknown,
    };
    Ok(ChangeHint { repo, item, kind })
}

fn parse_repo(value: &Value) -> Result<RepositoryPath, TriggerError> {
    if let Some(full) = value
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
    {
        if let Some((owner, name)) = full.split_once('/') {
            return Ok(RepositoryPath::new(owner, name));
        }
    }
    let owner = value
        .pointer("/repository/owner/login")
        .or_else(|| value.pointer("/repository/owner/username"))
        .and_then(Value::as_str);
    let name = value.pointer("/repository/name").and_then(Value::as_str);
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(RepositoryPath::new(owner, name)),
        _ => Err(TriggerError::BadRequest(
            "payload has no repository owner/name".into(),
        )),
    }
}

fn verify_signature(
    headers: &BTreeMap<String, String>,
    body: &[u8],
    secret: &str,
) -> Result<(), TriggerError> {
    let supplied = header(headers, "x-forgejo-signature")
        .or_else(|| header(headers, "x-gitea-signature"))
        .or_else(|| header(headers, "x-hub-signature-256"))
        .ok_or_else(|| TriggerError::BadRequest("missing webhook signature".into()))?;
    let supplied = supplied.strip_prefix("sha256=").unwrap_or(supplied);
    let expected = signature_hex(secret.as_bytes(), body);
    let supplied_bytes = decode_hex(supplied)
        .ok_or_else(|| TriggerError::BadRequest("signature is not hex".into()))?;
    let expected_bytes = decode_hex(&expected).expect("hex-encoded HMAC is valid");
    if supplied_bytes.len() == expected_bytes.len()
        && constant_time_eq(&supplied_bytes, &expected_bytes)
    {
        Ok(())
    } else {
        Err(TriggerError::BadRequest("invalid webhook signature".into()))
    }
}

fn signature_hex(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    encode_hex(&mac.finalize().into_bytes())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() % 2 != 0 {
        return None;
    }
    raw.as_bytes()
        .chunks(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let diff = left
        .iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right));
    diff == 0
}

fn read_secret(path: &std::path::Path) -> Result<String, std::io::Error> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(name).map(String::as_str)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn read_from(stream: &mut TcpStream) -> Result<Self, TriggerError> {
        let mut raw = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let n = stream.read(&mut buf)?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if let Some(header_end) = find_header_end(&raw) {
                let headers = parse_headers(&raw[..header_end])?;
                let body_start = header_end + 4;
                let content_len = header(&headers.2, "content-length")
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(0);
                while raw.len() < body_start + content_len {
                    let n = stream.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                return Ok(Self {
                    method: headers.0,
                    path: headers.1,
                    headers: headers.2,
                    body: raw[body_start..body_start + content_len].to_vec(),
                });
            }
        }
        Err(TriggerError::BadRequest("incomplete HTTP request".into()))
    }
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_headers(raw: &[u8]) -> Result<(String, String, BTreeMap<String, String>), TriggerError> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| TriggerError::BadRequest("HTTP headers are not UTF-8".into()))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| TriggerError::BadRequest("missing request line".into()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Ok((method, path, headers))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    fn to_http(&self) -> String {
        let reason = match self.status {
            202 => "Accepted",
            401 => "Unauthorized",
            _ => "OK",
        };
        format!(
            "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            self.status,
            reason,
            self.body.len(),
            self.body
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(body: &[u8], secret: &str, event: &str) -> HttpRequest {
        let mut headers = BTreeMap::new();
        headers.insert("x-gitea-event".into(), event.into());
        headers.insert(
            "x-gitea-signature".into(),
            signature_hex(secret.as_bytes(), body),
        );
        HttpRequest {
            method: "POST".into(),
            path: "/forgejo/webhook".into(),
            headers,
            body: body.to_vec(),
        }
    }

    #[test]
    fn verifies_signature_and_parses_pull_request_hint() {
        let body = br#"{"repository":{"full_name":"acme/service"},"pull_request":{"number":42}}"#;
        let hint = accept_webhook(&request(body, "secret", "pull_request"), "secret")
            .expect("valid webhook");
        assert_eq!(hint.repo, RepositoryPath::new("acme", "service"));
        assert_eq!(hint.item, Some(ItemNumber::new(42)));
        assert_eq!(hint.kind, ChangeKind::PullRequest);
    }

    #[test]
    fn rejects_bad_signature() {
        let body = br#"{"repository":{"full_name":"acme/service"}}"#;
        let mut request = request(body, "secret", "issues");
        request
            .headers
            .insert("x-gitea-signature".into(), signature_hex(b"wrong", body));
        let error = accept_webhook(&request, "secret").unwrap_err();
        assert!(error.to_string().contains("invalid webhook signature"));
    }

    #[test]
    fn unknown_event_still_yields_hint() {
        let body = br#"{"repository":{"owner":{"login":"acme"},"name":"service"}}"#;
        let hint = accept_webhook(&request(body, "secret", "mystery"), "secret")
            .expect("unknown events wake broadly");
        assert_eq!(hint.kind, ChangeKind::Unknown);
    }
}
