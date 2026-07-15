// SPDX-License-Identifier: MPL-2.0

//! Same-origin, server-side proxy for the engine-owned trace journal.
//!
//! Finite JSON reads and the long-lived drawer SSE stream share the injected
//! [`TraceApiClient`]. The engine credential stays encapsulated by that client;
//! no browser-facing request or response contains it.

use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use crate::trace::TraceClientError;

use super::{AppState, Response, sse};

/// Route the bounded same-origin trace JSON facade. The engine credential is
/// consumed only by the injected server-side client and can never appear in the
/// response DTO. The run-specific streaming route is handled by the connection
/// loop because it remains open.
#[must_use]
pub fn route_trace_get(state: &AppState, target: &str) -> Response {
    let Some(client) = state.trace_client() else {
        return Response::service_unavailable("agent trace API disabled");
    };
    let (path, query) = target
        .split_once('?')
        .map_or((target, ""), |(path, query)| (path, query));
    let result: Result<Vec<u8>, TraceClientError> = if path == "/api/agent-runs" {
        let params = match parse_trace_query(query) {
            Ok(params) => params,
            Err(message) => return Response::bad_request(message),
        };
        client
            .list_runs(
                params.artifact_ref.as_deref(),
                params.cursor.as_deref(),
                params.limit,
            )
            .and_then(to_json)
    } else {
        let Some(remainder) = path.strip_prefix("/api/agent-runs/") else {
            return Response::not_found();
        };
        let parts = remainder.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            [encoded_run_id] if query.is_empty() => decode_component(encoded_run_id)
                .map_err(TraceClientError::new)
                .and_then(|run_id| client.run_summary(&run_id))
                .and_then(to_json),
            [encoded_run_id, "events"] => {
                let run_id = match decode_component(encoded_run_id) {
                    Ok(run_id) => run_id,
                    Err(message) => return Response::bad_request(message),
                };
                let params = match parse_event_query(query) {
                    Ok(params) => params,
                    Err(message) => return Response::bad_request(message),
                };
                client
                    .events(&run_id, params.after_seq, params.limit)
                    .and_then(to_json)
            }
            _ => return Response::not_found(),
        }
    };
    match result {
        Ok(body) => Response::json(body),
        Err(error) => {
            tracing::debug!(target: "temper_web", %error, "engine trace request failed");
            Response::trace_bad_gateway()
        }
    }
}

fn to_json<T: serde::Serialize>(value: T) -> Result<Vec<u8>, TraceClientError> {
    serde_json::to_vec(&value)
        .map_err(|_| TraceClientError::new("trace response serialization failed"))
}

pub(super) struct RunQuery {
    pub(super) artifact_ref: Option<String>,
    cursor: Option<String>,
    pub(super) limit: usize,
}

pub(super) fn parse_trace_query(query: &str) -> Result<RunQuery, &'static str> {
    let mut parsed = RunQuery {
        artifact_ref: None,
        cursor: None,
        limit: 50,
    };
    let mut limit_seen = false;
    for (name, value) in query_pairs(query)? {
        match name.as_str() {
            "artifact_ref" if parsed.artifact_ref.is_none() => parsed.artifact_ref = Some(value),
            "cursor" if parsed.cursor.is_none() => parsed.cursor = Some(value),
            "limit" if !limit_seen => {
                parsed.limit = parse_limit(&value, 200)?;
                limit_seen = true;
            }
            _ => return Err("invalid trace query"),
        }
    }
    Ok(parsed)
}

pub(super) struct EventQuery {
    pub(super) after_seq: u64,
    limit: usize,
}

pub(super) fn parse_event_query(query: &str) -> Result<EventQuery, &'static str> {
    let mut after_seq = None;
    let mut limit = None;
    for (name, value) in query_pairs(query)? {
        match name.as_str() {
            "after_seq" if after_seq.is_none() => {
                after_seq = Some(value.parse().map_err(|_| "invalid after_seq")?)
            }
            "limit" if limit.is_none() => limit = Some(parse_limit(&value, 1_000)?),
            _ => return Err("invalid trace event query"),
        }
    }
    Ok(EventQuery {
        after_seq: after_seq.unwrap_or(0),
        limit: limit.unwrap_or(500),
    })
}

fn parse_stream_after_seq(query: &str) -> Result<u64, &'static str> {
    let mut after_seq = None;
    for (name, value) in query_pairs(query)? {
        if name != "after_seq" || after_seq.is_some() {
            return Err("invalid trace stream query");
        }
        after_seq = Some(value.parse().map_err(|_| "invalid after_seq")?);
    }
    Ok(after_seq.unwrap_or(0))
}

fn parse_limit(value: &str, maximum: usize) -> Result<usize, &'static str> {
    let parsed = value.parse::<usize>().map_err(|_| "invalid trace limit")?;
    if parsed == 0 || parsed > maximum {
        return Err("invalid trace limit");
    }
    Ok(parsed)
}

fn query_pairs(query: &str) -> Result<Vec<(String, String)>, &'static str> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|pair| {
            let (name, value) = pair.split_once('=').ok_or("invalid trace query")?;
            Ok((decode_component(name)?, decode_component(value)?))
        })
        .collect()
}

fn decode_component(value: &str) -> Result<String, &'static str> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex(bytes[index + 1]).ok_or("invalid percent encoding")?;
                let low = hex(bytes[index + 2]).ok_or("invalid percent encoding")?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            b'%' => return Err("invalid percent encoding"),
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "trace query is not UTF-8")?;
    if decoded.is_empty() || decoded.len() > 1_024 || decoded.chars().any(char::is_control) {
        return Err("invalid trace query value");
    }
    Ok(decoded)
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) fn parse_trace_stream_target(
    target: &str,
) -> Result<Option<(String, u64)>, &'static str> {
    let (path, query) = target
        .split_once('?')
        .map_or((target, ""), |(path, query)| (path, query));
    let Some(remainder) = path.strip_prefix("/api/agent-runs/") else {
        return Ok(None);
    };
    let parts = remainder.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [encoded_run_id, "stream"] => Ok(Some((
            decode_component(encoded_run_id)?,
            parse_stream_after_seq(query)?,
        ))),
        _ => Ok(None),
    }
}

/// Poll the engine's finite event route and project it onto one long-lived web
/// SSE connection. Sequence IDs are emitted before every event; URL `after_seq`
/// and `Last-Event-ID` share the same run-local cursor. Upstream outages keep the
/// connection alive and retry, while a browser close is observed on the next
/// write and immediately stops this connection's polling loop.
pub(super) fn serve_trace_events(
    mut stream: TcpStream,
    state: &AppState,
    run_id: &str,
    mut after_seq: u64,
    poll_interval: Duration,
) -> std::io::Result<()> {
    stream.write_all(sse::sse_response_head().as_bytes())?;
    stream.flush()?;
    let client = state
        .trace_client()
        .expect("trace client checked before SSE");

    loop {
        match client.events(run_id, after_seq, 500) {
            Ok(page) => {
                let requested_after = after_seq;
                let mut wrote = false;
                for event in page.events {
                    if event.seq <= after_seq {
                        continue;
                    }
                    let Ok(json) = serde_json::to_string(&event) else {
                        continue;
                    };
                    stream.write_all(sse::id_data_frame(event.seq, &json).as_bytes())?;
                    after_seq = event.seq;
                    wrote = true;
                }
                if page.next_after_seq > after_seq {
                    after_seq = page.next_after_seq;
                }
                if wrote {
                    stream.flush()?;
                }
                if page.has_more && after_seq > requested_after {
                    continue;
                }
            }
            Err(error) => {
                tracing::debug!(
                    target: "temper_web",
                    %error,
                    run_id,
                    after_seq,
                    "detailed trace poll failed; retrying from retained cursor"
                );
            }
        }
        std::thread::sleep(poll_interval);
        // A comment both keeps intermediaries from timing out and detects a
        // closed browser socket so no detached polling worker survives it.
        stream.write_all(sse::keep_alive().as_bytes())?;
        stream.flush()?;
    }
}

#[cfg(test)]
#[path = "trace_proxy_tests.rs"]
mod tests;
