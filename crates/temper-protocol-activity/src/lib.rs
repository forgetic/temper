//! Shared agent activity protocol contracts.
//!
//! This crate owns only versioned serde data-transfer objects and pure
//! validation. It deliberately has no dependency on an agent, worker, engine,
//! web, process-protocol, logging, or runtime crate. In particular, stdout,
//! stderr, and `WorkspaceResult` are not activity transport channels.
//!
//! There are two trust domains:
//!
//! * [`AgentActivityFrameV1`] is emitted by an untrusted child. It carries only
//!   source timing, scope/turn context, and typed event data.
//! * [`AgentRunEventV1`] is stamped by the worker with run ordering and immutable
//!   assignment identity before it can enter the durable stream.

mod export;
mod model;
mod validation;

#[cfg(test)]
mod tests;

pub use export::*;
pub use model::*;
pub use temper_protocol_context::{W3cTraceContext, W3cTraceContextError};
pub use validation::{
    ActivityValidationCode, ActivityValidationError, validate_acknowledgement, validate_batch,
    validate_blob_attachment, validate_blob_reference, validate_capture_policy,
    validate_child_record, validate_frame, validate_run_event, validate_run_stream,
    validate_scope_ancestry,
};

/// CLI flag carrying a worker-written, non-secret capture policy JSON file to
/// the first-party agent process.
pub const TRACE_POLICY_FLAG: &str = "--trace-policy";
/// CLI flag carrying the worker-owned per-run loopback activity endpoint.
pub const ACTIVITY_ADDRESS_FLAG: &str = "--activity-address";

/// Current agent activity wire-contract version.
pub const ACTIVITY_PROTOCOL_VERSION: u32 = 1;

/// Fixed allowlisted summary for model-call retries.
///
/// Provider diagnostics are untrusted and must never enter the canonical
/// activity plane. Producers and trust boundaries normalize retry messages to
/// this value while retaining the typed failure code and retryability.
pub const MODEL_CALL_RETRY_FAILURE_MESSAGE: &str = "model call failed; retry scheduled";

/// Absolute wire limit for any inline captured value.
pub const MAX_INLINE_CONTENT_BYTES: usize = 16 * 1024;
/// Absolute wire limit for a single transported blob attachment.
pub const MAX_BLOB_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Absolute JSON wire limit for one attachment-bearing child record.
///
/// Canonical base64 expands a legal blob by at most 4/3; the fixed allowance
/// covers the bounded frame, blob reference, and JSON envelope. The trailing
/// newline used by the socket transport is not included.
pub const MAX_CHILD_ACTIVITY_RECORD_BYTES: usize =
    ((MAX_BLOB_ATTACHMENT_BYTES + 2) / 3) * 4 + 64 * 1024;
/// Limit for opaque identifiers and short provider/tool labels.
pub const MAX_IDENTIFIER_BYTES: usize = 256;
