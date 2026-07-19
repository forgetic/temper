// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{AgentRunEventV1, CaptureModeV1, UsageV1};

use crate::trace_journal::AgentTraceRunStatus;

/// Trusted identity attached to an authorized run summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRunIdentity {
    pub worker_id: String,
    pub assignment_id: String,
    pub job_id: String,
    pub repository: String,
    pub artifact_ref: String,
    pub role: String,
    pub action: String,
    pub correlation_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

/// Boundary counts derived from canonical events rather than cached files.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRunCounts {
    pub events: u64,
    pub scopes: u64,
    pub turns: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub retries: u64,
}

/// Typed run projection returned by list and single-run routes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRunSummary {
    pub version: u32,
    pub run_id: String,
    pub identity: TraceRunIdentity,
    pub status: AgentTraceRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub counts: TraceRunCounts,
    pub usage: UsageV1,
    pub capture_mode: CaptureModeV1,
    pub has_truncated_content: bool,
    pub has_trace_gaps: bool,
    pub dropped_events: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seq: Option<u64>,
    pub last_seq: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRunPage {
    pub runs: Vec<TraceRunSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEventPage {
    pub run_id: String,
    pub events: Vec<AgentRunEventV1>,
    pub next_after_seq: u64,
    pub has_more: bool,
}
