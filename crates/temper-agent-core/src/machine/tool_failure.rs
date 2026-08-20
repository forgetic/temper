//! Closed, privacy-safe diagnostics for model-visible tool failures.

/// Stable failure categories for every model-visible tool. Existing graph
/// category spellings are retained for wire compatibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFailureCategory {
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

impl ToolFailureCategory {
    pub fn as_str(self) -> &'static str {
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

    pub fn from_stable_str(value: &str) -> Option<Self> {
        match value {
            "schema_argument_mismatch" => Some(Self::SchemaArgumentMismatch),
            "policy_denial" => Some(Self::PolicyDenial),
            "execution_failure" => Some(Self::ExecutionFailure),
            "configuration_startup" => Some(Self::ConfigurationStartup),
            "project_not_ready" => Some(Self::ProjectNotReady),
            "index_failure" => Some(Self::IndexFailure),
            "timeout" => Some(Self::Timeout),
            "transport" => Some(Self::Transport),
            "process_exit" => Some(Self::ProcessExit),
            "provider_protocol" => Some(Self::ProviderProtocol),
            "invalid_model_input" => Some(Self::InvalidModelInput),
            "circuit_open" => Some(Self::CircuitOpen),
            "cancellation" => Some(Self::Cancellation),
            "graph_lifecycle_denial" => Some(Self::GraphLifecycleDenial),
            "circuit_redirect" => Some(Self::CircuitRedirect),
            _ => None,
        }
    }

    pub const fn default_reason(self) -> ToolFailureReason {
        match self {
            Self::SchemaArgumentMismatch => ToolFailureReason::InvalidArguments,
            Self::PolicyDenial => ToolFailureReason::AccessDenied,
            Self::ExecutionFailure => ToolFailureReason::ToolExecutionError,
            Self::ConfigurationStartup => ToolFailureReason::ConfigurationStartup,
            Self::ProjectNotReady => ToolFailureReason::ProjectNotReady,
            Self::IndexFailure => ToolFailureReason::IndexFailure,
            Self::Timeout => ToolFailureReason::GraphTimeout,
            Self::Transport => ToolFailureReason::Transport,
            Self::ProcessExit => ToolFailureReason::ProcessExit,
            Self::ProviderProtocol => ToolFailureReason::ProviderProtocol,
            Self::InvalidModelInput => ToolFailureReason::InvalidModelInput,
            Self::CircuitOpen => ToolFailureReason::GraphCircuitOpen,
            Self::Cancellation => ToolFailureReason::RunCancelled,
            Self::GraphLifecycleDenial => ToolFailureReason::ExplorationClosed,
            Self::CircuitRedirect => ToolFailureReason::RepeatedNonRetryable,
        }
    }

    pub fn safe_message(self) -> &'static str {
        self.default_reason().safe_message()
    }

    pub fn retryable(self) -> bool {
        self.default_reason().retry_disposition() == ToolRetryDisposition::Retryable
    }
}

/// Closed causes within the stable top-level categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFailureReason {
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

impl ToolFailureReason {
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

    pub const fn retry_disposition(self) -> ToolRetryDisposition {
        match self {
            Self::ProjectNotReady
            | Self::GraphTimeout
            | Self::Transport
            | Self::DeadlineExceeded => ToolRetryDisposition::Retryable,
            Self::UnknownTool
            | Self::InvalidArguments
            | Self::ToolReportedFailure
            | Self::ToolExecutionError
            | Self::InvalidModelInput
            | Self::RepeatedNonRetryable => ToolRetryDisposition::CorrectInvocation,
            Self::PolicyPrecondition | Self::AccessDenied => ToolRetryDisposition::SatisfyPolicy,
            Self::ExplorationClosed
            | Self::ConfigurationStartup
            | Self::IndexFailure
            | Self::ProcessExit
            | Self::ProviderProtocol
            | Self::GraphCircuitOpen => ToolRetryDisposition::ConventionalDiscovery,
            Self::RunCancelled => ToolRetryDisposition::DoNotRetry,
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

    pub const fn valid_for(self, category: ToolFailureCategory) -> bool {
        matches!(
            (category, self),
            (
                ToolFailureCategory::SchemaArgumentMismatch,
                Self::UnknownTool | Self::InvalidArguments
            ) | (
                ToolFailureCategory::PolicyDenial,
                Self::PolicyPrecondition | Self::AccessDenied
            ) | (
                ToolFailureCategory::ExecutionFailure,
                Self::ToolReportedFailure | Self::ToolExecutionError
            ) | (
                ToolFailureCategory::ConfigurationStartup,
                Self::ConfigurationStartup
            ) | (ToolFailureCategory::ProjectNotReady, Self::ProjectNotReady)
                | (ToolFailureCategory::IndexFailure, Self::IndexFailure)
                | (
                    ToolFailureCategory::Timeout,
                    Self::DeadlineExceeded | Self::GraphTimeout
                )
                | (ToolFailureCategory::Transport, Self::Transport)
                | (ToolFailureCategory::ProcessExit, Self::ProcessExit)
                | (
                    ToolFailureCategory::ProviderProtocol,
                    Self::ProviderProtocol
                )
                | (
                    ToolFailureCategory::InvalidModelInput,
                    Self::InvalidModelInput
                )
                | (ToolFailureCategory::CircuitOpen, Self::GraphCircuitOpen)
                | (ToolFailureCategory::Cancellation, Self::RunCancelled)
                | (
                    ToolFailureCategory::GraphLifecycleDenial,
                    Self::ExplorationClosed
                )
                | (
                    ToolFailureCategory::CircuitRedirect,
                    Self::RepeatedNonRetryable
                )
        )
    }
}

/// Canonical next-action guidance shared by the next model turn, durable
/// activity, logging, and machine policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolRetryDisposition {
    Retryable,
    CorrectInvocation,
    SatisfyPolicy,
    ConventionalDiscovery,
    DoNotRetry,
}

impl ToolRetryDisposition {
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

/// Safe, bounded evidence for any model-visible tool failure. Messages are
/// reason-owned constants; raw tool output, errors, stderr, commands,
/// arguments, patches, paths, credentials, provider payloads, and host-gate
/// output can never populate this type.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolFailureDiagnostic {
    pub category: ToolFailureCategory,
    pub reason: ToolFailureReason,
    pub retry_disposition: ToolRetryDisposition,
    pub retryable: bool,
    pub fallback_to_conventional_discovery: bool,
    pub message: String,
}

impl ToolFailureDiagnostic {
    pub fn new(category: ToolFailureCategory, reason: ToolFailureReason) -> Self {
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
            retryable: retry_disposition == ToolRetryDisposition::Retryable,
            fallback_to_conventional_discovery: reason.fallback_to_conventional_discovery(),
            message: reason.safe_message().to_string(),
        }
    }

    pub fn codebase_memory(category: ToolFailureCategory) -> Self {
        Self::new(category, category.default_reason())
    }

    pub fn schema(reason: ToolFailureReason) -> Self {
        Self::new(ToolFailureCategory::SchemaArgumentMismatch, reason)
    }

    pub fn policy_denial() -> Self {
        Self::new(
            ToolFailureCategory::PolicyDenial,
            ToolFailureReason::PolicyPrecondition,
        )
    }

    pub fn access_denied() -> Self {
        Self::new(
            ToolFailureCategory::PolicyDenial,
            ToolFailureReason::AccessDenied,
        )
    }

    pub fn execution(reason: ToolFailureReason) -> Self {
        Self::new(ToolFailureCategory::ExecutionFailure, reason)
    }

    pub fn timeout() -> Self {
        Self::new(
            ToolFailureCategory::Timeout,
            ToolFailureReason::DeadlineExceeded,
        )
    }

    pub fn cancelled() -> Self {
        Self::new(
            ToolFailureCategory::Cancellation,
            ToolFailureReason::RunCancelled,
        )
    }

    pub fn canonical(&self) -> Self {
        Self::new(self.category, self.reason)
    }

    /// Canonical model-visible rendering. Conventional-discovery guidance is
    /// derived only from the closed diagnostic, never from wrapper output.
    pub fn model_message(&self) -> String {
        let canonical = self.canonical();
        if canonical.fallback_to_conventional_discovery {
            format!(
                "{}; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead",
                canonical.message
            )
        } else {
            canonical.message
        }
    }
}

impl std::fmt::Debug for ToolFailureDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let canonical = self.canonical();
        formatter
            .debug_struct("ToolFailureDiagnostic")
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
