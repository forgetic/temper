//! Product-manager interactive responder profile for Temper's process protocol.
//!
//! This is anvil's copy of the concrete product-manager behavior that Temper can
//! call out of process. It receives only the provider-neutral
//! `ConversationRequest`, runs one LLM turn through anvil's provider core, and
//! returns a `ConversationReply` with inert issue proposals. It does not receive
//! Forge handles, Forge tokens, or workflow mutation tools.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use temper_process_protocol::{
    ConversationReply, ConversationRequest, InteractionProtocolError, IssueProposal,
    ParticipantKind, Proposal, ProposalId,
};

use crate::decision::{DecisionError, run_decision};
use crate::provider::ProviderConfig;

/// Stable profile id used by the product-manager interactive profile.
pub const PRODUCT_MANAGER_PROFILE_ID: &str = "product-manager";

/// Product-manager profile system prompt.
pub const PRODUCT_MANAGER_SYSTEM_PROMPT: &str = include_str!("prompts/product_manager.md");

/// Non-workflow product-manager interactive responder for one-turn planning.
pub struct ProductManagerResponder {
    provider: ProviderConfig,
}

impl ProductManagerResponder {
    /// Builds a product-manager responder using anvil's provider config.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }

    /// Runs one LLM turn over a generic interaction request.
    pub async fn respond(
        &self,
        handle: skein::runtime::RuntimeHandle,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, ProductManagerError> {
        if request.profile_id.as_str() != PRODUCT_MANAGER_PROFILE_ID {
            return Err(ProductManagerError::InvalidRequest(format!(
                "product-manager responder cannot serve profile `{}`",
                request.profile_id
            )));
        }
        let request = ProductManagerRequest::from_conversation_request(request)?;
        let response = self.run_turn(handle, &request).await?;
        response.to_conversation_reply()
    }

    /// Runs one LLM turn over the supplied product-manager request.
    pub async fn run_turn(
        &self,
        handle: skein::runtime::RuntimeHandle,
        request: &ProductManagerRequest,
    ) -> Result<ProductManagerResponse, ProductManagerError> {
        let context = render_request_context(request)?;
        let response = run_decision::<ProductManagerResponse>(
            handle,
            &self.provider,
            PRODUCT_MANAGER_SYSTEM_PROMPT,
            &context,
        )
        .await?;
        response.validate()?;
        Ok(response)
    }
}

/// One author in a product-manager transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductManagerAuthor {
    /// A human product stakeholder or operator.
    Human,
    /// A prior product-manager assistant reply.
    ProductManager,
}

/// One turn in the product-manager conversation transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductManagerConversationTurn {
    /// Who authored the turn.
    pub author: ProductManagerAuthor,
    /// Turn text as shown to the model.
    pub body: String,
}

/// Input for one product-manager LLM turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductManagerRequest {
    /// Repository the conversation is about (for example, `owner/repo`).
    pub repository: String,
    /// Optional URL of the transcript issue or external transcript.
    pub transcript_url: Option<String>,
    /// Ordered conversation turns.
    pub turns: Vec<ProductManagerConversationTurn>,
}

impl ProductManagerRequest {
    /// Maps a generic interaction request into the product-manager profile input.
    pub fn from_conversation_request(
        request: &ConversationRequest,
    ) -> Result<Self, ProductManagerError> {
        let repository = request
            .context
            .get("repository")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProductManagerError::InvalidRequest("missing repository context".into())
            })?
            .to_string();
        let transcript_url = request
            .context
            .get("transcript_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let turns = request
            .turns
            .iter()
            .filter_map(|turn| {
                let author = match turn.participant.kind {
                    ParticipantKind::Human => ProductManagerAuthor::Human,
                    ParticipantKind::Agent => ProductManagerAuthor::ProductManager,
                    ParticipantKind::System => return None,
                };
                Some(ProductManagerConversationTurn {
                    author,
                    body: turn.body.clone(),
                })
            })
            .collect();
        Ok(Self {
            repository,
            transcript_url,
            turns,
        })
    }
}

/// Structured result of one product-manager LLM turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductManagerResponse {
    /// Conversational reply to show to the human.
    pub reply: String,
    /// Draft intake issues. These are proposals only; Temper decides whether and
    /// when to file them.
    pub drafts: Vec<ProductManagerDraftIssue>,
}

impl ProductManagerResponse {
    /// Validates draft slugs are safe to use in deterministic filing correlation keys.
    pub fn validate(&self) -> Result<(), ProductManagerError> {
        let mut seen = HashSet::new();
        for draft in &self.drafts {
            if !is_valid_draft_slug(&draft.slug) {
                return Err(ProductManagerError::InvalidDraftSlug {
                    slug: draft.slug.clone(),
                });
            }
            if !seen.insert(draft.slug.as_str()) {
                return Err(ProductManagerError::DuplicateDraftSlug {
                    slug: draft.slug.clone(),
                });
            }
        }
        Ok(())
    }

    /// Maps this profile-specific response onto the generic interaction reply.
    pub fn to_conversation_reply(&self) -> Result<ConversationReply, ProductManagerError> {
        let proposals = self
            .drafts
            .iter()
            .map(ProductManagerDraftIssue::to_proposal)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ConversationReply {
            message: self.reply.clone(),
            proposals,
        })
    }
}

/// One draft intake issue proposed by the product-manager profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductManagerDraftIssue {
    /// Stable deterministic identifier for explicit filing correlation keys.
    pub slug: String,
    /// Issue title to file if the human chooses this draft.
    pub title: String,
    /// Issue body to file as workflow intake.
    pub body: String,
    /// Optional reason this draft is worth filing.
    pub rationale: Option<String>,
}

impl ProductManagerDraftIssue {
    /// Maps this draft to a generic inert proposal.
    pub fn to_proposal(&self) -> Result<Proposal, ProductManagerError> {
        Proposal::issue(
            ProposalId::new(self.slug.clone())?,
            IssueProposal {
                title: self.title.clone(),
                body: self.body.clone(),
                rationale: self.rationale.clone(),
            },
        )
        .map_err(ProductManagerError::from)
    }
}

/// Returns whether `slug` is safe and deterministic-looking for draft filing.
///
/// A valid slug is non-empty, at most 80 bytes, and contains lowercase ASCII
/// letters/digits separated by single hyphens. It cannot start or end with a
/// hyphen. This validates the stable shape; the prompt is responsible for
/// avoiding random IDs, dates, or timestamps.
pub fn is_valid_draft_slug(slug: &str) -> bool {
    temper_process_protocol::is_valid_deterministic_slug(slug)
}

/// Product-manager profile responder failure.
#[derive(Debug)]
pub enum ProductManagerError {
    /// Building the provider, running the model, or parsing the model JSON failed.
    Decision(DecisionError),
    /// The process-protocol request or proposal mapping was invalid.
    Protocol(InteractionProtocolError),
    /// The request could not be serialized into the model context.
    RequestContext(serde_json::Error),
    /// The request is not a product-manager request anvil can serve.
    InvalidRequest(String),
    /// A draft slug does not match the deterministic slug shape.
    InvalidDraftSlug { slug: String },
    /// Two drafts used the same slug, making explicit filing ambiguous.
    DuplicateDraftSlug { slug: String },
}

impl std::fmt::Display for ProductManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProductManagerError::Decision(error) => write!(formatter, "{error}"),
            ProductManagerError::Protocol(error) => write!(formatter, "{error}"),
            ProductManagerError::RequestContext(error) => {
                write!(
                    formatter,
                    "serializing product-manager request failed: {error}"
                )
            }
            ProductManagerError::InvalidRequest(message) => formatter.write_str(message),
            ProductManagerError::InvalidDraftSlug { slug } => {
                write!(formatter, "invalid product-manager draft slug `{slug}`")
            }
            ProductManagerError::DuplicateDraftSlug { slug } => {
                write!(formatter, "duplicate product-manager draft slug `{slug}`")
            }
        }
    }
}

impl std::error::Error for ProductManagerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProductManagerError::Decision(error) => Some(error),
            ProductManagerError::Protocol(error) => Some(error),
            ProductManagerError::RequestContext(error) => Some(error),
            ProductManagerError::InvalidRequest(_)
            | ProductManagerError::InvalidDraftSlug { .. }
            | ProductManagerError::DuplicateDraftSlug { .. } => None,
        }
    }
}

impl From<DecisionError> for ProductManagerError {
    fn from(error: DecisionError) -> Self {
        Self::Decision(error)
    }
}

impl From<InteractionProtocolError> for ProductManagerError {
    fn from(error: InteractionProtocolError) -> Self {
        Self::Protocol(error)
    }
}

fn render_request_context(request: &ProductManagerRequest) -> Result<String, ProductManagerError> {
    let json =
        serde_json::to_string_pretty(request).map_err(ProductManagerError::RequestContext)?;
    Ok(format!(
        "Run one product-manager turn over this transcript. Return only the JSON response.\n\n{json}"
    ))
}

#[cfg(test)]
#[path = "product_manager_tests.rs"]
mod tests;
