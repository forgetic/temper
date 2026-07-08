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
    let hint =
        accept_webhook(&request(body, "secret", "pull_request"), "secret").expect("valid webhook");
    assert_eq!(hint.repo, RepositoryPath::new("acme", "service"));
    assert_eq!(hint.item, Some(ItemNumber::new(42)));
    assert_eq!(hint.kind, ChangeKind::PullRequest);
}

#[test]
fn selected_forgejo_contract_accepts_forgejo_headers_and_sha256_prefix() {
    let body = br#"{"repository":{"full_name":"acme/service"},"issue":{"number":192}}"#;
    let mut headers = BTreeMap::new();
    headers.insert("x-forgejo-event".into(), "issues".into());
    headers.insert(
        "x-forgejo-signature".into(),
        format!("sha256={}", signature_hex(b"secret", body)),
    );
    let request = HttpRequest {
        method: "POST".into(),
        path: "/forgejo/webhook".into(),
        headers,
        body: body.to_vec(),
    };

    let hint = accept_webhook(&request, "secret").expect("Forgejo webhook contract is accepted");

    assert_eq!(hint.repo, RepositoryPath::new("acme", "service"));
    assert_eq!(hint.item, Some(ItemNumber::new(192)));
    assert_eq!(hint.kind, ChangeKind::Issue);
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
        "temper-trigger-forgejo-{name}-{}-{}",
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
