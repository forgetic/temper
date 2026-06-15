// SPDX-License-Identifier: MPL-2.0

//! Coordinated pull-request set primitives (ADR 0023): the per-job shared
//! context, the topological landing order, and the per-repo PR creation input.

use std::collections::{BTreeMap, BTreeSet};

use temper_forge_model::{CreatePullRequest, ItemNumber, RepositoryId};
use temper_worker_protocol::{JobContext, RepoOutcome};
use temper_workflow::ArtifactKindId;

use crate::InFlightJob;

/// The shared, per-job context for opening one coordinated PR set: everything
/// every member PR needs that does not vary by repo outcome.
pub(super) struct CoordinatedSet<'a> {
    pub(super) job: &'a InFlightJob,
    pub(super) primary_id: &'a RepositoryId,
    pub(super) issue_title: &'a str,
    pub(super) number: ItemNumber,
    pub(super) summary: &'a str,
    pub(super) coordination_key: &'a str,
    pub(super) lookup_labels: &'a [String],
    pub(super) create_labels: &'a [String],
    pub(super) depends_on: &'a BTreeMap<String, Vec<String>>,
}

impl CoordinatedSet<'_> {
    /// Cross-repo dependency links to the prerequisite PRs (opened earlier in
    /// topological order). A prerequisite that produced no diff opened no PR —
    /// there is nothing to wait on, so drop it.
    pub(super) fn dependency_refs(
        &self,
        repo: &str,
        opened: &BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) -> Vec<temper_workflow::ArtifactRef> {
        let mut dependencies = Vec::new();
        if let Some(prerequisites) = self.depends_on.get(repo) {
            for prerequisite in prerequisites {
                match opened.get(prerequisite) {
                    Some((prereq_id, prereq_number)) => {
                        dependencies.push(temper_workflow::ArtifactRef::in_repo(
                            prereq_id.clone(),
                            *prereq_number,
                        ));
                    }
                    None => eprintln!(
                        "temper-daemon: coordinated landing prerequisite {prerequisite} for {repo} opened no pull request (no diff); not gating on it"
                    ),
                }
            }
        }
        dependencies
    }
}

/// The coordinated-landing order map: repo path -> the repo paths whose PRs must
/// land first. Built from the job's manifest (`WorkspaceRepo.depends_on`).
pub(super) fn manifest_depends_on(context: &JobContext) -> BTreeMap<String, Vec<String>> {
    context
        .workspace
        .as_ref()
        .map(|workspace| {
            workspace
                .repos
                .iter()
                .filter(|repo| !repo.depends_on.is_empty())
                .map(|repo| (repo.repo.clone(), repo.depends_on.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Orders the repo outcomes so a repo comes after every repo it depends on
/// (topological / Kahn order over the outcomes that actually opened a PR).
/// Dependencies on repos absent from `outcomes` (produced no diff) are ignored.
/// Coordinated landing is acyclic; a cycle (not expected) degrades to manifest
/// order so every PR still opens.
pub(super) fn coordinated_landing_order(
    outcomes: &[RepoOutcome],
    depends_on: &BTreeMap<String, Vec<String>>,
) -> Vec<usize> {
    let present: BTreeSet<&str> = outcomes
        .iter()
        .map(|outcome| outcome.repo.as_str())
        .collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut order = Vec::with_capacity(outcomes.len());
    while order.len() < outcomes.len() {
        let mut progressed = false;
        for (index, outcome) in outcomes.iter().enumerate() {
            if done.contains(&outcome.repo) {
                continue;
            }
            let ready = depends_on.get(&outcome.repo).is_none_or(|deps| {
                deps.iter()
                    .all(|dep| !present.contains(dep.as_str()) || done.contains(dep))
            });
            if ready {
                order.push(index);
                done.insert(outcome.repo.clone());
                progressed = true;
            }
        }
        if !progressed {
            for (index, outcome) in outcomes.iter().enumerate() {
                if done.insert(outcome.repo.clone()) {
                    order.push(index);
                }
            }
            break;
        }
    }
    order
}

/// Projects one writable repo's head into a member of a *coordinated* pull
/// request set (ADR 0023): the parent link is a repo-qualified ref to the
/// coordinating issue (which may live in another repo) and the shared
/// `coordination_key` is stamped into the metadata so the set is discoverable.
#[allow(clippy::too_many_arguments)]
pub(super) fn coordinated_pr_pull_request_input(
    repo: RepositoryId,
    coordinating: temper_workflow::ArtifactRef,
    coordinating_number: ItemNumber,
    issue_title: &str,
    head_branch: String,
    base_branch: String,
    summary: &str,
    labels: Vec<String>,
    coordination_key: &str,
    dependencies: Vec<temper_workflow::ArtifactRef>,
) -> CreatePullRequest {
    let gated = !dependencies.is_empty();
    let metadata = temper_workflow::WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![coordinating],
        // Cross-repo dependency links encoding the coordinated landing order:
        // this PR's `dependency_gate` stays closed until each target PR merges
        // (ADR 0023, acyclic).
        dependencies,
        correlation_key: Some(coordination_key.to_string()),
        ..temper_workflow::WorkflowMetadata::default()
    };
    let summary = summary.trim();
    let landing_note = if gated {
        "\n\nThis PR lands after its prerequisite PR(s) in the set merge."
    } else {
        ""
    };
    let body = format!(
        "Coordinated implementation for issue #{coordinating_number} (set `{coordination_key}`).{landing_note}\n\nSummary: {}\n\n{}",
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        temper_workflow::render_metadata_block(&metadata)
    );
    CreatePullRequest {
        title: format!("Implement #{coordinating_number}: {issue_title}"),
        body,
        source: temper_forge_model::BranchRef {
            repository_id: repo.clone(),
            branch: head_branch,
        },
        target: temper_forge_model::BranchRef {
            repository_id: repo,
            branch: base_branch,
        },
        labels,
        assignees: Vec::new(),
    }
}
