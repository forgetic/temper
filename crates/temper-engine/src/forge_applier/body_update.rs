// SPDX-License-Identifier: MPL-2.0

//! Optimistic title/body updates for implementation PR finalization.

use temper_forge::{Forge, ForgeError, PullRequest, UpdatePullRequest};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::body_merge::merge_implementation_pr_body;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn update_implementation_pr_handoff(
        &self,
        job: &InFlightJob,
        mut pull_request: PullRequest,
        desired_title: &str,
        desired_body: &str,
        operation: &'static str,
    ) -> PullRequest {
        for _ in 0..3 {
            let title = (pull_request.title != desired_title).then(|| desired_title.to_string());
            let body = match merge_implementation_pr_body(&pull_request.body, desired_body) {
                Ok(body) => body,
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

            if title.is_none() && body.is_none() {
                return pull_request;
            }

            match self
                .forge
                .update_pull_request(
                    &pull_request.id,
                    UpdatePullRequest {
                        title,
                        body,
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
                                "forge applier could not reload PR after handoff conflict for {operation}"
                            );
                            return pull_request;
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                %error,
                                "forge applier could not reload PR after handoff conflict for {operation}"
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
                        "forge applier could not update implementation PR handoff for {operation}"
                    );
                    return pull_request;
                }
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            pull_request = %pull_request.number,
            "forge applier gave up updating implementation PR handoff after conflicts for {operation}"
        );
        pull_request
    }
}
