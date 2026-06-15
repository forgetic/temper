//! Seeding the entry "intake" issue and resolving its labels and author token.
//!
//! These helpers are host-neutral: they inspect the workflow and drive the
//! portable [`Forge`] trait, so the same logic seeds an intake issue against any
//! backend.

use std::collections::BTreeSet;

use temper_forge::{CreateIssue, Forge, IssueQuery, ItemNumber, RepositoryId};
use temper_workflow::{ArtifactTarget, Effect, IntakeAuthor, RoleId, ValidatedWorkflow};

use crate::error::{ProvisionError, Result};
use crate::model::{IntakeIssueSeed, Provisioned};

/// Files the entry intake issue into `repo`, returning its repository-scoped
/// number.
///
/// Idempotent in the find-or-create sense: an existing issue with the seed's
/// title is reused rather than duplicated.
pub async fn seed_intake_issue(
    forge: &dyn Forge,
    repo: &RepositoryId,
    seed: &IntakeIssueSeed,
    workflow: &ValidatedWorkflow,
) -> Result<ItemNumber> {
    let labels = intake_labels(workflow);
    // An empty label set is valid when the workflow declares a default
    // (catch-all) issue kind: the entry issue is seeded as raw human intake with
    // no labels, and a mechanical queue stamps it (e.g. `untriaged`) so a triage
    // role picks it up. Only error when there is no intake entry point at all.
    if labels.is_empty() && !has_default_issue_kind(workflow) {
        return Err(ProvisionError::Shape {
            what: "intake labels".into(),
            detail: "workflow declares no queued entry issue artifact".into(),
        });
    }

    let existing = forge.list_issues(repo, IssueQuery::default()).await?;
    if let Some(found) = existing.iter().find(|issue| issue.title == seed.title) {
        return Ok(found.number);
    }

    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: seed.title.clone(),
                body: seed.body.clone(),
                labels,
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(issue.number)
}

/// Resolves the identifying labels of the workflow's queued entry issue kind.
pub fn intake_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    let produced: BTreeSet<&str> = workflow
        .transitions()
        .iter()
        .flat_map(|transition| transition.effects.iter())
        .filter_map(|effect| match effect {
            Effect::AddLabel(label) => Some(label.as_str()),
            _ => None,
        })
        .collect();
    let queue_labels: BTreeSet<&str> = workflow
        .queues()
        .iter()
        .flat_map(|queue| {
            queue
                .labels
                .iter()
                .chain(queue.any_of.iter().flat_map(|set| set.labels.iter()))
        })
        .map(|label| label.as_str())
        .collect();

    workflow
        .artifact_kinds()
        .iter()
        .filter(|kind| kind.target == ArtifactTarget::Issue)
        .find(|kind| {
            !kind.identifying_labels.is_empty()
                && kind.identifying_labels.iter().all(|label| {
                    !produced.contains(label.as_str()) && queue_labels.contains(label.as_str())
                })
        })
        .map(|kind| {
            kind.identifying_labels
                .iter()
                .map(|label| label.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the workflow declares a default (catch-all) issue kind — one with no
/// identifying labels. Such a kind admits raw human intake filed with no labels;
/// the entry issue is seeded unlabeled and a mechanical queue stamps it.
pub fn has_default_issue_kind(workflow: &ValidatedWorkflow) -> bool {
    workflow
        .artifact_kinds()
        .iter()
        .any(|kind| kind.target == ArtifactTarget::Issue && kind.identifying_labels.is_empty())
}

/// Resolves the token that authors the seeded intake issue from the workflow's
/// `intake_author` knob.
///
/// - `SiteAdmin` uses the provisioning admin token (the "external filer").
/// - `Role(r)` uses that role's minted token; errors if the role was not
///   provisioned.
/// - `None` keeps the legacy `human`-role lookup for back-compat.
pub fn resolve_intake_seed_token<'a>(
    workflow: &ValidatedWorkflow,
    provisioned: &'a Provisioned,
    admin_token: &'a str,
) -> Result<&'a str> {
    match workflow.intake_author() {
        Some(IntakeAuthor::SiteAdmin) => Ok(admin_token),
        Some(IntakeAuthor::Role(role)) => role_seed_token(provisioned, role),
        None => role_seed_token(provisioned, &RoleId::new("human")),
    }
}

/// Looks up a provisioned role's minted token, erroring if the role was not
/// provisioned.
fn role_seed_token<'a>(provisioned: &'a Provisioned, role: &RoleId) -> Result<&'a str> {
    provisioned
        .roles
        .get(role)
        .map(|identity| identity.token.as_str())
        .ok_or_else(|| ProvisionError::Shape {
            what: "intake seed author".into(),
            detail: format!(
                "workflow provisioning did not create a `{role}` role token for intake authoring"
            ),
        })
}
