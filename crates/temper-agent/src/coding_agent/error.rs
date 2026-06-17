//! Coding-workspace run errors.

use crate::provider::ProviderError;

/// Why a coding-workspace run could not produce a result.
#[derive(Debug)]
pub enum CodingAgentError {
    /// Building the provider or loading credentials failed.
    Provider(ProviderError),
    /// The SDK agent run failed (network, provider rejection, abort).
    Run(String),
    /// The agent stopped with an error stop reason.
    AgentStopped(String),
    /// The provider reported the requested model is unavailable (e.g. a model
    /// alias was suspended, or the subscription tier does not grant it). Kept
    /// distinct from a generic abnormal stop so the failure names the model and
    /// any provider-suggested fallback, and an operator can fix it by passing a
    /// different `--model` (or setting the provider profile's `models.main`).
    ModelUnavailable { model: String, detail: String },
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
}

impl std::fmt::Display for CodingAgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodingAgentError::Provider(error) => write!(formatter, "{error}"),
            CodingAgentError::Run(message) => write!(formatter, "LLM run failed: {message}"),
            CodingAgentError::AgentStopped(reason) => {
                write!(formatter, "agent stopped abnormally: {reason}")
            }
            CodingAgentError::ModelUnavailable { model, detail } => write!(
                formatter,
                "model `{model}` is unavailable: {detail}. Pass --model (or set the \
                 provider profile's models.main) to a model the credential grants."
            ),
            CodingAgentError::Parse { snippet, error } => {
                write!(
                    formatter,
                    "could not parse agent result ({error}): {snippet}"
                )
            }
            CodingAgentError::NoProduct => formatter.write_str(
                "engineer run produced no product diff and emitted no verdict; nothing to land",
            ),
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
