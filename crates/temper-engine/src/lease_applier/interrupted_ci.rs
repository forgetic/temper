// SPDX-License-Identifier: MPL-2.0

//! Durable publication fence for interrupted-CI diagnostic assignments.

use super::*;

impl<F: Forge + ?Sized> LeaseApplier<F> {
    pub(super) async fn interrupted_ci_diagnostic_already_published(
        &self,
        job: &InFlightJob,
    ) -> bool {
        let Ok(context) = serde_json::from_value::<JobContext>(job.job_payload.clone()) else {
            return false;
        };
        if context
            .pull_request_freshness
            .as_ref()
            .and_then(|check| check.queue_condition.as_deref())
            != Some("ci_recovery_required")
        {
            return false;
        }
        let Some((repo, ArtifactSource::PullRequest { number })) =
            resolve_target(self.forge.as_ref(), job)
                .await
                .ok()
                .flatten()
        else {
            return false;
        };
        let Ok(Some(pull_request)) = self.forge.get_pull_request_by_number(&repo, number).await
        else {
            return false;
        };
        temper_workflow::parse_metadata_block(&pull_request.body)
            .ok()
            .flatten()
            .and_then(|metadata| metadata.interrupted_ci_recovery)
            .and_then(|recovery| recovery.diagnostic)
            .and_then(|diagnostic| diagnostic.job_id)
            .is_some()
    }
}
