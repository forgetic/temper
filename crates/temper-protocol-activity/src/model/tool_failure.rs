//! Safe typed diagnostics for model-visible tool failures.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::CapturedContentV1;

/// Stable failure categories emitted by Temper's codebase-memory bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
