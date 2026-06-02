//! Product-manager interactive profile wiring.
//!
//! This module keeps the historical product-chat API names while layering the
//! `product-manager` profile over `temper-interaction`'s generic session,
//! transcript, responder, and proposal-acceptance runtime. The profile proposes
//! replies and draft intake issues only; filing still happens through explicit
//! issue-proposal acceptance.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use temper_agents::{
    ProductManagerAgent, ProductManagerDraftIssue, ProductManagerError, ProductManagerResponse,
    ProviderConfig, ProviderError, PRODUCT_MANAGER_PROFILE_ID,
};
use temper_forge::{Forge, ForgeError, Issue, ItemNumber, Repository, RepositoryPath};
use temper_interaction::{
    find_issue_by_marker as find_interaction_issue_by_marker,
    parse_transcript_session_key as parse_interaction_transcript_session_key,
    render_filing_marker as render_interaction_filing_marker,
    render_transcript_marker as render_interaction_transcript_marker, ConversationId,
    ConversationProfileId, ConversationReply, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, ForgeTranscriptConfig, InteractionError, InteractiveResponder,
    IssueIntakeAcceptanceConfig, Participant, ProcessResponder, ProcessResponderConfig, Proposal,
    ProposalId,
};

pub const PRODUCT_LABEL: &str = "product";
pub const WORKFLOW_INTAKE_LABEL: &str = "untriaged";
pub const PRODUCT_MARKER_NAMESPACE: &str = "product-chat";
pub const PRODUCT_PROFILE_ID: &str = PRODUCT_MANAGER_PROFILE_ID;
pub const PRODUCT_CONVERSATION_ID_PREFIX: &str = "pc";

/// Product-manager profile policy over the generic interaction runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductChatProfileConfig {
    /// Generic interaction profile id.
    pub profile_id: &'static str,
    /// Sole label used for transcript issues.
    pub transcript_label: &'static str,
    /// Workflow intake label used when accepted issue proposals are filed.
    pub workflow_intake_label: &'static str,
    /// Prefix for newly-created transcript issue titles.
    pub transcript_title_prefix: &'static str,
    /// Hidden marker namespace for transcripts and accepted proposals.
    pub marker_namespace: &'static str,
    /// Prefix for generated conversation ids.
    pub conversation_id_prefix: &'static str,
}

impl Default for ProductChatProfileConfig {
    fn default() -> Self {
        Self {
            profile_id: PRODUCT_PROFILE_ID,
            transcript_label: PRODUCT_LABEL,
            workflow_intake_label: WORKFLOW_INTAKE_LABEL,
            transcript_title_prefix: "Product conversation",
            marker_namespace: PRODUCT_MARKER_NAMESPACE,
            conversation_id_prefix: PRODUCT_CONVERSATION_ID_PREFIX,
        }
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

/// Builds the configured product-manager profile responder.
///
/// A process responder is selected only when configured; otherwise the in-repo
/// in-process product-manager implementation remains as a compatibility path.
pub fn build_product_profile_responder(
    process: Option<ProcessResponderConfig>,
    provider_config: impl FnOnce() -> Result<ProviderConfig, ProviderError>,
) -> Result<Arc<dyn InteractiveResponder>, ProductChatError> {
    if let Some(config) = process {
        Ok(Arc::new(ProcessResponder::new(config)?) as Arc<dyn InteractiveResponder>)
    } else {
        Ok(Arc::new(ProductManagerAgent::new(provider_config()?)) as Arc<dyn InteractiveResponder>)
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
    R: InteractiveResponder + ?Sized,
> {
    inner: ForgeInteractionSession<H, P, R>,
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
        let inner = ForgeInteractionSession::open(
            human_forge,
            product_forge,
            responder,
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

fn product_session_config() -> Result<ForgeSessionConfig, ProductChatError> {
    let profile = ProductChatProfileConfig::default();
    let transcript = ForgeTranscriptConfig::new(
        ConversationProfileId::new(profile.profile_id)?,
        profile.transcript_label,
        profile.transcript_title_prefix,
        profile.marker_namespace,
        profile.conversation_id_prefix,
        Participant::human("human"),
        Participant::agent(profile.profile_id),
    )?;
    Ok(ForgeSessionConfig::new(
        transcript,
        IssueIntakeAcceptanceConfig::new(profile.marker_namespace, profile.workflow_intake_label)?,
    )?)
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
    render_interaction_transcript_marker(PRODUCT_MARKER_NAMESPACE, session_key)
}

pub fn parse_transcript_session_key(body: &str) -> Option<String> {
    parse_interaction_transcript_session_key(PRODUCT_MARKER_NAMESPACE, body)
}

pub fn render_filing_marker(session_key: &str, draft_slug: &str) -> String {
    render_interaction_filing_marker(PRODUCT_MARKER_NAMESPACE, session_key, draft_slug)
}
