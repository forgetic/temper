// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use super::{
    CiFailureEvidenceFixture, EvidenceDocument, FailureStatement, MAX_REQUEST_BYTES, SERVICE_PATH,
    ServiceState, StoredRecord,
};
use crate::live_manifest::CiRequestEvidence;

pub(super) fn handle_connection(
    stream: &mut TcpStream,
    fixture: &CiFailureEvidenceFixture,
    read_token: &str,
    publish_token: &str,
    hmac_key: &str,
    state: &Arc<Mutex<ServiceState>>,
) -> (&'static str, String, u16, Vec<String>) {
    let request = match HttpRequestData::read(stream) {
        Ok(request) => request,
        Err(_) => {
            write_response(stream, 400, json!({ "error": "malformed" }).to_string());
            return ("INVALID", SERVICE_PATH.to_string(), 400, Vec::new());
        }
    };
    let (path, raw_query) = request
        .target
        .split_once('?')
        .map_or((request.target.as_str(), ""), |(path, query)| (path, query));
    let query = parse_query(raw_query);
    let query_keys = query.keys().cloned().collect::<Vec<_>>();
    let expected_token = match request.method.as_str() {
        "GET" => read_token,
        "POST" => publish_token,
        _ => "",
    };
    if path != SERVICE_PATH
        || expected_token.is_empty()
        || request.authorization() != Some(format!("Bearer {expected_token}").as_str())
    {
        write_response(stream, 401, json!({ "error": "unauthorized" }).to_string());
        return (
            method_label(&request.method),
            path.to_string(),
            401,
            query_keys,
        );
    }

    let (status, body) = if request.method == "POST" {
        match publish(&request.body, fixture, hmac_key, state) {
            Ok(()) => (201, json!({ "accepted": true }).to_string()),
            Err(error) => (400, json!({ "error": error }).to_string()),
        }
    } else {
        match (query.get("repository_id"), query.get("run_id")) {
            (Some(repository), Some(run)) => {
                let state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let records = state
                    .records
                    .iter()
                    .filter(|record| {
                        record.statement.repository_id == *repository
                            && record.statement.run_id == *run
                    })
                    .map(|record| record.signed.clone())
                    .collect::<Vec<_>>();
                (
                    200,
                    json!({ "schema_version": 1, "records": records }).to_string(),
                )
            }
            _ => (400, json!({ "error": "missing_query" }).to_string()),
        }
    };
    if status < 300 {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.requests.push(CiRequestEvidence {
            method: request.method.clone(),
            path: path.to_string(),
            query_keys: query_keys.clone(),
            authentication_present: true,
            authentication_scheme: Some("bearer".to_string()),
            accepts_json: request.accepts_json(),
        });
    }
    write_response(stream, status, body);
    (
        method_label(&request.method),
        path.to_string(),
        status,
        query_keys,
    )
}

fn publish(
    body: &[u8],
    fixture: &CiFailureEvidenceFixture,
    hmac_key: &str,
    state: &Arc<Mutex<ServiceState>>,
) -> Result<(), &'static str> {
    let document: EvidenceDocument =
        serde_json::from_slice(body).map_err(|_| "malformed_document")?;
    if document.schema_version != 1 || document.records.len() != 1 {
        return Err("unsupported_document");
    }
    let signed = document.records.into_iter().next().unwrap();
    verify_hmac(hmac_key, signed.statement.as_bytes(), &signed.hmac_sha256)
        .map_err(|_| "invalid_integrity")?;
    let statement: FailureStatement =
        serde_json::from_str(&signed.statement).map_err(|_| "malformed_statement")?;
    if statement.schema_version != 1
        || !matches!(statement.category.as_str(), "source" | "build" | "test")
        || statement.issuer_id != fixture.issuer
        || !fixture.protected_producers.contains(&statement.producer_id)
        || [
            statement.repository_id.as_str(),
            statement.pull_request_id.as_str(),
            statement.commit_sha.as_str(),
            statement.run_id.as_str(),
            statement.job_id.as_str(),
            statement.attempt.as_str(),
            statement.task_id.as_str(),
            statement.created_at.as_str(),
            statement.expires_at.as_str(),
        ]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err("invalid_statement");
    }
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.records.iter().any(|record| {
        record.statement.run_id == statement.run_id
            && record.statement.job_id == statement.job_id
            && record.statement.attempt == statement.attempt
            && record.statement.task_id == statement.task_id
    }) {
        return Err("duplicate_coordinate");
    }
    state.records.push(StoredRecord { signed, statement });
    Ok(())
}

fn verify_hmac(secret: &str, statement: &[u8], signature: &str) -> Result<(), ()> {
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let supplied = decode_hex(signature).ok_or(())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| ())?;
    mac.update(statement);
    mac.verify_slice(&supplied).map_err(|_| ())
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

struct HttpRequestData {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequestData {
    fn read(stream: &mut TcpStream) -> Result<Self, String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = stream
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Err("request ended before headers".to_string());
            }
            raw.extend_from_slice(&buffer[..count]);
            if raw.len() > MAX_REQUEST_BYTES {
                return Err("request too large".to_string());
            }
            if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers_text = std::str::from_utf8(&raw[..header_end])
            .map_err(|_| "headers are not UTF-8".to_string())?;
        let mut lines = headers_text.split("\r\n");
        let mut request_line = lines
            .next()
            .ok_or_else(|| "missing request line".to_string())?
            .split_whitespace();
        let method = request_line.next().unwrap_or_default().to_string();
        let target = request_line.next().unwrap_or_default().to_string();
        let mut headers = BTreeMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| "malformed header".to_string())?;
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
        let content_length = headers
            .get("content-length")
            .map(|value| value.parse::<usize>())
            .transpose()
            .map_err(|_| "invalid content length".to_string())?
            .unwrap_or(0);
        if header_end + content_length > MAX_REQUEST_BYTES {
            return Err("request too large".to_string());
        }
        while raw.len() < header_end + content_length {
            let count = stream
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Err("request ended before body".to_string());
            }
            raw.extend_from_slice(&buffer[..count]);
        }
        Ok(Self {
            method,
            target,
            headers,
            body: raw[header_end..header_end + content_length].to_vec(),
        })
    }

    fn authorization(&self) -> Option<&str> {
        self.headers.get("authorization").map(String::as_str)
    }

    fn accepts_json(&self) -> bool {
        self.headers.get("accept").is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim() == "application/json")
        })
    }
}

fn parse_query(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((url_decode(key)?, url_decode(value)?))
        })
        .collect()
}

fn url_decode(value: &str) -> Option<String> {
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16)?;
                let low = (bytes[index + 2] as char).to_digit(16)?;
                output.push(((high << 4) | low) as u8);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

fn write_response(stream: &mut TcpStream, status: u16, body: String) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn method_label(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "POST" => "POST",
        _ => "OTHER",
    }
}

pub(super) fn append_log(log_path: &Path, event: (&str, String, u16, Vec<String>)) {
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let line = json!({
        "method": event.0,
        "path": event.1,
        "status": event.2,
        "query_keys": event.3,
        "authentication_scheme": "bearer",
    });
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(file, "{line}");
    }
}
