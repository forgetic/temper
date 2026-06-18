// SPDX-License-Identifier: MPL-2.0

//! A tiny blocking HTTP client for the daemon's `GET /v1/state` snapshot.
//!
//! Lives in the binary (not the library) so the library stays free of network
//! and ambient I/O concerns and remains a dependency-light depgraph leaf. It
//! speaks just enough HTTP/1.0 to fetch one JSON body over a plain TCP socket —
//! the daemon binds localhost, no TLS — mirroring the workspace's other
//! minimal blocking HTTP code.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use temper_web::feeds::snapshot_source::SnapshotSource;
use temper_web::project::snapshot::RawDaemonSnapshot;

/// Fetches the raw daemon dispatch snapshot from a configured daemon base URL.
pub struct HttpSnapshotSource {
    base_url: String,
}

impl HttpSnapshotSource {
    /// Build from the daemon base URL (e.g. `http://127.0.0.1:9000`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

impl SnapshotSource for HttpSnapshotSource {
    fn fetch(&self) -> Option<RawDaemonSnapshot> {
        let body = http_get(&self.base_url, "/v1/state").ok()?;
        serde_json::from_str(&body).ok()
    }
}

/// Issue a blocking `GET {path}` against `{base_url}` and return the response
/// body. Best-effort: returns `Err` on any connection/parse failure so the
/// server falls back to an empty board rather than failing to boot.
fn http_get(base_url: &str, path: &str) -> Result<String, String> {
    let (host, port) = parse_host_port(base_url)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|error| format!("connect {host}:{port}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;

    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response".to_string())?;
    let status_ok = head
        .lines()
        .next()
        .map(|line| line.contains(" 200 "))
        .unwrap_or(false);
    if !status_ok {
        return Err(format!(
            "daemon returned non-200: {}",
            head.lines().next().unwrap_or("")
        ));
    }
    Ok(body.to_string())
}

/// Parse `http://host:port` (scheme optional, port defaults to 80) into the
/// host and port to connect to.
fn parse_host_port(base_url: &str) -> Result<(String, u16), String> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse()
                .map_err(|_| format!("invalid port in {base_url}"))?;
            Ok((host.to_string(), port))
        }
        None => Ok((authority.to_string(), 80)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_port() {
        assert_eq!(
            parse_host_port("http://127.0.0.1:9000").unwrap(),
            ("127.0.0.1".to_string(), 9000)
        );
        assert_eq!(
            parse_host_port("127.0.0.1:9000/v1/state").unwrap(),
            ("127.0.0.1".to_string(), 9000)
        );
        assert_eq!(
            parse_host_port("http://daemon.local").unwrap(),
            ("daemon.local".to_string(), 80)
        );
    }
}
