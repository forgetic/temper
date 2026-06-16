// SPDX-License-Identifier: MPL-2.0

//! Forge artifact lookups shared by the applier's success, failure, and verdict
//! paths: resolve a job's source issue, pull request, or repository, logging and
//! returning `None` on malformed coordinates or lookup misses.

use temper_forge::{Forge, Issue, ItemNumber, PullRequest, Repository, RepositoryPath};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn resolve_issue(&self, job: &InFlightJob) -> Option<(Repository, Issue)> {
        if job.artifact.kind != "issue" {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "forge applier ignored non-issue job"
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_item = %job.artifact.item,
                "forge applier ignored job with non-numeric issue item"
            );
            return None;
        };

        let repository = self.resolve_repository(job, "issue", number).await?;

        let issue = match self.forge.get_issue_by_number(&repository.id, number).await {
            Ok(Some(issue)) => issue,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    "forge applier source issue not found"
                );
                return None;
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    %error,
                    "forge applier issue lookup failed"
                );
                return None;
            }
        };

        Some((repository, issue))
    }

    pub(super) async fn resolve_pull_request(
        &self,
        job: &InFlightJob,
    ) -> Option<(Repository, PullRequest)> {
        if job.artifact.kind != "pull_request" {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "forge applier ignored non-pull-request job"
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_item = %job.artifact.item,
                "forge applier ignored job with non-numeric pull request item"
            );
            return None;
        };

        let repository = self.resolve_repository(job, "pull_request", number).await?;

        let pull_request = match self
            .forge
            .get_pull_request_by_number(&repository.id, number)
            .await
        {
            Ok(Some(pull_request)) => pull_request,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    pull_request = %number,
                    "forge applier source pull request not found"
                );
                return None;
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    pull_request = %number,
                    %error,
                    "forge applier pull request lookup failed"
                );
                return None;
            }
        };

        Some((repository, pull_request))
    }

    pub(super) async fn resolve_repository(
        &self,
        job: &InFlightJob,
        artifact_label: &str,
        number: ItemNumber,
    ) -> Option<Repository> {
        let Some((owner, name)) = job.repo.split_once('/') else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                "forge applier ignored job with malformed repo path"
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
                    repo = %job.repo,
                    artifact_label = %artifact_label,
                    number = %number,
                    "forge applier repository not found"
                );
                None
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_label = %artifact_label,
                    number = %number,
                    %error,
                    "forge applier repository lookup failed"
                );
                None
            }
        }
    }
}
