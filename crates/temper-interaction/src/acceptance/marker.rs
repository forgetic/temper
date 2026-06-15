//! Idempotency markers, issue lookup, and acceptance-action selection.
//!
//! Acceptance is made idempotent by hidden correlation markers rendered from an
//! action's idempotency-key template. This module owns marker rendering, the
//! marker-based issue lookup, and the rule that resolves which acceptance action
//! handles a given proposal kind.

use temper_forge_model::{Forge, Issue, IssueQuery, Repository};

use crate::ids::AcceptanceActionId;
use crate::types::{ProposalId, ProposalKind};
use crate::{AcceptanceManifest, CompiledProfileManifest, InteractionError, IssueProposal};

pub(super) fn select_acceptance_action<'a>(
    profile: &'a CompiledProfileManifest,
    proposal_id: &ProposalId,
    proposal_kind: &ProposalKind,
    requested: Option<&AcceptanceActionId>,
) -> Result<&'a AcceptanceManifest, InteractionError> {
    if let Some(requested) = requested {
        let action = profile.acceptance_action(requested).ok_or_else(|| {
            InteractionError::InvalidConfig {
                field: "acceptance_action",
                message: format!("acceptance action `{requested}` is not declared"),
            }
        })?;
        if &action.proposal_kind != proposal_kind {
            return Err(InteractionError::UnsupportedProposalKind {
                id: proposal_id.clone(),
                kind: proposal_kind.clone(),
            });
        }
        return Ok(action);
    }

    let mut matches = profile
        .acceptance_actions
        .iter()
        .filter(|action| &action.proposal_kind == proposal_kind);
    let Some(action) = matches.next() else {
        return Err(InteractionError::UnsupportedProposalKind {
            id: proposal_id.clone(),
            kind: proposal_kind.clone(),
        });
    };
    if matches.next().is_some() {
        return Err(InteractionError::InvalidConfig {
            field: "acceptance_actions",
            message: format!(
                "multiple acceptance actions accept proposal kind `{proposal_kind}`; transport must select one"
            ),
        });
    }
    Ok(action)
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

/// Renders the generic hidden acceptance marker used for idempotency.
pub fn render_acceptance_marker(
    marker_namespace: &str,
    marker_key: &str,
    idempotency_key: &str,
) -> String {
    format!("<!-- temper:{marker_namespace}-{marker_key}={idempotency_key} -->")
}

pub(super) fn effect_marker_key<'a>(
    marker_key: Option<&'a str>,
    action: &'a AcceptanceManifest,
) -> &'a str {
    marker_key.unwrap_or_else(|| action.id.as_str())
}

pub(super) fn validate_marker_value(
    field: &'static str,
    value: &str,
) -> Result<(), InteractionError> {
    if !value.trim().is_empty() && !value.contains('\n') && !value.contains("-->") {
        Ok(())
    } else {
        Err(InteractionError::InvalidConfig {
            field,
            message: "rendered marker value must be non-empty and fit in one HTML comment".into(),
        })
    }
}

/// Renders the body used by the deprecated issue-intake helper.
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

pub(super) fn append_visible_line(mut body: String, line: &str) -> String {
    if body.trim().is_empty() {
        line.to_string()
    } else {
        body.push_str("\n\n---\n");
        body.push_str(line);
        body
    }
}
