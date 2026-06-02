//! Product-manager chat integration core.
//!
//! The product-manager LLM proposes replies and draft intake issues only. This
//! module owns the Forge-facing transcript and explicit `/file` boundary.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use harness_agents::{
    ProductManagerAgent, ProductManagerAuthor, ProductManagerConversationTurn,
    ProductManagerDraftIssue, ProductManagerError, ProductManagerRequest, ProductManagerResponse,
    ProviderError,
};
use harness_forge::{
    CreateComment, CreateIssue, Forge, ForgeError, Issue, IssueQuery, ItemNumber, Repository,
    RepositoryPath, UpdateIssue, User,
};

pub const PRODUCT_LABEL: &str = "product";
pub const WORKFLOW_INTAKE_LABEL: &str = "untriaged";
const RECENT_TURN_LIMIT: usize = 30;

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
    human_forge: Arc<H>,
    product_forge: Arc<P>,
    responder: Arc<R>,
    base_url: String,
    repo_path: RepositoryPath,
    repository: Repository,
    transcript: Issue,
    session_key: String,
    human_user: User,
    turns: Vec<ProductManagerConversationTurn>,
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
        let repository = human_forge
            .get_repository_by_path(&options.repo_path)
            .await?
            .ok_or_else(|| ProductChatError::RepositoryNotFound {
                owner: options.repo_path.owner.clone(),
                name: options.repo_path.name.clone(),
            })?;
        let human_user = human_forge.current_user().await?;
        let product_user = product_forge.current_user().await?;
        let (transcript, session_key) = match options.transcript_issue {
            Some(number) => {
                resume_transcript(
                    human_forge.as_ref(),
                    &repository,
                    number,
                    &options.repo_path,
                )
                .await?
            }
            None => create_transcript(human_forge.as_ref(), &repository).await?,
        };
        let turns = load_recent_turns(
            human_forge.as_ref(),
            &transcript,
            &human_user,
            &product_user,
        )
        .await?;
        Ok(Self {
            human_forge,
            product_forge,
            responder,
            base_url: options.base_url,
            repo_path: options.repo_path,
            repository,
            transcript,
            session_key,
            human_user,
            turns,
            latest_drafts: Vec::new(),
        })
    }

    pub fn transcript_url(&self) -> String {
        issue_url(&self.base_url, &self.repo_path, self.transcript.number)
    }

    pub fn issue_url_for(&self, number: ItemNumber) -> String {
        issue_url(&self.base_url, &self.repo_path, number)
    }

    pub fn transcript_issue(&self) -> &Issue {
        &self.transcript
    }

    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    pub fn latest_drafts(&self) -> &[ProductManagerDraftIssue] {
        &self.latest_drafts
    }

    pub async fn send_human_turn(
        &mut self,
        body: &str,
    ) -> Result<ProductManagerResponse, ProductChatError> {
        self.human_forge
            .add_issue_comment(&self.transcript.id, CreateComment { body: body.into() })
            .await?;
        self.turns.push(ProductManagerConversationTurn {
            author: ProductManagerAuthor::Human,
            body: body.to_string(),
        });

        let request = ProductManagerRequest {
            repository: format!("{}/{}", self.repo_path.owner, self.repo_path.name),
            transcript_url: Some(self.transcript_url()),
            turns: recent_turns(&self.turns),
        };
        let response = self.responder.respond(&request).await?;
        response.validate()?;
        let comment_body = format_product_manager_comment(&response);
        self.product_forge
            .add_issue_comment(
                &self.transcript.id,
                CreateComment {
                    body: comment_body.clone(),
                },
            )
            .await?;
        self.turns.push(ProductManagerConversationTurn {
            author: ProductManagerAuthor::ProductManager,
            body: comment_body,
        });
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
        let marker = render_filing_marker(&self.session_key, &draft.slug);
        if let Some(existing) =
            find_issue_by_marker(self.product_forge.as_ref(), &self.repository, &marker).await?
        {
            return Ok(FileDraftOutcome {
                issue: existing,
                created: false,
            });
        }
        let body = render_filed_issue_body(
            draft,
            &self.transcript_url(),
            &marker,
            Some(&self.human_user.handle),
        );
        let created = self
            .product_forge
            .create_issue(
                &self.repository.id,
                CreateIssue {
                    title: draft.title.clone(),
                    body,
                    labels: vec![WORKFLOW_INTAKE_LABEL.to_string()],
                    assignees: Vec::new(),
                },
            )
            .await?;
        Ok(FileDraftOutcome {
            issue: created,
            created: true,
        })
    }
}

async fn create_transcript<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
) -> Result<(Issue, String), ProductChatError> {
    let session_key = new_session_key();
    let title = format!(
        "Product conversation: {}",
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    );
    let body = render_transcript_marker(&session_key);
    let issue = forge
        .create_issue(
            &repository.id,
            CreateIssue {
                title,
                body,
                labels: vec![PRODUCT_LABEL.to_string()],
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok((issue, session_key))
}

async fn resume_transcript<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    number: ItemNumber,
    repo_path: &RepositoryPath,
) -> Result<(Issue, String), ProductChatError> {
    let issue = forge
        .get_issue_by_number(&repository.id, number)
        .await?
        .ok_or(ProductChatError::TranscriptNotFound {
            number: number.get(),
        })?;
    verify_product_only(&issue)?;
    if let Some(session_key) = parse_transcript_session_key(&issue.body) {
        return Ok((issue, session_key));
    }
    let session_key = format!(
        "issue-{}-{}-{}",
        repo_path
            .owner
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
        repo_path
            .name
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
        number.get()
    );
    let body = append_marker(&issue.body, &render_transcript_marker(&session_key));
    let updated = forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(body),
                set_labels: Some(vec![PRODUCT_LABEL.to_string()]),
                expected_version: Some(issue.version),
                ..UpdateIssue::default()
            },
        )
        .await?;
    Ok((updated, session_key))
}

fn verify_product_only(issue: &Issue) -> Result<(), ProductChatError> {
    let mut labels = issue.labels.clone();
    labels.sort();
    labels.dedup();
    if labels == [PRODUCT_LABEL.to_string()] {
        Ok(())
    } else {
        Err(ProductChatError::TranscriptNotProduct {
            number: issue.number.get(),
            labels: issue.labels.clone(),
        })
    }
}

async fn load_recent_turns<F: Forge + ?Sized>(
    forge: &F,
    transcript: &Issue,
    human: &User,
    product_manager: &User,
) -> Result<Vec<ProductManagerConversationTurn>, ProductChatError> {
    let comments = forge.list_issue_comments(&transcript.id).await?;
    let mut turns = Vec::new();
    for comment in comments {
        let author = if comment.author_id == human.id {
            Some(ProductManagerAuthor::Human)
        } else if comment.author_id == product_manager.id {
            Some(ProductManagerAuthor::ProductManager)
        } else {
            None
        };
        if let Some(author) = author {
            turns.push(ProductManagerConversationTurn {
                author,
                body: comment.body,
            });
        }
    }
    Ok(recent_turns(&turns))
}

fn recent_turns(turns: &[ProductManagerConversationTurn]) -> Vec<ProductManagerConversationTurn> {
    let start = turns.len().saturating_sub(RECENT_TURN_LIMIT);
    turns[start..].to_vec()
}

pub async fn find_issue_by_marker<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    marker: &str,
) -> Result<Option<Issue>, ProductChatError> {
    let issues = forge
        .list_issues(&repository.id, IssueQuery::default())
        .await?;
    Ok(issues.into_iter().find(|issue| issue.body.contains(marker)))
}

pub fn render_transcript_marker(session_key: &str) -> String {
    format!("<!-- harness:product-chat-session={session_key} -->")
}

pub fn parse_transcript_session_key(body: &str) -> Option<String> {
    parse_marker_value(body, "harness:product-chat-session")
}

pub fn render_filing_marker(session_key: &str, draft_slug: &str) -> String {
    format!("<!-- harness:product-chat-file={session_key}:{draft_slug} -->")
}

fn parse_marker_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("<!-- {key}=");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let end = rest.find(" -->")?;
    let value = &rest[..end];
    (!value.is_empty() && !value.contains('\n') && !value.contains("-->"))
        .then(|| value.to_string())
}

fn append_marker(body: &str, marker: &str) -> String {
    if body.trim().is_empty() {
        marker.to_string()
    } else {
        format!("{}\n\n{marker}", body.trim_end())
    }
}

fn render_filed_issue_body(
    draft: &ProductManagerDraftIssue,
    transcript_url: &str,
    marker: &str,
    requested_by: Option<&str>,
) -> String {
    let mut body = draft.body.trim_end().to_string();
    body.push_str("\n\n---\n");
    body.push_str(&format!("Transcript: {transcript_url}\n"));
    if let Some(human) = requested_by.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!("requested-by: {human}\n"));
    }
    body.push('\n');
    body.push_str(marker);
    body
}

fn format_product_manager_comment(response: &ProductManagerResponse) -> String {
    let mut body = response.reply.trim().to_string();
    if !response.drafts.is_empty() {
        body.push_str("\n\nDrafts:\n");
        for (index, draft) in response.drafts.iter().enumerate() {
            body.push_str(&format!("[{}] {}\n", index + 1, draft.title));
            body.push_str(&format!("    slug: {}\n", draft.slug));
            if let Some(rationale) = draft.rationale.as_deref().filter(|text| !text.is_empty()) {
                body.push_str(&format!("    rationale: {rationale}\n"));
            }
        }
    }
    body
}

fn issue_url(base_url: &str, repo: &RepositoryPath, number: ItemNumber) -> String {
    format!(
        "{}/{}/{}/issues/{}",
        base_url.trim_end_matches('/'),
        repo.owner,
        repo.name,
        number.get()
    )
}

fn new_session_key() -> String {
    let timestamp = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros() * 1_000);
    format!("pc-{timestamp}-{}", std::process::id())
}
