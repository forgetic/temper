use std::error::Error;
use std::io;
use std::time::Duration;

use temper_forge::ForgeError;
use temper_process_protocol::interaction::InteractionProtocolError;
use thiserror::Error;

use crate::types::{ProposalId, ProposalKind};

/// Errors produced by the provider-neutral interaction layer.
///
/// The `Profile` and `Provider` variants are intentionally source-preserving so
/// concrete responders can adapt their own error types without this crate taking
/// a dependency on those providers.
#[derive(Debug, Error)]
pub enum InteractionError {
    /// A deterministic id or slug did not match Temper's portable slug rule.
    #[error("invalid {field} `{value}`: {reason}")]
    InvalidSlug {
        /// Human-readable field name, such as `proposal id`.
        field: &'static str,
        /// The rejected value.
        value: String,
        /// The slug rule that was violated.
        reason: &'static str,
    },
    /// A marker namespace cannot be represented safely in a hidden Forge marker.
    #[error("invalid marker namespace `{value}`: {reason}")]
    InvalidMarkerNamespace {
        /// The rejected marker namespace.
        value: String,
        /// The namespace rule that was violated.
        reason: &'static str,
    },
    /// A responder returned two proposals with the same stable id.
    #[error("duplicate proposal id `{id}`")]
    DuplicateProposalId {
        /// The repeated proposal id.
        id: ProposalId,
    },
    /// A proposal acceptance path received an unsupported proposal kind.
    #[error("proposal `{id}` has unsupported kind `{kind}`")]
    UnsupportedProposalKind {
        /// Proposal being accepted.
        id: ProposalId,
        /// Actual proposal kind.
        kind: ProposalKind,
    },
    /// A proposal id was not present in the latest cached responder reply.
    #[error("proposal `{id}` is not available; latest proposal ids are {available:?}")]
    ProposalNotFound {
        /// Requested proposal id.
        id: ProposalId,
        /// Currently cached proposal ids.
        available: Vec<ProposalId>,
    },
    /// Configuration supplied to the generic interaction runtime is invalid.
    #[error("invalid interaction config field `{field}`: {message}")]
    InvalidConfig {
        /// Configuration field name.
        field: &'static str,
        /// Explanation of the invalid value.
        message: String,
    },
    /// Serializing or deserializing profile-specific JSON payload failed.
    #[error("interaction JSON payload failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A Forge operation failed while loading transcripts or accepting proposals.
    #[error("forge operation failed: {0}")]
    Forge(#[from] ForgeError),
    /// The configured repository was not visible to the Forge handle.
    #[error("repository {owner}/{name} not found or not readable by the interaction token")]
    RepositoryNotFound {
        /// Repository owner.
        owner: String,
        /// Repository name.
        name: String,
    },
    /// A requested transcript issue number was not found.
    #[error("transcript issue #{number} was not found")]
    TranscriptNotFound {
        /// Repository-scoped issue number.
        number: u64,
    },
    /// The issue requested for resume does not match the transcript label policy.
    #[error("issue #{number} is not a transcript with labels {expected_labels:?}: {labels:?}")]
    TranscriptLabelMismatch {
        /// Repository-scoped issue number.
        number: u64,
        /// Required transcript labels.
        expected_labels: Vec<String>,
        /// Labels found on the issue.
        labels: Vec<String>,
    },
    /// A responder failed without exposing a structured source error.
    #[error("interactive responder failed: {message}")]
    Responder {
        /// User-facing failure summary.
        message: String,
    },
    /// Spawning, writing to, or waiting for a process responder failed.
    #[error("process responder {operation} I/O failed: {source}")]
    ProcessResponderIo {
        /// Operation being attempted.
        operation: &'static str,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },
    /// A process responder exceeded its one-turn timeout.
    #[error("process responder timed out after {timeout:?}")]
    ProcessResponderTimeout {
        /// Configured timeout.
        timeout: Duration,
    },
    /// A process responder exited unsuccessfully.
    #[error("process responder exited unsuccessfully with status {status}: {stderr}")]
    ProcessResponderExit {
        /// Process exit status string.
        status: String,
        /// Stderr preview captured from the responder.
        stderr: String,
    },
    /// A process responder did not return exactly one valid ConversationReply JSON value.
    #[error("process responder returned malformed ConversationReply JSON: {source}")]
    ProcessResponderMalformedJson {
        /// JSON parse failure.
        #[source]
        source: serde_json::Error,
    },
    /// A profile-specific adapter failed and preserved its source error.
    #[error("interactive profile failed: {message}")]
    Profile {
        /// User-facing failure summary.
        message: String,
        /// Original profile error.
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    /// A concrete provider failed and preserved its source error.
    #[error("interactive provider failed: {message}")]
    Provider {
        /// User-facing failure summary.
        message: String,
        /// Original provider error.
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

impl From<InteractionProtocolError> for InteractionError {
    fn from(error: InteractionProtocolError) -> Self {
        match error {
            InteractionProtocolError::InvalidSlug {
                field,
                value,
                reason,
            } => Self::InvalidSlug {
                field,
                value,
                reason,
            },
            InteractionProtocolError::DuplicateProposalId { id } => {
                Self::DuplicateProposalId { id }
            }
            InteractionProtocolError::UnsupportedProposalKind { id, kind } => {
                Self::UnsupportedProposalKind { id, kind }
            }
            InteractionProtocolError::Json(source) => Self::Json(source),
        }
    }
}

impl InteractionError {
    /// Builds an unstructured responder error.
    pub fn responder(message: impl Into<String>) -> Self {
        Self::Responder {
            message: message.into(),
        }
    }

    /// Wraps a profile-specific error while preserving it as the source.
    pub fn profile(message: impl Into<String>, source: impl Error + Send + Sync + 'static) -> Self {
        Self::Profile {
            message: message.into(),
            source: Box::new(source),
        }
    }

    /// Wraps an LLM or service provider error while preserving it as the source.
    pub fn provider(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Provider {
            message: message.into(),
            source: Box::new(source),
        }
    }
}
