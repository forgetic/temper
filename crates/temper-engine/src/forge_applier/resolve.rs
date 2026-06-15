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
            eprintln!(
                "temper-daemon: forge applier ignored non-issue job for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            eprintln!(
                "temper-daemon: forge applier ignored job with non-numeric issue item for job_id={} repo={} artifact.item={}",
                job.job_id, job.repo, job.artifact.item
            );
            return None;
        };

        let repository = self.resolve_repository(job, "issue", number).await?;

        let issue = match self.forge.get_issue_by_number(&repository.id, number).await {
            Ok(Some(issue)) => issue,
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier source issue not found for job_id={} repo={} issue={}",
                    job.job_id, job.repo, number
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier issue lookup failed for job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
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
            eprintln!(
                "temper-daemon: forge applier ignored non-pull-request job for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            eprintln!(
                "temper-daemon: forge applier ignored job with non-numeric pull request item for job_id={} repo={} artifact.item={}",
                job.job_id, job.repo, job.artifact.item
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
                eprintln!(
                    "temper-daemon: forge applier source pull request not found for job_id={} repo={} pull_request={}",
                    job.job_id, job.repo, number
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier pull request lookup failed for job_id={} repo={} pull_request={}: {error}",
                    job.job_id, job.repo, number
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
            eprintln!(
                "temper-daemon: forge applier ignored job with malformed repo path for job_id={} repo={}",
                job.job_id, job.repo
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
                eprintln!(
                    "temper-daemon: forge applier repository not found for job_id={} repo={} {}={}",
                    job.job_id, job.repo, artifact_label, number
                );
                None
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier repository lookup failed for job_id={} repo={} {}={}: {error}",
                    job.job_id, job.repo, artifact_label, number
                );
                None
            }
        }
    }
}
