//! Content-free structured evidence for the codebase-memory lifecycle.

use std::time::Duration;

use crate::mcp::McpError;

const DISCOVERY_EVENT: &str = "codebase_memory.discovery.completed";
const IDENTITY_EVENT: &str = "codebase_memory.identity.selected";
const INDEX_EVENT: &str = "codebase_memory.index.lifecycle";
const READINESS_EVENT: &str = "codebase_memory.readiness.wait";
const MAX_IDENTIFIER_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FailureCategory {
    None,
    Timeout,
    Cancelled,
    Provider,
    Protocol,
    Process,
    Internal,
}

impl FailureCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Provider => "provider_error",
            Self::Protocol => "protocol_error",
            Self::Process => "process_error",
            Self::Internal => "internal_error",
        }
    }

    fn safe_message(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Timeout => "operation timed out",
            Self::Cancelled => "operation was cancelled",
            Self::Provider => "provider operation failed",
            Self::Protocol => "provider protocol was invalid",
            Self::Process => "provider process was unavailable",
            Self::Internal => "internal lifecycle operation failed",
        }
    }
}

impl From<&McpError> for FailureCategory {
    fn from(error: &McpError) -> Self {
        match error {
            McpError::Timeout { .. } => Self::Timeout,
            McpError::Cancelled { .. } => Self::Cancelled,
            McpError::Rpc { .. } => Self::Provider,
            McpError::Protocol(_) | McpError::ProtocolOverflow { .. } | McpError::Json { .. } => {
                Self::Protocol
            }
            McpError::Spawn { .. } | McpError::Io { .. } | McpError::ProcessExited { .. } => {
                Self::Process
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiscoveryOutcome {
    Success,
    Timeout,
    Failure,
}

impl DiscoveryOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::Failure => "failure",
        }
    }
}

pub(super) struct DiscoveryEvidence {
    pub method: &'static str,
    pub inventory: &'static str,
    pub duration: Duration,
    pub outcome: DiscoveryOutcome,
    pub record_count: usize,
    pub cache_bytes: Option<u64>,
    pub failure: FailureCategory,
}

pub(super) fn emit_discovery(evidence: DiscoveryEvidence) {
    let duration_ms = duration_ms(evidence.duration);
    let record_count = u64::try_from(evidence.record_count).unwrap_or(u64::MAX);
    let outcome = evidence.outcome.as_str();
    let timed_out = evidence.outcome == DiscoveryOutcome::Timeout;
    let targeted = evidence.inventory == "targeted";
    let cache_bytes_available = evidence.cache_bytes.is_some();
    let cache_bytes = evidence.cache_bytes.unwrap_or_default();
    let failure_category = evidence.failure.as_str();
    let failure_message = evidence.failure.safe_message();
    if evidence.outcome == DiscoveryOutcome::Success {
        tracing::debug!(
            target: "temper::agent",
            service = "agent",
            event = DISCOVERY_EVENT,
            discovery.method = evidence.method,
            discovery.inventory = evidence.inventory,
            discovery.targeted = targeted,
            duration_ms,
            outcome,
            timed_out,
            record_count,
            cache.bytes_available = cache_bytes_available,
            cache.bytes = cache_bytes,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory discovery completed",
        );
    } else {
        tracing::warn!(
            target: "temper::agent",
            service = "agent",
            event = DISCOVERY_EVENT,
            discovery.method = evidence.method,
            discovery.inventory = evidence.inventory,
            discovery.targeted = targeted,
            duration_ms,
            outcome,
            timed_out,
            record_count,
            cache.bytes_available = cache_bytes_available,
            cache.bytes = cache_bytes,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory discovery did not complete",
        );
    }
}

pub(super) fn emit_identity_selected(logical: &str, provider: &str, outcome: &'static str) {
    let logical = safe_identifier(logical);
    let provider = safe_identifier(provider);
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = IDENTITY_EVENT,
        identity.logical = logical.as_str(),
        identity.provider = provider.as_str(),
        identity.outcome = outcome,
        "agent:   codebase-memory stable identity selected",
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IndexOutcome {
    Requested,
    Started,
    Reused,
    SuppressedDuplicate,
    Completed,
    Failed,
    SkippedDiscoveryUnknown,
    Disabled,
}

impl IndexOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Started => "started",
            Self::Reused => "reused",
            Self::SuppressedDuplicate => "suppressed_duplicate",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::SkippedDiscoveryUnknown => "skipped_discovery_unknown",
            Self::Disabled => "disabled",
        }
    }
}

pub(super) fn emit_index(
    logical: &str,
    provider: &str,
    mode: &'static str,
    outcome: IndexOutcome,
    failure: FailureCategory,
) {
    let logical = safe_identifier(logical);
    let provider = safe_identifier(provider);
    let outcome_value = outcome.as_str();
    let failure_category = failure.as_str();
    let failure_message = failure.safe_message();
    if outcome == IndexOutcome::Failed {
        tracing::warn!(
            target: "temper::agent",
            service = "agent",
            event = INDEX_EVENT,
            identity.logical = logical.as_str(),
            identity.provider = provider.as_str(),
            index.mode = mode,
            index.outcome = outcome_value,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory index operation failed",
        );
    } else {
        tracing::debug!(
            target: "temper::agent",
            service = "agent",
            event = INDEX_EVENT,
            identity.logical = logical.as_str(),
            identity.provider = provider.as_str(),
            index.mode = mode,
            index.outcome = outcome_value,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory index lifecycle changed",
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReadinessOutcome {
    Success,
    Timeout,
    Failure,
}

pub(super) fn emit_readiness(
    provider: &str,
    duration: Duration,
    outcome: ReadinessOutcome,
    failure: FailureCategory,
) {
    let provider = safe_identifier(provider);
    let duration_ms = duration_ms(duration);
    let outcome_value = match outcome {
        ReadinessOutcome::Success => "success",
        ReadinessOutcome::Timeout => "timeout",
        ReadinessOutcome::Failure => "failure",
    };
    let timed_out = outcome == ReadinessOutcome::Timeout;
    let failure_category = failure.as_str();
    let failure_message = failure.safe_message();
    if outcome == ReadinessOutcome::Success {
        tracing::debug!(
            target: "temper::agent",
            service = "agent",
            event = READINESS_EVENT,
            identity.provider = provider.as_str(),
            duration_ms,
            outcome = outcome_value,
            timed_out,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory background readiness wait completed",
        );
    } else {
        tracing::warn!(
            target: "temper::agent",
            service = "agent",
            event = READINESS_EVENT,
            identity.provider = provider.as_str(),
            duration_ms,
            outcome = outcome_value,
            timed_out,
            failure.category = failure_category,
            failure.message = failure_message,
            "agent:   codebase-memory background readiness wait failed",
        );
    }
}

fn safe_identifier(value: &str) -> String {
    let safe = !value.is_empty()
        && !value.starts_with('/')
        && !value.starts_with('~')
        && !value.contains('\\')
        && !value.contains("..")
        && value.chars().count() <= MAX_IDENTIFIER_CHARS
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.:/@".contains(character));
    if safe {
        value.to_string()
    } else {
        "<redacted-identifier>".to_string()
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
