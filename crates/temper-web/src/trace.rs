// SPDX-License-Identifier: MPL-2.0

//! Server-side access to the engine-owned agent trace journal.
//!
//! The browser never talks to the engine and never receives its read credential.
//! A [`TraceApiClient`] is injected into the web server, which exposes only
//! same-origin, bounded JSON and SSE projections.  The engine remains the
//! authority: this module keeps cursors, not a second durable journal.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, CaptureModeV1, CapturedContentV1, UsageV1,
};

use crate::board::{StreamEvent, StreamEventKind};
use crate::server::AppState;

/// The engine's run status, duplicated as a wire-only enum so temper-web does
/// not depend on the engine implementation crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceRunStatus {
    Active,
    Succeeded,
    Cancelled,
    Failed,
}

/// Trusted assignment identity returned by the authorized query API.
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

/// Run summary consumed by the card drawer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRunSummary {
    pub version: u32,
    pub run_id: String,
    pub identity: TraceRunIdentity,
    pub status: TraceRunStatus,
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

/// Sanitized client failure. It deliberately carries no request headers or
/// credential values and is safe to log at the web boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceClientError {
    message: String,
}

impl TraceClientError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TraceClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TraceClientError {}

/// Explicit server-side engine query seam. Implementations own the read token;
/// neither this trait nor its DTOs expose a way to serialize that credential.
pub trait TraceApiClient: Send + Sync {
    fn list_runs(
        &self,
        artifact_ref: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TraceRunPage, TraceClientError>;

    fn run_summary(&self, run_id: &str) -> Result<TraceRunSummary, TraceClientError>;

    fn events(
        &self,
        run_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<TraceEventPage, TraceClientError>;
}

/// Cursor state for the low-rate global board projection.
#[derive(Default)]
pub struct TraceActivityPoller {
    cursors: BTreeMap<String, u64>,
}

impl TraceActivityPoller {
    pub const RUN_PAGE_LIMIT: usize = 100;
    pub const EVENT_PAGE_LIMIT: usize = 500;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Poll every run page, resume each event stream by sequence, and project
    /// only low-rate boundaries into the board feed. Cursor advancement includes
    /// filtered deltas, so a high-volume transcript is read once but never fanned
    /// out to every board subscriber.
    pub fn poll_once(
        &mut self,
        state: &AppState,
        client: &dyn TraceApiClient,
    ) -> Result<usize, TraceClientError> {
        let mut projected = 0;
        let mut seen_runs = BTreeSet::new();
        for artifact_ref in state.trace_artifact_refs() {
            let mut cursor = None;
            loop {
                let page = client.list_runs(
                    Some(&artifact_ref),
                    cursor.as_deref(),
                    Self::RUN_PAGE_LIMIT,
                )?;
                for run in page.runs {
                    seen_runs.insert(run.run_id.clone());
                    projected += self.poll_run(state, client, &run)?;
                }
                let Some(next) = page.next_cursor else { break };
                if cursor.as_ref() == Some(&next) {
                    return Err(TraceClientError::new(
                        "trace run pagination returned a repeated cursor",
                    ));
                }
                cursor = Some(next);
            }
        }
        // Retention removes old engine runs. Prune their in-memory cursors only
        // after a complete successful listing so a partial/outage response can
        // never discard resume state for runs that still exist.
        self.cursors.retain(|run_id, _| seen_runs.contains(run_id));
        Ok(projected)
    }

    fn poll_run(
        &mut self,
        state: &AppState,
        client: &dyn TraceApiClient,
        run: &TraceRunSummary,
    ) -> Result<usize, TraceClientError> {
        let mut after = self.cursors.get(&run.run_id).copied().unwrap_or(0);
        let mut projected = 0;
        loop {
            let requested_after = after;
            let page = client.events(&run.run_id, after, Self::EVENT_PAGE_LIMIT)?;
            for event in &page.events {
                if event.seq <= after {
                    continue;
                }
                if let Some(stream_event) = board_projection(event) {
                    if state.ingest_trace_activity(&event.assignment.artifact_ref, stream_event) {
                        projected += 1;
                    }
                }
                after = event.seq;
                self.cursors.insert(run.run_id.clone(), after);
            }

            // Empty pages still carry the authoritative cursor. Advance only
            // forwards, and reject a has_more page that cannot make progress.
            if page.next_after_seq > after {
                after = page.next_after_seq;
                self.cursors.insert(run.run_id.clone(), after);
            }
            if !page.has_more {
                break;
            }
            if after <= requested_after {
                return Err(TraceClientError::new(
                    "trace event pagination made no cursor progress",
                ));
            }
        }
        Ok(projected)
    }
}

/// Run the low-rate projection forever. Engine outages are isolated from the
/// board: the last cursor is retained and the next interval resumes it.
pub fn pump_trace_activity(
    state: Arc<AppState>,
    client: Arc<dyn TraceApiClient>,
    interval: Duration,
) {
    std::thread::spawn(move || {
        let mut poller = TraceActivityPoller::new();
        loop {
            if let Err(error) = poller.poll_once(&state, client.as_ref()) {
                tracing::debug!(
                    target: "temper_web",
                    %error,
                    "agent trace activity poll failed; retaining cursor for retry"
                );
            }
            std::thread::sleep(interval);
        }
    });
}

/// Convert one canonical event to the deliberately low-rate board shape.
/// Transcript-bearing messages and text/thinking deltas are intentionally
/// absent: detailed events are available only on a run-specific drawer stream.
#[must_use]
pub fn board_projection(event: &AgentRunEventV1) -> Option<StreamEvent> {
    let (kind, label, value) = match &event.event {
        AgentActivityEventV1::RunStarted(_) => (
            StreamEventKind::Text,
            Some("run".to_string()),
            "run started".to_string(),
        ),
        AgentActivityEventV1::RunFinished(data) => (
            StreamEventKind::Text,
            Some("run".to_string()),
            format!("run finished: {:?} ({} ms)", data.status, data.duration_ms).to_lowercase(),
        ),
        AgentActivityEventV1::ScopeStarted(data) => (
            StreamEventKind::Text,
            Some("scope".to_string()),
            format!(
                "scope started: {}",
                data.display_name.as_deref().unwrap_or(&event.scope.id)
            ),
        ),
        AgentActivityEventV1::ScopeFinished(data) => (
            StreamEventKind::Text,
            Some("scope".to_string()),
            format!(
                "scope finished: {:?} ({} ms)",
                data.status, data.duration_ms
            )
            .to_lowercase(),
        ),
        AgentActivityEventV1::TurnStarted(_) => (
            StreamEventKind::Text,
            Some("turn".to_string()),
            format!("turn {} started", event.turn.unwrap_or(0)),
        ),
        AgentActivityEventV1::TurnFinished(data) => (
            StreamEventKind::Text,
            Some("turn".to_string()),
            format!(
                "turn {} finished ({} ms)",
                event.turn.unwrap_or(0),
                data.duration_ms
            ),
        ),
        AgentActivityEventV1::ToolStarted(data) => (
            StreamEventKind::Tool,
            Some(data.name.clone()),
            "started".to_string(),
        ),
        AgentActivityEventV1::ToolFinished(data) => (
            StreamEventKind::Tool,
            Some(data.name.clone()),
            format!("{:?} ({} ms)", data.status, data.duration_ms).to_lowercase(),
        ),
        AgentActivityEventV1::ModelCallRetrying(data) => (
            StreamEventKind::Text,
            Some("retry".to_string()),
            format!(
                "model call retry {} in {} ms",
                data.next_attempt, data.delay_ms
            ),
        ),
        AgentActivityEventV1::TraceGap(data) => (
            StreamEventKind::Text,
            Some("gap".to_string()),
            format!("{} detailed events dropped", data.dropped_events),
        ),
        AgentActivityEventV1::RunFailed(data) => (
            StreamEventKind::Text,
            Some("error".to_string()),
            format!("run failed: {:?}", data.failure.code).to_lowercase(),
        ),
        AgentActivityEventV1::PromptPrepared(_)
        | AgentActivityEventV1::ModelCallStarted(_)
        | AgentActivityEventV1::ModelCallFinished(_)
        | AgentActivityEventV1::AssistantMessage(_)
        | AgentActivityEventV1::OutputTextDelta(_)
        | AgentActivityEventV1::OutputThinkingDelta(_)
        | AgentActivityEventV1::SteeringApplied(_)
        | AgentActivityEventV1::Usage(_) => return None,
    };
    Some(StreamEvent {
        kind,
        k: label,
        v: bounded(value, 512),
    })
}

fn bounded(mut value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let boundary = value
        .char_indices()
        .nth(max_chars)
        .map_or(value.len(), |(index, _)| index);
    value.truncate(boundary);
    value.push('…');
    value
}

/// Extract a bounded inline preview without resolving blobs. Used by tests and
/// server projections; blob bodies remain behind the authorized engine API.
#[must_use]
pub fn inline_preview(content: &CapturedContentV1) -> Option<&str> {
    match content {
        CapturedContentV1::Inline(value) => Some(&value.text),
        CapturedContentV1::Blob { .. } => None,
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod trace_tests;
