//! Raw (deserialized) interaction-profile config DTOs and their validation.
//!
//! These are the on-disk JSON shapes (`deny_unknown_fields`) plus the functions
//! that validate them into the public [`InteractionProfileConfig`] field types.
//! Kept separate from the public config surface so the validation rules live in
//! one place.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use temper_protocol_interaction::ProposalKind;

use crate::interaction_profile_config::{
    InteractionAllowedProposalKind, InteractionProfileError, InteractionProposalPayloadContract,
    InteractionResponseFormat,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawInteractionProfileConfig {
    pub(crate) profile_id: String,
    pub(crate) system_prompt: RawPromptSource,
    #[serde(default)]
    pub(crate) required_context: Vec<String>,
    #[serde(default)]
    pub(crate) allowed_proposal_kinds: Vec<RawAllowedProposalKind>,
    pub(crate) response_format: InteractionResponseFormat,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPromptSource {
    text: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAllowedProposalKind {
    id: String,
    payload: InteractionProposalPayloadContract,
}

pub(crate) fn load_prompt(
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

pub(crate) fn validate_required_context(
    fields: Vec<String>,
) -> Result<Vec<String>, InteractionProfileError> {
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

pub(crate) fn validate_allowed_proposal_kinds(
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
