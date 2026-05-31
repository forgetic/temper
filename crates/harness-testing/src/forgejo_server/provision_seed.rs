//! Seeding an initial intake issue into a provisioned Forgejo repository.
//!
//! After [`super::provision_world`] has set up the org, repo, labels, and CI
//! workflow, the operator binary calls [`seed_intake_issue`] so the running
//! workers have something to pick up: one realistic, freshly-filed issue that
//! lands in the workflow's intake queue.
//!
//! The intake label(s) are **derived from the compiled workflow**, never
//! hardcoded — [`intake_labels`] resolves them from the artifact-kind, queue,
//! and transition manifests so this keeps working if the reference workflow's
//! intake label is renamed.

use super::provision::{ProvisionError, Result};
use crate::workflow;
use harness_forge::{CreateIssue, IssueQuery, ItemNumber, RepositoryPath};
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_workflow::{ArtifactTarget, Effect, ValidatedWorkflow};
use std::collections::BTreeSet;

/// Title of the seeded intake issue. Stable so a re-seed is idempotent (an
/// existing issue with this title is reused rather than duplicated).
const INTAKE_TITLE: &str = "Add a configurable greeting to the service banner";

/// Body of the seeded intake issue — a small, realistic human request.
const INTAKE_BODY: &str = "As an operator I want the service banner to show a \
configurable greeting so I can tell environments apart at a glance.\n\n\
Acceptance: a `BANNER_GREETING` setting whose value is printed on startup, \
defaulting to the current text when unset.";

/// Seeds one intake issue into the provisioned repository and returns its number.
///
/// Idempotent: if an issue with [`INTAKE_TITLE`] already exists (a prior run),
/// its number is returned and no duplicate is created. The issue is filed with
/// the workflow's derived intake labels (see [`intake_labels`]) so a triage role
/// picks it up immediately.
///
/// `token` is the admin (or any repo-writing) token; it is used only to create
/// the issue and is never logged.
pub async fn seed_intake_issue(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<ItemNumber> {
    let workflow = workflow();
    let labels = intake_labels(&workflow);
    if labels.is_empty() {
        return Err(ProvisionError::Shape {
            what: "intake labels".into(),
            detail: "workflow declares no entry issue artifact for an intake queue".into(),
        });
    }

    let config = ForgejoConfig::new(base_url, token).with_default_repo(owner, name);
    let forge = ForgejoForge::new(config);
    let repo = forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .ok_or_else(|| ProvisionError::Shape {
            what: "repository".into(),
            detail: format!("{owner}/{name} not readable when seeding intake issue"),
        })?;

    // Idempotency: reuse an existing intake issue rather than file a duplicate.
    let existing = forge.list_issues(&repo.id, IssueQuery::default()).await?;
    if let Some(found) = existing.iter().find(|issue| issue.title == INTAKE_TITLE) {
        return Ok(found.number);
    }

    let issue = forge
        .create_issue(
            &repo.id,
            CreateIssue {
                title: INTAKE_TITLE.into(),
                body: INTAKE_BODY.into(),
                labels,
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(issue.number)
}

/// Resolves the labels that mark a fresh intake issue, derived from the workflow.
///
/// An *intake* issue is the entry point for brand-new work. We define it
/// mechanically: an issue-target artifact kind whose identifying labels are all
/// **entry labels** — labels no transition produces via an `add_label` effect —
/// and that **some queue filters on** (so a role actually draws it). The first
/// such artifact kind (in declaration order) supplies the intake labels.
///
/// For the reference workflow this resolves to the `intake` artifact's
/// `untriaged` label: `untriaged` is removed by triage transitions but added by
/// none, and the `design_triage` queue filters on it. An `epic` artifact is also
/// entry-only but no queue filters on it, so it is correctly excluded.
pub fn intake_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    // Labels produced by some transition (target of an `add_label` effect).
    let produced: BTreeSet<&str> = workflow
        .transitions()
        .iter()
        .flat_map(|transition| transition.effects.iter())
        .filter_map(|effect| match effect {
            Effect::AddLabel(label) => Some(label.as_str()),
            _ => None,
        })
        .collect();

    // Labels some queue filters on (a worker can pick the artifact up).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intake_labels_resolve_to_the_entry_issue_artifact() {
        // The reference workflow's intake artifact is `untriaged`: an entry label
        // no transition produces, filtered by the triage queue. `epic` is also
        // entry-only but no queue draws it, so it must not be chosen.
        let workflow = workflow();
        let labels = intake_labels(&workflow);
        assert_eq!(labels, vec!["untriaged".to_string()]);
    }

    #[test]
    fn intake_labels_are_a_subset_of_declared_labels() {
        // Whatever the derivation picks must be real workflow labels, so the
        // upserted Forge labels cover them.
        let workflow = workflow();
        let declared: BTreeSet<String> =
            workflow.labels().iter().map(|id| id.to_string()).collect();
        for label in intake_labels(&workflow) {
            assert!(
                declared.contains(&label),
                "intake label {label:?} is not a declared workflow label",
            );
        }
    }
}
