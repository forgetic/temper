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

mod model;
mod validation;

#[cfg(test)]
mod tests;

pub use model::*;
pub use validation::{
    ActivityValidationCode, ActivityValidationError, validate_acknowledgement, validate_batch,
    validate_blob_attachment, validate_blob_reference, validate_capture_policy, validate_frame,
    validate_run_event, validate_run_stream, validate_scope_ancestry,
};

/// Current agent activity wire-contract version.
pub const ACTIVITY_PROTOCOL_VERSION: u32 = 1;

/// Absolute wire limit for any inline captured value.
pub const MAX_INLINE_CONTENT_BYTES: usize = 16 * 1024;
/// Absolute wire limit for a single transported blob attachment.
pub const MAX_BLOB_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Limit for opaque identifiers and short provider/tool labels.
pub const MAX_IDENTIFIER_BYTES: usize = 256;
