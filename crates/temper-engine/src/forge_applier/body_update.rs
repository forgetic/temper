// SPDX-License-Identifier: MPL-2.0

//! Optimistic body updates for implementation PR plan publication/finalization.

use temper_forge::{Forge, ForgeError, PullRequest, UpdatePullRequest};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::body_merge::merge_implementation_pr_body;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn update_implementation_pr_body(
        &self,
        job: &InFlightJob,
        mut pull_request: PullRequest,
        desired_body: &str,
        operation: &'static str,
    ) -> PullRequest {
        for _ in 0..3 {
            let body = match merge_implementation_pr_body(&pull_request.body, desired_body) {
                Ok(Some(body)) => body,
                Ok(None) => return pull_request,
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        pull_request = %pull_request.number,
                        %error,
                        "forge applier could not merge implementation PR body for {operation}"
                    );
                    return pull_request;
                }
            };

            match self
                .forge
                .update_pull_request(
                    &pull_request.id,
                    UpdatePullRequest {
                        body: Some(body),
                        expected_version: Some(pull_request.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
            {
                Ok(updated) => return updated,
                Err(ForgeError::Conflict(_)) => {
                    match self.forge.get_pull_request(&pull_request.id).await {
                        Ok(Some(reloaded)) => {
                            pull_request = reloaded;
                            continue;
                        }
                        Ok(None) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                "forge applier could not reload PR after body conflict for {operation}"
                            );
                            return pull_request;
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                %error,
                                "forge applier could not reload PR after body conflict for {operation}"
                            );
                            return pull_request;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        pull_request = %pull_request.number,
                        %error,
                        "forge applier could not update implementation PR body for {operation}"
                    );
                    return pull_request;
                }
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            pull_request = %pull_request.number,
            "forge applier gave up updating implementation PR body after conflicts for {operation}"
        );
        pull_request
    }
}
