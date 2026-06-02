//! Conversational product-manager interactive profile responder.
//!
//! [`ProductManagerAgent`] is deliberately **not** a workflow
//! [`temper_runner::Agent`]. It runs one LLM turn over a conversation transcript
//! and returns a conversational reply plus draft intake issues. It does not see
//! Forge handles, register SDK tools, or mutate workflow state; the interaction
//! layer owns transcript persistence and explicit issue filing.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use temper_interaction::{
    ConversationReply, ConversationRequest, InteractionError, InteractiveResponder, IssueProposal,
    ParticipantKind, Proposal, ProposalId,
};

use crate::decision::{DecisionError, run_decision};
use crate::prompts::PRODUCT_MANAGER_SYSTEM_PROMPT;
use crate::provider::ProviderConfig;

/// Stable profile id used by the product-manager interactive profile.
pub const PRODUCT_MANAGER_PROFILE_ID: &str = "product-manager";

/// Non-workflow product-manager interactive profile for one-turn planning.
pub struct ProductManagerAgent {
    provider: ProviderConfig,
}

impl ProductManagerAgent {
    /// Builds a product-manager responder using the shared LLM provider config.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }

    /// Runs one LLM turn over the supplied transcript.
    ///
    /// This method performs no Forge mutation and registers no SDK tools. It
    /// returns typed errors so callers can decide how to surface provider,
    /// model-run, parse, or draft-validation failures.
    pub async fn run_turn(
        &self,
        request: &ProductManagerRequest,
    ) -> Result<ProductManagerResponse, ProductManagerError> {
        let context = render_request_context(request)?;
        let response = run_decision::<ProductManagerResponse>(
            &self.provider,
            PRODUCT_MANAGER_SYSTEM_PROMPT,
            &context,
        )
        .await?;
        response.validate()?;
        Ok(response)
    }
}

#[async_trait]
impl InteractiveResponder for ProductManagerAgent {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        if request.profile_id.as_str() != PRODUCT_MANAGER_PROFILE_ID {
            return Err(InteractionError::responder(format!(
                "product-manager responder cannot serve profile `{}`",
                request.profile_id
            )));
        }
        let request = ProductManagerRequest::from_conversation_request(request)?;
        let response = self
            .run_turn(&request)
            .await
            .map_err(|error| InteractionError::profile("product-manager failed", error))?;
        response
            .to_conversation_reply()
            .map_err(|error| InteractionError::profile("product-manager response invalid", error))
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
    ) -> Result<Self, InteractionError> {
        let repository = request
            .context
            .get("repository")
            .and_then(Value::as_str)
            .ok_or_else(|| InteractionError::responder("missing repository context"))?
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
    /// Draft intake issues. These are proposals only; callers decide whether and
    /// when to file them.
    pub drafts: Vec<ProductManagerDraftIssue>,
}

impl ProductManagerResponse {
    /// Validates draft slugs are safe to use in deterministic filing
    /// correlation keys.
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
    pub fn to_conversation_reply(&self) -> Result<ConversationReply, InteractionError> {
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
    /// Builds a product-manager draft from a generic issue proposal.
    pub fn from_issue_proposal(slug: impl Into<String>, proposal: IssueProposal) -> Self {
        Self {
            slug: slug.into(),
            title: proposal.title,
            body: proposal.body,
            rationale: proposal.rationale,
        }
    }

    /// Maps this product-manager compatibility DTO to a generic issue proposal.
    pub fn to_issue_proposal(&self) -> IssueProposal {
        IssueProposal {
            title: self.title.clone(),
            body: self.body.clone(),
            rationale: self.rationale.clone(),
        }
    }

    /// Maps this draft to a generic inert proposal.
    pub fn to_proposal(&self) -> Result<Proposal, InteractionError> {
        Proposal::issue(
            ProposalId::new(self.slug.clone())?,
            self.to_issue_proposal(),
        )
    }
}

/// Returns whether `slug` is safe and deterministic-looking for draft filing.
///
/// A valid slug is non-empty, at most 80 bytes, and contains lowercase ASCII
/// letters/digits separated by single hyphens. It cannot start or end with a
/// hyphen. This validates the stable shape; the prompt is responsible for
/// avoiding random IDs, dates, or timestamps.
pub fn is_valid_draft_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 80 {
        return false;
    }

    let mut previous_hyphen = false;
    for (index, byte) in slug.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' => {
                if index == 0 || index + 1 == slug.len() || previous_hyphen {
                    return false;
                }
                previous_hyphen = true;
            }
            _ => return false,
        }
    }
    true
}

/// Product-manager profile responder failure.
#[derive(Debug)]
pub enum ProductManagerError {
    /// Building the provider, running the model, or parsing the model JSON
    /// failed.
    Decision(DecisionError),
    /// The request could not be serialized into the model context.
    RequestContext(serde_json::Error),
    /// A draft slug does not match the deterministic slug shape.
    InvalidDraftSlug { slug: String },
    /// Two drafts used the same slug, making explicit filing ambiguous.
    DuplicateDraftSlug { slug: String },
}

impl std::fmt::Display for ProductManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProductManagerError::Decision(error) => write!(formatter, "{error}"),
            ProductManagerError::RequestContext(error) => {
                write!(
                    formatter,
                    "serializing product-manager request failed: {error}"
                )
            }
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
            ProductManagerError::RequestContext(error) => Some(error),
            ProductManagerError::InvalidDraftSlug { .. }
            | ProductManagerError::DuplicateDraftSlug { .. } => None,
        }
    }
}

impl From<DecisionError> for ProductManagerError {
    fn from(error: DecisionError) -> Self {
        Self::Decision(error)
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
mod tests {
    use super::*;
    use crate::prompts::PRODUCT_MANAGER_SYSTEM_PROMPT;

    #[test]
    fn product_manager_parses_json_response_with_zero_drafts() {
        let response: ProductManagerResponse = serde_json::from_str(
            r#"{
              "reply": "Let's first clarify the mobile use case.",
              "drafts": []
            }"#,
        )
        .expect("response parses");

        response.validate().expect("drafts validate");
        assert_eq!(response.reply, "Let's first clarify the mobile use case.");
        assert!(response.drafts.is_empty());
    }

    #[test]
    fn product_manager_parses_json_response_with_multiple_drafts() {
        let response: ProductManagerResponse = serde_json::from_str(
            r#"{
              "reply": "I would split this into two cheap dogfood steps.",
              "drafts": [
                {
                  "slug": "matrix-text-adapter",
                  "title": "Add Matrix text adapter for product-manager chat",
                  "body": "Create a Matrix text bridge so Android users can dogfood product-manager chat.",
                  "rationale": "Matrix gives mobile access without building a custom app first."
                },
                {
                  "slug": "local-chat-service-api",
                  "title": "Expose product-manager chat through a local service API",
                  "body": "Add a loopback API that external clients can call for transcript turns and drafts.",
                  "rationale": null
                }
              ]
            }"#,
        )
        .expect("response parses");

        response.validate().expect("drafts validate");
        assert_eq!(response.drafts.len(), 2);
        assert_eq!(response.drafts[0].slug, "matrix-text-adapter");
        assert_eq!(response.drafts[1].rationale, None);
    }

    #[test]
    fn product_manager_maps_generic_interaction_request_and_reply() {
        let request = ConversationRequest {
            profile_id: temper_interaction::ConversationProfileId::new(PRODUCT_MANAGER_PROFILE_ID)
                .expect("valid profile"),
            conversation_id: temper_interaction::ConversationId::new("conversation-1")
                .expect("valid conversation"),
            turns: vec![
                temper_interaction::ConversationTurn::new(
                    temper_interaction::Participant::human("human"),
                    "I want a mobile chat loop.",
                ),
                temper_interaction::ConversationTurn::new(
                    temper_interaction::Participant::agent("product-manager"),
                    "Let's keep it small.",
                ),
                temper_interaction::ConversationTurn::new(
                    temper_interaction::Participant::new(ParticipantKind::System),
                    "ignored runtime note",
                ),
            ],
            context: serde_json::json!({
                "repository": "ai/temper",
                "transcript_url": "https://git.example.test/ai/temper/issues/1"
            }),
        };

        let mapped = ProductManagerRequest::from_conversation_request(&request).unwrap();
        assert_eq!(mapped.repository, "ai/temper");
        assert_eq!(mapped.turns.len(), 2);
        assert_eq!(mapped.turns[0].author, ProductManagerAuthor::Human);
        assert_eq!(mapped.turns[1].author, ProductManagerAuthor::ProductManager);

        let response = ProductManagerResponse {
            reply: "File one small issue.".into(),
            drafts: vec![ProductManagerDraftIssue {
                slug: "mobile-chat-loop".into(),
                title: "Add mobile chat loop".into(),
                body: "Expose chat from a phone-friendly client.".into(),
                rationale: Some("Dogfood from mobile.".into()),
            }],
        };
        let reply = response.to_conversation_reply().unwrap();
        assert_eq!(reply.message, "File one small issue.");
        assert_eq!(reply.proposals[0].id.as_str(), "mobile-chat-loop");
        assert_eq!(
            reply.proposals[0].kind,
            temper_interaction::ProposalKind::issue()
        );
    }

    #[test]
    fn product_manager_validates_draft_slugs() {
        for slug in ["mvp", "matrix-text-adapter", "api-v1", "a1-b2"] {
            assert!(is_valid_draft_slug(slug), "{slug} should be valid");
        }

        for slug in [
            "",
            "Matrix",
            "matrix_text",
            "matrix--text",
            "-matrix",
            "matrix-",
            "matrix text",
            "mátřix",
        ] {
            assert!(!is_valid_draft_slug(slug), "{slug} should be invalid");
        }

        let response = ProductManagerResponse {
            reply: "draft".to_string(),
            drafts: vec![ProductManagerDraftIssue {
                slug: "bad_slug".to_string(),
                title: "Bad slug".to_string(),
                body: "Body".to_string(),
                rationale: None,
            }],
        };
        assert!(matches!(
            response.validate(),
            Err(ProductManagerError::InvalidDraftSlug { .. })
        ));
    }

    #[test]
    fn product_manager_rejects_duplicate_draft_slugs() {
        let draft = ProductManagerDraftIssue {
            slug: "same-draft".to_string(),
            title: "Draft".to_string(),
            body: "Body".to_string(),
            rationale: None,
        };
        let response = ProductManagerResponse {
            reply: "drafts".to_string(),
            drafts: vec![draft.clone(), draft],
        };

        assert!(matches!(
            response.validate(),
            Err(ProductManagerError::DuplicateDraftSlug { .. })
        ));
    }

    #[test]
    fn product_manager_prompt_export_is_wired() {
        assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("product-manager"));
        assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("exactly one"));
        assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("stable"));
    }
}
