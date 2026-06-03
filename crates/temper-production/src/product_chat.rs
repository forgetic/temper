//! Product-manager interactive profile wiring.
//!
//! This module keeps the historical product-chat API names while layering the
//! `product-manager` profile over `temper-interaction`'s generic session,
//! transcript, responder, and proposal-acceptance runtime. The profile proposes
//! replies and draft intake issues only; filing still happens through explicit
//! issue-proposal acceptance.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use temper_forge::{Forge, ForgeError, Issue, ItemNumber, Repository, RepositoryPath};
use temper_interaction::{
    find_issue_by_marker as find_interaction_issue_by_marker,
    parse_transcript_session_key as parse_interaction_transcript_session_key,
    render_filing_marker as render_interaction_filing_marker,
    render_transcript_marker as render_interaction_transcript_marker, CompiledProfileManifest,
    ConversationId, ConversationReply, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, InteractionError, InteractiveResponder, IssueProposal,
    ProcessResponder, ProcessResponderConfig, Proposal, ProposalId, RawInteractionSpec,
};

const PRODUCT_INTERACTION_SPEC_FIXTURE: &str =
    include_str!("../../temper-interaction/fixtures/product-manager-interaction-spec.json");

/// Structured result of one product-manager profile turn.
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

    /// Maps this product-chat compatibility DTO onto the generic interaction
    /// reply used by fake responders and tests.
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

    /// Maps this compatibility DTO to a generic issue proposal.
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

/// Product-manager profile DTO validation failure.
#[derive(Debug)]
pub enum ProductManagerError {
    InvalidDraftSlug { slug: String },
    DuplicateDraftSlug { slug: String },
}

impl fmt::Display for ProductManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDraftSlug { slug } => {
                write!(formatter, "invalid product-manager draft slug `{slug}`")
            }
            Self::DuplicateDraftSlug { slug } => {
                write!(formatter, "duplicate product-manager draft slug `{slug}`")
            }
        }
    }
}

impl Error for ProductManagerError {}

#[derive(Debug)]
pub enum ProductChatError {
    Forge(ForgeError),
    Interaction(InteractionError),
    ProductManager(ProductManagerError),
    RepositoryNotFound {
        owner: String,
        name: String,
    },
    TranscriptNotFound {
        number: u64,
    },
    TranscriptNotProduct {
        number: u64,
        labels: Vec<String>,
    },
    InvalidDraftNumber {
        requested: usize,
        available: usize,
    },
    DraftNotFound {
        slug: String,
        available: Vec<String>,
    },
    Runtime(String),
    Io(io::Error),
}

impl fmt::Display for ProductChatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProductChatError::Forge(error) => write!(formatter, "forge operation failed: {error}"),
            ProductChatError::Interaction(error) => {
                write!(formatter, "interaction failed: {error}")
            }
            ProductChatError::ProductManager(error) => {
                write!(
                    formatter,
                    "product-manager response failed validation: {error}"
                )
            }
            ProductChatError::RepositoryNotFound { owner, name } => write!(
                formatter,
                "repository {owner}/{name} not found or not readable by the chat token"
            ),
            ProductChatError::TranscriptNotFound { number } => {
                write!(formatter, "transcript issue #{number} was not found")
            }
            ProductChatError::TranscriptNotProduct { number, labels } => write!(
                formatter,
                "issue #{number} is not a product transcript with product-only labels: {labels:?}"
            ),
            ProductChatError::InvalidDraftNumber {
                requested,
                available,
            } => write!(
                formatter,
                "draft #{requested} is not available; latest draft count is {available}"
            ),
            ProductChatError::DraftNotFound { slug, available } => write!(
                formatter,
                "draft slug `{slug}` is not available; latest draft slugs are {available:?}"
            ),
            ProductChatError::Runtime(message) => {
                write!(formatter, "runtime setup failed: {message}")
            }
            ProductChatError::Io(error) => write!(formatter, "terminal I/O failed: {error}"),
        }
    }
}

impl Error for ProductChatError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ProductChatError::Forge(error) => Some(error),
            ProductChatError::Interaction(error) => Some(error),
            ProductChatError::ProductManager(error) => Some(error),
            ProductChatError::Io(error) => Some(error),
            ProductChatError::RepositoryNotFound { .. }
            | ProductChatError::TranscriptNotFound { .. }
            | ProductChatError::TranscriptNotProduct { .. }
            | ProductChatError::InvalidDraftNumber { .. }
            | ProductChatError::DraftNotFound { .. }
            | ProductChatError::Runtime(_) => None,
        }
    }
}

impl From<ForgeError> for ProductChatError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<InteractionError> for ProductChatError {
    fn from(error: InteractionError) -> Self {
        match error {
            InteractionError::Forge(error) => Self::Forge(error),
            InteractionError::RepositoryNotFound { owner, name } => {
                Self::RepositoryNotFound { owner, name }
            }
            InteractionError::TranscriptNotFound { number } => Self::TranscriptNotFound { number },
            InteractionError::TranscriptLabelMismatch { number, labels, .. } => {
                Self::TranscriptNotProduct { number, labels }
            }
            other => Self::Interaction(other),
        }
    }
}

impl From<ProductManagerError> for ProductChatError {
    fn from(error: ProductManagerError) -> Self {
        Self::ProductManager(error)
    }
}

impl From<io::Error> for ProductChatError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Loads the compiled product-manager fixture profile manifest.
pub fn product_profile_manifest() -> Result<CompiledProfileManifest, ProductChatError> {
    let raw: RawInteractionSpec =
        serde_json::from_str(PRODUCT_INTERACTION_SPEC_FIXTURE).map_err(|error| {
            ProductChatError::Runtime(format!("product profile fixture JSON failed: {error}"))
        })?;
    let validated = raw.validate().map_err(|error| {
        ProductChatError::Runtime(format!(
            "product profile fixture validation failed: {error}"
        ))
    })?;
    validated
        .compile()
        .profiles()
        .first()
        .cloned()
        .ok_or_else(|| ProductChatError::Runtime("product profile fixture has no profiles".into()))
}

fn expect_product_profile_manifest() -> CompiledProfileManifest {
    product_profile_manifest().expect("product-manager fixture manifest is valid")
}

/// Builds the configured product-manager profile responder.
///
/// Product-manager behavior is now external to Temper. Operators must configure
/// a process responder such as Smith's `smith-product-manager-responder`; Temper
/// owns transcript persistence, reply validation, and proposal acceptance.
pub fn build_product_profile_responder(
    process: Option<ProcessResponderConfig>,
) -> Result<Arc<dyn InteractiveResponder>, ProductChatError> {
    let Some(config) = process else {
        return Err(ProductChatError::Runtime(
            "product-manager responder process is required; configure --responder-command or TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND".into(),
        ));
    };
    Ok(Arc::new(ProcessResponder::new(config)?) as Arc<dyn InteractiveResponder>)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductChatOpenOptions {
    pub base_url: String,
    pub repo_path: RepositoryPath,
    pub transcript_issue: Option<ItemNumber>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDraftOutcome {
    pub issue: Issue,
    pub created: bool,
}

pub struct ProductChatSession<
    H: Forge + ?Sized,
    P: Forge + ?Sized,
    R: InteractiveResponder + ?Sized,
> {
    inner: ForgeInteractionSession<H, P, R>,
    profile: CompiledProfileManifest,
    latest_drafts: Vec<ProductManagerDraftIssue>,
}

impl<H, P, R> ProductChatSession<H, P, R>
where
    H: Forge + ?Sized,
    P: Forge + ?Sized,
    R: InteractiveResponder + ?Sized,
{
    pub async fn open(
        human_forge: Arc<H>,
        product_forge: Arc<P>,
        responder: Arc<R>,
        options: ProductChatOpenOptions,
    ) -> Result<Self, ProductChatError> {
        Self::open_with_profile_manifest(
            human_forge,
            product_forge,
            responder,
            product_profile_manifest()?,
            options,
        )
        .await
    }

    pub async fn open_with_profile_manifest(
        human_forge: Arc<H>,
        product_forge: Arc<P>,
        responder: Arc<R>,
        profile: CompiledProfileManifest,
        options: ProductChatOpenOptions,
    ) -> Result<Self, ProductChatError> {
        let config = ForgeSessionConfig::from_profile_manifest(&profile)?;
        let inner = ForgeInteractionSession::open(
            human_forge,
            product_forge,
            responder,
            config,
            ForgeSessionOpenOptions {
                base_url: options.base_url,
                repo_path: options.repo_path,
                transcript_issue: options.transcript_issue,
                context: serde_json::json!({}),
            },
        )
        .await?;
        Ok(Self {
            inner,
            profile,
            latest_drafts: Vec::new(),
        })
    }

    pub fn profile(&self) -> &CompiledProfileManifest {
        &self.profile
    }

    pub fn transcript_url(&self) -> String {
        self.inner.transcript_url()
    }

    pub fn issue_url_for(&self, number: ItemNumber) -> String {
        self.inner.issue_url_for(number)
    }

    pub fn transcript_issue(&self) -> &Issue {
        self.inner.transcript_issue()
    }

    pub fn session_key(&self) -> &str {
        self.inner.conversation_id().as_str()
    }

    pub fn conversation_id(&self) -> &ConversationId {
        self.inner.conversation_id()
    }

    pub fn latest_drafts(&self) -> &[ProductManagerDraftIssue] {
        &self.latest_drafts
    }

    pub fn latest_proposals(&self) -> &[Proposal] {
        self.inner.latest_proposals()
    }

    pub async fn send_conversation_turn(
        &mut self,
        body: &str,
    ) -> Result<ConversationReply, ProductChatError> {
        let reply = self.inner.send_human_turn(body).await?;
        let response = product_response_from_reply(&reply)?;
        self.latest_drafts = response.drafts;
        Ok(reply)
    }

    pub async fn send_human_turn(
        &mut self,
        body: &str,
    ) -> Result<ProductManagerResponse, ProductChatError> {
        let reply = self.send_conversation_turn(body).await?;
        Ok(ProductManagerResponse {
            reply: reply.message,
            drafts: self.latest_drafts.clone(),
        })
    }

    pub async fn file_draft(&self, number: usize) -> Result<FileDraftOutcome, ProductChatError> {
        if number == 0 || number > self.latest_drafts.len() {
            return Err(ProductChatError::InvalidDraftNumber {
                requested: number,
                available: self.latest_drafts.len(),
            });
        }
        self.file_draft_issue(&self.latest_drafts[number - 1]).await
    }

    pub async fn file_draft_slug(&self, slug: &str) -> Result<FileDraftOutcome, ProductChatError> {
        let draft = self
            .latest_drafts
            .iter()
            .find(|draft| draft.slug == slug)
            .ok_or_else(|| ProductChatError::DraftNotFound {
                slug: slug.to_string(),
                available: self
                    .latest_drafts
                    .iter()
                    .map(|draft| draft.slug.clone())
                    .collect(),
            })?;
        self.file_draft_issue(draft).await
    }

    pub async fn accept_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<FileDraftOutcome, ProductChatError> {
        let outcome = self.inner.accept_issue_proposal(proposal_id).await?;
        Ok(FileDraftOutcome {
            issue: outcome.issue,
            created: outcome.created,
        })
    }

    async fn file_draft_issue(
        &self,
        draft: &ProductManagerDraftIssue,
    ) -> Result<FileDraftOutcome, ProductChatError> {
        let id = ProposalId::new(draft.slug.clone())?;
        self.accept_proposal(&id).await
    }
}

fn product_response_from_reply(
    reply: &ConversationReply,
) -> Result<ProductManagerResponse, ProductChatError> {
    let drafts = reply
        .proposals
        .iter()
        .filter_map(|proposal| match proposal.issue_payload() {
            Ok(Some(issue)) => Some(Ok(ProductManagerDraftIssue::from_issue_proposal(
                proposal.id.to_string(),
                issue,
            ))),
            Ok(None) => None,
            Err(error) => Some(Err(ProductChatError::from(error))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let response = ProductManagerResponse {
        reply: reply.message.clone(),
        drafts,
    };
    response.validate()?;
    Ok(response)
}

pub async fn find_issue_by_marker<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    marker: &str,
) -> Result<Option<Issue>, ProductChatError> {
    Ok(find_interaction_issue_by_marker(forge, repository, marker).await?)
}

pub fn render_transcript_marker(session_key: &str) -> String {
    let manifest = expect_product_profile_manifest();
    render_interaction_transcript_marker(&manifest.transcript.marker_namespace, session_key)
}

pub fn parse_transcript_session_key(body: &str) -> Option<String> {
    let manifest = expect_product_profile_manifest();
    parse_interaction_transcript_session_key(&manifest.transcript.marker_namespace, body)
}

pub fn render_filing_marker(session_key: &str, draft_slug: &str) -> String {
    let manifest = expect_product_profile_manifest();
    let marker_namespace = manifest
        .acceptance_actions
        .iter()
        .flat_map(|action| action.effects.iter())
        .map(|effect| match effect {
            temper_interaction::AcceptanceEffect::CreateIssue(effect) => effect.marker_namespace(),
        })
        .next()
        .unwrap_or(&manifest.transcript.marker_namespace);
    render_interaction_filing_marker(marker_namespace, session_key, draft_slug)
}
