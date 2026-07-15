// SPDX-License-Identifier: MPL-2.0

use std::cmp::Ordering;

use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use temper_protocol_activity::MAX_IDENTIFIER_BYTES;

use crate::trace_journal::AgentTraceRun;

use super::{ApiError, MAX_QUERY_VALUE_BYTES};

const CURSOR_VERSION: u32 = 1;

#[derive(Clone, Eq, PartialEq)]
pub(super) struct RunOrderKey {
    started_at: DateTime<Utc>,
    run_id: String,
}

impl Ord for RunOrderKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.started_at
            .cmp(&other.started_at)
            .then_with(|| self.run_id.cmp(&other.run_id))
    }
}

impl PartialOrd for RunOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) fn run_order_key(run: &AgentTraceRun) -> Result<RunOrderKey, ApiError> {
    let started_at = run
        .summary
        .started_at
        .as_deref()
        .unwrap_or(&run.manifest.created_at);
    let started_at = DateTime::parse_from_rfc3339(started_at)
        .map_err(|_| ApiError::Unavailable)?
        .with_timezone(&Utc);
    Ok(RunOrderKey {
        started_at,
        run_id: run.manifest.run_id.clone(),
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCursor {
    version: u32,
    started_at: String,
    run_id: String,
    filter_hash: String,
}

pub(super) fn encode_cursor(key: &RunOrderKey, filter_hash: &str) -> Result<String, ApiError> {
    let cursor = RunCursor {
        version: CURSOR_VERSION,
        started_at: key.started_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        run_id: key.run_id.clone(),
        filter_hash: filter_hash.to_string(),
    };
    let encoded = serde_json::to_vec(&cursor).map_err(|_| ApiError::Unavailable)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(encoded))
}

pub(super) fn decode_cursor(
    value: &str,
    expected_filter_hash: &str,
) -> Result<RunOrderKey, ApiError> {
    if value.len() > MAX_QUERY_VALUE_BYTES {
        return Err(ApiError::BadRequest("cursor is malformed"));
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ApiError::BadRequest("cursor is malformed"))?;
    let cursor: RunCursor = serde_json::from_slice(&decoded)
        .map_err(|_| ApiError::BadRequest("cursor is malformed"))?;
    if cursor.version != CURSOR_VERSION
        || cursor.run_id.is_empty()
        || cursor.run_id.len() > MAX_IDENTIFIER_BYTES
        || cursor.filter_hash != expected_filter_hash
    {
        return Err(ApiError::BadRequest("cursor is malformed"));
    }
    let started_at = DateTime::parse_from_rfc3339(&cursor.started_at)
        .map_err(|_| ApiError::BadRequest("cursor is malformed"))?
        .with_timezone(&Utc);
    Ok(RunOrderKey {
        started_at,
        run_id: cursor.run_id,
    })
}
