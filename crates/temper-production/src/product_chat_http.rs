//! Minimal HTTP parsing/rendering helpers for product-chat transports.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpStream;

use serde::Serialize;

use crate::product_chat::ProductChatError;

const MAX_REQUEST_BYTES: usize = 1_048_576;

#[derive(Debug)]
pub(crate) struct HttpRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Vec<u8>,
}

impl HttpRequest {
    #[cfg(test)]
    pub(crate) fn new(method: &str, path: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: body.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .insert(name.to_ascii_lowercase(), value.to_string());
        self
    }

    pub(crate) fn read_from(stream: &mut TcpStream) -> Result<Self, ProductChatError> {
        let mut raw = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let n = stream.read(&mut buf)?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if raw.len() > MAX_REQUEST_BYTES {
                return Err(ProductChatError::Runtime(
                    "HTTP request is too large".into(),
                ));
            }
            if let Some(header_end) = find_header_end(&raw) {
                let (method, path, headers) = parse_headers(&raw[..header_end])?;
                let body_start = header_end + 4;
                let content_len = header(&headers, "content-length")
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(0);
                if body_start + content_len > MAX_REQUEST_BYTES {
                    return Err(ProductChatError::Runtime(
                        "HTTP request is too large".into(),
                    ));
                }
                while raw.len() < body_start + content_len {
                    let n = stream.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                if raw.len() < body_start + content_len {
                    return Err(ProductChatError::Runtime("incomplete HTTP request".into()));
                }
                return Ok(Self {
                    method,
                    path,
                    headers,
                    body: raw[body_start..body_start + content_len].to_vec(),
                });
            }
        }
        Err(ProductChatError::Runtime("incomplete HTTP request".into()))
    }
}

#[derive(Debug)]
pub(crate) struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    #[cfg(test)]
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &str {
        &self.body
    }

    pub(crate) fn json<T: Serialize + ?Sized>(status: u16, value: &T) -> Self {
        let body = serde_json::to_string(value).expect("serializing API response succeeds");
        Self { status, body }
    }

    pub(crate) fn to_http(&self) -> String {
        format!(
            "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            self.status,
            reason(self.status),
            self.body.len(),
            self.body
        )
    }
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_headers(
    raw: &[u8],
) -> Result<(String, String, BTreeMap<String, String>), ProductChatError> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| ProductChatError::Runtime("HTTP headers are not UTF-8".into()))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ProductChatError::Runtime("missing request line".into()))?;
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

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(name).map(String::as_str)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}
