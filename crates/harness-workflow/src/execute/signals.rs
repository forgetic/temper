use super::{ExecutionError, Executor, Loaded};
use crate::classify::ArtifactSource;
use crate::dependency_state;
use crate::plan::{CiStatus, GateSignals, ReviewStatus};
use harness_forge::{CiJobQuery, Forge, PullRequestReviewStatus, RepositoryId};

impl<'a, F: Forge + ?Sized> Executor<'a, F> {
    /// Reads runtime gate signals for a target from fresh Forge state.
    ///
    /// Loads and classifies the artifact, then derives the same dependency, CI,
    /// and review signal bundle that `execute` and `plan` use before planning.
    /// It performs no mutation.
    pub async fn read_gate_signals(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<GateSignals, ExecutionError> {
        let loaded = self.load(repo_id, target).await?;
        self.gate_signals(repo_id, &loaded).await
    }

    /// Reads runtime gate signals for the loaded artifact from fresh Forge state.
    pub(super) async fn gate_signals(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
    ) -> Result<GateSignals, ExecutionError> {
        let dependencies =
            dependency_state::status_for_artifact(self.forge, repo_id, loaded.classified()).await?;
        let signals = GateSignals::new().with_dependencies(dependencies);

        match loaded {
            Loaded::Issue { .. } => Ok(signals),
            Loaded::PullRequest {
                id,
                head_sha,
                requested_reviewers,
                ..
            } => {
                let query = CiJobQuery {
                    pull_request_id: Some(id.clone()),
                    commit_sha: head_sha.clone(),
                    ..CiJobQuery::default()
                };
                let jobs = self.forge.list_ci_jobs(repo_id, query).await?;
                let reviews = self.forge.list_pull_request_reviews(id).await?;
                let review_status =
                    PullRequestReviewStatus::from_reviews(requested_reviewers, &reviews);
                Ok(signals
                    .with_ci(CiStatus::from_jobs(&jobs))
                    .with_review(ReviewStatus::from_aggregate(&review_status)))
            }
        }
    }
}
