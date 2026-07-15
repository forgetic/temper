// SPDX-License-Identifier: MPL-2.0

use serde::Serialize;
use sha2::{Digest, Sha256};
use temper_protocol_activity::MAX_IDENTIFIER_BYTES;

use crate::trace_journal::{AgentTraceRun, AgentTraceRunStatus};

use super::{
    AGENT_RUNS_PATH, ApiError, DEFAULT_EVENT_PAGE_LIMIT, DEFAULT_RUN_PAGE_LIMIT,
    MAX_EVENT_PAGE_LIMIT, MAX_QUERY_VALUE_BYTES, MAX_RUN_PAGE_LIMIT,
};

pub(super) enum TraceRoute {
    List(RunListQuery),
    Summary(String),
    Events(String, EventQuery),
    Export(String),
}

pub(super) fn parse_route(method: &str, uri: &str) -> Result<TraceRoute, ApiError> {
    if method != "GET" {
        return Err(ApiError::NotFound);
    }
    let (path, query) = split_uri(uri);
    if path == AGENT_RUNS_PATH {
        return parse_run_list_query(query).map(TraceRoute::List);
    }
    let Some(remainder) = path.strip_prefix(&format!("{AGENT_RUNS_PATH}/")) else {
        return Err(ApiError::NotFound);
    };
    match remainder.split('/').collect::<Vec<_>>().as_slice() {
        [run_id] => {
            require_empty_query(query)?;
            Ok(TraceRoute::Summary(decode_run_id(run_id)?))
        }
        [run_id, "events"] => Ok(TraceRoute::Events(
            decode_run_id(run_id)?,
            parse_event_query(query)?,
        )),
        [run_id, "export"] => {
            require_empty_query(query)?;
            Ok(TraceRoute::Export(decode_run_id(run_id)?))
        }
        _ => Err(ApiError::NotFound),
    }
}

pub(crate) fn is_trace_uri(uri: &str) -> bool {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    path == AGENT_RUNS_PATH || path.starts_with(&format!("{AGENT_RUNS_PATH}/"))
}

#[derive(Clone, Default, Serialize)]
pub(super) struct RunFilters {
    artifact_ref: Option<String>,
    role: Option<String>,
    correlation_key: Option<String>,
    agent_session_id: Option<String>,
    status: Option<AgentTraceRunStatus>,
    run_id: Option<String>,
}

impl RunFilters {
    pub(super) fn matches(&self, run: &AgentTraceRun) -> bool {
        option_matches(&self.artifact_ref, &run.manifest.assignment.artifact_ref)
            && option_matches(&self.role, &run.manifest.assignment.role)
            && option_matches(
                &self.correlation_key,
                &run.manifest.assignment.correlation_key,
            )
            && match &self.agent_session_id {
                Some(expected) => run.manifest.agent_session_id.as_ref() == Some(expected),
                None => true,
            }
            && self
                .status
                .is_none_or(|status| run.summary.status == status)
            && option_matches(&self.run_id, &run.manifest.run_id)
    }

    pub(super) fn hash(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("run filters serialize");
        render_hex(&Sha256::digest(encoded))
    }
}

fn option_matches(filter: &Option<String>, value: &str) -> bool {
    filter.as_ref().is_none_or(|expected| expected == value)
}

pub(super) struct RunListQuery {
    pub(super) filters: RunFilters,
    pub(super) limit: usize,
    pub(super) cursor: Option<String>,
}

pub(super) struct EventQuery {
    pub(super) after_seq: u64,
    pub(super) limit: usize,
}

fn parse_run_list_query(query: Option<&str>) -> Result<RunListQuery, ApiError> {
    let mut filters = RunFilters::default();
    let mut limit = None;
    let mut cursor = None;
    for (name, value) in query_pairs(query)? {
        match name.as_str() {
            "artifact_ref" => set_once(&mut filters.artifact_ref, value)?,
            "role" => set_once(&mut filters.role, value)?,
            "correlation_key" => set_once(&mut filters.correlation_key, value)?,
            "agent_session_id" => set_once(&mut filters.agent_session_id, value)?,
            "run_id" => set_once(&mut filters.run_id, value)?,
            "status" => {
                if filters.status.is_some() {
                    return Err(ApiError::BadRequest("query parameters must be unique"));
                }
                filters.status = Some(parse_status(&value)?);
            }
            "limit" => set_once(&mut limit, value)?,
            "cursor" => set_once(&mut cursor, value)?,
            _ => return Err(ApiError::BadRequest("unknown trace query parameter")),
        }
    }
    Ok(RunListQuery {
        filters,
        limit: parse_limit(limit.as_deref(), DEFAULT_RUN_PAGE_LIMIT, MAX_RUN_PAGE_LIMIT)?,
        cursor,
    })
}

fn parse_event_query(query: Option<&str>) -> Result<EventQuery, ApiError> {
    let mut after_seq = None;
    let mut limit = None;
    for (name, value) in query_pairs(query)? {
        match name.as_str() {
            "after_seq" => set_once(&mut after_seq, value)?,
            "limit" => set_once(&mut limit, value)?,
            _ => return Err(ApiError::BadRequest("unknown trace query parameter")),
        }
    }
    let after_seq = after_seq
        .as_deref()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| ApiError::BadRequest("after_seq must be an unsigned integer"))
        })
        .transpose()?
        .unwrap_or(0);
    Ok(EventQuery {
        after_seq,
        limit: parse_limit(
            limit.as_deref(),
            DEFAULT_EVENT_PAGE_LIMIT,
            MAX_EVENT_PAGE_LIMIT,
        )?,
    })
}

fn split_uri(uri: &str) -> (&str, Option<&str>) {
    uri.split_once('?')
        .map_or((uri, None), |(path, query)| (path, Some(query)))
}

fn require_empty_query(query: Option<&str>) -> Result<(), ApiError> {
    if query.is_none_or(str::is_empty) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "this trace route does not accept query parameters",
        ))
    }
}

fn decode_run_id(encoded: &str) -> Result<String, ApiError> {
    let run_id = percent_decode(encoded, false)?;
    if run_id.is_empty()
        || run_id.len() > MAX_IDENTIFIER_BYTES
        || run_id.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest("run_id is malformed"));
    }
    Ok(run_id)
}

fn query_pairs(query: Option<&str>) -> Result<Vec<(String, String)>, ApiError> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|pair| {
            let (name, value) = pair
                .split_once('=')
                .ok_or(ApiError::BadRequest("query parameters require values"))?;
            let name = percent_decode(name, true)?;
            let value = percent_decode(value, true)?;
            if name.is_empty()
                || value.is_empty()
                || name.len() > MAX_QUERY_VALUE_BYTES
                || value.len() > MAX_QUERY_VALUE_BYTES
                || name.chars().any(char::is_control)
                || value.chars().any(char::is_control)
            {
                return Err(ApiError::BadRequest("query parameter is malformed"));
            }
            Ok((name, value))
        })
        .collect()
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), ApiError> {
    if slot.replace(value).is_some() {
        Err(ApiError::BadRequest("query parameters must be unique"))
    } else {
        Ok(())
    }
}

fn parse_limit(value: Option<&str>, default: usize, maximum: usize) -> Result<usize, ApiError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let limit = value
        .parse::<usize>()
        .map_err(|_| ApiError::BadRequest("limit must be a positive integer"))?;
    if limit == 0 || limit > maximum {
        return Err(ApiError::BadRequest("limit is outside the allowed range"));
    }
    Ok(limit)
}

fn parse_status(value: &str) -> Result<AgentTraceRunStatus, ApiError> {
    match value {
        "active" => Ok(AgentTraceRunStatus::Active),
        "succeeded" => Ok(AgentTraceRunStatus::Succeeded),
        "cancelled" => Ok(AgentTraceRunStatus::Cancelled),
        "failed" => Ok(AgentTraceRunStatus::Failed),
        _ => Err(ApiError::BadRequest("status is not recognized")),
    }
}

fn percent_decode(value: &str, plus_as_space: bool) -> Result<String, ApiError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(ApiError::BadRequest("percent encoding is malformed"));
                }
                let high = hex_value(bytes[index + 1])
                    .ok_or(ApiError::BadRequest("percent encoding is malformed"))?;
                let low = hex_value(bytes[index + 2])
                    .ok_or(ApiError::BadRequest("percent encoding is malformed"))?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            b'+' if plus_as_space => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| ApiError::BadRequest("query text is not UTF-8"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn render_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push(char::from(HEX[usize::from(byte >> 4)]));
        rendered.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_does_not_capture_unrelated_routes() {
        assert!(is_trace_uri("/v1/agent-runs?limit=2"));
        assert!(is_trace_uri("/v1/agent-runs/run/events"));
        assert!(!is_trace_uri("/v1/agent-runs-extra"));
        assert!(!is_trace_uri("/v1/state"));
    }
}
