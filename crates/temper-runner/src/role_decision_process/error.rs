//! Errors produced by the workflow-role decision process adapter.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use crate::WorkflowRoleDecisionProtocolError;

/// Errors produced by the workflow-role decision process adapter before they
/// are mapped onto generic [`AgentError`](crate::AgentError) values.
#[derive(Debug)]
pub enum WorkflowRoleDecisionProcessError {
    /// Static process configuration is invalid.
    InvalidConfig {
        field: &'static str,
        message: String,
    },
    /// Spawning, writing to, or waiting for the process failed.
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    /// The process exceeded its timeout.
    Timeout { timeout: Duration },
    /// The process exited unsuccessfully.
    Exit { status: String, stderr: String },
    /// Stdout did not contain exactly one valid reply JSON value.
    MalformedJson { source: serde_json::Error },
    /// The reply did not satisfy the request contract.
    Protocol(WorkflowRoleDecisionProtocolError),
}

impl fmt::Display for WorkflowRoleDecisionProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { field, message } => {
                write!(
                    formatter,
                    "invalid role decision process config `{field}`: {message}"
                )
            }
            Self::Io { operation, source } => {
                write!(
                    formatter,
                    "role decision process {operation} I/O failed: {source}"
                )
            }
            Self::Timeout { timeout } => {
                write!(
                    formatter,
                    "role decision process timed out after {timeout:?}"
                )
            }
            Self::Exit { status, stderr } => write!(
                formatter,
                "role decision process exited unsuccessfully with status {status}: {stderr}"
            ),
            Self::MalformedJson { source } => {
                write!(
                    formatter,
                    "role decision process returned malformed JSON: {source}"
                )
            }
            Self::Protocol(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for WorkflowRoleDecisionProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::MalformedJson { source } => Some(source),
            Self::Protocol(error) => Some(error),
            Self::InvalidConfig { .. } | Self::Timeout { .. } | Self::Exit { .. } => None,
        }
    }
}
