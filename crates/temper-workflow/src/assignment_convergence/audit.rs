//! Bounded, actionable audit publication for assignment quarantine.

use std::collections::BTreeSet;

use temper_forge::{CreateComment, Forge, ForgeError, RepositoryId};

use super::{
    ASSIGNMENT_RECOVERY_AUDIT_MARKER, ArtifactSource, AssignmentConvergenceError, Classifier,
};
use crate::artifact::ArtifactTarget;
use crate::classify::ClassificationDiagnostic;
use crate::metadata::parse_metadata_block;
use crate::validated::ValidatedWorkflow;

const MAX_REASON_CHARS: usize = 512;
const MAX_EVIDENCE_ITEMS: usize = 16;
const MAX_EVIDENCE_VALUE_CHARS: usize = 80;

pub(super) async fn publish_assignment_recovery_audit<F: Forge + ?Sized>(
    workflow: &ValidatedWorkflow,
    forge: &F,
    repo: &RepositoryId,
    source: ArtifactSource,
    reason: &str,
    only_if_unassigned: bool,
) -> Result<(), AssignmentConvergenceError> {
    match source {
        ArtifactSource::Issue { number } => {
            let issue = forge
                .get_issue_by_number(repo, number)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("issue {number}")))?;
            if only_if_unassigned && has_assignment(&issue.body) {
                return Ok(());
            }
            let comments = forge.list_issue_comments(&issue.id).await?;
            if !comments
                .iter()
                .any(|comment| comment.body.contains(ASSIGNMENT_RECOVERY_AUDIT_MARKER))
            {
                let body = audit_body(
                    workflow,
                    ArtifactTarget::Issue,
                    &issue.labels,
                    &issue.body,
                    reason,
                );
                forge
                    .add_issue_comment(&issue.id, CreateComment { body })
                    .await?;
            }
        }
        ArtifactSource::PullRequest { number } => {
            let pull_request = forge
                .get_pull_request_by_number(repo, number)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("pull request {number}")))?;
            if only_if_unassigned && has_assignment(&pull_request.body) {
                return Ok(());
            }
            let comments = forge.list_pull_request_comments(&pull_request.id).await?;
            if !comments
                .iter()
                .any(|comment| comment.body.contains(ASSIGNMENT_RECOVERY_AUDIT_MARKER))
            {
                let body = audit_body(
                    workflow,
                    ArtifactTarget::PullRequest,
                    &pull_request.labels,
                    &pull_request.body,
                    reason,
                );
                forge
                    .add_pull_request_comment(&pull_request.id, CreateComment { body })
                    .await?;
            }
        }
    }
    Ok(())
}

pub(super) fn has_assignment(body: &str) -> bool {
    parse_metadata_block(body)
        .ok()
        .flatten()
        .and_then(|metadata| metadata.assignment)
        .is_some()
}

fn audit_body(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &[String],
    body: &str,
    reason: &str,
) -> String {
    let metadata_kind = match parse_metadata_block(body) {
        Ok(Some(metadata)) => metadata
            .kind
            .map(|kind| format!("present ({})", inline_value(&kind.to_string())))
            .unwrap_or_else(|| "absent".to_string()),
        Ok(None) => "absent (no workflow metadata block)".to_string(),
        Err(_) => "unreadable (malformed workflow metadata)".to_string(),
    };
    let relevant_labels = relevant_identifying_labels(workflow, target, labels);
    let candidates = label_kind_candidates(workflow, target, labels);

    format!(
        "Startup recovery could not safely converge a durable assignment. The artifact was parked for human inspection.\n\nReason: {}\n\nClassification evidence:\n- Metadata kind: {metadata_kind}\n- Relevant identifying labels: {}\n- Label-derived kind candidates: {}\n\n{ASSIGNMENT_RECOVERY_AUDIT_MARKER}",
        bounded_text(reason, MAX_REASON_CHARS),
        bounded_values(relevant_labels),
        bounded_values(candidates),
    )
}

fn relevant_identifying_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &[String],
) -> Vec<String> {
    workflow
        .artifact_kinds()
        .iter()
        .filter(|kind| kind.target == target)
        .flat_map(|kind| kind.identifying_labels.iter())
        .filter(|label| labels.iter().any(|present| present == label.as_str()))
        .map(|label| label.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn label_kind_candidates(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &[String],
) -> Vec<String> {
    match Classifier::new(workflow).resolve_kind(target, labels, None) {
        Ok(kind) => vec![kind.to_string()],
        Err(error) => error
            .diagnostics()
            .iter()
            .filter_map(|diagnostic| match diagnostic {
                ClassificationDiagnostic::AmbiguousArtifactKind { candidates, .. } => {
                    Some(candidates)
                }
                _ => None,
            })
            .flatten()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn bounded_values(values: Vec<String>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    let total = values.len();
    let mut rendered = values
        .into_iter()
        .take(MAX_EVIDENCE_ITEMS)
        .map(|value| inline_value(&value))
        .collect::<Vec<_>>()
        .join(", ");
    if total > MAX_EVIDENCE_ITEMS {
        rendered.push_str(&format!(" (+{} more)", total - MAX_EVIDENCE_ITEMS));
    }
    rendered
}

fn inline_value(value: &str) -> String {
    format!(
        "`{}`",
        bounded_text(value, MAX_EVIDENCE_VALUE_CHARS).replace('`', "'")
    )
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut rendered = String::new();
    let mut previous_was_space = false;
    let mut truncated = false;
    let mut count = 0;
    for character in value.chars() {
        let character = if character.is_whitespace() {
            ' '
        } else if character.is_control() {
            '�'
        } else {
            character
        };
        if character == ' ' && previous_was_space {
            continue;
        }
        if count == max_chars {
            truncated = true;
            break;
        }
        rendered.push(character);
        previous_was_space = character == ' ';
        count += 1;
    }
    if truncated {
        rendered.push('…');
    }
    rendered.trim().to_string()
}
