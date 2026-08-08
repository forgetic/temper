//! Safe typed diagnostics for model-visible tool failures.

use std::fmt::Write as _;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use super::CapturedContentV1;

/// Stable failure categories emitted by Temper's codebase-memory bridge.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureCategoryV1 {
    ConfigurationStartup,
    ProjectNotReady,
    IndexFailure,
    Timeout,
    Transport,
    ProcessExit,
    ProviderProtocol,
    InvalidModelInput,
    CircuitOpen,
}

impl ToolFailureCategoryV1 {
    /// Fixed content-free summary. A category, never provider text, owns the
    /// message projected into the activity stream.
    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::ConfigurationStartup => "codebase-memory setup did not complete",
            Self::ProjectNotReady => "codebase-memory project is not ready",
            Self::IndexFailure => "codebase-memory indexing failed",
            Self::Timeout => "codebase-memory request timed out",
            Self::Transport => "codebase-memory transport failed",
            Self::ProcessExit => "codebase-memory provider process exited",
            Self::ProviderProtocol => "codebase-memory provider or protocol request failed",
            Self::InvalidModelInput => "codebase-memory request input was invalid",
            Self::CircuitOpen => {
                "codebase-memory is disabled for this run after a systemic failure"
            }
        }
    }

    /// Whether another call may succeed without changing model input or the
    /// provider configuration.
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::ProjectNotReady | Self::Timeout | Self::Transport
        )
    }

    /// Whether ordinary filesystem discovery is the safe fallback.
    pub const fn fallback_to_conventional_discovery(self) -> bool {
        true
    }
}

/// Bounded diagnostic safe for metadata, transcript, and diagnostic capture.
///
/// Serialization and deserialization always replace `message` with the fixed
/// summary for `category`. This keeps even directly forged or retained wire
/// values from projecting raw stderr, credentials, arguments, cache contents,
/// provider text, or repository content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolFailureDiagnosticV1 {
    pub category: ToolFailureCategoryV1,
    pub retryable: bool,
    pub fallback_to_conventional_discovery: bool,
    pub message: String,
}

impl ToolFailureDiagnosticV1 {
    pub fn new(category: ToolFailureCategoryV1) -> Self {
        Self {
            category,
            retryable: category.retryable(),
            fallback_to_conventional_discovery: category.fallback_to_conventional_discovery(),
            message: category.safe_message().to_string(),
        }
    }

    pub fn normalize(&mut self) {
        *self = Self::new(self.category);
    }
}

/// Current version of the closed codebase-memory graph correlation contract.
pub const GRAPH_CORRELATION_VERSION: u32 = 1;
/// A correlation target is a declared symbol, pattern, or query, never provider
/// output, a prompt, or a generic tool preview.
pub const MAX_GRAPH_CORRELATION_TARGET_BYTES: usize = 256;
const GRAPH_CORRELATION_DIGEST_BYTES: usize = 32;

/// The targeted codebase-memory tools that may carry graph correlation.
///
/// This is intentionally closed: broad graph tools and arbitrary prefixed tool
/// names cannot gain relevance evidence by constructing an extension value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphCorrelationToolV1 {
    SearchGraph,
    SearchCode,
    TracePath,
    GetCodeSnippet,
}

impl GraphCorrelationToolV1 {
    /// Model-visible wrapper name for this closed tool identity.
    pub const fn public_name(self) -> &'static str {
        match self {
            Self::SearchGraph => "codebase_memory_search_graph",
            Self::SearchCode => "codebase_memory_search_code",
            Self::TracePath => "codebase_memory_trace_path",
            Self::GetCodeSnippet => "codebase_memory_get_code_snippet",
        }
    }

    /// Resolves the closed model-visible wrapper name to a correlation tool.
    pub fn from_public_name(name: &str) -> Option<Self> {
        match name {
            "codebase_memory_search_graph" => Some(Self::SearchGraph),
            "codebase_memory_search_code" => Some(Self::SearchCode),
            "codebase_memory_trace_path" => Some(Self::TracePath),
            "codebase_memory_get_code_snippet" => Some(Self::GetCodeSnippet),
            _ => None,
        }
    }

    const fn supports_target_kind(self, target_kind: GraphCorrelationTargetKindV1) -> bool {
        matches!(
            (self, target_kind),
            (Self::SearchGraph, GraphCorrelationTargetKindV1::GraphQuery)
                | (Self::SearchGraph, GraphCorrelationTargetKindV1::NamePattern)
                | (
                    Self::SearchGraph,
                    GraphCorrelationTargetKindV1::QualifiedNamePattern
                )
                | (Self::SearchCode, GraphCorrelationTargetKindV1::Pattern)
                | (Self::TracePath, GraphCorrelationTargetKindV1::FunctionName)
                | (
                    Self::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName
                )
        )
    }
}

/// The allowlisted structured field that declared a correlation target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphCorrelationTargetKindV1 {
    GraphQuery,
    Pattern,
    NamePattern,
    QualifiedNamePattern,
    FunctionName,
    QualifiedName,
}

/// Privacy-safe correlation for one targeted graph tool completion.
///
/// Call identity, completion ordering, and scope linkage are provided by the
/// enclosing [`ToolFinishedV1`] and activity frame. This value deliberately
/// carries only the versioned closed tool identity, target type, and a
/// fingerprint of one normalized declared target extracted by the trusted
/// wrapper. It never serializes the raw model argument or provider result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphCorrelationV1 {
    pub version: u32,
    pub tool: GraphCorrelationToolV1,
    pub target_kind: GraphCorrelationTargetKindV1,
    pub target_digest: String,
}

impl GraphCorrelationV1 {
    /// Builds a correlation record only when `target` has a complete,
    /// normalized bounded representation.
    pub fn new(
        tool: GraphCorrelationToolV1,
        target_kind: GraphCorrelationTargetKindV1,
        target: &str,
    ) -> Option<Self> {
        tool.supports_target_kind(target_kind).then_some(())?;
        Some(Self {
            version: GRAPH_CORRELATION_VERSION,
            tool,
            target_kind,
            target_digest: Self::target_digest(target)?,
        })
    }

    /// Returns the deterministic fingerprint for one complete declared target.
    ///
    /// This is public so a policy author or offline analyzer can compare a
    /// known declaration without ever requiring captured arguments. The raw
    /// target is normalized only in process and never retained by this DTO.
    pub fn target_digest(target: &str) -> Option<String> {
        let normalized = Self::normalize_target(target)?;
        let digest = Sha256::digest(normalized.as_bytes());
        let mut encoded = String::with_capacity(GRAPH_CORRELATION_DIGEST_BYTES * 2);
        for byte in digest {
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        Some(encoded)
    }

    /// Normalizes a declared structured target without interpreting or
    /// retaining its contents. Control characters, empty values, and overlong
    /// values are omitted rather than truncated.
    pub fn normalize_target(target: &str) -> Option<String> {
        if target.chars().any(char::is_control) {
            return None;
        }
        let normalized = target.split_whitespace().collect::<Vec<_>>().join(" ");
        (!normalized.is_empty() && normalized.len() <= MAX_GRAPH_CORRELATION_TARGET_BYTES)
            .then_some(normalized)
    }

    /// Returns whether this value is a complete canonical V1 record.
    pub fn is_valid(&self) -> bool {
        self.version == GRAPH_CORRELATION_VERSION
            && self.tool.supports_target_kind(self.target_kind)
            && self.target_digest.len() == GRAPH_CORRELATION_DIGEST_BYTES * 2
            && self
                .target_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }
}

/// Monotonic timing components emitted by a codebase-memory wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodebaseMemoryTimingV1 {
    pub readiness_wait_ms: u64,
    pub graph_execution_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolStartedV1 {
    pub call_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<CapturedContentV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFinishedV1 {
    pub call_id: String,
    pub name: String,
    pub status: ToolStatusV1,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CapturedContentV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ToolFailureDiagnosticV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase_memory_timing: Option<CodebaseMemoryTimingV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_correlation: Option<GraphCorrelationV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolFailureDiagnosticWire {
    category: ToolFailureCategoryV1,
    retryable: bool,
    fallback_to_conventional_discovery: bool,
    message: String,
}

impl Serialize for ToolFailureDiagnosticV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let canonical = Self::new(self.category);
        ToolFailureDiagnosticWire {
            category: canonical.category,
            retryable: canonical.retryable,
            fallback_to_conventional_discovery: canonical.fallback_to_conventional_discovery,
            message: canonical.message,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolFailureDiagnosticV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ToolFailureDiagnosticWire::deserialize(deserializer)?;
        Ok(Self::new(wire.category))
    }
}
