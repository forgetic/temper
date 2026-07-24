//! Workflow metadata blocks embedded in Forge artifact bodies.
//!
//! Labels are the public Forge projection of workflow state, but portable Forge
//! fields cannot hold every relation, idempotency key, and claim lease. The
//! workflow layer stores that information in an embedded metadata block.
//!
//! # Format choice
//!
//! The block is JSON wrapped in an HTML comment. Its opening marker is exposed
//! as `temper_workflow::METADATA_BEGIN`:
//!
//! ```text
//! temper_workflow::METADATA_BEGIN
//! {
//!   "kind": "code",
//!   "parents": [12],
//!   "dependencies": [34],
//!   "correlation_key": "code-issue-42",
//!   "target_branch": "feature/144-plan-branch",
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
//!
//! Cross-repository fan-out uses globally unique correlation keys derived only
//! from the parent artifact and child intent. [`global_child_correlation_key`]
//! encodes the stable parent repository id, parent item number, and a
//! caller-chosen child slug with length prefixes, so re-running the same intent
//! recomputes the same key without delimiter collisions.

use crate::artifact::ArtifactRef;
use crate::context::TransitionCompletionAudit;
use crate::ids::{ArtifactKindId, RoleId};
use crate::interrupted_ci::InterruptedCiRecoveryState;
use crate::missing_ci::MissingCiRecoveryState;

mod assignment;
pub use assignment::DurableAssignment;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use temper_forge::{ItemNumber, RepositoryId, UserId};

/// Marker that opens a workflow metadata block.
pub const METADATA_BEGIN: &str = "<!-- temper:workflow";

/// Marker that closes a workflow metadata block.
pub const METADATA_END: &str = "-->";

/// Metadata keys that a workflow effect may require from an authored product.
///
/// Keeping this vocabulary typed prevents a workflow from advertising a
/// contract that the authoritative metadata parser cannot enforce.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMetadataKey {
    TargetBranch,
}

impl WorkflowMetadataKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetBranch => "target_branch",
        }
    }
}

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
    /// Non-empty branch name source work should use as the implementation PR
    /// target/base branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    /// Active claim lease, if the artifact is currently claimed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
    /// Durable assignment identity written before an `Assign` message is
    /// published. Older metadata blocks omit this field and continue to parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<DurableAssignment>,
    /// PR head produced by the most recently published in-place repair. While
    /// CI for this exact SHA is absent or pending, stale CI from an earlier head
    /// must not make the pull request eligible to land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repaired_head: Option<String>,
    /// Durable marker distinguishing interrupted missing-CI parking from unrelated attention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_ci_recovery: Option<MissingCiRecoveryState>,
    /// Exact-attempt interrupted-CI recovery progress. This identity never
    /// overlaps `missing_ci_recovery`, which has no visible provider attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_ci_recovery: Option<InterruptedCiRecoveryState>,
    /// True while a newly-created child is deliberately hidden from every
    /// dispatch scan. Activation clears this only after the complete sibling
    /// relation graph has been written.
    #[serde(default, skip_serializing_if = "is_false")]
    pub staged: bool,
    /// Durable multi-child creation records owned by this source artifact.
    /// The map key includes transition/effect/correlation identity, allowing a
    /// restarted process to finish fan-out without the worker result.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub create_issue_intents: BTreeMap<String, CreateIssuesIntent>,
}

impl WorkflowMetadata {
    /// Returns `true` when no metadata field is populated.
    pub fn is_empty(&self) -> bool {
        self == &WorkflowMetadata::default()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A restart-safe description of one `create_issues` effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssuesIntent {
    pub transition: String,
    pub effect_index: usize,
    pub correlation_key: String,
    #[serde(default)]
    pub record_parent_dependencies: bool,
    #[serde(default)]
    pub children: Vec<CreateIssueIntentChild>,
    /// Source-artifact mutation that commits the routed transition after every
    /// child is wired and activated. Legacy intents omit this field; recovery
    /// still finishes their boolean progress without inventing a transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CreateIssuesCompletion>,
    #[serde(default)]
    pub parent_wired: bool,
    #[serde(default)]
    pub completed: bool,
}

/// Durable source-artifact update committed together with fan-out completion.
///
/// Bodies are hex encoded for the same reason as child bodies: an authored body
/// can contain the HTML-comment terminator and must not truncate the intent
/// embedded in the source artifact's metadata block.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssuesCompletion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_assignees: Vec<UserId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_assignees: Vec<UserId>,
    /// Runtime-bound comment that must be published after child activation and
    /// before this source update commits. Older intents omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_audit: Option<TransitionCompletionAudit>,
}

/// Persisted normalized input and progress for one intended child issue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueIntentChild {
    pub slug: String,
    pub title: String,
    /// Hex-encoded UTF-8 body. Encoding is required because a child body may
    /// itself contain the `-->` workflow-comment terminator; embedding that
    /// sequence in the parent's HTML comment would truncate the durable intent.
    pub body_hex: String,
    #[serde(default)]
    pub final_labels: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub repository_id: RepositoryId,
    pub correlation_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<ItemNumber>,
    #[serde(default)]
    pub wired: bool,
    #[serde(default)]
    pub activated: bool,
}

/// Builds the canonical cross-repository child correlation key.
///
/// The key is stable for a parent artifact plus child intent and unique across
/// repositories. `child_slug` is chosen by the planner/agent for one intended
/// child, such as `api-schema` or `web-client`. Length prefixes make the format
/// collision-free even if repository ids or slugs contain separators.
pub fn global_child_correlation_key(
    parent_repo: &RepositoryId,
    parent_number: ItemNumber,
    child_slug: &str,
) -> String {
    format!(
        "parent-repo:{}:{}#parent:{}/child:{}:{}",
        parent_repo.as_str().len(),
        parent_repo.as_str(),
        parent_number.get(),
        child_slug.len(),
        child_slug
    )
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
    /// More than one managed metadata block was found in the body.
    DuplicateBlocks { count: usize },
    /// A metadata block opened but was never closed with `-->`.
    Unterminated,
    /// The metadata block contained invalid JSON.
    InvalidJson(String),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataError::DuplicateBlocks { count } => write!(
                formatter,
                "artifact body contained {count} managed workflow metadata blocks; expected at \
                 most one; remove the duplicate blocks before retrying"
            ),
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

/// Exact byte span occupied by one real managed metadata block.
///
/// The start is inclusive and the end is exclusive. Callers can use these
/// offsets to sanitize or canonicalize managed blocks without changing any
/// authored bytes around them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetadataBlockSpan {
    start: usize,
    end: usize,
}

impl MetadataBlockSpan {
    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }
}

/// Structural inspection of all real managed metadata blocks in a body.
///
/// Inline and fenced examples are excluded. Inspection deliberately does not
/// parse block JSON, allowing downstream sanitizers to identify the exact spans
/// of malformed as well as valid managed blocks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetadataBlockInspection {
    blocks: Vec<MetadataBlockSpan>,
}

impl MetadataBlockInspection {
    pub fn blocks(&self) -> &[MetadataBlockSpan] {
        &self.blocks
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

mod inspection;

/// Inspects every real managed workflow metadata block in an artifact body.
///
/// Markers in inline or fenced code remain authored examples and are omitted.
/// An opening marker without a closing `-->` returns [`MetadataError::Unterminated`].
/// Valid and malformed JSON blocks are both included because this function only
/// identifies structural managed-block boundaries.
pub fn inspect_metadata_blocks(body: &str) -> Result<MetadataBlockInspection, MetadataError> {
    let blocks = inspection::metadata_block_spans(body)?;
    Ok(MetadataBlockInspection { blocks })
}

fn locate_metadata_block(body: &str) -> Result<Option<MetadataBlockSpan>, MetadataError> {
    let inspection = inspect_metadata_blocks(body)?;
    match inspection.blocks.as_slice() {
        [] => Ok(None),
        [span] => Ok(Some(*span)),
        blocks => Err(MetadataError::DuplicateBlocks {
            count: blocks.len(),
        }),
    }
}

fn parse_metadata_in_span(
    body: &str,
    span: MetadataBlockSpan,
) -> Result<WorkflowMetadata, MetadataError> {
    let json = body[span.start + METADATA_BEGIN.len()..span.end - METADATA_END.len()].trim();
    serde_json::from_str(json).map_err(|err| MetadataError::InvalidJson(err.to_string()))
}

/// Parses the sole managed workflow metadata block found in an artifact body.
///
/// Returns `Ok(None)` when the body contains no managed block, `Ok(Some(_))`
/// when one block parses, and `Err(_)` when a real block is malformed,
/// unterminated, or duplicated. Surrounding prose is ignored. Occurrences in
/// inline and fenced code are authored examples and do not count as managed
/// blocks.
pub fn parse_metadata_block(body: &str) -> Result<Option<WorkflowMetadata>, MetadataError> {
    let Some(span) = locate_metadata_block(body)? else {
        return Ok(None);
    };
    parse_metadata_in_span(body, span).map(Some)
}

/// Separates exact authored body bytes from managed workflow metadata.
///
/// A valid managed block is removed in place by concatenating its unmodified
/// prefix and suffix. When no managed block exists, the returned body equals the
/// input exactly and metadata is `None`. Malformed, unterminated, or duplicate
/// real blocks return the same diagnostics as [`parse_metadata_block`] and are
/// never removed.
pub fn split_metadata_block(
    body: &str,
) -> Result<(String, Option<WorkflowMetadata>), MetadataError> {
    let Some(span) = locate_metadata_block(body)? else {
        return Ok((body.to_string(), None));
    };
    let metadata = parse_metadata_in_span(body, span)?;
    Ok((
        format!("{}{}", &body[..span.start], &body[span.end..]),
        Some(metadata),
    ))
}

/// Returns whether two complete artifact bodies differ only in lease-heartbeat
/// expiry fields.
///
/// This comparison is intentionally strict enough for webhook suppression. Both
/// bodies must contain valid workflow metadata, all prose outside the metadata
/// block must be byte-for-byte identical, and every metadata field must compare
/// equal after normalizing `lease.heartbeat_at`, `lease.expires_at`, and
/// `assignment.expires_at`. At least one of those three values must have
/// changed. Missing, malformed, unterminated, or duplicate metadata is never
/// classified as a heartbeat.
pub fn is_heartbeat_only_body_change(old_body: &str, new_body: &str) -> bool {
    let Ok((old_prose, Some(old_metadata))) = split_metadata_block(old_body) else {
        return false;
    };
    let Ok((new_prose, Some(mut normalized_new))) = split_metadata_block(new_body) else {
        return false;
    };
    if old_prose != new_prose
        || old_metadata.lease.is_some() != normalized_new.lease.is_some()
        || old_metadata.assignment.is_some() != normalized_new.assignment.is_some()
    {
        return false;
    }

    let lease_changed = old_metadata
        .lease
        .as_ref()
        .zip(normalized_new.lease.as_ref())
        .is_some_and(|(old, new)| {
            old.heartbeat_at != new.heartbeat_at || old.expires_at != new.expires_at
        });
    let assignment_changed = old_metadata
        .assignment
        .as_ref()
        .zip(normalized_new.assignment.as_ref())
        .is_some_and(|(old, new)| old.expires_at != new.expires_at);
    if !lease_changed && !assignment_changed {
        return false;
    }

    if let (Some(old), Some(new)) = (&old_metadata.lease, &mut normalized_new.lease) {
        new.heartbeat_at = old.heartbeat_at;
        new.expires_at = old.expires_at;
    }
    if let (Some(old), Some(new)) = (&old_metadata.assignment, &mut normalized_new.assignment) {
        new.expires_at = old.expires_at;
    }

    normalized_new == old_metadata
}

/// Returns `body` with its workflow metadata block set to `metadata`.
///
/// If the body already contains a block, it is replaced in place so surrounding
/// prose is preserved; otherwise a fresh block is appended (separated by a blank
/// line when the body is non-empty). The result round-trips through
/// [`parse_metadata_block`]. Malformed, unterminated, or duplicate existing
/// blocks are errors rather than being silently overwritten, so ambiguous or
/// authored-visible diagnostic content is surfaced, not clobbered.
pub fn replace_metadata_block(
    body: &str,
    metadata: &WorkflowMetadata,
) -> Result<String, MetadataError> {
    let block = render_metadata_block(metadata);
    match locate_metadata_block(body)? {
        Some(span) => {
            parse_metadata_in_span(body, span)?;
            Ok(format!(
                "{}{}{}",
                &body[..span.start],
                block,
                &body[span.end..]
            ))
        }
        None if body.is_empty() => Ok(block),
        None => Ok(format!("{body}\n\n{block}")),
    }
}
