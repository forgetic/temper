use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use temper_forge::ItemNumber;
use temper_testing::forgejo_server::{ForgejoServer, Provisioned};

/// Data the jig fake uses to verify the early PR before it asks the agent to
/// make product edits.
#[derive(Clone)]
pub(super) struct PlanGate {
    base_url: String,
    admin_token: String,
    owner: String,
    name: String,
    engineer_user: String,
    issue: Arc<Mutex<Option<ItemNumber>>>,
}

impl PlanGate {
    pub(super) fn new(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        engineer: &temper_testing::forgejo_server::RoleIdentity,
        issue: Arc<Mutex<Option<ItemNumber>>>,
    ) -> Self {
        Self {
            base_url: server.base_url().to_string(),
            admin_token: provisioned.admin_token.clone(),
            owner: provisioned.owner.clone(),
            name: provisioned.name.clone(),
            engineer_user: engineer.user.clone(),
            issue,
        }
    }

    pub(super) fn wait_for_plan_pr(&self, timeout: Duration) {
        let issue = self.issue_number();
        let deadline = Instant::now() + timeout;
        loop {
            match find_plan_first_pr_blocking(self, issue) {
                Ok(_) => return,
                Err(error) if Instant::now() < deadline => {
                    eprintln!("run_forgejo_e2e waiting for plan-first PR: {error}");
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(error) => panic!(
                    "plan-first PR was not visible before product edits within {timeout:?}: {error} (repo {}/{})",
                    self.owner, self.name
                ),
            }
        }
    }

    fn issue_number(&self) -> ItemNumber {
        let timeout = Duration::from_secs(30);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(issue) = self.issue.lock().expect("seeded issue lock").as_ref() {
                return *issue;
            }
            assert!(
                Instant::now() < deadline,
                "seeded issue number was not published to the fake LLM within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Find the early, plan-first implementation PR correlated to `issue`.
fn find_plan_first_pr_blocking(gate: &PlanGate, issue: ItemNumber) -> Result<(), String> {
    let prs = list_pull_requests_blocking(gate)?;
    let prs: Vec<&Value> = prs
        .iter()
        .filter(|pr| {
            json_labels(pr)
                .iter()
                .any(|label| label == "implementation")
        })
        .collect();
    let pr = match prs.len() {
        0 => return Err("no implementation PR yet".to_string()),
        1 => prs[0],
        n => return Err(format!("expected one implementation PR, found {n}")),
    };

    let number = pr.get("number").and_then(Value::as_u64).unwrap_or_default();
    let labels = json_labels(pr);
    let body = pr.get("body").and_then(Value::as_str).unwrap_or_default();
    let author = pr
        .get("user")
        .and_then(|user| user.get("login"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    verify_correlated_pr_body_author(body, issue, author, &gate.engineer_user)
        .map_err(|error| format!("PR #{number}: {error}"))?;
    if !labels.iter().any(|label| label == "in-progress") {
        return Err(format!(
            "PR #{number} is not still in-progress before product edits: {labels:?}",
        ));
    }
    if labels.iter().any(|label| label == "needs-reviewer") {
        return Err(format!("PR #{number} entered review too early: {labels:?}",));
    }
    if !body.contains("Summary: Plan-first delivery proof") {
        return Err("early PR missing published plan summary".to_string());
    }
    if !body.contains("- [ ] Create delivery file\n- [ ] Verify delivery") {
        return Err(format!(
            "early PR did not have an unchecked two-phase plan:\n{body}",
        ));
    }
    Ok(())
}

fn list_pull_requests_blocking(gate: &PlanGate) -> Result<Vec<Value>, String> {
    let (host, port) = parse_http_host_port(&gate.base_url)?;
    let mut addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("resolve {host}:{port} failed: {error}"))?;
    let addr = addrs
        .next()
        .ok_or_else(|| format!("resolve {host}:{port} returned no addresses"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|error| format!("connect {addr} failed: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("set read timeout failed: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("set write timeout failed: {error}"))?;
    let path = format!("/api/v1/repos/{}/{}/pulls?state=all", gate.owner, gate.name);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: token {token}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        token = gate.admin_token,
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write Forgejo PR-list request failed: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("read Forgejo PR-list response failed: {error}"))?;
    let response_text = String::from_utf8_lossy(&response);
    let header_end = response_text
        .find("\r\n\r\n")
        .ok_or_else(|| format!("malformed HTTP response from Forgejo: {response_text}"))?;
    let headers = &response_text[..header_end];
    let body_start = header_end + "\r\n\r\n".len();
    let body_bytes = &response[body_start..];
    if !headers.starts_with("HTTP/1.1 200") && !headers.starts_with("HTTP/1.0 200") {
        let body = String::from_utf8_lossy(body_bytes);
        return Err(format!(
            "Forgejo PR-list returned non-200 response:\n{headers}\n{body}"
        ));
    }
    let body = if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_http_chunks(body_bytes)?
    } else {
        body_bytes.to_vec()
    };
    let body = String::from_utf8(body)
        .map_err(|error| format!("Forgejo PR-list response was not UTF-8: {error}"))?;
    serde_json::from_str::<Vec<Value>>(&body)
        .map_err(|error| format!("parse Forgejo PR-list JSON failed: {error}; body={body}"))
}

fn decode_http_chunks(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut cursor = 0usize;
    let mut decoded = Vec::new();
    loop {
        let size_end = find_crlf(body, cursor)
            .ok_or_else(|| "chunked response missing chunk-size terminator".to_string())?;
        let size_line = std::str::from_utf8(&body[cursor..size_end])
            .map_err(|error| format!("chunk size was not UTF-8: {error}"))?;
        let size_token = size_line.split(';').next().unwrap_or(size_line).trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|error| format!("invalid chunk size {size_token:?}: {error}"))?;
        cursor = size_end + 2;
        if size == 0 {
            break;
        }
        let chunk_end = cursor
            .checked_add(size)
            .ok_or_else(|| "chunk size overflow".to_string())?;
        if chunk_end + 2 > body.len() {
            return Err("chunked response ended inside a chunk".to_string());
        }
        decoded.extend_from_slice(&body[cursor..chunk_end]);
        if &body[chunk_end..chunk_end + 2] != b"\r\n" {
            return Err("chunked response missing post-chunk CRLF".to_string());
        }
        cursor = chunk_end + 2;
    }
    Ok(decoded)
}

fn find_crlf(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

fn parse_http_host_port(base_url: &str) -> Result<(String, u16), String> {
    let authority = base_url
        .strip_prefix("http://")
        .ok_or_else(|| format!("test helper only supports http:// URLs, got {base_url}"))?
        .trim_end_matches('/');
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| format!("base URL has no explicit port: {base_url}"))?;
    let port = port
        .parse::<u16>()
        .map_err(|error| format!("invalid port in {base_url}: {error}"))?;
    Ok((host.to_string(), port))
}

fn json_labels(pr: &Value) -> Vec<String> {
    pr.get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|label| label.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn verify_correlated_pr_body_author(
    body: &str,
    issue: ItemNumber,
    author: &str,
    engineer_user: &str,
) -> Result<(), String> {
    let metadata = temper_workflow::parse_metadata_block(body)
        .map_err(|error| format!("PR metadata malformed: {error}"))?
        .ok_or("PR missing workflow metadata")?;
    let expected = format!("pr-for-code-{issue}");
    if metadata.correlation_key.as_deref() != Some(expected.as_str()) {
        return Err(format!(
            "PR correlation key {:?} != {expected:?}",
            metadata.correlation_key
        ));
    }
    if author != engineer_user {
        return Err(format!(
            "PR authored by {author:?}, not the engineer identity {engineer_user:?}"
        ));
    }
    Ok(())
}
