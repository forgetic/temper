use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::InteractionError;
use crate::types::{ConversationReply, ProposalId, ProposalKind};

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

impl ConversationReply {
    /// Validates reply proposals for deterministic, unambiguous acceptance.
    pub fn validate(&self) -> Result<(), InteractionError> {
        validate_proposal_ids(&self.proposals)
    }
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
