//! Correctness-critical first-party agent lifecycle protocol.
//!
//! This channel is intentionally content-free. It describes operation
//! boundaries and liveness without carrying prompts, tool arguments, model
//! output, credentials, or worker/assignment identity. The worker binds an
//! accepted stream to the attempt that owns its loopback endpoint and stamps
//! receipt time from the worker runtime clock.

use serde::{Deserialize, Serialize};

/// Lifecycle wire version.
pub const AGENT_LIFECYCLE_PROTOCOL_VERSION: u32 = 1;
/// First-party process flag naming the worker-owned lifecycle endpoint.
pub const AGENT_LIFECYCLE_ADDRESS_FLAG: &str = "--agent-lifecycle-address";

/// Hard bounds applied before an untrusted child frame is accepted.
pub const MAX_AGENT_LIFECYCLE_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_AGENT_LIFECYCLE_ID_BYTES: usize = 128;
pub const MAX_AGENT_LIFECYCLE_TOOL_NAME_BYTES: usize = 128;
pub const MAX_AGENT_LIFECYCLE_CANCEL_REASON_BYTES: usize = 512;

/// The first record on a lifecycle connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleHelloV1 {
    pub version: u32,
}

impl Default for AgentLifecycleHelloV1 {
    fn default() -> Self {
        Self {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
        }
    }
}

impl AgentLifecycleHelloV1 {
    pub fn validate(self) -> Result<Self, AgentLifecycleValidationError> {
        require_version(self.version)?;
        Ok(self)
    }
}

/// Opaque invocation identity. A nested scope names its parent by opaque ID;
/// display labels and task/prompt content are deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleScopeV1 {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

impl AgentLifecycleScopeV1 {
    pub fn validate(&self) -> Result<(), AgentLifecycleValidationError> {
        bounded_nonempty("scope.id", &self.id, MAX_AGENT_LIFECYCLE_ID_BYTES)?;
        if let Some(parent_id) = &self.parent_id {
            bounded_nonempty("scope.parent_id", parent_id, MAX_AGENT_LIFECYCLE_ID_BYTES)?;
            if parent_id == &self.id {
                return Err(AgentLifecycleValidationError::new(
                    "scope.parent_id must differ from scope.id",
                ));
            }
        }
        Ok(())
    }
}

/// A monotonic, content-free lifecycle frame. Sequence numbers start at one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleFrameV1 {
    pub version: u32,
    pub seq: u64,
    pub scope: AgentLifecycleScopeV1,
    pub event: AgentLifecycleEventV1,
}

impl AgentLifecycleFrameV1 {
    pub fn validate(&self) -> Result<(), AgentLifecycleValidationError> {
        require_version(self.version)?;
        if self.seq == 0 {
            return Err(AgentLifecycleValidationError::new(
                "lifecycle sequence must start at one",
            ));
        }
        self.scope.validate()?;
        self.event.validate()
    }
}

/// Closed lifecycle vocabulary used by worker liveness supervision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentLifecycleEventV1 {
    ModelStarted {
        call_id: String,
        attempt: u32,
    },
    /// Meaningful stream activity. It carries no text, thinking, arguments, or
    /// tool-call payload and is coalesced by the first-party producer.
    ModelProgress {
        call_id: String,
    },
    ModelFinished {
        call_id: String,
        attempt: u32,
        status: AgentLifecycleModelStatusV1,
    },
    ModelRetrying {
        call_id: String,
        next_attempt: u32,
    },
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolFinished {
        call_id: String,
        name: String,
        status: AgentLifecycleToolStatusV1,
    },
    SteeringApplied,
    /// Nested managed-bash/MCP containment evidence. Assignment identity is
    /// supplied by the worker-owned endpoint rather than this child frame.
    Containment {
        observation: crate::AgentContainmentEventV1,
    },
    AgentFinished {
        status: AgentLifecycleAgentStatusV1,
    },
}

impl AgentLifecycleEventV1 {
    pub fn validate(&self) -> Result<(), AgentLifecycleValidationError> {
        match self {
            Self::ModelStarted { call_id, .. }
            | Self::ModelProgress { call_id }
            | Self::ModelFinished { call_id, .. }
            | Self::ModelRetrying { call_id, .. } => {
                bounded_nonempty("call_id", call_id, MAX_AGENT_LIFECYCLE_ID_BYTES)
            }
            Self::ToolStarted { call_id, name } | Self::ToolFinished { call_id, name, .. } => {
                bounded_nonempty("call_id", call_id, MAX_AGENT_LIFECYCLE_ID_BYTES)?;
                bounded_nonempty("tool name", name, MAX_AGENT_LIFECYCLE_TOOL_NAME_BYTES)
            }
            Self::Containment { observation } => observation
                .validate()
                .map_err(AgentLifecycleValidationError::new),
            Self::SteeringApplied | Self::AgentFinished { .. } => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleModelStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleToolStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleAgentStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

/// Monotonic cancellation stages understood by first-party native agents.
///
/// A receiver must consume every intermediate stage even when a later stage was
/// published before it next polls. Forced and hard stages also drive the
/// attempt-owned emergency process registry, independently of the agent future.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCancellationStage {
    #[default]
    Graceful,
    ForcedTermination,
    HardKill,
}

/// Worker-to-agent lifecycle commands. The command contains no assignment
/// identity; endpoint ownership supplies that binding. `stage` defaults to
/// graceful so version-one peers that only sent `reason` remain compatible.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentLifecycleCommandV1 {
    Cancel {
        #[serde(default)]
        stage: AgentCancellationStage,
        reason: String,
    },
}

impl AgentLifecycleCommandV1 {
    pub fn validate(&self) -> Result<(), AgentLifecycleValidationError> {
        match self {
            Self::Cancel { reason, .. } => bounded_nonempty(
                "cancel reason",
                reason,
                MAX_AGENT_LIFECYCLE_CANCEL_REASON_BYTES,
            ),
        }
    }
}

/// Child acknowledgement that a lifecycle cancellation command was received.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycleCancellationAcknowledgementV1 {
    pub version: u32,
}

/// Concise spelling for transport implementations.
pub type AgentLifecycleCancellationAckV1 = AgentLifecycleCancellationAcknowledgementV1;

impl AgentLifecycleCancellationAcknowledgementV1 {
    pub fn validate(self) -> Result<Self, AgentLifecycleValidationError> {
        require_version(self.version)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLifecycleValidationError {
    message: String,
}

impl AgentLifecycleValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AgentLifecycleValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentLifecycleValidationError {}

fn require_version(version: u32) -> Result<(), AgentLifecycleValidationError> {
    if version == AGENT_LIFECYCLE_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(AgentLifecycleValidationError::new(format!(
            "lifecycle protocol version {version} is unsupported"
        )))
    }
}

fn bounded_nonempty(
    field: &str,
    value: &str,
    maximum: usize,
) -> Result<(), AgentLifecycleValidationError> {
    if value.trim().is_empty() {
        return Err(AgentLifecycleValidationError::new(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > maximum {
        return Err(AgentLifecycleValidationError::new(format!(
            "{field} exceeds {maximum} bytes"
        )));
    }
    Ok(())
}
