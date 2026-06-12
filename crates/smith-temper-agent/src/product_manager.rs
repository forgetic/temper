//! Product-manager interactive responder profile for Temper's process protocol.
//!
//! This is Smith's copy of the concrete product-manager behavior that Temper can
//! call out of process. It receives only the provider-neutral
//! `ConversationRequest`, runs one LLM turn through Smith's provider core, and
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
    /// Builds a product-manager responder using Smith's provider config.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }

    /// Runs one LLM turn over a generic interaction request.
    pub async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, ProductManagerError> {
        if request.profile_id.as_str() != PRODUCT_MANAGER_PROFILE_ID {
            return Err(ProductManagerError::InvalidRequest(format!(
                "product-manager responder cannot serve profile `{}`",
                request.profile_id
            )));
        }
        let request = ProductManagerRequest::from_conversation_request(request)?;
        let response = self.run_turn(&request).await?;
        response.to_conversation_reply()
    }

    /// Runs one LLM turn over the supplied product-manager request.
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
    /// The request is not a product-manager request Smith can serve.
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
mod tests {
    use super::*;
    use temper_process_protocol::{
        ConversationId, ConversationProfileId, ConversationTurn, Participant, ProposalKind,
    };

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
            profile_id: ConversationProfileId::new(PRODUCT_MANAGER_PROFILE_ID)
                .expect("valid profile"),
            conversation_id: ConversationId::new("conversation-1").expect("valid conversation"),
            turns: vec![
                ConversationTurn::new(Participant::human("human"), "I want a mobile chat loop."),
                ConversationTurn::new(
                    Participant::agent("product-manager"),
                    "Let's keep it small.",
                ),
                ConversationTurn::new(
                    Participant::new(ParticipantKind::System),
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
        reply.validate().expect("reply proposals validate");
        assert_eq!(reply.message, "File one small issue.");
        assert_eq!(reply.proposals[0].id.as_str(), "mobile-chat-loop");
        assert_eq!(reply.proposals[0].kind, ProposalKind::issue());
        assert_eq!(reply.proposals[0].title, "Add mobile chat loop");
        assert_eq!(
            reply.proposals[0].summary.as_deref(),
            Some("Dogfood from mobile.")
        );
        let issue = reply.proposals[0]
            .issue_payload()
            .expect("issue payload decodes")
            .expect("issue payload is present");
        assert_eq!(issue.title, "Add mobile chat loop");
        assert_eq!(issue.body, "Expose chat from a phone-friendly client.");
        assert_eq!(issue.rationale.as_deref(), Some("Dogfood from mobile."));
    }

    #[test]
    fn product_manager_reads_temper_process_request_fixture() {
        let fixture = include_str!(
            "../../../../temper/crates/temper-process-protocol/fixtures/interactive-responder-request.json"
        );
        let request: ConversationRequest = serde_json::from_str(fixture).expect("fixture parses");
        let mapped = ProductManagerRequest::from_conversation_request(&request).unwrap();

        assert_eq!(request.profile_id.as_str(), PRODUCT_MANAGER_PROFILE_ID);
        assert_eq!(mapped.repository, "owner/repo");
        assert_eq!(mapped.turns.len(), 1);
        assert_eq!(mapped.turns[0].author, ProductManagerAuthor::Human);
    }

    #[test]
    fn product_manager_reads_temper_process_reply_fixture_and_issue_payload_contract() {
        let fixture = include_str!(
            "../../../../temper/crates/temper-process-protocol/fixtures/interactive-responder-reply.json"
        );
        let reply: ConversationReply = serde_json::from_str(fixture).expect("fixture parses");
        reply.validate().expect("fixture reply validates");

        assert_eq!(
            reply.message,
            "I would file one issue for the adapter first."
        );
        assert_eq!(reply.proposals.len(), 1);
        let proposal = &reply.proposals[0];
        assert_eq!(proposal.id.as_str(), "mobile-chat-adapter");
        assert_eq!(proposal.kind, ProposalKind::issue());
        assert_eq!(proposal.title, "Add mobile chat adapter");
        assert_eq!(
            proposal.summary.as_deref(),
            Some("Mobile access lets humans keep the conversation moving.")
        );
        let issue = proposal
            .issue_payload()
            .expect("issue payload decodes")
            .expect("issue payload is present");
        assert_eq!(issue.title, "Add mobile chat adapter");
        assert_eq!(
            issue.body,
            "Expose the interaction service through a mobile-friendly adapter."
        );
        assert_eq!(
            issue.rationale.as_deref(),
            Some("Mobile access lets humans keep the conversation moving.")
        );
    }

    // NOTE: a former test here loaded temper-interaction's
    // `product-manager-interaction-spec.json` fixture via `RawInteractionSpec`
    // to cross-check Smith's profile/proposal assumptions against temper's
    // compiled interaction manifest. It was removed when Smith dropped its
    // dependency on `temper-interaction`: that crate is no longer serde-only
    // (it pulls `temper-forge` + `temper-io-engine`), and Smith depends on
    // temper only through the pure serde DTO crates. Smith's own
    // ConversationRequest/Reply parsing stays covered by the
    // `temper-process-protocol` fixture tests above.

    #[tokio::test]
    async fn product_manager_responder_rejects_other_profiles_without_provider_call() {
        let responder = ProductManagerResponder::new(ProviderConfig::new(
            "test-provider",
            "test-model",
            "http://127.0.0.1",
            "dummy-api-key",
        ));
        let request = ConversationRequest::new(
            ConversationProfileId::new("support-agent").expect("valid profile"),
            ConversationId::new("conversation-1").expect("valid conversation"),
            Vec::new(),
        );

        let error = responder
            .respond(&request)
            .await
            .expect_err("non-product profile is rejected before provider call");
        assert!(matches!(
            error,
            ProductManagerError::InvalidRequest(message) if message.contains("support-agent")
        ));
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
