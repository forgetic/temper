//! Error returned by an [`Agent`](super::Agent) while servicing a work item.

use std::error::Error;
use std::fmt;
use temper_forge::ForgeError;
use temper_workflow::ExecutionError;

/// Error returned by an [`Agent`](super::Agent) while servicing a work item.
#[derive(Debug)]
pub enum AgentError {
    /// A workflow transition or idempotent create failed.
    Execution(ExecutionError),
    /// A read through the Forge backend failed.
    Forge(ForgeError),
    /// Agent-provider or behavior-specific failure.
    Message(String),
}

impl AgentError {
    /// Creates a behavior/provider error from a displayable message.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Execution(error) => write!(formatter, "workflow execution failed: {error}"),
            AgentError::Forge(error) => write!(formatter, "forge read failed: {error}"),
            AgentError::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for AgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AgentError::Execution(error) => Some(error),
            AgentError::Forge(error) => Some(error),
            AgentError::Message(_) => None,
        }
    }
}

impl From<ExecutionError> for AgentError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<ForgeError> for AgentError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}
