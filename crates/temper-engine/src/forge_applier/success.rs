// SPDX-License-Identifier: MPL-2.0

//! Success-path application: opening one coordinated implementation PR per
//! writable repo that produced a diff, in coordinated landing order (ADR 0023).

use std::collections::BTreeMap;

use temper_forge::{Forge, ItemNumber, Repository, RepositoryId, RepositoryPath};
use temper_worker_protocol::{JobContext, JobResult, RepoOutcome};
use temper_workflow::{ArtifactKindId, Executor};

use temper_runner::pr_correlation_key;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{
    CoordinatedSet, coordinated_landing_order, coordinated_pr_pull_request_input,
    manifest_depends_on,
};
use crate::workflow_meta::{
    default_base_branch, implementation_pr_create_labels, implementation_pr_labels,
};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_success(&self, job: InFlightJob, result: JobResult) {
        if result.verdict.is_some() {
            self.apply_verdict(job, result).await;
            return;
        }

        if result.repos.is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "forge applier ignored success result with no repo outcomes"
            );
            return;
        }

        // A pull-request job (e.g. a CI-failure fix) pushes its diff straight to
        // the existing PR head branch; that push itself re-triggers CI and the
        // landing queue takes over once CI is green. There is no new PR to open,
        // so the success-path PR-opening below (which keys on a coordinating
        // *issue*) does not apply.
        if job.artifact.kind == "pull_request" {
            tracing::info!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_item = %job.artifact.item,
                head_branch = %result
                    .repos
                    .first()
                    .map(|repo| repo.branch.name.as_str())
                    .unwrap_or("-"),
                "forge applier pushed pull-request fix to head; awaiting fresh CI"
            );
            return;
        }

        // The coordinating issue lives in the primary repo; every PR in the set
        // links back to it with a repo-qualified ref (ADR 0023).
        let Some((primary_repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };
        let number = issue.number;

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    %error,
                    "forge applier could not parse JobContext"
                );
                return;
            }
        };
        let source_kind = ArtifactKindId::new(context.artifact_kind.clone());
        // The coordination key keys every PR in the set; fall back to the
        // single-issue correlation key when the payload carries no manifest.
        let coordination_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.clone())
            .unwrap_or_else(|| pr_correlation_key(&source_kind, number));

        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let create_labels = implementation_pr_create_labels(self.workflow.as_ref());
        let summary = result.summary.unwrap_or_default();

        // Open one PR per writable repo that produced a diff, in coordinated
        // landing order: a repo's PR is created after the PRs it depends on, so
        // their numbers are known and can be wired as cross-repo dependency
        // links (ADR 0023, acyclic). The `dependency_gate` then holds each PR
        // closed until its prerequisites merge.
        let depends_on = manifest_depends_on(&context);
        let order = coordinated_landing_order(&result.repos, &depends_on);
        // repo path -> (forge id, opened PR number) for wiring dependents.
        let mut opened: BTreeMap<String, (RepositoryId, ItemNumber)> = BTreeMap::new();

        let set = CoordinatedSet {
            job: &job,
            primary_id: &primary_repository.id,
            issue_title: &issue.title,
            number,
            summary: &summary,
            coordination_key: &coordination_key,
            lookup_labels: &lookup_labels,
            create_labels: &create_labels,
            depends_on: &depends_on,
        };
        for index in order {
            self.open_coordinated_pr(&set, &result.repos[index], &mut opened)
                .await;
        }
    }

    /// Opens (or ensures) the coordinated PR for one repo outcome, recording the
    /// opened PR in `opened` so later dependents can wire dependency links.
    async fn open_coordinated_pr(
        &self,
        set: &CoordinatedSet<'_>,
        outcome: &RepoOutcome,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) {
        if outcome.branch.name.trim().is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                outcome_repo = %outcome.repo,
                "forge applier ignored blank branch in repo outcome"
            );
            return;
        }
        let Some(target_repository) = self.resolve_repo_path(set.job, &outcome.repo).await else {
            return;
        };
        let base_branch = default_base_branch(&target_repository);
        // A PR in the primary repo links to the coordinating issue with a bare
        // same-repo ref; cross-repo PRs use a repo-qualified one.
        let coordinating = if &target_repository.id == set.primary_id {
            temper_workflow::ArtifactRef::same_repo(set.number)
        } else {
            temper_workflow::ArtifactRef::in_repo(set.primary_id.clone(), set.number)
        };
        let dependencies = set.dependency_refs(&outcome.repo, opened);

        let input = coordinated_pr_pull_request_input(
            target_repository.id.clone(),
            coordinating,
            set.number,
            set.issue_title,
            outcome.branch.name.clone(),
            base_branch,
            set.summary,
            set.create_labels.to_vec(),
            set.coordination_key,
            dependencies,
        );

        match Executor::new(self.workflow.as_ref(), self.forge.as_ref())
            .ensure_pull_request_with_lookup(
                &target_repository.id,
                set.coordination_key,
                set.lookup_labels,
                input,
            )
            .await
        {
            Ok(ensured) => {
                opened.insert(
                    outcome.repo.clone(),
                    (target_repository.id.clone(), ensured.artifact().number),
                );
            }
            Err(error) => tracing::error!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                issue = %set.number,
                target_repo = %outcome.repo,
                coordination_key = %set.coordination_key,
                %error,
                "forge applier ensure_pull_request failed"
            ),
        }
    }

    /// Resolves an `owner/name` repo path (a [`RepoOutcome::repo`]) to its Forge
    /// [`Repository`]. Logs and returns `None` on a malformed path or lookup
    /// miss.
    pub(super) async fn resolve_repo_path(
        &self,
        job: &InFlightJob,
        path: &str,
    ) -> Option<Repository> {
        let Some((owner, name)) = path.split_once('/') else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                outcome_repo = %path,
                "forge applier ignored malformed repo outcome path"
            );
            return None;
        };
        match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    outcome_repo = %path,
                    "forge applier repo outcome repository not found"
                );
                None
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    outcome_repo = %path,
                    %error,
                    "forge applier repo outcome lookup failed"
                );
                None
            }
        }
    }
}
