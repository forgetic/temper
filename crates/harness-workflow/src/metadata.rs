//! Workflow metadata blocks embedded in Forge artifact bodies.
//!
//! Labels are the public Forge projection of workflow state, but some workflow
//! information has no portable Forge field: the artifact's workflow kind,
//! parent/produced-PR links, fallback dependency links, idempotency correlation
//! keys, and claim leases. The workflow layer stores that information in a machine-readable
//! metadata block embedded in an issue or pull-request body.
//!
//! # Format choice
//!
//! The block is JSON wrapped in an HTML comment:
//!
//! ```text
//! <!-- harness:workflow
//! {
//!   "kind": "code",
//!   "parents": [12],
//!   "dependencies": [34],
//!   "correlation_key": "code-issue-42",
//!   "lease": { ... }
//! }
//! -->
//! ```
//!
//! JSON inside an HTML comment is used deliberately:
//!
//! - it renders invisibly in Forge markdown, so the public body stays readable;
//! - JSON needs no extra dependency beyond `serde_json`, which the crate already
//!   uses, so no YAML or TOML parser is pulled in;
//! - serialization is deterministic because field order follows the struct
//!   declaration order, which makes render/parse round-trips easy to test.
//!
//! The block ends at the first `-->`, so metadata values must not contain that
//! sequence. The current fields cannot, so this limitation is acceptable.
//!
//! Metadata relations ([`WorkflowMetadata::parents`] and fallback
//! [`WorkflowMetadata::dependencies`]) are stored as same-repository Forge item
//! numbers by default. New metadata may use `{ "repository_id": "...",
//! "number": 34 }` objects to point at another repository.

use crate::artifact::ArtifactRef;
use crate::ids::{ArtifactKindId, RoleId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// Marker that opens a workflow metadata block.
pub const METADATA_BEGIN: &str = "<!-- harness:workflow";

/// Marker that closes a workflow metadata block.
pub const METADATA_END: &str = "-->";

/// Machine-readable workflow metadata embedded in a Forge artifact body.
///
/// Every field is optional so a partially populated block still parses. An
/// empty value serializes to `{}` and round-trips back to the default.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowMetadata {
    /// Authoritative workflow artifact kind for this Forge artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKindId>,
    /// Parent artifacts. Bare numbers mean the same repository as the source;
    /// object values may name an explicit target repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<ArtifactRef>,
    /// Fallback dependency artifacts that must land first. Bare numbers mean the
    /// same repository as the source; object values may name another repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<ArtifactRef>,
    /// Idempotency key used to avoid creating duplicate artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_key: Option<String>,
    /// Active claim lease, if the artifact is currently claimed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
}

impl WorkflowMetadata {
    /// Returns `true` when no metadata field is populated.
    pub fn is_empty(&self) -> bool {
        self == &WorkflowMetadata::default()
    }
}

/// A claim lease, recording who holds an artifact and until when.
///
/// A claim is a lease, not permanent ownership. The reconciler uses
/// [`Lease::is_expired`] to detect abandoned work and apply recovery policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    /// Role that holds the lease.
    pub role: RoleId,
    /// Worker or run identifier that claimed the artifact.
    pub worker: String,
    /// When the artifact was claimed.
    pub claimed_at: DateTime<Utc>,
    /// Most recent heartbeat from the worker.
    pub heartbeat_at: DateTime<Utc>,
    /// When the lease expires if no further heartbeat arrives.
    pub expires_at: DateTime<Utc>,
}

impl Lease {
    /// Returns `true` when the lease has expired at `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

/// Error returned when a metadata block is present but cannot be parsed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetadataError {
    /// A metadata block opened but was never closed with `-->`.
    Unterminated,
    /// The metadata block contained invalid JSON.
    InvalidJson(String),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataError::Unterminated => {
                formatter.write_str("workflow metadata block was not terminated with `-->`")
            }
            MetadataError::InvalidJson(reason) => {
                write!(
                    formatter,
                    "workflow metadata block contained invalid JSON: {reason}"
                )
            }
        }
    }
}

impl Error for MetadataError {}

/// Renders a metadata block as JSON wrapped in an HTML comment.
///
/// The output is deterministic: JSON keys follow the struct declaration order
/// and empty fields are omitted. The result round-trips through
/// [`parse_metadata_block`].
pub fn render_metadata_block(metadata: &WorkflowMetadata) -> String {
    let json =
        serde_json::to_string_pretty(metadata).expect("WorkflowMetadata always serializes to JSON");
    format!("{METADATA_BEGIN}\n{json}\n{METADATA_END}")
}

/// Locates the byte span of the first metadata block in a body.
///
/// Returns `Ok(Some((start, end)))` with the inclusive-start, exclusive-end
/// byte offsets of the whole `<!-- ... -->` block when one is present and
/// terminated, `Ok(None)` when no block opens, and `Err(Unterminated)` when a
/// block opens but never closes. Shared by [`parse_metadata_block`] and
/// [`replace_metadata_block`] so both agree on where a block starts and ends.
fn block_span(body: &str) -> Result<Option<(usize, usize)>, MetadataError> {
    let Some(start) = body.find(METADATA_BEGIN) else {
        return Ok(None);
    };
    let after = &body[start + METADATA_BEGIN.len()..];
    let Some(end) = after.find(METADATA_END) else {
        return Err(MetadataError::Unterminated);
    };
    let block_end = start + METADATA_BEGIN.len() + end + METADATA_END.len();
    Ok(Some((start, block_end)))
}

/// Parses the first workflow metadata block found in an artifact body.
///
/// Returns `Ok(None)` when the body contains no block at all, `Ok(Some(_))`
/// when a block parses, and `Err(_)` when a block is present but malformed.
/// Surrounding prose is ignored, so a block can be embedded among human text.
pub fn parse_metadata_block(body: &str) -> Result<Option<WorkflowMetadata>, MetadataError> {
    let Some((start, block_end)) = block_span(body)? else {
        return Ok(None);
    };
    let json = body[start + METADATA_BEGIN.len()..block_end - METADATA_END.len()].trim();
    let metadata =
        serde_json::from_str(json).map_err(|err| MetadataError::InvalidJson(err.to_string()))?;
    Ok(Some(metadata))
}

/// Returns `body` with its workflow metadata block set to `metadata`.
///
/// If the body already contains a block, it is replaced in place so surrounding
/// prose is preserved; otherwise a fresh block is appended (separated by a blank
/// line when the body is non-empty). The result round-trips through
/// [`parse_metadata_block`]. An unterminated existing block is an error rather
/// than being silently overwritten, so malformed bodies are surfaced, not
/// clobbered.
pub fn replace_metadata_block(
    body: &str,
    metadata: &WorkflowMetadata,
) -> Result<String, MetadataError> {
    let block = render_metadata_block(metadata);
    match block_span(body)? {
        Some((start, block_end)) => {
            Ok(format!("{}{}{}", &body[..start], block, &body[block_end..]))
        }
        None if body.is_empty() => Ok(block),
        None => Ok(format!("{body}\n\n{block}")),
    }
}
