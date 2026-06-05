//! Forgejo webhook receiver for host-local worker wakes.

use crate::trigger_args::TriggerArgs;
use crate::wake::{send_wake_with_hint, WakeError};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use temper_forge::{ItemNumber, RepositoryPath};
use temper_runner::{ChangeHint, ChangeKind};

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct WakeDeliveryReport {
    targets: u64,
    sent: u64,
    failed: u64,
    failures: Vec<WakeDeliveryFailure>,
}

impl WakeDeliveryReport {
    fn outcome(&self) -> &'static str {
        if self.targets == 0 {
            "no_sockets"
        } else if self.sent > 0 {
            "sent"
        } else {
            "all_failed"
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WakeDeliveryFailure {
    target: String,
    path: String,
    error: String,
}

pub fn run(args: &TriggerArgs) -> Result<(), TriggerError> {
    let listener = TcpListener::bind(args.bind)?;
    run_with_listener(args, listener)
}

/// Runs the trigger using an already-bound listener.
///
/// Tests use this to allocate `127.0.0.1:0` and keep the listener bound through
/// the serving loop, avoiding the free-port allocation gap. Production still
/// calls [`run`], which binds from [`TriggerArgs`].
pub fn run_with_listener(args: &TriggerArgs, listener: TcpListener) -> Result<(), TriggerError> {
    let webhook_secret = read_secret(&args.webhook_secret_file)?;
    let wake_secret = args
        .wake_secret_file
        .as_ref()
        .map(|path| read_secret(path))
        .transpose()?;
    let addr = listener.local_addr()?;
    eprintln!("temper-trigger-forgejo: listening on {addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) =
                    handle_connection(stream, args, &webhook_secret, wake_secret.as_deref())
                {
                    eprintln!("temper-trigger-forgejo: {error}");
                }
            }
            Err(error) => eprintln!("temper-trigger-forgejo: accept failed: {error}"),
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
    let event = header(&request.headers, "x-forgejo-event")
        .or_else(|| header(&request.headers, "x-gitea-event"))
        .unwrap_or("unknown");
    match accept_webhook(request, webhook_secret) {
        Ok(hint) => {
            let delivery = deliver_wakes(args, wake_secret, &hint);
            eprintln!(
                "temper-trigger-forgejo: webhook accepted event={event} kind={:?} repo={}/{} item={:?} wake_outcome={} targets={} sent={} failed={}",
                hint.kind,
                hint.repo.owner,
                hint.repo.name,
                hint.item.map(ItemNumber::get),
                delivery.outcome(),
                delivery.targets,
                delivery.sent,
                delivery.failed
            );
            HttpResponse::new(202, "accepted\n")
        }
        Err(error) => {
            eprintln!("temper-trigger-forgejo: webhook rejected reason={error}");
            HttpResponse::new(401, "rejected\n")
        }
    }
}

fn deliver_wakes(
    args: &TriggerArgs,
    wake_secret: Option<&str>,
    hint: &ChangeHint,
) -> WakeDeliveryReport {
    let sockets = wake_sockets(args);
    let mut report = WakeDeliveryReport {
        targets: sockets.len() as u64,
        ..WakeDeliveryReport::default()
    };
    if sockets.is_empty() {
        eprintln!(
            "temper-trigger-forgejo: wake_delivery outcome=no_sockets targets=0 sent=0 failed=0"
        );
        return report;
    }
    for (target, path) in sockets {
        match send_wake_with_hint(&path, wake_secret, hint) {
            Ok(()) => report.sent = report.sent.saturating_add(1),
            Err(error) => {
                report.failed = report.failed.saturating_add(1);
                let path_text = path.display().to_string();
                let error_text = error.to_string();
                eprintln!(
                    "temper-trigger-forgejo: wake_send_failed target={target} path={path_text} error={error_text}"
                );
                report.failures.push(WakeDeliveryFailure {
                    target,
                    path: path_text,
                    error: error_text,
                });
            }
        }
    }
    eprintln!(
        "temper-trigger-forgejo: wake_delivery outcome={} targets={} sent={} failed={}",
        report.outcome(),
        report.targets,
        report.sent,
        report.failed
    );
    report
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
        | "pull_request_approved"
        | "pull_request_rejected"
        | "review" => ChangeKind::Review,
        "push" => ChangeKind::Push,
        "status" | "check_run" | "workflow_run" | "workflow_job" | "action_run_failure"
        | "action_run_recover" | "action_run_success" => ChangeKind::Ci,
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
    use crate::trigger_args::NamedSocket;
    use std::io::{Read, Write};
    use std::path::PathBuf;

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
    fn workflow_events_are_ci_hints() {
        let body = br#"{"repository":{"owner":{"login":"acme"},"name":"service"}}"#;
        for event in [
            "status",
            "workflow_job",
            "workflow_run",
            "action_run_success",
        ] {
            let hint = accept_webhook(&request(body, "secret", event), "secret")
                .expect("CI events wake workers");
            assert_eq!(hint.kind, ChangeKind::Ci, "event {event}");
        }
    }

    #[test]
    fn forgejo_pull_request_review_events_are_review_hints() {
        let body = br#"{"repository":{"full_name":"acme/service"},"pull_request":{"number":7}}"#;
        for event in ["pull_request_approved", "pull_request_rejected"] {
            let hint = accept_webhook(&request(body, "secret", event), "secret")
                .expect("review events wake workers");
            assert_eq!(hint.kind, ChangeKind::Review, "event {event}");
            assert_eq!(hint.item, Some(ItemNumber::new(7)));
        }
    }

    #[test]
    fn unknown_event_still_yields_hint() {
        let body = br#"{"repository":{"owner":{"login":"acme"},"name":"service"}}"#;
        let hint = accept_webhook(&request(body, "secret", "mystery"), "secret")
            .expect("unknown events wake broadly");
        assert_eq!(hint.kind, ChangeKind::Unknown);
    }

    fn trigger_args_with_dir(dir: PathBuf) -> TriggerArgs {
        TriggerArgs {
            bind: "127.0.0.1:0".parse().expect("valid bind"),
            webhook_secret_file: PathBuf::from("webhook-secret"),
            wake_secret_file: None,
            wake_dir: Some(dir),
            wake_sockets: Vec::new(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "temper-production-trigger-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanos")
        ));
        std::fs::create_dir_all(&dir).expect("temp dir is created");
        dir
    }

    #[test]
    fn run_with_listener_keeps_allocated_addr_reachable() {
        let dir = temp_dir("bound-listener");
        let webhook_secret = dir.join("webhook-secret");
        let wake_secret = dir.join("wake-secret");
        let wake_dir = dir.join("wake");
        std::fs::write(&webhook_secret, "webhook-secret\n").expect("webhook secret writes");
        std::fs::write(&wake_secret, "wake-secret\n").expect("wake secret writes");
        std::fs::create_dir_all(&wake_dir).expect("wake dir creates");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let addr = listener.local_addr().expect("listener has addr");
        let args = TriggerArgs {
            bind: addr,
            webhook_secret_file: webhook_secret,
            wake_secret_file: Some(wake_secret),
            wake_dir: Some(wake_dir),
            wake_sockets: Vec::new(),
        };
        std::thread::spawn(move || {
            if let Err(error) = run_with_listener(&args, listener) {
                eprintln!("trigger listener test exited: {error}");
            }
        });

        let mut stream = TcpStream::connect(addr).expect("already-bound addr is reachable");
        stream
            .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\ncontent-length: 0\r\n\r\n")
            .expect("request writes");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("response reads");
        assert!(
            response.starts_with("HTTP/1.1 401 Unauthorized"),
            "unexpected response: {response:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn wake_dir_discovers_socket_created_after_startup() {
        use std::os::unix::net::UnixDatagram;
        use std::time::Duration as StdDuration;

        let dir = temp_dir("late-socket");
        let args = trigger_args_with_dir(dir.clone());
        assert!(wake_sockets(&args).is_empty());
        let socket_path = dir.join("engineer.sock");
        let socket = UnixDatagram::bind(&socket_path).expect("socket binds");
        socket
            .set_read_timeout(Some(StdDuration::from_secs(1)))
            .expect("timeout is set");
        let body = br#"{"repository":{"full_name":"acme/service"},"issue":{"number":7}}"#;

        let response = handle_request(&request(body, "secret", "issues"), &args, "secret", None);

        assert_eq!(response.status, 202);
        let mut buf = [0_u8; 512];
        let size = socket.recv(&mut buf).expect("wake is received");
        let payload = std::str::from_utf8(&buf[..size]).expect("wake payload is utf8");
        assert!(payload.starts_with("wake\n"));
        assert!(payload.contains("\"owner\":\"acme\""));
        assert!(payload.contains("\"name\":\"service\""));
        drop(socket);
        let _ = std::fs::remove_file(socket_path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn missing_socket_paths_are_reported_but_webhook_is_accepted() {
        let dir = temp_dir("missing-socket");
        let missing = dir.join("missing.sock");
        let args = TriggerArgs {
            bind: "127.0.0.1:0".parse().expect("valid bind"),
            webhook_secret_file: PathBuf::from("webhook-secret"),
            wake_secret_file: None,
            wake_dir: None,
            wake_sockets: vec![NamedSocket {
                name: "missing".into(),
                path: missing.clone(),
            }],
        };
        let body = br#"{"repository":{"full_name":"acme/service"},"issue":{"number":7}}"#;

        let hint = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
        let report = deliver_wakes(&args, None, &hint);
        let response = handle_request(&request(body, "secret", "issues"), &args, "secret", None);

        assert_eq!(response.status, 202);
        assert_eq!(report.targets, 1);
        assert_eq!(report.sent, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(report.outcome(), "all_failed");
        assert_eq!(report.failures[0].target, "missing");
        assert_eq!(report.failures[0].path, missing.display().to_string());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_sockets_found_is_a_distinct_delivery_outcome() {
        let dir = temp_dir("no-sockets");
        let args = trigger_args_with_dir(dir.clone());

        let hint = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
        let report = deliver_wakes(&args, None, &hint);

        assert_eq!(report.targets, 0);
        assert_eq!(report.sent, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.outcome(), "no_sockets");
        let _ = std::fs::remove_dir_all(dir);
    }
}
