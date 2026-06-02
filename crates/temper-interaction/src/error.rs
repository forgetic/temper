use std::error::Error;

use thiserror::Error;

use crate::types::ProposalId;

/// Errors produced by the provider-neutral interaction domain layer.
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
    /// A responder returned two proposals with the same stable id.
    #[error("duplicate proposal id `{id}`")]
    DuplicateProposalId {
        /// The repeated proposal id.
        id: ProposalId,
    },
    /// Serializing or deserializing profile-specific JSON payload failed.
    #[error("interaction JSON payload failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A responder failed without exposing a structured source error.
    #[error("interactive responder failed: {message}")]
    Responder {
        /// User-facing failure summary.
        message: String,
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
