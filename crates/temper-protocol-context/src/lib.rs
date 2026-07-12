//! Versioned, serialization-only contracts for artifact graph context.
//!
//! This crate deliberately contains no forge or runtime types. Producers can
//! build a bundle from any forge, while worker and agent protocols can carry it
//! without coupling their independently-versioned wire contracts.

use serde::{Deserialize, Serialize};

/// Current [`ArtifactContextBundle`] schema version.
pub const ARTIFACT_CONTEXT_VERSION: u32 = 1;

/// A versioned collection of artifact snapshots and their relationships.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactContextBundle {
    /// Artifact-context schema version, independent of worker/agent versions.
    pub version: u32,
    /// Repository containing the coordinating artifact.
    pub repository: ArtifactRepository,
    /// Type of the coordinating artifact.
    pub artifact_type: ArtifactType,
    /// Full artifact records selected for prompt context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<ArtifactSnapshot>,
    /// Compact records for discovered artifacts whose full content is omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index: Vec<ArtifactIndexEntry>,
    /// Directed edges describing how artifacts were discovered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ArtifactRelation>,
    /// Non-fatal collection failures and policy decisions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ArtifactContextDiagnostic>,
    /// Explicit limits reached while constructing this bundle.
    #[serde(default)]
    pub truncation: ArtifactContextTruncation,
}

impl ArtifactContextBundle {
    /// Creates an empty, complete v1 bundle for a coordinating artifact.
    pub fn new(repository: ArtifactRepository, artifact_type: ArtifactType) -> Self {
        Self {
            version: ARTIFACT_CONTEXT_VERSION,
            repository,
            artifact_type,
            snapshots: Vec::new(),
            index: Vec::new(),
            relations: Vec::new(),
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
}

/// Compact index record for graph navigation without full artifact content.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(name),
        )
        .expect("fixture is readable")
    }

    #[test]
    fn golden_bundles_round_trip() {
        for name in ["complete.json", "diagnostics-truncation.json"] {
            let json = fixture(name);
            let bundle: ArtifactContextBundle =
                serde_json::from_str(&json).expect("fixture parses");
            assert_eq!(bundle.version, ARTIFACT_CONTEXT_VERSION);
            let round_trip = serde_json::to_value(&bundle).expect("bundle serializes");
            let golden: serde_json::Value = serde_json::from_str(&json).expect("valid json");
            assert_eq!(round_trip, golden, "fixture {name} is canonical");
        }
    }

    #[test]
    fn diagnostic_codes_are_stable_snake_case() {
        let codes = [
            ArtifactContextDiagnosticCode::MissingArtifact,
            ArtifactContextDiagnosticCode::ClosedAncestor,
            ArtifactContextDiagnosticCode::MalformedMetadata,
            ArtifactContextDiagnosticCode::RepositoryNotAllowed,
            ArtifactContextDiagnosticCode::CycleDetected,
            ArtifactContextDiagnosticCode::DepthExceeded,
            ArtifactContextDiagnosticCode::CountExceeded,
            ArtifactContextDiagnosticCode::ContentTruncated,
            ArtifactContextDiagnosticCode::ForgeReadFailed,
        ];
        let names: Vec<String> = codes
            .into_iter()
            .map(|code| {
                serde_json::to_value(code)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            [
                "missing_artifact",
                "closed_ancestor",
                "malformed_metadata",
                "repository_not_allowed",
                "cycle_detected",
                "depth_exceeded",
                "count_exceeded",
                "content_truncated",
                "forge_read_failed",
            ]
        );
    }

    #[test]
    fn forge_operations_use_closed_snake_case_shapes() {
        let operation = ForgeContextOperation::ForgeListRelated(ForgeListRelatedOperation {
            repo: "ai/temper".into(),
            number: 7,
            artifact_type: Some(ArtifactType::Issue),
            relations: vec![ForgeRelationType::Child, ForgeRelationType::ProducedPr],
            depth: Some(2),
            limit: Some(50),
        });
        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["operation"], "forge_list_related");
        assert_eq!(json["repo"], "ai/temper");
        assert_eq!(json["type"], "issue");
        assert_eq!(json["relations"][1], "produced_pr");
        assert_eq!(
            serde_json::from_value::<ForgeContextOperation>(json).unwrap(),
            operation
        );
    }

    #[test]
    fn stable_context_error_vocabulary_is_snake_case() {
        let errors = [
            ForgeContextErrorCode::InvalidRequest,
            ForgeContextErrorCode::NotAuthorized,
            ForgeContextErrorCode::NotFound,
            ForgeContextErrorCode::ForgeUnavailable,
            ForgeContextErrorCode::LimitExceeded,
        ];
        let values: Vec<_> = errors
            .into_iter()
            .map(|error| serde_json::to_value(error).unwrap())
            .collect();
        assert_eq!(
            values,
            [
                "invalid_request",
                "not_authorized",
                "not_found",
                "forge_unavailable",
                "limit_exceeded",
            ]
        );
    }
}
