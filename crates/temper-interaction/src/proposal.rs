use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use temper_forge::{CreateIssue, Forge, Issue, IssueQuery, Repository};

use crate::error::InteractionError;
use crate::transcript::render_filing_marker;
use crate::types::{ConversationId, ConversationReply, ProposalId, ProposalKind};

/// Draft shape for a proposal that can create a normal Forge issue when accepted.
///
/// The proposal is inert by itself. A later acceptance service decides whether
/// and how to file it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueProposal {
    /// Issue title to use if this proposal is accepted.
    pub title: String,
    /// Issue body to use if this proposal is accepted.
    pub body: String,
    /// Optional human-readable reason the issue is worth filing.
    pub rationale: Option<String>,
}

impl IssueProposal {
    /// Builds an issue proposal with no rationale.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            rationale: None,
        }
    }

    /// Builds an issue proposal with a rationale.
    pub fn with_rationale(
        title: impl Into<String>,
        body: impl Into<String>,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            rationale: Some(rationale.into()),
        }
    }
}

/// A responder-suggested action that remains inert until explicitly accepted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Stable deterministic id used for display, de-duplication, and acceptance.
    pub id: ProposalId,
    /// Typed proposal kind. Profile-specific kinds use the same slug rule.
    pub kind: ProposalKind,
    /// Short display title.
    pub title: String,
    /// Optional display summary or rationale.
    pub summary: Option<String>,
    /// Profile-specific payload for the acceptance path.
    #[serde(default)]
    pub payload: Value,
}

impl Proposal {
    /// Builds a custom proposal with an arbitrary JSON payload.
    pub fn custom(
        id: ProposalId,
        kind: ProposalKind,
        title: impl Into<String>,
        summary: Option<String>,
        payload: Value,
    ) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            summary,
            payload,
        }
    }

    /// Builds an issue proposal and stores the typed issue draft as JSON payload.
    pub fn issue(id: ProposalId, issue: IssueProposal) -> Result<Self, InteractionError> {
        let title = issue.title.clone();
        let summary = issue.rationale.clone();
        Ok(Self {
            id,
            kind: ProposalKind::issue(),
            title,
            summary,
            payload: serde_json::to_value(issue)?,
        })
    }

    /// Decodes this proposal's payload as an [`IssueProposal`] when its kind is issue.
    pub fn issue_payload(&self) -> Result<Option<IssueProposal>, InteractionError> {
        if self.kind != ProposalKind::issue() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(self.payload.clone())?))
    }
}

/// Profile configuration for filing issue-intake proposals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueIntakeAcceptanceConfig {
    /// Hidden marker namespace shared with the transcript config.
    pub marker_namespace: String,
    /// Workflow intake label applied to created issues.
    pub workflow_intake_label: String,
}

impl IssueIntakeAcceptanceConfig {
    /// Builds issue-intake acceptance configuration.
    pub fn new(
        marker_namespace: impl Into<String>,
        workflow_intake_label: impl Into<String>,
    ) -> Result<Self, InteractionError> {
        let marker_namespace = marker_namespace.into();
        crate::transcript::validate_marker_namespace(&marker_namespace)?;
        let workflow_intake_label = workflow_intake_label.into();
        if workflow_intake_label.trim().is_empty() {
            return Err(InteractionError::InvalidConfig {
                field: "workflow_intake_label",
                message: "must not be empty".into(),
            });
        }
        Ok(Self {
            marker_namespace,
            workflow_intake_label,
        })
    }
}

/// Result of accepting an issue proposal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueAcceptanceOutcome {
    /// Existing or newly-created issue.
    pub issue: Issue,
    /// Whether this acceptance call created the issue.
    pub created: bool,
}

/// Idempotently files an issue proposal as a normal workflow intake issue.
///
/// The helper searches for the hidden correlation marker before creating the
/// issue. Repeating the same acceptance returns the existing issue instead of
/// creating a duplicate.
pub async fn accept_issue_intake_proposal<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    config: &IssueIntakeAcceptanceConfig,
    conversation_id: &ConversationId,
    proposal: &Proposal,
    transcript_url: &str,
    requested_by: Option<&str>,
) -> Result<IssueAcceptanceOutcome, InteractionError> {
    let draft =
        proposal
            .issue_payload()?
            .ok_or_else(|| InteractionError::UnsupportedProposalKind {
                id: proposal.id.clone(),
                kind: proposal.kind.clone(),
            })?;
    let marker = render_filing_marker(
        &config.marker_namespace,
        conversation_id.as_str(),
        proposal.id.as_str(),
    );
    if let Some(existing) = find_issue_by_marker(forge, repository, &marker).await? {
        return Ok(IssueAcceptanceOutcome {
            issue: existing,
            created: false,
        });
    }
    let body = render_filed_issue_body(&draft, transcript_url, &marker, requested_by);
    let issue = forge
        .create_issue(
            &repository.id,
            CreateIssue {
                title: draft.title,
                body,
                labels: vec![config.workflow_intake_label.clone()],
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(IssueAcceptanceOutcome {
        issue,
        created: true,
    })
}

/// Finds an issue whose body contains a hidden marker.
pub async fn find_issue_by_marker<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    marker: &str,
) -> Result<Option<Issue>, InteractionError> {
    let issues = forge
        .list_issues(&repository.id, IssueQuery::default())
        .await?;
    Ok(issues.into_iter().find(|issue| issue.body.contains(marker)))
}

/// Renders the body used for an accepted issue proposal.
pub fn render_filed_issue_body(
    draft: &IssueProposal,
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

impl ConversationReply {
    /// Validates reply proposals for deterministic, unambiguous acceptance.
    pub fn validate(&self) -> Result<(), InteractionError> {
        validate_proposals(&self.proposals)
    }
}

/// Validates proposal ids and built-in kind payloads in one responder reply.
pub fn validate_proposals(proposals: &[Proposal]) -> Result<(), InteractionError> {
    validate_proposal_ids(proposals)?;
    for proposal in proposals {
        if proposal.kind == ProposalKind::issue() {
            proposal.issue_payload()?;
        }
    }
    Ok(())
}

/// Rejects duplicate proposal ids in one responder reply.
pub fn validate_proposal_ids(proposals: &[Proposal]) -> Result<(), InteractionError> {
    let mut seen = HashSet::new();
    for proposal in proposals {
        if !seen.insert(proposal.id.clone()) {
            return Err(InteractionError::DuplicateProposalId {
                id: proposal.id.clone(),
            });
        }
    }
    Ok(())
}
