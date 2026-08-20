//! Safe typed diagnostics for model-visible tool failures.

use std::fmt::Write as _;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::DecisionAnchorLineageV1;
use sha2::{Digest as _, Sha256};

use super::CapturedContentV1;

/// Stable failure categories emitted for every model-visible tool. Existing
/// graph category wire names remain unchanged.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureCategoryV1 {
    SchemaArgumentMismatch,
    PolicyDenial,
    ExecutionFailure,
    ConfigurationStartup,
    ProjectNotReady,
    IndexFailure,
    Timeout,
    Transport,
    ProcessExit,
    ProviderProtocol,
    InvalidModelInput,
    CircuitOpen,
    Cancellation,
    GraphLifecycleDenial,
    CircuitRedirect,
}

impl ToolFailureCategoryV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaArgumentMismatch => "schema_argument_mismatch",
            Self::PolicyDenial => "policy_denial",
            Self::ExecutionFailure => "execution_failure",
            Self::ConfigurationStartup => "configuration_startup",
            Self::ProjectNotReady => "project_not_ready",
            Self::IndexFailure => "index_failure",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::ProcessExit => "process_exit",
            Self::ProviderProtocol => "provider_protocol",
            Self::InvalidModelInput => "invalid_model_input",
            Self::CircuitOpen => "circuit_open",
            Self::Cancellation => "cancellation",
            Self::GraphLifecycleDenial => "graph_lifecycle_denial",
            Self::CircuitRedirect => "circuit_redirect",
        }
    }

    pub const fn default_reason(self) -> ToolFailureReasonV1 {
        match self {
            Self::SchemaArgumentMismatch => ToolFailureReasonV1::InvalidArguments,
            Self::PolicyDenial => ToolFailureReasonV1::AccessDenied,
            Self::ExecutionFailure => ToolFailureReasonV1::ToolExecutionError,
            Self::ConfigurationStartup => ToolFailureReasonV1::ConfigurationStartup,
            Self::ProjectNotReady => ToolFailureReasonV1::ProjectNotReady,
            Self::IndexFailure => ToolFailureReasonV1::IndexFailure,
            Self::Timeout => ToolFailureReasonV1::GraphTimeout,
            Self::Transport => ToolFailureReasonV1::Transport,
            Self::ProcessExit => ToolFailureReasonV1::ProcessExit,
            Self::ProviderProtocol => ToolFailureReasonV1::ProviderProtocol,
            Self::InvalidModelInput => ToolFailureReasonV1::InvalidModelInput,
            Self::CircuitOpen => ToolFailureReasonV1::GraphCircuitOpen,
            Self::Cancellation => ToolFailureReasonV1::RunCancelled,
            Self::GraphLifecycleDenial => ToolFailureReasonV1::ExplorationClosed,
            Self::CircuitRedirect => ToolFailureReasonV1::RepeatedNonRetryable,
        }
    }

    /// Fixed content-free summary for legacy callers that only know a
    /// category. New diagnostics use the more precise closed reason.
    pub const fn safe_message(self) -> &'static str {
        self.default_reason().safe_message()
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self.default_reason().retry_disposition(),
            ToolRetryDispositionV1::Retryable
        )
    }

    pub const fn fallback_to_conventional_discovery(self) -> bool {
        self.default_reason().fallback_to_conventional_discovery()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureReasonV1 {
    UnknownTool,
    InvalidArguments,
    PolicyPrecondition,
    AccessDenied,
    ToolReportedFailure,
    ToolExecutionError,
    DeadlineExceeded,
    RunCancelled,
    ExplorationClosed,
    RepeatedNonRetryable,
    ConfigurationStartup,
    ProjectNotReady,
    IndexFailure,
    GraphTimeout,
    Transport,
    ProcessExit,
    ProviderProtocol,
    InvalidModelInput,
    GraphCircuitOpen,
}

impl ToolFailureReasonV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::PolicyPrecondition => "policy_precondition",
            Self::AccessDenied => "access_denied",
            Self::ToolReportedFailure => "tool_reported_failure",
            Self::ToolExecutionError => "tool_execution_error",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::RunCancelled => "run_cancelled",
            Self::ExplorationClosed => "exploration_closed",
            Self::RepeatedNonRetryable => "repeated_non_retryable",
            Self::ConfigurationStartup => "configuration_startup",
            Self::ProjectNotReady => "project_not_ready",
            Self::IndexFailure => "index_failure",
            Self::GraphTimeout => "graph_timeout",
            Self::Transport => "transport",
            Self::ProcessExit => "process_exit",
            Self::ProviderProtocol => "provider_protocol",
            Self::InvalidModelInput => "invalid_model_input",
            Self::GraphCircuitOpen => "graph_circuit_open",
        }
    }

    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::UnknownTool => "tool name is not registered; use a listed canonical tool name",
            Self::InvalidArguments => {
                "tool arguments did not match the canonical schema; correct the call and try again"
            }
            Self::PolicyPrecondition => {
                "workspace mutation blocked until the successful decision anchor is consumed through later result-derived codebase-memory evidence for the implementation, caller/model, and focused behavioral tests"
            }
            Self::AccessDenied => {
                "tool execution was denied by policy; use only authorized resources or satisfy the required precondition"
            }
            Self::ToolReportedFailure | Self::ToolExecutionError => {
                "tool execution failed; correct the invocation or choose a different action"
            }
            Self::DeadlineExceeded => {
                "tool execution timed out; retry with a bounded operation or a different approach"
            }
            Self::RunCancelled => "tool execution was cancelled; do not assume it completed",
            Self::ExplorationClosed => {
                "codebase-memory exploration is closed for this run; continue with conventional tools"
            }
            Self::RepeatedNonRetryable => {
                "this tool call repeats a non-retryable failure; change the invocation before trying again"
            }
            Self::ConfigurationStartup => "codebase-memory setup did not complete",
            Self::ProjectNotReady => "codebase-memory project is not ready",
            Self::IndexFailure => "codebase-memory indexing failed",
            Self::GraphTimeout => "codebase-memory request timed out",
            Self::Transport => "codebase-memory transport failed",
            Self::ProcessExit => "codebase-memory provider process exited",
            Self::ProviderProtocol => "codebase-memory provider or protocol request failed",
            Self::InvalidModelInput => "codebase-memory request input was invalid",
            Self::GraphCircuitOpen => {
                "codebase-memory is disabled for this run after a systemic failure"
            }
        }
    }

    pub const fn retry_disposition(self) -> ToolRetryDispositionV1 {
        match self {
            Self::ProjectNotReady
            | Self::GraphTimeout
            | Self::Transport
            | Self::DeadlineExceeded => ToolRetryDispositionV1::Retryable,
            Self::UnknownTool
            | Self::InvalidArguments
            | Self::ToolReportedFailure
            | Self::ToolExecutionError
            | Self::InvalidModelInput
            | Self::RepeatedNonRetryable => ToolRetryDispositionV1::CorrectInvocation,
            Self::PolicyPrecondition | Self::AccessDenied => ToolRetryDispositionV1::SatisfyPolicy,
            Self::ExplorationClosed
            | Self::ConfigurationStartup
            | Self::IndexFailure
            | Self::ProcessExit
            | Self::ProviderProtocol
            | Self::GraphCircuitOpen => ToolRetryDispositionV1::ConventionalDiscovery,
            Self::RunCancelled => ToolRetryDispositionV1::DoNotRetry,
        }
    }

    pub const fn fallback_to_conventional_discovery(self) -> bool {
        matches!(
            self,
            Self::ExplorationClosed
                | Self::ConfigurationStartup
                | Self::ProjectNotReady
                | Self::IndexFailure
                | Self::GraphTimeout
                | Self::Transport
                | Self::ProcessExit
                | Self::ProviderProtocol
                | Self::InvalidModelInput
                | Self::GraphCircuitOpen
        )
    }

    pub const fn valid_for(self, category: ToolFailureCategoryV1) -> bool {
        matches!(
            (category, self),
            (
                ToolFailureCategoryV1::SchemaArgumentMismatch,
                Self::UnknownTool | Self::InvalidArguments
            ) | (
                ToolFailureCategoryV1::PolicyDenial,
                Self::PolicyPrecondition | Self::AccessDenied
            ) | (
                ToolFailureCategoryV1::ExecutionFailure,
                Self::ToolReportedFailure | Self::ToolExecutionError
            ) | (
                ToolFailureCategoryV1::ConfigurationStartup,
                Self::ConfigurationStartup
            ) | (
                ToolFailureCategoryV1::ProjectNotReady,
                Self::ProjectNotReady
            ) | (ToolFailureCategoryV1::IndexFailure, Self::IndexFailure)
                | (
                    ToolFailureCategoryV1::Timeout,
                    Self::DeadlineExceeded | Self::GraphTimeout
                )
                | (ToolFailureCategoryV1::Transport, Self::Transport)
                | (ToolFailureCategoryV1::ProcessExit, Self::ProcessExit)
                | (
                    ToolFailureCategoryV1::ProviderProtocol,
                    Self::ProviderProtocol
                )
                | (
                    ToolFailureCategoryV1::InvalidModelInput,
                    Self::InvalidModelInput
                )
                | (ToolFailureCategoryV1::CircuitOpen, Self::GraphCircuitOpen)
                | (ToolFailureCategoryV1::Cancellation, Self::RunCancelled)
                | (
                    ToolFailureCategoryV1::GraphLifecycleDenial,
                    Self::ExplorationClosed
                )
                | (
                    ToolFailureCategoryV1::CircuitRedirect,
                    Self::RepeatedNonRetryable
                )
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRetryDispositionV1 {
    Retryable,
    CorrectInvocation,
    SatisfyPolicy,
    ConventionalDiscovery,
    DoNotRetry,
}

impl ToolRetryDispositionV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::CorrectInvocation => "correct_invocation",
            Self::SatisfyPolicy => "satisfy_policy",
            Self::ConventionalDiscovery => "conventional_discovery",
            Self::DoNotRetry => "do_not_retry",
        }
    }
}

/// Bounded diagnostic safe for metadata, transcript, logs, and diagnostic
/// capture. Serialization, deserialization, and Debug reconstruct all derived
/// values from the closed category/reason pair.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolFailureDiagnosticV1 {
    pub category: ToolFailureCategoryV1,
    pub reason: ToolFailureReasonV1,
    pub retry_disposition: ToolRetryDispositionV1,
    pub retryable: bool,
    pub fallback_to_conventional_discovery: bool,
    pub message: String,
}

impl ToolFailureDiagnosticV1 {
    pub fn new(category: ToolFailureCategoryV1) -> Self {
        Self::with_reason(category, category.default_reason())
    }

    pub fn with_reason(category: ToolFailureCategoryV1, reason: ToolFailureReasonV1) -> Self {
        let reason = if reason.valid_for(category) {
            reason
        } else {
            category.default_reason()
        };
        let retry_disposition = reason.retry_disposition();
        Self {
            category,
            reason,
            retry_disposition,
            retryable: retry_disposition == ToolRetryDispositionV1::Retryable,
            fallback_to_conventional_discovery: reason.fallback_to_conventional_discovery(),
            message: reason.safe_message().to_string(),
        }
    }

    pub fn normalize(&mut self) {
        *self = Self::with_reason(self.category, self.reason);
    }
}

impl std::fmt::Debug for ToolFailureDiagnosticV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let canonical = Self::with_reason(self.category, self.reason);
        formatter
            .debug_struct("ToolFailureDiagnosticV1")
            .field("category", &canonical.category)
            .field("reason", &canonical.reason)
            .field("retry_disposition", &canonical.retry_disposition)
            .field("retryable", &canonical.retryable)
            .field(
                "fallback_to_conventional_discovery",
                &canonical.fallback_to_conventional_discovery,
            )
            .field("message", &canonical.message)
            .finish()
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

    /// Checks the fixed-width digest representation retained by the activity
    /// protocol. This validates a digest without revealing or reinterpreting
    /// the source target that produced it.
    pub fn is_valid_target_digest(value: &str) -> bool {
        value.len() == GRAPH_CORRELATION_DIGEST_BYTES * 2
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
            && Self::is_valid_target_digest(&self.target_digest)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_anchor_lineage: Option<DecisionAnchorLineageV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ToolFailureDiagnosticWire {
    category: ToolFailureCategoryV1,
    reason: ToolFailureReasonV1,
    retry_disposition: ToolRetryDispositionV1,
    retryable: bool,
    fallback_to_conventional_discovery: bool,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolFailureDiagnosticWireInput {
    category: ToolFailureCategoryV1,
    #[serde(default)]
    reason: Option<ToolFailureReasonV1>,
    #[serde(default)]
    retry_disposition: Option<ToolRetryDispositionV1>,
    retryable: bool,
    fallback_to_conventional_discovery: bool,
    message: String,
}

impl Serialize for ToolFailureDiagnosticV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let canonical = Self::with_reason(self.category, self.reason);
        ToolFailureDiagnosticWire {
            category: canonical.category,
            reason: canonical.reason,
            retry_disposition: canonical.retry_disposition,
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
        let wire = ToolFailureDiagnosticWireInput::deserialize(deserializer)?;
        // Read legacy diagnostics that predate reason/disposition. Every
        // untrusted derived field is intentionally ignored and reconstructed.
        let _ = (
            wire.retry_disposition,
            wire.retryable,
            wire.fallback_to_conventional_discovery,
            wire.message,
        );
        Ok(Self::with_reason(
            wire.category,
            wire.reason
                .unwrap_or_else(|| wire.category.default_reason()),
        ))
    }
}
