use temper_forge::{CreateIssue, Forge, Issue, IssueQuery, Repository};

pub use temper_process_protocol::interaction::{
    validate_proposal_ids, validate_proposals, IssueProposal, Proposal,
};

use crate::acceptance::IssueAcceptanceOutcome;
use crate::error::InteractionError;
use crate::transcript::{render_filing_marker, validate_marker_namespace};
use crate::types::ConversationId;
use crate::validated::CreateIssueEffect;

/// Deprecated compatibility configuration for filing issue-intake proposals.
/// Prefer compiled acceptance manifests with [`crate::AcceptanceExecutor`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueIntakeAcceptanceConfig {
    /// Hidden marker namespace used for idempotent issue creation.
    pub marker_namespace: String,
    /// Labels applied to created issues.
    pub issue_labels: Vec<String>,
}

impl IssueIntakeAcceptanceConfig {
    /// Builds issue-intake acceptance configuration with one created-issue label.
    pub fn new(
        marker_namespace: impl Into<String>,
        workflow_intake_label: impl Into<String>,
    ) -> Result<Self, InteractionError> {
        Self::with_issue_labels(marker_namespace, vec![workflow_intake_label.into()])
    }

    /// Builds issue-intake acceptance configuration with explicit issue labels.
    pub fn with_issue_labels(
        marker_namespace: impl Into<String>,
        issue_labels: Vec<String>,
    ) -> Result<Self, InteractionError> {
        let marker_namespace = marker_namespace.into();
        validate_marker_namespace(&marker_namespace)?;
        let issue_labels: Vec<String> = issue_labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .collect();
        if issue_labels.is_empty() || issue_labels.iter().any(|label| label.is_empty()) {
            return Err(InteractionError::InvalidConfig {
                field: "issue_labels",
                message: "must not be empty".into(),
            });
        }
        Ok(Self {
            marker_namespace,
            issue_labels,
        })
    }

    /// Builds issue-intake acceptance configuration from a compiled create-issue effect.
    pub fn from_create_issue_effect(effect: &CreateIssueEffect) -> Self {
        Self {
            marker_namespace: effect.marker_namespace().to_string(),
            issue_labels: effect.labels().to_vec(),
        }
    }
}

/// Deprecated compatibility helper that files an issue proposal as workflow intake.
///
/// The helper searches for the hidden correlation marker before creating the
/// issue. Repeating the same acceptance returns the existing issue instead of
/// creating a duplicate. Prefer compiled acceptance manifests with
/// [`crate::AcceptanceExecutor`].
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
                labels: config.issue_labels.clone(),
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
