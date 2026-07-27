use super::{ExecutionError, Executor, Loaded};
use crate::ClassifiedArtifact;
use crate::classify::ArtifactSource;
use crate::dependency_state;
use crate::plan::{CiStatus, GateSignals, ReviewStatus, SignalNeeds};
use crate::{ExactHeadValidationAuthority, parse_metadata_block, replace_metadata_block};
use temper_forge::{
    CiJobQuery, Forge, ForgeError, Issue, ItemNumber, PullRequest, PullRequestReviewStatus,
    PullRequestState, RepositoryId, UpdateIssue, UpdatePullRequest,
};

impl<'a, F: Forge + ?Sized> Executor<'a, F> {
    /// Reads runtime gate signals for a target from fresh Forge state.
    ///
    /// Loads and classifies the artifact, then derives the same dependency, CI,
    /// and review signal bundle that `execute` and `plan` use before planning.
    /// Ordinary signal reads are mutation-free. A stale Temper-issued exact-head
    /// authority is the deliberate exception: evaluation durably fences that
    /// authority, requeues its plan, and clears it before returning `false`.
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
        // unnecessary provider CI read. This narrows ONLY the scanner's
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

    /// Reads only the requested runtime gate signals for an issue snapshot that
    /// the caller already loaded and classified.
    ///
    /// Targeted scanners use this entry point so an item-addressed wake does not
    /// refetch the selected issue merely to evaluate dependency gates.
    pub async fn read_classified_issue_gate_signals_with_needs(
        &self,
        repo_id: &RepositoryId,
        issue: &Issue,
        classified: &ClassifiedArtifact,
        needs: SignalNeeds,
    ) -> Result<GateSignals, ExecutionError> {
        let loaded = Loaded::Issue {
            id: issue.id.clone(),
            version: issue.version,
            snapshot: issue.clone(),
            classified: classified.clone(),
        };
        self.gate_signals_with_needs(repo_id, &loaded, needs).await
    }

    /// Reads only the requested runtime gate signals for a pull-request
    /// snapshot that the caller already loaded and classified.
    ///
    /// CI and review reads are derived directly from the supplied PR
    /// representation. Terminal PRs retain the same CI short-circuit as the
    /// ordinary fresh-load path.
    pub async fn read_classified_pull_request_gate_signals_with_needs(
        &self,
        repo_id: &RepositoryId,
        pull_request: &PullRequest,
        classified: &ClassifiedArtifact,
        needs: SignalNeeds,
    ) -> Result<GateSignals, ExecutionError> {
        let terminal = matches!(
            pull_request.state,
            PullRequestState::Closed | PullRequestState::Merged
        );
        let needs = SignalNeeds {
            ci: needs.ci && !terminal,
            ..needs
        };
        let loaded = Loaded::PullRequest {
            id: pull_request.id.clone(),
            merged: pull_request.state == PullRequestState::Merged,
            terminal,
            head_sha: pull_request.head_sha.clone(),
            source_branch: pull_request.source.branch.clone(),
            requested_reviewers: pull_request.requested_reviewers.clone(),
            classified: classified.clone(),
        };
        self.gate_signals_with_needs(repo_id, &loaded, needs).await
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
                source_branch,
                requested_reviewers,
                classified,
                ..
            } => {
                if needs.exact_head_validation {
                    let authority = classified.metadata.exact_head_validation.as_ref();
                    let authority_current = if let Some(authority) = authority {
                        let branch_head =
                            self.forge.get_branch_head(repo_id, source_branch).await?;
                        authority.authorizes(source_branch, branch_head.as_deref())
                            && head_sha.as_deref() == branch_head.as_deref()
                    } else {
                        false
                    };
                    if !authority_current {
                        if let Some(authority) = authority.cloned() {
                            // Keep the rare CAS invalidation state machine off
                            // the hot gate future's stack.
                            Box::pin(
                                self.invalidate_exact_head_validation(
                                    repo_id, classified, &authority,
                                ),
                            )
                            .await?;
                        }
                    }
                    signals = signals.with_exact_head_validation(authority_current);
                }
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

    pub(super) async fn invalidate_exact_head_validation(
        &self,
        repo_id: &RepositoryId,
        classified: &ClassifiedArtifact,
        expected: &ExactHeadValidationAuthority,
    ) -> Result<(), ExecutionError> {
        let ArtifactSource::PullRequest { number } = classified.source else {
            return Ok(());
        };
        let Some(marked) = self
            .mark_exact_head_authority_invalidated(repo_id, number, expected)
            .await?
        else {
            return Ok(());
        };
        self.requeue_exact_head_plan(repo_id, &marked).await?;
        self.clear_invalidated_exact_head_authority(repo_id, number, &marked)
            .await
    }

    async fn mark_exact_head_authority_invalidated(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        expected: &ExactHeadValidationAuthority,
    ) -> Result<Option<ExactHeadValidationAuthority>, ExecutionError> {
        for _ in 0..3 {
            let pull = self
                .forge
                .get_pull_request_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::PullRequest { number },
                })?;
            let mut metadata = parse_metadata_block(&pull.body)
                .map_err(|error| ExecutionError::Backend {
                    message: format!(
                        "invalid landing PR metadata during validation invalidation: {error}"
                    ),
                })?
                .unwrap_or_default();
            let Some(current) = metadata.exact_head_validation.as_mut() else {
                return Ok(None);
            };
            if !same_validation_attempt(current, expected) {
                return Ok(None);
            }
            if current.invalidated {
                return Ok(Some(current.clone()));
            }
            current.invalidated = true;
            let marked = current.clone();
            let body = replace_metadata_block(&pull.body, &metadata).map_err(|error| {
                ExecutionError::Backend {
                    message: format!("mark exact-head authority invalidated: {error}"),
                }
            })?;
            match self
                .forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        body: Some(body),
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(Some(marked)),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not fence stale exact-head authority after concurrent updates"
                .to_string(),
        })
    }

    async fn requeue_exact_head_plan(
        &self,
        repo_id: &RepositoryId,
        authority: &ExactHeadValidationAuthority,
    ) -> Result<(), ExecutionError> {
        let number = authority
            .plan
            .rsplit_once('#')
            .and_then(|(_, number)| number.parse::<u64>().ok())
            .filter(|number| *number > 0)
            .map(ItemNumber::new)
            .ok_or_else(|| ExecutionError::Backend {
                message: "exact-head authority has an invalid plan identity".to_string(),
            })?;
        for _ in 0..3 {
            let issue = self
                .forge
                .get_issue_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue { number },
                })?;
            if issue.labels.iter().any(|label| label == "needs-validation")
                && !issue.labels.iter().any(|label| label == "validated")
            {
                return Ok(());
            }
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        add_labels: vec!["needs-validation".to_string()],
                        remove_labels: vec!["validated".to_string()],
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not requeue stale exact-head plan after concurrent updates".to_string(),
        })
    }

    async fn clear_invalidated_exact_head_authority(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        expected: &ExactHeadValidationAuthority,
    ) -> Result<(), ExecutionError> {
        for _ in 0..3 {
            let pull = self
                .forge
                .get_pull_request_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::PullRequest { number },
                })?;
            let mut metadata = parse_metadata_block(&pull.body)
                .map_err(|error| ExecutionError::Backend {
                    message: format!(
                        "invalid landing PR metadata while clearing validation authority: {error}"
                    ),
                })?
                .unwrap_or_default();
            let Some(current) = metadata.exact_head_validation.as_ref() else {
                return Ok(());
            };
            if !current.invalidated || !same_validation_attempt(current, expected) {
                return Ok(());
            }
            metadata.exact_head_validation = None;
            let body = replace_metadata_block(&pull.body, &metadata).map_err(|error| {
                ExecutionError::Backend {
                    message: format!("clear stale exact-head authority: {error}"),
                }
            })?;
            match self
                .forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        body: Some(body),
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not clear stale exact-head authority after concurrent updates"
                .to_string(),
        })
    }
}

fn same_validation_attempt(
    left: &ExactHeadValidationAuthority,
    right: &ExactHeadValidationAuthority,
) -> bool {
    left.binding_id == right.binding_id
        && left.attempt_id == right.attempt_id
        && left.evidence_sha256 == right.evidence_sha256
}
