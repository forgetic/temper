// SPDX-License-Identifier: MPL-2.0

//! The [`ResultApplier`] trait impl for [`ForgeApplier`].

use temper_protocol_worker::{
    JobContext, JobResult, PullRequestFreshness, PullRequestFreshnessResponse, ResultStatus,
};

use crate::InFlightJob;
use crate::applier::ResultApplier;
use crate::forge_applier::ForgeApplier;

#[async_trait::async_trait]
impl<F: temper_forge::Forge + ?Sized + 'static> ResultApplier for ForgeApplier<F> {
    async fn claim(&self, job: InFlightJob) {
        self.apply_source_action_claim(&job).await;
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let self_pushed_head = result
            .repos
            .iter()
            .find(|repo| repo.repo == job.repo)
            .or_else(|| result.repos.first())
            .map(|repo| repo.branch.head_sha.clone());
        if self
            .drop_stale_pr_job(&job, self_pushed_head.as_deref())
            .await
        {
            return;
        }
        match result.status {
            ResultStatus::Success => self.apply_success(job, result).await,
            ResultStatus::Failure => self.apply_failure(job, result).await,
        }
    }

    async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        crate::pr_freshness::check_pull_request_freshness(self.forge.as_ref(), &check).await
    }
}

impl<F: temper_forge::Forge + ?Sized> ForgeApplier<F> {
    async fn drop_stale_pr_job(&self, job: &InFlightJob, self_pushed_head: Option<&str>) -> bool {
        let Ok(context) = serde_json::from_value::<JobContext>(job.job_payload.clone()) else {
            return false;
        };
        let Some(check) = context.pull_request_freshness.as_ref() else {
            return false;
        };
        let response = crate::pr_freshness::check_pull_request_freshness_with_self_pushed_head(
            self.forge.as_ref(),
            check,
            self_pushed_head,
        )
        .await;
        if !crate::pr_freshness::is_stale(&response) {
            return false;
        }
        tracing::debug!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            reason = response.reason.as_deref().unwrap_or("stale"),
            "forge applier dropped stale pull request job update"
        );
        true
    }
}
