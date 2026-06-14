use chrono::Utc;
use temper_forge::{
    CreateIssue, Forge, Issue, ItemNumber, Repository, RepositoryPath, UpdateIssue, User,
};

use crate::proposal_state::{parse_proposal_snapshot_marker, strip_proposal_snapshot_marker};
use crate::{
    CompiledProfileManifest, ConversationId, ConversationProfileId, ConversationTurn,
    InteractionError, Participant, Proposal, is_valid_deterministic_slug,
    validate_deterministic_slug,
};

/// Default number of recent Forge-backed turns supplied to a responder.
pub const DEFAULT_RECENT_TURN_LIMIT: usize = 30;

const MARKER_NAMESPACE_RULE: &str =
    "use 1-80 lowercase ASCII letters or digits separated by single hyphens";

/// Configuration for Forge-backed transcript issues.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeTranscriptConfig {
    /// Profile id supplied to responder requests.
    pub profile_id: ConversationProfileId,
    /// Exact labels allowed on transcript issues.
    pub transcript_labels: Vec<String>,
    /// Prefix used for newly-created transcript issue titles.
    pub transcript_title_prefix: String,
    /// Namespace used in hidden transcript and proposal markers.
    pub marker_namespace: String,
    /// Prefix for generated conversation ids.
    pub conversation_id_prefix: String,
    /// Participant representation for comments authored by the human Forge user.
    pub human_participant: Participant,
    /// Participant representation for comments authored by the agent Forge user.
    pub agent_participant: Participant,
    /// Maximum number of recent comments reconstructed into responder turns.
    pub recent_turn_limit: usize,
}

impl ForgeTranscriptConfig {
    /// Builds validated Forge transcript configuration.
    pub fn new(
        profile_id: ConversationProfileId,
        transcript_label: impl Into<String>,
        transcript_title_prefix: impl Into<String>,
        marker_namespace: impl Into<String>,
        conversation_id_prefix: impl Into<String>,
        human_participant: Participant,
        agent_participant: Participant,
    ) -> Result<Self, InteractionError> {
        let transcript_label = transcript_label.into();
        let transcript_labels = normalize_labels(vec![transcript_label])?;
        let transcript_title_prefix = transcript_title_prefix.into();
        if transcript_title_prefix.trim().is_empty() {
            return Err(InteractionError::InvalidConfig {
                field: "transcript_title_prefix",
                message: "must not be empty".into(),
            });
        }
        let marker_namespace = marker_namespace.into();
        validate_marker_namespace(&marker_namespace)?;
        let conversation_id_prefix = conversation_id_prefix.into();
        validate_deterministic_slug("conversation id prefix", &conversation_id_prefix)?;
        Ok(Self {
            profile_id,
            transcript_labels,
            transcript_title_prefix,
            marker_namespace,
            conversation_id_prefix,
            human_participant,
            agent_participant,
            recent_turn_limit: DEFAULT_RECENT_TURN_LIMIT,
        })
    }

    /// Builds Forge transcript configuration from a compiled profile manifest.
    pub fn from_profile_manifest(manifest: &CompiledProfileManifest) -> Self {
        Self {
            profile_id: manifest.profile.id.clone(),
            transcript_labels: manifest.transcript.labels.clone(),
            transcript_title_prefix: manifest.transcript.title_prefix.clone(),
            marker_namespace: manifest.transcript.marker_namespace.clone(),
            conversation_id_prefix: manifest.profile.id.as_str().to_string(),
            human_participant: manifest.profile.human_participant.clone(),
            agent_participant: manifest.profile.agent_participant.clone(),
            recent_turn_limit: manifest.profile.recent_turn_limit,
        }
    }

    /// Overrides the recent-turn limit used when loading and dispatching turns.
    pub fn with_recent_turn_limit(mut self, limit: usize) -> Self {
        self.recent_turn_limit = limit;
        self
    }
}

fn normalize_labels(labels: Vec<String>) -> Result<Vec<String>, InteractionError> {
    let labels: Vec<String> = labels
        .into_iter()
        .map(|label| label.trim().to_string())
        .collect();
    if labels.is_empty() || labels.iter().any(|label| label.is_empty()) {
        return Err(InteractionError::InvalidConfig {
            field: "transcript_labels",
            message: "must not be empty".into(),
        });
    }
    Ok(labels)
}

/// Repository and optional transcript issue selector for opening a transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeTranscriptOpenOptions {
    /// Repository that owns transcript and intake issues.
    pub repo_path: RepositoryPath,
    /// Existing transcript issue to resume, or `None` to create a new issue.
    pub transcript_issue: Option<ItemNumber>,
}

/// Durable Forge-backed transcript state reconstructed for one conversation.
#[derive(Clone, Debug)]
pub struct ForgeTranscript {
    repository: Repository,
    issue: Issue,
    conversation_id: ConversationId,
    human_user: User,
    agent_user: User,
    turns: Vec<ConversationTurn>,
    latest_proposals: Vec<Proposal>,
}

impl ForgeTranscript {
    /// Repository that owns this transcript.
    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    /// Transcript issue.
    pub fn issue(&self) -> &Issue {
        &self.issue
    }

    /// Durable conversation id stored in the transcript marker.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Human Forge user used to author human turns.
    pub fn human_user(&self) -> &User {
        &self.human_user
    }

    /// Agent Forge user used to author agent turns.
    pub fn agent_user(&self) -> &User {
        &self.agent_user
    }

    /// Recent turns reconstructed from transcript comments.
    pub fn turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    /// Latest durable proposals reconstructed from the newest agent reply marker.
    pub fn latest_proposals(&self) -> &[Proposal] {
        &self.latest_proposals
    }

    pub(crate) fn push_turn(&mut self, turn: ConversationTurn, limit: usize) {
        self.turns.push(turn);
        trim_turns(&mut self.turns, limit);
    }
}

/// Opens a Forge-backed transcript, creating it when no issue number is supplied.
pub async fn open_forge_transcript<H, A>(
    human_forge: &H,
    agent_forge: &A,
    options: ForgeTranscriptOpenOptions,
    config: &ForgeTranscriptConfig,
) -> Result<ForgeTranscript, InteractionError>
where
    H: Forge + ?Sized,
    A: Forge + ?Sized,
{
    let repository = human_forge
        .get_repository_by_path(&options.repo_path)
        .await?
        .ok_or_else(|| InteractionError::RepositoryNotFound {
            owner: options.repo_path.owner.clone(),
            name: options.repo_path.name.clone(),
        })?;
    let human_user = human_forge.current_user().await?;
    let agent_user = agent_forge.current_user().await?;
    let (issue, conversation_id) = match options.transcript_issue {
        Some(number) => resume_transcript(human_forge, &repository, number, config).await?,
        None => create_transcript(human_forge, &repository, config).await?,
    };
    let (turns, latest_proposals) =
        load_recent_turns(human_forge, &issue, &human_user, &agent_user, config).await?;
    Ok(ForgeTranscript {
        repository,
        issue,
        conversation_id,
        human_user,
        agent_user,
        turns,
        latest_proposals,
    })
}

async fn create_transcript<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    config: &ForgeTranscriptConfig,
) -> Result<(Issue, ConversationId), InteractionError> {
    let conversation_id = new_conversation_id(&config.conversation_id_prefix)?;
    let title = format!(
        "{}: {}",
        config.transcript_title_prefix,
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    );
    let body = render_transcript_marker(&config.marker_namespace, conversation_id.as_str());
    let issue = forge
        .create_issue(
            &repository.id,
            CreateIssue {
                title,
                body,
                labels: config.transcript_labels.clone(),
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok((issue, conversation_id))
}

async fn resume_transcript<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    number: ItemNumber,
    config: &ForgeTranscriptConfig,
) -> Result<(Issue, ConversationId), InteractionError> {
    let issue = forge
        .get_issue_by_number(&repository.id, number)
        .await?
        .ok_or(InteractionError::TranscriptNotFound {
            number: number.get(),
        })?;
    verify_transcript_labels(&issue, &config.transcript_labels)?;
    if let Some(session_key) = parse_transcript_session_key(&config.marker_namespace, &issue.body) {
        return Ok((issue, ConversationId::new(session_key)?));
    }
    let conversation_id = legacy_conversation_id(&config.conversation_id_prefix, number)?;
    let body = append_marker(
        &issue.body,
        &render_transcript_marker(&config.marker_namespace, conversation_id.as_str()),
    );
    let updated = forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(body),
                set_labels: Some(config.transcript_labels.clone()),
                expected_version: Some(issue.version),
                ..UpdateIssue::default()
            },
        )
        .await?;
    Ok((updated, conversation_id))
}

fn verify_transcript_labels(
    issue: &Issue,
    transcript_labels: &[String],
) -> Result<(), InteractionError> {
    let mut labels = issue.labels.clone();
    labels.sort();
    labels.dedup();
    let mut expected = transcript_labels.to_vec();
    expected.sort();
    expected.dedup();
    if labels == expected {
        Ok(())
    } else {
        Err(InteractionError::TranscriptLabelMismatch {
            number: issue.number.get(),
            expected_labels: transcript_labels.to_vec(),
            labels: issue.labels.clone(),
        })
    }
}

async fn load_recent_turns<F: Forge + ?Sized>(
    forge: &F,
    transcript: &Issue,
    human: &User,
    agent: &User,
    config: &ForgeTranscriptConfig,
) -> Result<(Vec<ConversationTurn>, Vec<Proposal>), InteractionError> {
    let comments = forge.list_issue_comments(&transcript.id).await?;
    let mut turns = Vec::new();
    let mut latest_proposals = Vec::new();
    for comment in comments {
        let participant = if comment.author_id == human.id {
            Some(config.human_participant.clone())
        } else if comment.author_id == agent.id {
            if let Some(proposals) =
                parse_proposal_snapshot_marker(&config.marker_namespace, &comment.body)?
            {
                latest_proposals = proposals;
            }
            Some(config.agent_participant.clone())
        } else {
            None
        };
        if let Some(participant) = participant {
            turns.push(ConversationTurn::new(
                participant,
                strip_proposal_snapshot_marker(&config.marker_namespace, &comment.body),
            ));
        }
    }
    trim_turns(&mut turns, config.recent_turn_limit);
    Ok((turns, latest_proposals))
}

pub(crate) fn trim_turns(turns: &mut Vec<ConversationTurn>, limit: usize) {
    if limit == 0 {
        turns.clear();
    } else if turns.len() > limit {
        let drop = turns.len() - limit;
        turns.drain(0..drop);
    }
}

/// Renders the hidden marker that stores a transcript conversation id.
pub fn render_transcript_marker(marker_namespace: &str, conversation_id: &str) -> String {
    format!("<!-- temper:{marker_namespace}-session={conversation_id} -->")
}

/// Parses the hidden transcript conversation id marker from an issue body.
pub fn parse_transcript_session_key(marker_namespace: &str, body: &str) -> Option<String> {
    parse_marker_value(body, &format!("temper:{marker_namespace}-session"))
}

/// Renders the hidden marker that correlates an accepted issue proposal.
pub fn render_filing_marker(
    marker_namespace: &str,
    conversation_id: &str,
    proposal_id: &str,
) -> String {
    format!("<!-- temper:{marker_namespace}-file={conversation_id}:{proposal_id} -->")
}

/// Parses a single hidden HTML marker value.
pub fn parse_marker_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("<!-- {key}=");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let end = rest.find(" -->")?;
    let value = &rest[..end];
    (!value.is_empty() && !value.contains('\n') && !value.contains("-->"))
        .then(|| value.to_string())
}

/// Appends a hidden marker to a possibly-empty Markdown body.
pub fn append_marker(body: &str, marker: &str) -> String {
    if body.trim().is_empty() {
        marker.to_string()
    } else {
        format!("{}\n\n{marker}", body.trim_end())
    }
}

/// Renders a Forge issue URL from common owner/name URL shape.
pub fn issue_url(base_url: &str, repo: &RepositoryPath, number: ItemNumber) -> String {
    format!(
        "{}/{}/{}/issues/{}",
        base_url.trim_end_matches('/'),
        repo.owner,
        repo.name,
        number.get()
    )
}

/// Validates a marker namespace.
pub fn validate_marker_namespace(value: &str) -> Result<(), InteractionError> {
    if is_valid_deterministic_slug(value) {
        Ok(())
    } else {
        Err(InteractionError::InvalidMarkerNamespace {
            value: value.to_string(),
            reason: MARKER_NAMESPACE_RULE,
        })
    }
}

fn new_conversation_id(prefix: &str) -> Result<ConversationId, InteractionError> {
    let timestamp = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros() * 1_000);
    Ok(ConversationId::new(format!(
        "{prefix}-{timestamp}-{}",
        std::process::id()
    ))?)
}

fn legacy_conversation_id(
    prefix: &str,
    number: ItemNumber,
) -> Result<ConversationId, InteractionError> {
    Ok(ConversationId::new(format!(
        "{prefix}-issue-{}",
        number.get()
    ))?)
}
