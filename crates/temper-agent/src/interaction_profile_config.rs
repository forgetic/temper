//! anvil-side config loading and validation for generic interaction profiles.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_interaction::{ConversationProfileId, InteractionProtocolError, ProposalKind};

use crate::decision::DecisionError;
use crate::interaction_profile_config_raw::{
    RawInteractionProfileConfig, load_prompt, validate_allowed_proposal_kinds,
    validate_required_context,
};

/// Supported response format declared by a anvil interaction profile config.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionResponseFormat {
    /// Temper `ConversationReply` v1 JSON.
    ConversationReplyV1,
}

/// Payload contract anvil should tell the model to use for one proposal kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalPayloadContract {
    /// Built-in Temper issue draft payload for the built-in `issue` kind.
    IssueDraft,
    /// Stable generic Temper proposal kind with arbitrary JSON payload.
    CustomJson,
}

/// One proposal kind allowed by a anvil interaction profile config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InteractionAllowedProposalKind {
    /// Stable proposal kind id.
    pub id: ProposalKind,
    /// Payload contract for this kind.
    pub payload: InteractionProposalPayloadContract,
}

/// Validated anvil-side prompt/profile config for a generic interactive responder.
#[derive(Clone, Debug, PartialEq)]
pub struct InteractionProfileConfig {
    profile_id: ConversationProfileId,
    system_prompt: String,
    required_context: Vec<String>,
    allowed_proposal_kinds: Vec<InteractionAllowedProposalKind>,
    response_format: InteractionResponseFormat,
}

impl InteractionProfileConfig {
    /// Loads and validates a JSON anvil interaction profile config from `path`.
    ///
    /// Relative prompt paths inside the config are resolved relative to the
    /// config file's parent directory. Absolute prompt paths are used as-is.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, InteractionProfileError> {
        let path = path.as_ref();
        let contents =
            std::fs::read_to_string(path).map_err(|source| InteractionProfileError::ConfigIo {
                path: path.to_path_buf(),
                source,
            })?;
        let raw: RawInteractionProfileConfig =
            serde_json::from_str(&contents).map_err(|source| {
                InteractionProfileError::ConfigJson {
                    path: Some(path.to_path_buf()),
                    source,
                }
            })?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_raw(raw, Some(base_dir))
    }

    /// Parses and validates a JSON anvil interaction profile config.
    ///
    /// This helper is best for inline `system_prompt.text` configs. Use
    /// [`Self::from_json_str_with_base`] or [`Self::load_from_path`] when the
    /// config uses `system_prompt.path`.
    pub fn from_json_str(contents: &str) -> Result<Self, InteractionProfileError> {
        let raw: RawInteractionProfileConfig = serde_json::from_str(contents)
            .map_err(|source| InteractionProfileError::ConfigJson { path: None, source })?;
        Self::from_raw(raw, None)
    }

    /// Parses and validates a JSON config, resolving relative prompt paths from
    /// `base_dir`.
    pub fn from_json_str_with_base(
        contents: &str,
        base_dir: impl AsRef<Path>,
    ) -> Result<Self, InteractionProfileError> {
        let raw: RawInteractionProfileConfig = serde_json::from_str(contents)
            .map_err(|source| InteractionProfileError::ConfigJson { path: None, source })?;
        Self::from_raw(raw, Some(base_dir.as_ref()))
    }

    /// Returns this profile's deterministic id.
    pub fn profile_id(&self) -> &ConversationProfileId {
        &self.profile_id
    }

    /// Returns the loaded provider-facing system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Returns required top-level `request.context` fields.
    pub fn required_context(&self) -> &[String] {
        &self.required_context
    }

    /// Returns proposal kinds this config allows the model to emit.
    pub fn allowed_proposal_kinds(&self) -> &[InteractionAllowedProposalKind] {
        &self.allowed_proposal_kinds
    }

    /// Returns the configured response format.
    pub fn response_format(&self) -> InteractionResponseFormat {
        self.response_format
    }

    pub(crate) fn proposal_contract(
        &self,
        kind: &ProposalKind,
    ) -> Option<InteractionProposalPayloadContract> {
        self.allowed_proposal_kinds
            .iter()
            .find(|allowed| &allowed.id == kind)
            .map(|allowed| allowed.payload)
    }

    fn from_raw(
        raw: RawInteractionProfileConfig,
        base_dir: Option<&Path>,
    ) -> Result<Self, InteractionProfileError> {
        let profile_id = ConversationProfileId::new(raw.profile_id.clone()).map_err(|error| {
            InteractionProfileError::invalid_config("profile_id", error.to_string())
        })?;
        let system_prompt = load_prompt(raw.system_prompt, base_dir)?;
        let required_context = validate_required_context(raw.required_context)?;
        let allowed_proposal_kinds = validate_allowed_proposal_kinds(raw.allowed_proposal_kinds)?;
        Ok(Self {
            profile_id,
            system_prompt,
            required_context,
            allowed_proposal_kinds,
            response_format: raw.response_format,
        })
    }
}

/// Generic interaction profile failure.
#[derive(Debug)]
pub enum InteractionProfileError {
    /// Reading a profile config or prompt file failed.
    ConfigIo { path: PathBuf, source: io::Error },
    /// Deserializing profile config JSON failed.
    ConfigJson {
        path: Option<PathBuf>,
        source: serde_json::Error,
    },
    /// Static profile config validation failed.
    InvalidConfig {
        field: &'static str,
        message: String,
    },
    /// The request cannot be served by this profile.
    InvalidRequest(String),
    /// Rendering provider context JSON failed.
    RequestContext(serde_json::Error),
    /// Building the provider, running the model, or parsing the model JSON failed.
    Decision(DecisionError),
    /// Temper's process-protocol validation rejected the reply.
    Protocol(InteractionProtocolError),
}

impl InteractionProfileError {
    pub(crate) fn invalid_config(field: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidConfig {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for InteractionProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigIo { path, source } => {
                write!(formatter, "reading {} failed: {source}", path.display())
            }
            Self::ConfigJson { path, source } => match path {
                Some(path) => write!(
                    formatter,
                    "invalid interaction profile config JSON in {}: {source}",
                    path.display()
                ),
                None => write!(
                    formatter,
                    "invalid interaction profile config JSON: {source}"
                ),
            },
            Self::InvalidConfig { field, message } => {
                write!(
                    formatter,
                    "invalid interaction profile config field `{field}`: {message}"
                )
            }
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::RequestContext(error) => {
                write!(
                    formatter,
                    "serializing interaction request context failed: {error}"
                )
            }
            Self::Decision(error) => write!(formatter, "{error}"),
            Self::Protocol(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for InteractionProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConfigIo { source, .. } => Some(source),
            Self::ConfigJson { source, .. } => Some(source),
            Self::RequestContext(error) => Some(error),
            Self::Decision(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::InvalidConfig { .. } | Self::InvalidRequest(_) => None,
        }
    }
}

impl From<DecisionError> for InteractionProfileError {
    fn from(error: DecisionError) -> Self {
        Self::Decision(error)
    }
}

impl From<InteractionProtocolError> for InteractionProfileError {
    fn from(error: InteractionProtocolError) -> Self {
        Self::Protocol(error)
    }
}
