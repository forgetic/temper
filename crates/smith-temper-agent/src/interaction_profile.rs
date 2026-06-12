//! Generic prompt-driven interactive responder profiles for Temper.
//!
//! This module uses a Smith-owned interactive profile config to run Temper's
//! generic `ConversationRequest`/`ConversationReply` process contract through
//! Smith's existing provider/decision core. The config is deliberately provider-
//! facing only: prompt text, required request-context keys, and optional
//! proposal-kind output restrictions. Transcript policy, commands, proposal
//! acceptance, idempotency markers, labels, and every Forge/workflow mutation
//! remain Temper-owned.

use serde::Serialize;
use serde_json::Value;
use temper_process_protocol::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, InteractionProtocolError, ProposalPayloadValidator,
};

use crate::decision::run_decision;
pub use crate::interaction_profile_config::{
    InteractionAllowedProposalKind, InteractionProfileConfig, InteractionProfileError,
    InteractionProposalPayloadContract, InteractionResponseFormat,
};
use crate::provider::ProviderConfig;

/// Fixed Smith protocol instruction for provider calls that must return Temper's
/// v1 conversation reply shape.
pub const CONVERSATION_REPLY_V1_PROTOCOL_INSTRUCTION: &str = r#"Return exactly one JSON object matching Temper's ConversationReply v1 shape and nothing else.

The object must have this shape:
{
  "message": "short conversational response",
  "proposals": [
    {
      "id": "stable-lowercase-id",
      "kind": "declared-proposal-kind",
      "title": "Proposal title",
      "summary": "optional short summary or rationale",
      "payload": {}
    }
  ]
}

Rules:
- Output only the JSON object. Do not wrap it in markdown fences or extra prose.
- Always include `message`. Use an empty `proposals` array when there is nothing to propose.
- Proposal `id` and `kind` values must be deterministic lowercase slugs.
- For proposal kind `issue`, `payload` must be an issue draft object with `title`, `body`, and optional `rationale` fields.
- Do not claim that a proposal has already been accepted or applied."#;

/// Runs one generic prompt-driven interactive profile turn.
pub struct GenericInteractionResponder {
    profile: InteractionProfileConfig,
    provider: ProviderConfig,
}

impl GenericInteractionResponder {
    /// Builds a generic responder that calls Smith's provider decision core.
    pub fn new(profile: InteractionProfileConfig, provider: ProviderConfig) -> Self {
        Self { profile, provider }
    }

    /// Returns the loaded profile config.
    pub fn profile(&self) -> &InteractionProfileConfig {
        &self.profile
    }

    /// Renders the complete system prompt passed to the provider.
    pub fn render_system_prompt(&self) -> String {
        render_system_prompt(self.profile.system_prompt())
    }

    /// Renders the generic provider context for one conversation request.
    pub fn render_provider_context(
        &self,
        request: &ConversationRequest,
    ) -> Result<String, InteractionProfileError> {
        render_provider_context(&self.profile, request)
    }

    /// Validates the request profile id and required context keys.
    pub fn validate_request(
        &self,
        request: &ConversationRequest,
    ) -> Result<(), InteractionProfileError> {
        validate_request(&self.profile, request)
    }

    /// Validates duplicate proposal ids, built-in payloads, and this profile's
    /// proposal-kind allow-list.
    pub fn validate_reply(&self, reply: &ConversationReply) -> Result<(), InteractionProfileError> {
        validate_reply(&self.profile, reply)
    }

    /// Runs one model turn for `request` and returns a validated
    /// `ConversationReply`.
    pub async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionProfileError> {
        self.validate_request(request)?;
        let system_prompt = self.render_system_prompt();
        let user_context = self.render_provider_context(request)?;
        let reply =
            run_decision::<ConversationReply>(&self.provider, &system_prompt, &user_context)
                .await?;
        self.validate_reply(&reply)?;
        Ok(reply)
    }
}

fn render_system_prompt(profile_prompt: &str) -> String {
    format!("{profile_prompt}\n\n---\n\n{CONVERSATION_REPLY_V1_PROTOCOL_INSTRUCTION}")
}

fn render_provider_context(
    profile: &InteractionProfileConfig,
    request: &ConversationRequest,
) -> Result<String, InteractionProfileError> {
    let context = ProviderConversationContext {
        profile_id: &request.profile_id,
        conversation_id: &request.conversation_id,
        turns: &request.turns,
        context: &request.context,
    };
    let request_json =
        serde_json::to_string_pretty(&context).map_err(InteractionProfileError::RequestContext)?;
    let allowed_proposal_kinds = serde_json::to_string_pretty(profile.allowed_proposal_kinds())
        .map_err(InteractionProfileError::RequestContext)?;
    Ok(format!(
        "Run one interactive conversation turn for this request.\n\nAllowed proposal kinds from the Smith profile config:\n{allowed_proposal_kinds}\n\nConversation request:\n{request_json}"
    ))
}

#[derive(Serialize)]
struct ProviderConversationContext<'a> {
    profile_id: &'a ConversationProfileId,
    conversation_id: &'a ConversationId,
    turns: &'a [ConversationTurn],
    context: &'a Value,
}

fn validate_request(
    profile: &InteractionProfileConfig,
    request: &ConversationRequest,
) -> Result<(), InteractionProfileError> {
    if request.profile_id != *profile.profile_id() {
        return Err(InteractionProfileError::InvalidRequest(format!(
            "interaction profile `{}` cannot serve request for profile `{}`",
            profile.profile_id(),
            request.profile_id
        )));
    }
    if profile.required_context().is_empty() {
        return Ok(());
    }
    let Some(context) = request.context.as_object() else {
        return Err(InteractionProfileError::InvalidRequest(
            "request context must be a JSON object when required context fields are declared"
                .into(),
        ));
    };
    for field in profile.required_context() {
        match context.get(field) {
            Some(value) if !value.is_null() => {}
            _ => {
                return Err(InteractionProfileError::InvalidRequest(format!(
                    "missing required context field `{field}` for profile `{}`",
                    profile.profile_id()
                )));
            }
        }
    }
    Ok(())
}

fn validate_reply(
    profile: &InteractionProfileConfig,
    reply: &ConversationReply,
) -> Result<(), InteractionProfileError> {
    reply.validate()?;
    for proposal in &reply.proposals {
        let Some(contract) = profile.proposal_contract(&proposal.kind) else {
            return Err(InteractionProtocolError::UnsupportedProposalKind {
                id: proposal.id.clone(),
                kind: proposal.kind.clone(),
            }
            .into());
        };
        match contract {
            InteractionProposalPayloadContract::IssueDraft => {
                ProposalPayloadValidator::IssueDraft.validate(proposal)?;
            }
            InteractionProposalPayloadContract::CustomJson => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "interaction_profile_tests.rs"]
mod tests;
