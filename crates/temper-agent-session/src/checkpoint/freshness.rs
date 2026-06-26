// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;
use temper_protocol_agent::PullRequestFreshness;

pub(super) async fn ensure_fresh(
    url: Option<&str>,
    check: Option<&PullRequestFreshness>,
) -> Result<(), String> {
    let Some(url) = url else {
        return Ok(());
    };
    let Some(check) = check else {
        return Ok(());
    };
    let body = serde_json::to_vec(check).map_err(|error| format!("serialize freshness check: {error}"))?;
    let url = url.to_string();
    let response = skein::runtime::spawn_blocking(move || post_json(&url, &body)).await?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| "freshness response missing status".to_string())?;
    match status {
        "fresh" => Ok(()),
        "stale" => Err(format!(
            "pull request is stale: {}",
            response
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("stale")
        )),
        "unavailable" => Err(format!(
            "pull request freshness unavailable: {}",
            response
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
        )),
        other => Err(format!("unknown freshness response status `{other}`")),
    }
}

fn post_json(endpoint: &str, body: &[u8]) -> Result<Value, String> {
    let (host, port, path) = parse_http_url(endpoint)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|error| format!("connect {host}:{port}: {error}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| format!("write freshness request: {error}"))?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read freshness response: {error}"))?;
    parse_http_response(&bytes)
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported freshness URL `{url}` (expected http://)"))?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(format!("invalid freshness URL `{url}`: missing host"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port)) if !host.is_empty() => {
            let port = raw_port
                .parse::<u16>()
                .map_err(|error| format!("invalid freshness URL port `{raw_port}`: {error}"))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    Ok((host, port, path))
}

fn parse_http_response(bytes: &[u8]) -> Result<Value, String> {
    let response = String::from_utf8_lossy(bytes);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed freshness HTTP response".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| "malformed freshness HTTP status".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "freshness endpoint returned HTTP {status}: {}",
            body.trim()
        ));
    }
    serde_json::from_str(body).map_err(|error| format!("parse freshness response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::parse_http_url;

    #[test]
    fn parses_http_url() {
        assert_eq!(
            parse_http_url("http://localhost:9999/v1/pr-freshness").unwrap(),
            (
                "localhost".to_string(),
                9999,
                "/v1/pr-freshness".to_string()
            )
        );
    }
}
