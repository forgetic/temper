//! Conversational product-manager LLM adapter.
//!
//! [`ProductManagerAgent`] is deliberately **not** a workflow
//! [`temper_runner::Agent`]. It runs one LLM turn over a conversation transcript
//! and returns a conversational reply plus draft intake issues. It does not see
//! Forge handles, register SDK tools, or mutate workflow state; Phase 3's
//! integration layer owns transcript persistence and explicit issue filing.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::decision::{DecisionError, run_decision};
use crate::prompts::PRODUCT_MANAGER_SYSTEM_PROMPT;
use crate::provider::ProviderConfig;

/// Non-workflow product-manager agent for one-turn conversation planning.
pub struct ProductManagerAgent {
    provider: ProviderConfig,
}

impl ProductManagerAgent {
    /// Builds a product-manager adapter using the shared LLM provider config.
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
}

/// One draft intake issue proposed by the product manager.
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

/// Product-manager adapter failure.
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
