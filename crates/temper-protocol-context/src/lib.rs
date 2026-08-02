//! Versioned, serialization-only contracts for artifact graph context.
//!
//! This crate deliberately contains no forge or runtime types. Producers can
//! build a bundle from any forge, while worker and agent protocols can carry it
//! without coupling their independently-versioned wire contracts.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Current [`ArtifactContextBundle`] schema version.
pub const ARTIFACT_CONTEXT_VERSION: u32 = 1;

/// Returns whether `version` is supported by this crate.
pub const fn is_supported_artifact_context_version(version: u32) -> bool {
    version == ARTIFACT_CONTEXT_VERSION
}

/// A versioned, explicitly categorized artifact-context bundle.
///
/// The coordinating artifact is always identified by [`Self::primary`]. Its
/// identity must never be inferred from a vector position. Mandatory ancestors
/// and the two body-omitted summary scopes are separate so consumers do not
/// need to reverse-engineer semantics from relation direction or ordering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactContextBundle {
    /// Artifact-context schema version, independent of worker/agent versions.
    pub version: u32,
    /// Repository containing the coordinating artifact.
    pub repository: ArtifactRepository,
    /// Type of the coordinating artifact.
    pub artifact_type: ArtifactType,
    /// Full snapshot of the coordinating artifact.
    pub primary: ArtifactSnapshot,
    /// Mandatory ancestors, ordered root-to-leaf and excluding `primary`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage: Vec<ArtifactSnapshot>,
    /// Declared validation dependencies and verified implementation pull requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_scope: Vec<ArtifactSummary>,
    /// Incidental artifacts discovered from Markdown references only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_references: Vec<ArtifactSummary>,
    /// Non-fatal collection failures and policy decisions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ArtifactContextDiagnostic>,
    /// Explicit limits reached while constructing this bundle.
    #[serde(default)]
    pub truncation: ArtifactContextTruncation,
}

impl ArtifactContextBundle {
    /// Creates a complete v1 bundle around an explicit coordinating artifact.
    pub fn new(primary: ArtifactSnapshot) -> Self {
        Self {
            version: ARTIFACT_CONTEXT_VERSION,
            repository: primary.artifact.repository.clone(),
            artifact_type: primary.artifact.artifact_type,
            primary,
            lineage: Vec::new(),
            validation_scope: Vec::new(),
            optional_references: Vec::new(),
            diagnostics: Vec::new(),
            truncation: ArtifactContextTruncation::default(),
        }
    }
}

/// Stable repository identity plus its human-facing `owner/name` path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRepository {
    pub id: String,
    pub path: String,
}

/// Portable artifact vocabulary used by the context graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Issue,
    PullRequest,
}

/// Repository-scoped identity of an artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub repository: ArtifactRepository,
    pub artifact_type: ArtifactType,
    pub number: u64,
}

/// Full immutable artifact content captured during context collection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSnapshot {
    pub artifact: ArtifactReference,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    pub state: String,
    /// Workflow artifact kind when classification succeeded.
    ///
    /// Retained for compatibility with context bundles produced before the
    /// compact [`Self::workflow`] projection was introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_kind: Option<String>,
    /// Compact, protocol-owned projection of useful workflow context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<ArtifactWorkflowContext>,
}

/// Compact workflow context safe to carry across protocol boundaries.
///
/// This deliberately models only authored-context relations and persisted
/// child identities. It has no extension payload capable of carrying workflow
/// runtime bookkeeping or recursively embedded artifact bodies.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactWorkflowContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<WorkflowArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<WorkflowArtifactReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<WorkflowChildIdentity>,
}

/// Fully qualified, normalized workflow relation target.
///
/// Unlike the workflow metadata shorthand, `repository_id` is always present,
/// so same- and cross-repository references have one transport shape.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowArtifactReference {
    pub repository_id: String,
    pub number: u64,
}

/// Identity of a workflow child that already has a persisted artifact number.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowChildIdentity {
    pub repository_id: String,
    pub number: u64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Body-omitted record in one of the bundle's explicit summary scopes.
///
/// `source` is the artifact whose metadata or body exposed this relation. The
/// summary therefore remains self-describing after standalone sorting and
/// truncation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub artifact: ArtifactReference,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_kind: Option<String>,
    pub relation_type: ArtifactRelationType,
    pub source: ArtifactReference,
}

/// Compact index record for bounded on-demand graph navigation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactIndexEntry {
    pub artifact: ArtifactReference,
    pub title: String,
    pub state: String,
    /// Index in `snapshots` when full content is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_index: Option<usize>,
}

/// Stable relation vocabulary for artifact graph edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRelationType {
    Parent,
    Dependency,
    Related,
}

/// Directed relation, preserving both the source that exposed the edge and its target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRelation {
    pub relation_type: ArtifactRelationType,
    pub source: ArtifactReference,
    pub target: ArtifactReference,
}

/// Stable machine-readable diagnostic vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactContextDiagnosticCode {
    MissingArtifact,
    ClosedAncestor,
    MalformedMetadata,
    RepositoryNotAllowed,
    CycleDetected,
    DepthExceeded,
    CountExceeded,
    ContentTruncated,
    ForgeReadFailed,
}

/// A non-fatal context collection diagnostic and, when known, its source artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactContextDiagnostic {
    pub code: ArtifactContextDiagnosticCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ArtifactReference>,
}

/// Explicit dimensions along which a bundle was truncated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactContextTruncation {
    pub depth_exceeded: bool,
    pub count_exceeded: bool,
    pub content_truncated: bool,
}

impl ArtifactContextTruncation {
    pub const fn is_truncated(self) -> bool {
        self.depth_exceeded || self.count_exceeded || self.content_truncated
    }
}

/// A closed-vocabulary, read-only Forge context operation.
///
/// Transport layers add authentication and assignment identity around this DTO;
/// model callers can choose only one of these bounded read operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ForgeContextOperation {
    ForgeGetItem(ForgeGetItemOperation),
    ForgeListRelated(ForgeListRelatedOperation),
}

impl ForgeContextOperation {
    pub fn repository(&self) -> &str {
        match self {
            Self::ForgeGetItem(operation) => &operation.repo,
            Self::ForgeListRelated(operation) => &operation.repo,
        }
    }

    pub const fn number(&self) -> u64 {
        match self {
            Self::ForgeGetItem(operation) => operation.number,
            Self::ForgeListRelated(operation) => operation.number,
        }
    }
}

/// Arguments for `forge_get_item`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeGetItemOperation {
    pub repo: String,
    pub number: u64,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<ArtifactType>,
    #[serde(default)]
    pub include_comments: bool,
}

/// Arguments for `forge_list_related`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeListRelatedOperation {
    pub repo: String,
    pub number: u64,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<ArtifactType>,
    pub relations: Vec<ForgeRelationType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Relations understood by the bounded on-demand Forge graph reader.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeRelationType {
    Parent,
    Child,
    Dependency,
    Dependent,
    ProducedPr,
    BodyReference,
    ReferencedBy,
}

/// A portable, bounded Forge comment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeItemComment {
    pub id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Successful result of `forge_get_item`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeGetItemResult {
    pub item: ArtifactSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<ForgeItemComment>,
    #[serde(default)]
    pub truncation: ArtifactContextTruncation,
}

/// One typed edge returned by `forge_list_related`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeRelatedEdge {
    pub relation: ForgeRelationType,
    pub source: ArtifactReference,
    pub target: ArtifactReference,
}

/// Successful result of `forge_list_related`.
///
/// `items` excludes the requested root and is sorted by stable artifact
/// identity. Edges retain their semantic direction (for example child → parent).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeListRelatedResult {
    pub root: ArtifactReference,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ArtifactIndexEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<ForgeRelatedEdge>,
    #[serde(default)]
    pub truncation: ArtifactContextTruncation,
}

/// Transport-neutral successful Forge context result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ForgeContextResult {
    Item(ForgeGetItemResult),
    Related(ForgeListRelatedResult),
}

/// Stable public failures for Forge context reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeContextErrorCode {
    InvalidRequest,
    NotAuthorized,
    NotFound,
    ForgeUnavailable,
    LimitExceeded,
}

/// Optional W3C Trace Context propagated with one assignment.
///
/// This is transport metadata, not durable workstream identity. A later run in
/// the same workstream receives a fresh context and is linked by correlation
/// and agent-session identifiers instead of becoming a child of a multi-day
/// span.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct W3cTraceContext {
    pub traceparent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
}

impl W3cTraceContext {
    /// Validates the bounded W3C header values before they cross a trust boundary.
    pub fn validate(&self) -> Result<(), W3cTraceContextError> {
        validate_traceparent(&self.traceparent)?;
        if let Some(tracestate) = &self.tracestate {
            validate_tracestate(tracestate)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct W3cTraceContextError(&'static str);

impl std::fmt::Display for W3cTraceContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for W3cTraceContextError {}

fn validate_traceparent(value: &str) -> Result<(), W3cTraceContextError> {
    let mut parts = value.split('-');
    let version = parts.next();
    let trace_id = parts.next();
    let parent_id = parts.next();
    let flags = parts.next();
    if parts.next().is_some()
        || version.is_none_or(|part| !lower_hex(part, 2) || part == "ff")
        || trace_id.is_none_or(|part| !lower_hex(part, 32) || all_zero(part))
        || parent_id.is_none_or(|part| !lower_hex(part, 16) || all_zero(part))
        || flags.is_none_or(|part| !lower_hex(part, 2))
    {
        return Err(W3cTraceContextError(
            "traceparent is not canonical W3C trace context",
        ));
    }
    Ok(())
}

fn validate_tracestate(value: &str) -> Result<(), W3cTraceContextError> {
    if value.is_empty()
        || value.len() > 512
        || value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))
    {
        return Err(W3cTraceContextError(
            "tracestate is empty, oversized, or contains control characters",
        ));
    }

    let mut keys = BTreeSet::new();
    let members = value.split(',').collect::<Vec<_>>();
    if members.len() > 32
        || members.iter().any(|member| {
            let member = member.trim_matches(' ');
            let Some((key, member_value)) = member.split_once('=') else {
                return true;
            };
            !valid_tracestate_key(key) || !keys.insert(key) || !valid_tracestate_value(member_value)
        })
    {
        return Err(W3cTraceContextError(
            "tracestate contains an invalid or duplicate member",
        ));
    }
    Ok(())
}

fn valid_tracestate_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 256 {
        return false;
    }
    if let Some((tenant, system)) = key.split_once('@') {
        !system.contains('@')
            && valid_tracestate_key_part(tenant, true, 241)
            && valid_tracestate_key_part(system, false, 14)
    } else {
        valid_tracestate_key_part(key, false, 256)
    }
}

fn valid_tracestate_key_part(value: &str, digit_may_start: bool, max_len: usize) -> bool {
    if value.is_empty() || value.len() > max_len {
        return false;
    }
    let mut bytes = value.bytes();
    let first = bytes.next().expect("non-empty checked above");
    (first.is_ascii_lowercase() || (digit_may_start && first.is_ascii_digit()))
        && bytes.all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'*' | b'/')
        })
}

fn valid_tracestate_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.ends_with(' ')
        && value
            .bytes()
            .all(|byte| matches!(byte, 0x20..=0x2b | 0x2d..=0x3c | 0x3e..=0x7e))
}

fn lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn all_zero(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

#[cfg(test)]
mod tests;
