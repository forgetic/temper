//! Product-manager chat integration core.
//!
//! The product-manager LLM proposes replies and draft intake issues only. This
//! module is now a compatibility wrapper over `temper-interaction`: the generic
//! interaction layer owns Forge-backed transcripts and explicit idempotent issue
//! acceptance, while this module maps product-manager request/response names.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use temper_agents::{
    ProductManagerAgent, ProductManagerAuthor, ProductManagerConversationTurn,
    ProductManagerDraftIssue, ProductManagerError, ProductManagerRequest, ProductManagerResponse,
    ProviderError,
};
use temper_forge::{Forge, ForgeError, Issue, ItemNumber, Repository, RepositoryPath};
use temper_interaction::{
    find_issue_by_marker as find_interaction_issue_by_marker,
    parse_transcript_session_key as parse_interaction_transcript_session_key,
    render_filing_marker as render_interaction_filing_marker,
    render_transcript_marker as render_interaction_transcript_marker, ConversationProfileId,
    ConversationReply, ConversationRequest, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, ForgeTranscriptConfig, InteractionError, InteractiveResponder,
    IssueIntakeAcceptanceConfig, IssueProposal, Participant, ParticipantKind, Proposal, ProposalId,
};

pub const PRODUCT_LABEL: &str = "product";
pub const WORKFLOW_INTAKE_LABEL: &str = "untriaged";
pub const PRODUCT_MARKER_NAMESPACE: &str = "product-chat";
const PRODUCT_PROFILE_ID: &str = "product-manager";
const PRODUCT_CONVERSATION_ID_PREFIX: &str = "pc";

#[async_trait]
pub trait ProductManagerResponder: Send + Sync {
    async fn respond(
        &self,
        request: &ProductManagerRequest,
    ) -> Result<ProductManagerResponse, ProductManagerError>;
}

#[async_trait]
impl ProductManagerResponder for ProductManagerAgent {
    async fn respond(
        &self,
        request: &ProductManagerRequest,
    ) -> Result<ProductManagerResponse, ProductManagerError> {
        self.run_turn(request).await
    }
}

#[derive(Debug)]
pub enum ProductChatError {
    Forge(ForgeError),
    Interaction(InteractionError),
    ProductManager(ProductManagerError),
    Provider(ProviderError),
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
                write!(formatter, "product-manager failed: {error}")
            }
            ProductChatError::Provider(error) => {
                write!(formatter, "provider setup failed: {error}")
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
            ProductChatError::Provider(error) => Some(error),
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

impl From<ProviderError> for ProductChatError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

impl From<io::Error> for ProductChatError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
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
    R: ProductManagerResponder + ?Sized,
> {
    inner: ForgeInteractionSession<H, P, ProductManagerInteractionAdapter<R>>,
    latest_drafts: Vec<ProductManagerDraftIssue>,
}

impl<H, P, R> ProductChatSession<H, P, R>
where
    H: Forge + ?Sized,
    P: Forge + ?Sized,
    R: ProductManagerResponder + ?Sized,
{
    pub async fn open(
        human_forge: Arc<H>,
        product_forge: Arc<P>,
        responder: Arc<R>,
        options: ProductChatOpenOptions,
    ) -> Result<Self, ProductChatError> {
        let adapter = Arc::new(ProductManagerInteractionAdapter { inner: responder });
        let inner = ForgeInteractionSession::open(
            human_forge,
            product_forge,
            adapter,
            product_session_config()?,
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
            latest_drafts: Vec::new(),
        })
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

    pub fn latest_drafts(&self) -> &[ProductManagerDraftIssue] {
        &self.latest_drafts
    }

    pub async fn send_human_turn(
        &mut self,
        body: &str,
    ) -> Result<ProductManagerResponse, ProductChatError> {
        let reply = self.inner.send_human_turn(body).await?;
        let response = product_response_from_reply(&reply)?;
        self.latest_drafts = response.drafts.clone();
        Ok(response)
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

    async fn file_draft_issue(
        &self,
        draft: &ProductManagerDraftIssue,
    ) -> Result<FileDraftOutcome, ProductChatError> {
        let id = ProposalId::new(draft.slug.clone())?;
        let outcome = self.inner.accept_issue_proposal(&id).await?;
        Ok(FileDraftOutcome {
            issue: outcome.issue,
            created: outcome.created,
        })
    }
}

struct ProductManagerInteractionAdapter<R: ProductManagerResponder + ?Sized> {
    inner: Arc<R>,
}

#[async_trait]
impl<R> InteractiveResponder for ProductManagerInteractionAdapter<R>
where
    R: ProductManagerResponder + ?Sized,
{
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
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
        let response = self
            .inner
            .respond(&ProductManagerRequest {
                repository,
                transcript_url,
                turns,
            })
            .await
            .map_err(|error| InteractionError::profile("product-manager failed", error))?;
        response.validate().map_err(|error| {
            InteractionError::profile("product-manager response invalid", error)
        })?;
        conversation_reply_from_product_response(&response)
    }
}

fn product_session_config() -> Result<ForgeSessionConfig, ProductChatError> {
    let transcript = ForgeTranscriptConfig::new(
        ConversationProfileId::new(PRODUCT_PROFILE_ID)?,
        PRODUCT_LABEL,
        "Product conversation",
        PRODUCT_MARKER_NAMESPACE,
        PRODUCT_CONVERSATION_ID_PREFIX,
        Participant::human("human"),
        Participant::agent("product-manager"),
    )?;
    Ok(ForgeSessionConfig::new(
        transcript,
        IssueIntakeAcceptanceConfig::new(PRODUCT_MARKER_NAMESPACE, WORKFLOW_INTAKE_LABEL)?,
    )?)
}

fn conversation_reply_from_product_response(
    response: &ProductManagerResponse,
) -> Result<ConversationReply, InteractionError> {
    let proposals = response
        .drafts
        .iter()
        .map(draft_to_proposal)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ConversationReply {
        message: response.reply.clone(),
        proposals,
    })
}

fn draft_to_proposal(draft: &ProductManagerDraftIssue) -> Result<Proposal, InteractionError> {
    Proposal::issue(
        ProposalId::new(draft.slug.clone())?,
        IssueProposal {
            title: draft.title.clone(),
            body: draft.body.clone(),
            rationale: draft.rationale.clone(),
        },
    )
}

fn product_response_from_reply(
    reply: &ConversationReply,
) -> Result<ProductManagerResponse, ProductChatError> {
    let drafts = reply
        .proposals
        .iter()
        .filter_map(|proposal| match proposal.issue_payload() {
            Ok(Some(issue)) => Some(Ok(ProductManagerDraftIssue {
                slug: proposal.id.to_string(),
                title: issue.title,
                body: issue.body,
                rationale: issue.rationale,
            })),
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
    render_interaction_transcript_marker(PRODUCT_MARKER_NAMESPACE, session_key)
}

pub fn parse_transcript_session_key(body: &str) -> Option<String> {
    parse_interaction_transcript_session_key(PRODUCT_MARKER_NAMESPACE, body)
}

pub fn render_filing_marker(session_key: &str, draft_slug: &str) -> String {
    render_interaction_filing_marker(PRODUCT_MARKER_NAMESPACE, session_key, draft_slug)
}
