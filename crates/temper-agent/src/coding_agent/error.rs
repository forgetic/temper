//! Coding-workspace run errors.

use crate::provider::ProviderError;
use temper_agent_core::ModelFailureDiagnostic;

/// Authority associated with an aborted coding-agent run.
///
/// Only a validated cancellation command received on the worker-owned
/// lifecycle channel may produce [`Self::WorkerRequested`]. Provider-originated
/// aborts and aborts without that command remain [`Self::Unrequested`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentAbortAuthority {
    WorkerRequested,
    Unrequested,
}

impl std::fmt::Display for AgentAbortAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::WorkerRequested => "worker_requested",
            Self::Unrequested => "unrequested",
        })
    }
}

/// Why a coding-workspace run could not produce a result.
#[derive(Debug)]
pub enum CodingAgentError {
    /// Building the provider or loading credentials failed.
    Provider(ProviderError),
    /// The SDK agent run failed (network or provider rejection).
    Run(String),
    /// A model call failed with a safe provider-neutral diagnostic.
    ModelFailure(Box<ModelFailureDiagnostic>),
    /// The agent stopped with an error stop reason but no typed provider
    /// diagnostic (legacy/defensive compatibility path).
    AgentStopped(String),
    /// The model requested another tool round after the configured budget.
    BudgetExhausted { max_iterations: usize },
    /// The run was aborted before normal completion.
    Aborted { authority: AgentAbortAuthority },
    /// The provider reported the requested model is unavailable (e.g. a model
    /// alias was suspended, or the subscription tier does not grant it). Kept
    /// distinct from a generic abnormal stop so the failure names the model and
    /// any provider-suggested fallback, and an operator can fix it by passing a
    /// different `--model` (or setting the provider profile's `models.main`).
    ModelUnavailable { model: String, detail: String },
    /// The configured codebase-memory MCP toolset was required but could not be
    /// started or listed.
    CodebaseMemory(String),
    /// The model's reply was not the expected JSON result object.
    Parse { snippet: String, error: String },
    /// A writable (engineer) run finished without leaving a product diff and
    /// without a routing verdict — there is nothing for temper to land.
    NoProduct,
    /// The model emitted a verdict that is not in the action's declared verdict
    /// vocabulary (W3). The engine would reject it as an undeclared verdict; we
    /// fail earlier here with a clearer message naming the allowed set.
    UndeclaredVerdict {
        emitted: String,
        allowed: Vec<String>,
    },
    /// A parseable terminal result violated the workflow-derived product shape.
    InvalidVerdictResult(String),
}

impl std::fmt::Display for CodingAgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodingAgentError::Provider(error) => write!(formatter, "{error}"),
            CodingAgentError::Run(message) => write!(formatter, "LLM run failed: {message}"),
            CodingAgentError::ModelFailure(diagnostic) => {
                write!(formatter, "model failure: {diagnostic}")
            }
            CodingAgentError::AgentStopped(reason) => {
                write!(formatter, "agent stopped abnormally: {reason}")
            }
            CodingAgentError::BudgetExhausted { max_iterations } => write!(
                formatter,
                "budget_exhausted: agent exceeded the {max_iterations}-iteration tool budget"
            ),
            CodingAgentError::Aborted { authority } => {
                write!(
                    formatter,
                    "aborted: agent run stopped (authority={authority})"
                )
            }
            CodingAgentError::ModelUnavailable { model, detail } => write!(
                formatter,
                "model `{model}` is unavailable: {detail}. Pass --model (or set the \
                 provider profile's models.main) to a model the credential grants."
            ),
            CodingAgentError::CodebaseMemory(message) => {
                write!(formatter, "codebase-memory tool setup failed: {message}")
            }
            CodingAgentError::Parse { snippet, error } => {
                write!(
                    formatter,
                    "could not parse agent result ({error}): {snippet}"
                )
            }
            CodingAgentError::NoProduct => formatter.write_str(
                "engineer run produced no product diff and emitted no verdict; nothing to land",
            ),
            CodingAgentError::InvalidVerdictResult(message) => {
                write!(formatter, "invalid WorkspaceResult: {message}")
            }
            CodingAgentError::UndeclaredVerdict { emitted, allowed } => write!(
                formatter,
                "agent emitted undeclared verdict `{emitted}`; this workflow step allows only: {}",
                allowed
                    .iter()
                    .map(|verdict| format!("`{verdict}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for CodingAgentError {}

impl From<ProviderError> for CodingAgentError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}
