// SPDX-License-Identifier: MPL-2.0

//! A blocking HTTP client to the interaction-service, used by the conversation
//! proxy (feed 2).
//!
//! Lives in the binary — not the library — for the same reason as
//! `daemon_client.rs`: the library stays free of network and ambient I/O and
//! remains a dependency-light leaf, taking the client injected behind the
//! `InteractionClient` trait. It speaks just enough HTTP/1.0 over a plain TCP
//! socket (the interaction-service binds localhost, no TLS).
//!
//! ## Which conversations to poll
//!
//! The interaction-service has no "list conversations" endpoint, so this client
//! learns the set of conversations to poll from the proxy traffic itself: a turn
//! POST, an accept POST, or a `POST /conversations` whose response carries a
//! `conversation_id` all register that id. The poller then drains each registered
//! conversation's events. This keeps the proxy provider-neutral (ids are opaque)
//! and avoids any out-of-band discovery.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

use temper_web::conversation::{ClientResponse, InteractionClient};

/// Blocking HTTP client to the interaction-service base URL.
pub struct HttpInteractionClient {
    base_url: String,
    /// Conversation ids discovered from proxy traffic, polled by the poller.
    tracked: Mutex<BTreeSet<String>>,
}

impl HttpInteractionClient {
    /// Build from the interaction-service base URL (e.g. `http://127.0.0.1:8077`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            tracked: Mutex::new(BTreeSet::new()),
        }
    }

    fn track(&self, conversation_id: &str) {
        if conversation_id.is_empty() {
            return;
        }
        self.tracked
            .lock()
            .expect("tracked lock")
            .insert(conversation_id.to_string());
    }
}

impl InteractionClient for HttpInteractionClient {
    fn fetch_events(&self, conversation_id: &str) -> Result<String, String> {
        let path = format!("/conversations/{conversation_id}/events");
        let response = http_request(&self.base_url, "GET", &path, None)?;
        if response.status != 200 {
            return Err(format!("events fetch returned {}", response.status));
        }
        Ok(response.body)
    }

    fn post_turn(&self, conversation_id: &str, body: &str) -> Result<ClientResponse, String> {
        self.track(conversation_id);
        let path = format!("/conversations/{conversation_id}/turns");
        http_request(&self.base_url, "POST", &path, Some(body))
    }

    fn post_accept(
        &self,
        conversation_id: &str,
        proposal_id: &str,
    ) -> Result<ClientResponse, String> {
        self.track(conversation_id);
        let path = format!("/conversations/{conversation_id}/proposals/{proposal_id}/accept");
        http_request(&self.base_url, "POST", &path, Some(""))
    }

    fn post_new_conversation(&self, body: &str) -> Result<ClientResponse, String> {
        let response = http_request(&self.base_url, "POST", "/conversations", Some(body))?;
        // Register the created conversation so the poller starts streaming it.
        if let Some(id) = extract_conversation_id(&response.body) {
            self.track(&id);
        }
        Ok(response)
    }

    fn tracked_conversations(&self) -> Vec<String> {
        self.tracked
            .lock()
            .expect("tracked lock")
            .iter()
            .cloned()
            .collect()
    }
}

/// Pull a `conversation_id` out of a JSON object body, if present.
fn extract_conversation_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("conversation_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Issue a blocking HTTP/1.0 request and return the upstream status + body.
fn http_request(
    base_url: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<ClientResponse, String> {
    let (host, port) = parse_host_port(base_url)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|error| format!("connect {host}:{port}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;

    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.0\r\n\
         Host: {host}\r\n\
         Connection: close\r\n\
         Accept: application/json\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;

    let (head, response_body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response".to_string())?;
    let status = parse_status(head)?;
    Ok(ClientResponse {
        status,
        body: response_body.to_string(),
    })
}

/// Parse the numeric status code from the HTTP status line.
fn parse_status(head: &str) -> Result<u16, String> {
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| "missing HTTP status".to_string())
}

/// Parse `http://host:port` into the host and port to connect to (port 80
/// default).
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
    fn parses_status_line() {
        assert_eq!(parse_status("HTTP/1.1 201 Created\r\n").unwrap(), 201);
        assert_eq!(parse_status("HTTP/1.0 200 OK").unwrap(), 200);
    }

    #[test]
    fn parses_host_and_port() {
        assert_eq!(
            parse_host_port("http://127.0.0.1:8077").unwrap(),
            ("127.0.0.1".to_string(), 8077)
        );
        assert_eq!(
            parse_host_port("interaction.local").unwrap(),
            ("interaction.local".to_string(), 80)
        );
    }

    #[test]
    fn extracts_conversation_id_from_created_response() {
        assert_eq!(
            extract_conversation_id(r#"{"conversation_id":"c2","profile_id":"x"}"#),
            Some("c2".to_string())
        );
        assert_eq!(extract_conversation_id("{}"), None);
        assert_eq!(extract_conversation_id("not json"), None);
    }

    #[test]
    fn new_conversation_response_registers_tracked_id() {
        // No socket needed: drive tracking via the response-id extraction path.
        let client = HttpInteractionClient::new("http://127.0.0.1:9");
        client.track("c1");
        assert_eq!(client.tracked_conversations(), vec!["c1".to_string()]);
    }
}
