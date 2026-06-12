//! Smith-side config loading and validation for generic interaction profiles.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_process_protocol::{ConversationProfileId, InteractionProtocolError, ProposalKind};

use crate::decision::DecisionError;

/// Supported response format declared by a Smith interaction profile config.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionResponseFormat {
    /// Temper `ConversationReply` v1 JSON.
    ConversationReplyV1,
}

/// Payload contract Smith should tell the model to use for one proposal kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalPayloadContract {
    /// Built-in Temper issue draft payload for the built-in `issue` kind.
    IssueDraft,
    /// Stable generic Temper proposal kind with arbitrary JSON payload.
    CustomJson,
}

/// One proposal kind allowed by a Smith interaction profile config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InteractionAllowedProposalKind {
    /// Stable proposal kind id.
    pub id: ProposalKind,
    /// Payload contract for this kind.
    pub payload: InteractionProposalPayloadContract,
}

/// Validated Smith-side prompt/profile config for a generic interactive responder.
#[derive(Clone, Debug, PartialEq)]
pub struct InteractionProfileConfig {
    profile_id: ConversationProfileId,
    system_prompt: String,
    required_context: Vec<String>,
    allowed_proposal_kinds: Vec<InteractionAllowedProposalKind>,
    response_format: InteractionResponseFormat,
}

impl InteractionProfileConfig {
    /// Loads and validates a JSON Smith interaction profile config from `path`.
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

    /// Parses and validates a JSON Smith interaction profile config.
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInteractionProfileConfig {
    profile_id: String,
    system_prompt: RawPromptSource,
    #[serde(default)]
    required_context: Vec<String>,
    #[serde(default)]
    allowed_proposal_kinds: Vec<RawAllowedProposalKind>,
    response_format: InteractionResponseFormat,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPromptSource {
    text: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAllowedProposalKind {
    id: String,
    payload: InteractionProposalPayloadContract,
}

fn load_prompt(
    source: RawPromptSource,
    base_dir: Option<&Path>,
) -> Result<String, InteractionProfileError> {
    match (source.text, source.path) {
        (Some(text), None) => validate_prompt_text(text),
        (None, Some(path)) => load_prompt_path(path, base_dir),
        (None, None) => Err(InteractionProfileError::invalid_config(
            "system_prompt",
            "exactly one prompt source is required: `text` or `path`",
        )),
        (Some(_), Some(_)) => Err(InteractionProfileError::invalid_config(
            "system_prompt",
            "exactly one prompt source is allowed: `text` or `path`",
        )),
    }
}

fn validate_prompt_text(text: String) -> Result<String, InteractionProfileError> {
    if text.trim().is_empty() {
        return Err(InteractionProfileError::invalid_config(
            "system_prompt.text",
            "must not be empty",
        ));
    }
    Ok(text)
}

fn load_prompt_path(
    path: PathBuf,
    base_dir: Option<&Path>,
) -> Result<String, InteractionProfileError> {
    if path.as_os_str().is_empty() {
        return Err(InteractionProfileError::invalid_config(
            "system_prompt.path",
            "must not be empty",
        ));
    }
    let path = if path.is_absolute() {
        path
    } else {
        let base_dir = base_dir.ok_or_else(|| {
            InteractionProfileError::invalid_config(
                "system_prompt.path",
                "relative prompt paths require loading the config from a file or supplying a base directory",
            )
        })?;
        base_dir.join(path)
    };
    let text =
        std::fs::read_to_string(&path).map_err(|source| InteractionProfileError::ConfigIo {
            path: path.clone(),
            source,
        })?;
    validate_prompt_text(text)
}

fn validate_required_context(fields: Vec<String>) -> Result<Vec<String>, InteractionProfileError> {
    let mut seen = HashSet::new();
    let mut validated = Vec::with_capacity(fields.len());
    for field in fields {
        if !is_valid_context_field(&field) {
            return Err(InteractionProfileError::invalid_config(
                "required_context",
                format!(
                    "invalid context field `{field}`; use 1-80 ASCII letters, digits, underscores, or hyphens"
                ),
            ));
        }
        if !seen.insert(field.clone()) {
            return Err(InteractionProfileError::invalid_config(
                "required_context",
                format!("duplicate context field `{field}`"),
            ));
        }
        validated.push(field);
    }
    Ok(validated)
}

fn is_valid_context_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= 80
        && field
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-'))
}

fn validate_allowed_proposal_kinds(
    kinds: Vec<RawAllowedProposalKind>,
) -> Result<Vec<InteractionAllowedProposalKind>, InteractionProfileError> {
    let mut seen = HashSet::new();
    let mut validated = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let id = ProposalKind::new(kind.id.clone()).map_err(|error| {
            InteractionProfileError::invalid_config("allowed_proposal_kinds.id", error.to_string())
        })?;
        validate_payload_contract(&id, kind.payload)?;
        if !seen.insert(id.clone()) {
            return Err(InteractionProfileError::invalid_config(
                "allowed_proposal_kinds",
                format!("duplicate proposal kind `{id}`"),
            ));
        }
        validated.push(InteractionAllowedProposalKind {
            id,
            payload: kind.payload,
        });
    }
    Ok(validated)
}

fn validate_payload_contract(
    kind: &ProposalKind,
    payload: InteractionProposalPayloadContract,
) -> Result<(), InteractionProfileError> {
    match payload {
        InteractionProposalPayloadContract::IssueDraft if kind == &ProposalKind::issue() => Ok(()),
        InteractionProposalPayloadContract::IssueDraft => {
            Err(InteractionProfileError::invalid_config(
                "allowed_proposal_kinds.payload",
                format!(
                    "payload `issue_draft` is only supported for proposal kind `issue`, not `{kind}`"
                ),
            ))
        }
        InteractionProposalPayloadContract::CustomJson if kind == &ProposalKind::issue() => {
            Err(InteractionProfileError::invalid_config(
                "allowed_proposal_kinds.payload",
                "built-in proposal kind `issue` must use payload `issue_draft`",
            ))
        }
        InteractionProposalPayloadContract::CustomJson => Ok(()),
    }
}
