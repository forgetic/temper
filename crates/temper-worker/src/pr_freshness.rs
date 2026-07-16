// SPDX-License-Identifier: MPL-2.0

//! Worker-side seam for checking whether an assigned PR-head job is still
//! actionable before the worker or hosted agent pushes to the PR branch.

use std::future::Future;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::pin::Pin;
use std::time::Duration;

use temper_protocol_agent::PullRequestFreshness;
use temper_protocol_worker::{PullRequestFreshnessResponse, PullRequestFreshnessStatus};

use crate::managed_effect::JoinedBlocking;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrFreshnessFailure {
    Stale(String),
    Unavailable(String),
}

pub trait PrFreshnessGuard: Send + Sync {
    fn check<'a>(
        &'a self,
        check: &'a PullRequestFreshness,
    ) -> Pin<Box<dyn Future<Output = Result<(), PrFreshnessFailure>> + Send + 'a>>;
}

#[derive(Clone, Debug)]
pub struct HttpPrFreshnessGuard {
    endpoint: String,
}

impl HttpPrFreshnessGuard {
    pub fn new(daemon_url: &str) -> Self {
        Self {
            endpoint: format!("{}/v1/pr-freshness", daemon_url.trim_end_matches('/')),
        }
    }
}

impl PrFreshnessGuard for HttpPrFreshnessGuard {
    fn check<'a>(
        &'a self,
        check: &'a PullRequestFreshness,
    ) -> Pin<Box<dyn Future<Output = Result<(), PrFreshnessFailure>> + Send + 'a>> {
        Box::pin(async move {
            let endpoint = self.endpoint.clone();
            let body = serde_json::to_vec(check).map_err(|error| {
                PrFreshnessFailure::Unavailable(format!("serialize PR freshness check: {error}"))
            })?;
            let response =
                JoinedBlocking::spawn("temper-pr-freshness", move || post_json(&endpoint, &body))
                    .await
                    .map_err(|error| {
                        PrFreshnessFailure::Unavailable(format!("join PR freshness owner: {error}"))
                    })?;
            map_response(response)
        })
    }
}

pub fn map_response(
    response: Result<PullRequestFreshnessResponse, String>,
) -> Result<(), PrFreshnessFailure> {
    match response {
        Ok(response) => match response.status {
            PullRequestFreshnessStatus::Fresh => Ok(()),
            PullRequestFreshnessStatus::Stale => Err(PrFreshnessFailure::Stale(
                response
                    .reason
                    .unwrap_or_else(|| "pull request is stale".to_string()),
            )),
            PullRequestFreshnessStatus::Unavailable => Err(PrFreshnessFailure::Unavailable(
                response
                    .reason
                    .unwrap_or_else(|| "pull request freshness unavailable".to_string()),
            )),
        },
        Err(error) => Err(PrFreshnessFailure::Unavailable(error)),
    }
}

fn post_json(endpoint: &str, body: &[u8]) -> Result<PullRequestFreshnessResponse, String> {
    let (host, port, path) = parse_http_url(endpoint)?;
    let address = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("resolve {host}:{port}: {error}"))?
        .next()
        .ok_or_else(|| format!("resolve {host}:{port}: no addresses"))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))
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
        .map_err(|error| format!("write PR freshness request: {error}"))?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read PR freshness response: {error}"))?;
    parse_http_response(&bytes)
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported PR freshness URL `{url}` (expected http://)"))?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(format!("invalid PR freshness URL `{url}`: missing host"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port)) if !host.is_empty() => {
            let port = raw_port
                .parse::<u16>()
                .map_err(|error| format!("invalid PR freshness URL port `{raw_port}`: {error}"))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    Ok((host, port, path))
}

fn parse_http_response(bytes: &[u8]) -> Result<PullRequestFreshnessResponse, String> {
    let response = String::from_utf8_lossy(bytes);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response from PR freshness endpoint".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| "malformed HTTP status from PR freshness endpoint".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "PR freshness endpoint returned HTTP {status}: {}",
            body.trim()
        ));
    }
    serde_json::from_str::<PullRequestFreshnessResponse>(body)
        .map_err(|error| format!("parse PR freshness endpoint response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_stale_response() {
        let result = map_response(Ok(PullRequestFreshnessResponse::stale("merged")))
            .expect_err("stale response fails");
        assert_eq!(result, PrFreshnessFailure::Stale("merged".to_string()));
    }

    #[test]
    fn parses_http_url_with_default_port() {
        assert_eq!(
            parse_http_url("http://127.0.0.1/v1/pr-freshness").unwrap(),
            ("127.0.0.1".to_string(), 80, "/v1/pr-freshness".to_string())
        );
    }

    #[test]
    fn parses_http_url_with_explicit_port() {
        assert_eq!(
            parse_http_url("http://localhost:8080/v1/pr-freshness").unwrap(),
            (
                "localhost".to_string(),
                8080,
                "/v1/pr-freshness".to_string()
            )
        );
    }
}
