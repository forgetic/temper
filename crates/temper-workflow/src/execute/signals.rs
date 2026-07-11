use super::{ExecutionError, Executor, Loaded};
use crate::ClassifiedArtifact;
use crate::classify::ArtifactSource;
use crate::dependency_state;
use crate::plan::{CiStatus, GateSignals, ReviewStatus, SignalNeeds};
use temper_forge::{CiJobQuery, Forge, PullRequestReviewStatus, RepositoryId};

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

    /// Reads only the requested runtime gate signals for a target from fresh Forge state.
    ///
    /// This preserves the load/classify freshness of [`read_gate_signals`](Self::read_gate_signals)
    /// while letting scanners avoid unrelated dependency, CI, or review reads.
    pub async fn read_gate_signals_with_needs(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        needs: SignalNeeds,
    ) -> Result<GateSignals, ExecutionError> {
        let (_, signals) = self
            .read_classified_gate_signals_with_needs(repo_id, target, needs)
            .await?;
        Ok(signals)
    }

    /// Reads a freshly classified artifact plus only the requested runtime gate signals.
    ///
    /// This is useful for scanners that first listed a cheap summary and then
    /// need dependency relations for an exact queue-condition check.
    pub async fn read_classified_gate_signals_with_needs(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        needs: SignalNeeds,
    ) -> Result<(ClassifiedArtifact, GateSignals), ExecutionError> {
        let loaded = self.load(repo_id, target).await?;
        // Scan-phase optimization: a terminal (merged/closed) pull request cannot
        // fire a CI-gated transition, so its CI status is irrelevant here. Drop
        // the CI need before reading — this is the dominant idle cost, since
        // historical PRs keep their workflow labels and are re-listed as
        // reconciliation candidates every mechanical tick, each triggering an
        // expensive web-UI CI read (ADR 0019). This narrows ONLY the scanner's
        // read; `execute`/`plan` still read CI for terminal targets so the merge
        // effect's own stale/closed detection is unchanged.
        let needs = match &loaded {
            Loaded::PullRequest { terminal: true, .. } => SignalNeeds { ci: false, ..needs },
            _ => needs,
        };
        let signals = self
            .gate_signals_with_needs(repo_id, &loaded, needs)
            .await?;
        Ok((loaded.classified().clone(), signals))
    }

    /// Reads every runtime gate signal for the loaded artifact from fresh Forge state.
    pub(super) async fn gate_signals(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
    ) -> Result<GateSignals, ExecutionError> {
        self.gate_signals_with_needs(repo_id, loaded, SignalNeeds::all())
            .await
    }

    /// Reads the requested runtime gate signals for the loaded artifact.
    pub(super) async fn gate_signals_with_needs(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        needs: SignalNeeds,
    ) -> Result<GateSignals, ExecutionError> {
        let mut signals = GateSignals::new();
        if needs.dependencies {
            let dependencies =
                dependency_state::status_for_artifact(self.forge, repo_id, loaded.classified())
                    .await;
            signals = signals.with_dependencies(dependencies);
        }

        match loaded {
            Loaded::Issue { .. } => Ok(signals),
            Loaded::PullRequest {
                id,
                head_sha,
                requested_reviewers,
                ..
            } => {
                if needs.ci {
                    let query = CiJobQuery {
                        pull_request_id: Some(id.clone()),
                        commit_sha: head_sha.clone(),
                        ..CiJobQuery::default()
                    };
                    let jobs = self.forge.list_ci_jobs(repo_id, query).await?;
                    signals =
                        signals.with_ci(CiStatus::from_jobs_for_head(&jobs, head_sha.as_deref()));
                }
                if needs.review {
                    let reviews = self.forge.list_pull_request_reviews(id).await?;
                    let review_status =
                        PullRequestReviewStatus::from_reviews(requested_reviewers, &reviews);
                    signals = signals.with_review(ReviewStatus::from_aggregate(&review_status));
                }
                Ok(signals)
            }
        }
    }
}
