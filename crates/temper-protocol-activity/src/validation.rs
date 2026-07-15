use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::Engine as _;

use crate::{
    AgentActivityAcknowledgement, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentActivityFrameV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1,
    BlobAttachmentV1, BlobReferenceV1, CaptureModeV1, MAX_BLOB_ATTACHMENT_BYTES,
    MAX_INLINE_CONTENT_BYTES,
};

mod fields;
use fields::{assignment, event, identifier, scope, timestamp, version};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityValidationCode {
    UnsupportedVersion,
    EmptyIdentifier,
    IdentifierTooLong,
    SequenceZero,
    InvalidTimestamp,
    MalformedScope,
    HostOnlyEvent,
    MissingTurn,
    InvalidEvent,
    EmptyBatch,
    NonContiguousBatch,
    RunIdMismatch,
    AssignmentMismatch,
    SessionMismatch,
    NonMonotonicElapsed,
    OversizedInlineValue,
    InvalidBlobReference,
    BlobReferenceMismatch,
    InvalidCapturePolicy,
    InvalidTraceContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityValidationError {
    pub code: ActivityValidationCode,
    pub field: String,
    pub detail: String,
}

impl fmt::Display for ActivityValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field, self.detail)
    }
}

impl std::error::Error for ActivityValidationError {}

pub fn validate_frame(frame: &AgentActivityFrameV1) -> Result<(), ActivityValidationError> {
    version(frame.version, "frame.version")?;
    timestamp(&frame.occurred_at, "frame.occurred_at")?;
    scope(&frame.scope, "frame.scope")?;
    if frame.event.is_host_only() {
        return Err(error(
            ActivityValidationCode::HostOnlyEvent,
            "frame.event",
            "run boundary and terminal events are worker-generated",
        ));
    }
    event(&frame.event, frame.turn, "frame.event")
}

pub fn validate_run_event(event_value: &AgentRunEventV1) -> Result<(), ActivityValidationError> {
    version(event_value.version, "event.version")?;
    identifier(&event_value.run_id, "event.run_id")?;
    if event_value.seq == 0 {
        return Err(error(
            ActivityValidationCode::SequenceZero,
            "event.seq",
            "sequence numbers start at 1",
        ));
    }
    timestamp(&event_value.occurred_at, "event.occurred_at")?;
    assignment(&event_value.assignment)?;
    if let Some(trace_context) = &event_value.assignment.trace_context {
        trace_context.validate().map_err(|source| {
            error(
                ActivityValidationCode::InvalidTraceContext,
                "event.assignment.trace_context",
                source.to_string(),
            )
        })?;
    }
    if let Some(session_id) = &event_value.agent_session_id {
        identifier(session_id, "event.agent_session_id")?;
    }
    scope(&event_value.scope, "event.scope")?;
    if matches!(
        event_value.event,
        AgentActivityEventV1::RunStarted(_)
            | AgentActivityEventV1::RunFinished(_)
            | AgentActivityEventV1::RunFailed(_)
    ) && event_value.scope.kind != AgentScopeKindV1::Main
    {
        return Err(error(
            ActivityValidationCode::MalformedScope,
            "event.scope.kind",
            "run events must use the main scope",
        ));
    }
    event(&event_value.event, event_value.turn, "event.event")
}

/// Validates a complete run stream, including the requirement that sequence
/// assignment begins at 1 and every scope has a consistent path to one main
/// scope. Forwarding batches use [`validate_batch`] because a later batch can
/// legitimately begin above 1 or omit scopes established by an earlier batch.
pub fn validate_run_stream(events: &[AgentRunEventV1]) -> Result<(), ActivityValidationError> {
    if events.is_empty() {
        return Err(error(
            ActivityValidationCode::EmptyBatch,
            "events",
            "a complete run stream must contain at least one event",
        ));
    }
    if events[0].seq != 1 {
        return Err(error(
            ActivityValidationCode::NonContiguousBatch,
            "events[0].seq",
            "a complete run stream must start at sequence 1",
        ));
    }
    validate_event_sequence(events, &events[0].run_id, 1)?;
    let scopes = events
        .iter()
        .map(|event| event.scope.clone())
        .collect::<Vec<_>>();
    validate_scope_ancestry(&scopes)
}

/// Validates a complete set of run scopes.
///
/// Duplicate observations of the same scope are allowed only when kind and
/// parent are identical. Exactly one main scope must exist, and every sub-agent
/// parent chain must be present, acyclic, and terminate at that main scope.
pub fn validate_scope_ancestry(scopes: &[AgentScopeV1]) -> Result<(), ActivityValidationError> {
    if scopes.is_empty() {
        return Err(malformed_scope(
            "scopes",
            "scope ancestry must contain a main scope",
        ));
    }

    let mut by_id = BTreeMap::<&str, &AgentScopeV1>::new();
    for (index, scope_value) in scopes.iter().enumerate() {
        scope(scope_value, &format!("scopes[{index}]"))?;
        if let Some(existing) = by_id.insert(scope_value.id.as_str(), scope_value) {
            if existing != scope_value {
                return Err(malformed_scope(
                    format!("scopes[{index}]"),
                    "a scope ID cannot change kind or parent",
                ));
            }
        }
    }

    let main_count = by_id
        .values()
        .filter(|scope| scope.kind == AgentScopeKindV1::Main)
        .count();
    if main_count != 1 {
        return Err(malformed_scope(
            "scopes",
            "scope ancestry must contain exactly one main scope",
        ));
    }

    for scope_value in by_id.values() {
        let mut current = *scope_value;
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.id.as_str()) {
                return Err(malformed_scope(
                    "scopes",
                    format!("scope ancestry contains a cycle at {}", current.id),
                ));
            }
            let Some(parent_id) = current.parent_id.as_deref() else {
                break;
            };
            current = by_id.get(parent_id).copied().ok_or_else(|| {
                malformed_scope(
                    "scopes",
                    format!("scope {} references missing parent {parent_id}", current.id),
                )
            })?;
        }
    }
    Ok(())
}

pub fn validate_batch(batch: &AgentActivityBatch) -> Result<(), ActivityValidationError> {
    version(batch.version, "batch.version")?;
    identifier(&batch.run_id, "batch.run_id")?;
    if batch.first_seq == 0 {
        return Err(error(
            ActivityValidationCode::SequenceZero,
            "batch.first_seq",
            "sequence numbers start at 1",
        ));
    }
    if batch.events.is_empty() {
        return Err(error(
            ActivityValidationCode::EmptyBatch,
            "batch.events",
            "an activity batch must contain at least one event",
        ));
    }
    validate_event_sequence(&batch.events, &batch.run_id, batch.first_seq)?;
    validate_batch_blobs(batch)
}

pub fn validate_acknowledgement(
    acknowledgement: &AgentActivityAcknowledgement,
) -> Result<(), ActivityValidationError> {
    version(acknowledgement.version, "acknowledgement.version")?;
    identifier(&acknowledgement.run_id, "acknowledgement.run_id")?;
    if acknowledgement.highest_contiguous_seq == 0 {
        return Err(error(
            ActivityValidationCode::SequenceZero,
            "acknowledgement.highest_contiguous_seq",
            "an acknowledgement must durably accept at least sequence 1",
        ));
    }
    Ok(())
}

pub fn validate_capture_policy(
    policy: &AgentActivityCapturePolicyV1,
) -> Result<(), ActivityValidationError> {
    version(policy.version, "capture_policy.version")?;
    if policy.retention_days == 0 {
        return Err(policy_error(
            "capture_policy.retention_days",
            "must be greater than zero",
        ));
    }
    if policy.max_run_bytes == 0 {
        return Err(policy_error(
            "capture_policy.max_run_bytes",
            "must be greater than zero",
        ));
    }
    if policy.max_inline_bytes == 0
        || u64::from(policy.max_inline_bytes) > MAX_INLINE_CONTENT_BYTES as u64
    {
        return Err(policy_error(
            "capture_policy.max_inline_bytes",
            format!("must be between 1 and {MAX_INLINE_CONTENT_BYTES}"),
        ));
    }
    if policy.max_blob_bytes == 0 || policy.max_blob_bytes > MAX_BLOB_ATTACHMENT_BYTES as u64 {
        return Err(policy_error(
            "capture_policy.max_blob_bytes",
            format!("must be between 1 and {MAX_BLOB_ATTACHMENT_BYTES}"),
        ));
    }
    if policy.max_run_bytes < u64::from(policy.max_inline_bytes)
        || policy.max_run_bytes < policy.max_blob_bytes
    {
        return Err(policy_error(
            "capture_policy.max_run_bytes",
            "must be at least the inline and blob limits",
        ));
    }
    if policy.capture_thinking && policy.capture != CaptureModeV1::Diagnostic {
        return Err(policy_error(
            "capture_policy.capture_thinking",
            "thinking may be captured only in diagnostic mode",
        ));
    }
    Ok(())
}

pub fn validate_blob_reference(reference: &BlobReferenceV1) -> Result<(), ActivityValidationError> {
    if reference.bytes == 0 || reference.bytes > MAX_BLOB_ATTACHMENT_BYTES as u64 {
        return Err(error(
            ActivityValidationCode::InvalidBlobReference,
            "blob.bytes",
            format!("must be between 1 and {MAX_BLOB_ATTACHMENT_BYTES}"),
        ));
    }
    let digest = reference.digest.as_bytes();
    if digest.len() != 71
        || !reference.digest.starts_with("sha256:")
        || !digest[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(error(
            ActivityValidationCode::InvalidBlobReference,
            "blob.digest",
            "must be lowercase sha256 followed by 64 hexadecimal characters",
        ));
    }
    Ok(())
}

pub fn validate_blob_attachment(
    attachment: &BlobAttachmentV1,
) -> Result<(), ActivityValidationError> {
    validate_blob_reference(&attachment.blob)?;
    let decoded = decode_attachment(attachment)?;
    let actual = BlobReferenceV1::for_bytes(attachment.blob.media_type, &decoded);
    if actual != attachment.blob {
        return Err(error(
            ActivityValidationCode::BlobReferenceMismatch,
            "attachment.blob",
            "declared digest, byte count, or media type does not match attachment data",
        ));
    }
    Ok(())
}

pub(crate) fn decode_attachment(
    attachment: &BlobAttachmentV1,
) -> Result<Vec<u8>, ActivityValidationError> {
    let maximum_encoded_len = MAX_BLOB_ATTACHMENT_BYTES.div_ceil(3) * 4;
    if attachment.data_base64.len() > maximum_encoded_len {
        return Err(error(
            ActivityValidationCode::InvalidBlobReference,
            "attachment.data_base64",
            "encoded attachment exceeds the absolute blob limit",
        ));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&attachment.data_base64)
        .map_err(|decode_error| {
            error(
                ActivityValidationCode::InvalidBlobReference,
                "attachment.data_base64",
                format!("is not canonical base64: {decode_error}"),
            )
        })?;
    if decoded.len() > MAX_BLOB_ATTACHMENT_BYTES
        || base64::engine::general_purpose::STANDARD.encode(&decoded) != attachment.data_base64
    {
        return Err(error(
            ActivityValidationCode::InvalidBlobReference,
            "attachment.data_base64",
            "must be canonical base64 within the absolute blob limit",
        ));
    }
    Ok(decoded)
}

fn validate_event_sequence(
    events: &[AgentRunEventV1],
    run_id: &str,
    first_seq: u64,
) -> Result<(), ActivityValidationError> {
    let first = &events[0];
    let assignment = &first.assignment;
    let session_id = &first.agent_session_id;
    let mut previous_elapsed = None;
    let mut scopes = BTreeMap::<&str, &AgentScopeV1>::new();
    for (index, event_value) in events.iter().enumerate() {
        validate_run_event(event_value)?;
        let expected = first_seq.checked_add(index as u64).ok_or_else(|| {
            error(
                ActivityValidationCode::NonContiguousBatch,
                "events.seq",
                "sequence range overflows u64",
            )
        })?;
        if event_value.seq != expected {
            return Err(error(
                ActivityValidationCode::NonContiguousBatch,
                format!("events[{index}].seq"),
                format!("expected contiguous sequence {expected}"),
            ));
        }
        if event_value.run_id != run_id {
            return Err(error(
                ActivityValidationCode::RunIdMismatch,
                format!("events[{index}].run_id"),
                "does not match the enclosing run",
            ));
        }
        if &event_value.assignment != assignment {
            return Err(error(
                ActivityValidationCode::AssignmentMismatch,
                format!("events[{index}].assignment"),
                "assignment identity must be immutable within a run",
            ));
        }
        if &event_value.agent_session_id != session_id {
            return Err(error(
                ActivityValidationCode::SessionMismatch,
                format!("events[{index}].agent_session_id"),
                "agent session identity must be immutable within a run",
            ));
        }
        if previous_elapsed.is_some_and(|previous| event_value.elapsed_ms < previous) {
            return Err(error(
                ActivityValidationCode::NonMonotonicElapsed,
                format!("events[{index}].elapsed_ms"),
                "must not decrease as sequence numbers increase",
            ));
        }
        if let Some(existing) = scopes.insert(event_value.scope.id.as_str(), &event_value.scope) {
            if existing != &event_value.scope {
                return Err(malformed_scope(
                    format!("events[{index}].scope"),
                    "a scope ID cannot change kind or parent",
                ));
            }
        }
        previous_elapsed = Some(event_value.elapsed_ms);
    }
    Ok(())
}

fn validate_batch_blobs(batch: &AgentActivityBatch) -> Result<(), ActivityValidationError> {
    let mut references = BTreeMap::<&str, &BlobReferenceV1>::new();
    for event in &batch.events {
        for reference in event.event.content_references() {
            validate_blob_reference(reference)?;
            if references
                .insert(reference.digest.as_str(), reference)
                .is_some_and(|existing| existing != reference)
            {
                return Err(error(
                    ActivityValidationCode::BlobReferenceMismatch,
                    "batch.events",
                    "one digest is associated with conflicting blob metadata",
                ));
            }
        }
    }

    let mut attachments = BTreeMap::<&str, &BlobAttachmentV1>::new();
    for attachment in &batch.blobs {
        validate_blob_attachment(attachment)?;
        if attachments
            .insert(attachment.blob.digest.as_str(), attachment)
            .is_some()
        {
            return Err(error(
                ActivityValidationCode::BlobReferenceMismatch,
                "batch.blobs",
                "duplicate blob attachment",
            ));
        }
    }

    for (digest, reference) in &references {
        let Some(attachment) = attachments.get(digest) else {
            return Err(error(
                ActivityValidationCode::BlobReferenceMismatch,
                "batch.blobs",
                format!("missing attachment for {digest}"),
            ));
        };
        if &attachment.blob != *reference {
            return Err(error(
                ActivityValidationCode::BlobReferenceMismatch,
                "batch.blobs",
                format!("attachment metadata does not match reference {digest}"),
            ));
        }
    }
    if let Some(unreferenced) = attachments
        .keys()
        .find(|digest| !references.contains_key(**digest))
    {
        return Err(error(
            ActivityValidationCode::BlobReferenceMismatch,
            "batch.blobs",
            format!("unreferenced attachment {unreferenced}"),
        ));
    }
    Ok(())
}

fn policy_error(field: &str, detail: impl Into<String>) -> ActivityValidationError {
    error(ActivityValidationCode::InvalidCapturePolicy, field, detail)
}

fn malformed_scope(field: impl Into<String>, detail: impl Into<String>) -> ActivityValidationError {
    error(ActivityValidationCode::MalformedScope, field, detail)
}

pub(super) fn error(
    code: ActivityValidationCode,
    field: impl Into<String>,
    detail: impl Into<String>,
) -> ActivityValidationError {
    ActivityValidationError {
        code,
        field: field.into(),
        detail: detail.into(),
    }
}
