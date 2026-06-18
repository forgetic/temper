// SPDX-License-Identifier: MPL-2.0

//! A minimal blocking HTTP/1.1 request reader and response writer.
//!
//! temper-web runs its OWN server (the daemon's skein H1 responder is one-shot
//! and cannot hold a connection open for SSE — see the crate docs). We only need
//! the request line for routing (method + path) plus the ability to write a
//! buffered response or stream SSE frames, so this is a deliberately tiny
//! reader, in the same spirit as the workspace's other blocking servers
//! (interaction-service, trigger-forgejo) — no async runtime, no HTTP crate.

use std::io::{BufRead, BufReader, Read, Write};

/// A parsed request line: just what routing needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub method: String,
    /// The full request target including any query string.
    pub path: String,
}

impl RequestLine {
    /// The path without its query string.
    #[must_use]
    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }
}

/// Read and discard headers up to the blank line, returning the request line.
/// Returns `None` on a closed/empty connection or a malformed request line.
pub fn read_request_line<R: Read>(reader: &mut BufReader<R>) -> Option<RequestLine> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None; // connection closed
    }
    let request_line = parse_request_line(line.trim_end())?;
    // Drain the remaining headers (we route on the request line alone).
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
    }
    Some(request_line)
}

/// Parse a `METHOD path HTTP/1.1` request line.
#[must_use]
pub fn parse_request_line(line: &str) -> Option<RequestLine> {
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    // The third token is the HTTP version; we do not need it.
    Some(RequestLine { method, path })
}

/// Write a complete buffered HTTP response (status, content type, body).
pub fn write_response<W: Write>(
    writer: &mut W,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn parses_a_request_line() {
        let parsed = parse_request_line("GET /events?cursor=5 HTTP/1.1").expect("parses");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/events?cursor=5");
        assert_eq!(parsed.path_only(), "/events");
    }

    #[test]
    fn rejects_garbage_request_line() {
        assert!(parse_request_line("GET").is_none());
        assert!(parse_request_line("").is_none());
    }

    #[test]
    fn reads_request_line_and_skips_headers() {
        let raw = "GET /api/state HTTP/1.1\r\nHost: x\r\nAccept: */*\r\n\r\n";
        let mut reader = BufReader::new(raw.as_bytes());
        let line = read_request_line(&mut reader).expect("reads");
        assert_eq!(line.method, "GET");
        assert_eq!(line.path_only(), "/api/state");
    }

    #[test]
    fn empty_connection_yields_none() {
        let mut reader = BufReader::new(&b""[..]);
        assert!(read_request_line(&mut reader).is_none());
    }

    #[test]
    fn writes_a_buffered_response() {
        let mut out = Vec::new();
        write_response(&mut out, 200, "OK", "application/json", b"{}").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }
}
