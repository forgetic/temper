use std::sync::{Arc, Mutex};

use temper_protocol_activity::{AgentAssignmentIdentityV1, UsageV1, W3cTraceContext};

/// Canonical activity boundaries represented as OpenTelemetry-compatible spans.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivitySpanKind {
    Run,
    Scope,
    Turn,
    ModelCall,
    Tool,
}

impl ActivitySpanKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Run => "agent.run",
            Self::Scope => "agent.scope",
            Self::Turn => "agent.turn",
            Self::ModelCall => "llm.call",
            Self::Tool => "tool.call",
        }
    }

    pub(crate) const fn close_rank(self) -> u8 {
        match self {
            Self::Tool | Self::ModelCall => 4,
            Self::Turn => 3,
            Self::Scope => 2,
            Self::Run => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivitySpanStatus {
    Ok,
    Error,
    Cancelled,
}

impl ActivitySpanStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivitySpanAttributes {
    pub scope_id: Option<String>,
    pub parent_scope_id: Option<String>,
    pub turn: Option<u32>,
    pub call_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub attempt: Option<u32>,
    pub tool_name: Option<String>,
    pub time_to_first_token_ms: Option<u64>,
    pub stop_reason: Option<String>,
    pub usage: UsageV1,
    pub retry_count: u64,
    pub retry_delay_ms: u64,
    pub dropped_events: u64,
    pub dropped_bytes: u64,
    pub dropped_kinds: Vec<String>,
}

impl Default for ActivitySpanAttributes {
    fn default() -> Self {
        Self {
            scope_id: None,
            parent_scope_id: None,
            turn: None,
            call_id: None,
            provider: None,
            model: None,
            attempt: None,
            tool_name: None,
            time_to_first_token_ms: None,
            stop_reason: None,
            usage: UsageV1 {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            retry_count: 0,
            retry_delay_ms: 0,
            dropped_events: 0,
            dropped_bytes: 0,
            dropped_kinds: Vec::new(),
        }
    }
}

/// Immutable information delivered when a canonical boundary opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivitySpanStart {
    pub run_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub kind: ActivitySpanKind,
    pub started_at: String,
    pub assignment: AgentAssignmentIdentityV1,
    pub agent_session_id: Option<String>,
    pub remote_parent: Option<W3cTraceContext>,
    pub attributes: ActivitySpanAttributes,
}

/// Completed, privacy-safe span projected only from canonical event metadata.
///
/// Transcript text, tool arguments/results, headers, credentials, and process
/// environment values have no field in this type and therefore cannot become
/// span attributes accidentally.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedActivitySpan {
    pub start: ActivitySpanStart,
    pub ended_at: String,
    pub duration_ms: u64,
    pub status: ActivitySpanStatus,
    pub attributes: ActivitySpanAttributes,
}

/// Non-failing destination for canonical span projections.
pub trait ActivitySpanExporter: Send + Sync + 'static {
    fn span_started(&self, _span: &ActivitySpanStart) {}
    fn span_finished(&self, span: ProjectedActivitySpan);
}

/// Deterministic exporter used by protocol and recovery tests.
#[derive(Clone, Default)]
pub struct InMemoryActivitySpanExporter {
    spans: Arc<Mutex<Vec<ProjectedActivitySpan>>>,
}

impl InMemoryActivitySpanExporter {
    pub fn finished_spans(&self) -> Vec<ProjectedActivitySpan> {
        self.spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ActivitySpanExporter for InMemoryActivitySpanExporter {
    fn span_finished(&self, span: ProjectedActivitySpan) {
        self.spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(span);
    }
}
