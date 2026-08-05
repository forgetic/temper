//! Backend loaders that gather snapshots for reconciliation.
//!
//! These async methods read the [`Forge`] to build the [`ArtifactSnapshot`]
//! inputs the pure [`Reconciler::scan`](super::Reconciler::scan) consumes, in
//! both the bounded hot-path and the explicit deep-audit modes, then hand them
//! to `scan`. Split from the reconciler root to keep each file within the
//! source-size budget.

use super::finding::{ReconcileError, ReconcileReport, RecoveryPolicy};
use super::{
    ArtifactSnapshot, Reconciler, ReconciliationDetailCache, ReconciliationDetailCacheStats,
    ReconciliationMode,
};
use crate::classify::{ArtifactSource, Classifier};
use crate::dependency_state::{self, DependencyStateIndex};
use crate::journal::{CommandJournal, CommandRecord};
use crate::plan::DependencyStatus;
use temper_forge::{
    Forge, Issue, IssueQuery, ItemNumber, PullRequest, PullRequestQuery, RepositoryId,
};

impl<P: RecoveryPolicy> Reconciler<'_, P> {
    /// Runs bounded reconciliation without listing the whole repository.
    pub async fn reconcile<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        self.reconcile_bounded_candidates(forge, repo_id, journal, now, None)
            .await
    }

    /// Runs bounded reconciliation while retaining only native dependency
    /// enrichment in the caller-owned cache.
    pub async fn reconcile_with_detail_cache<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        now: chrono::DateTime<chrono::Utc>,
        cache: &ReconciliationDetailCache,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        self.reconcile_bounded_candidates(forge, repo_id, journal, now, Some(cache))
            .await
    }

    /// Reconciles one runner-owned bounded discovery page plus exact retained
    /// and journal targets. `entries` and candidate items are already
    /// deduplicated by the caller, so this path performs no second candidate or
    /// journal-target read before cheap filtering and dependency enrichment.
    #[allow(clippy::too_many_arguments)]
    pub async fn reconcile_discovered_candidates_with_detail_cache<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        entries: &[CommandRecord],
        issue_candidates: Vec<Issue>,
        pull_request_candidates: Vec<PullRequest>,
        forced_sources: &[ArtifactSource],
        now: chrono::DateTime<chrono::Utc>,
        cache: &ReconciliationDetailCache,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let candidates = self
            .load_candidate_snapshots_from_items(
                forge,
                repo_id,
                issue_candidates,
                pull_request_candidates,
                now,
                Some(cache),
                forced_sources,
            )
            .await?;
        Ok(self
            .reconcile_loaded_snapshots(
                forge,
                repo_id,
                candidates.snapshots,
                entries,
                now,
                Some(&candidates.state_index),
                candidates.cache_stats,
            )
            .await)
    }

    async fn reconcile_bounded_candidates<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        now: chrono::DateTime<chrono::Utc>,
        cache: Option<&ReconciliationDetailCache>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        let entries = journal.list().await?;
        let mut snapshots = self
            .load_incomplete_journal_snapshots(forge, repo_id, &entries)
            .await?;
        let candidates = self
            .load_bounded_candidate_snapshots_inner(forge, repo_id, now, cache)
            .await?;
        snapshots.extend(candidates.snapshots);
        Ok(self
            .reconcile_loaded_snapshots(
                forge,
                repo_id,
                snapshots,
                &entries,
                now,
                Some(&candidates.state_index),
                candidates.cache_stats,
            )
            .await)
    }

    /// Runs reconciliation using an explicit loading mode.
    pub async fn reconcile_with_mode<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        mode: ReconciliationMode,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        match mode {
            ReconciliationMode::Bounded => self.reconcile(forge, repo_id, journal, now).await,
            ReconciliationMode::DeepAudit => {
                self.reconcile_deep_audit(forge, repo_id, journal, now)
                    .await
            }
        }
    }

    /// Runs bounded reconciliation from exact incomplete journal targets plus
    /// caller-supplied candidate snapshots.
    pub async fn reconcile_bounded<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        bounded_snapshots: Vec<ArtifactSnapshot>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        let entries = journal.list().await?;
        let mut snapshots = self
            .load_incomplete_journal_snapshots(forge, repo_id, &entries)
            .await?;
        snapshots.extend(bounded_snapshots);
        Ok(self
            .reconcile_loaded_snapshots(
                forge,
                repo_id,
                snapshots,
                &entries,
                now,
                None,
                ReconciliationDetailCacheStats::default(),
            )
            .await)
    }

    /// Loads exact snapshots for incomplete journal command targets.
    ///
    /// Issue targets use [`Forge::get_issue_by_number`], pull-request targets use
    /// [`Forge::get_pull_request_by_number`], and missing targets are skipped.
    /// The result is deduplicated and ordered by item number, with issues before
    /// pull requests for the same number.
    pub async fn load_incomplete_journal_snapshots<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        records: &[CommandRecord],
    ) -> Result<Vec<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let mut targets = records
            .iter()
            .filter(|record| record.state.is_incomplete())
            .map(|record| record.target)
            .collect::<Vec<_>>();
        sort_artifact_sources(&mut targets);
        targets.dedup();
        let mut snapshots = Vec::new();
        for target in targets {
            match target {
                ArtifactSource::Issue { number } => {
                    if let Some(issue) = forge.get_issue_by_number(repo_id, number).await? {
                        snapshots.push(ArtifactSnapshot::from_issue(&issue));
                    }
                }
                ArtifactSource::PullRequest { number } => {
                    if let Some(pull_request) =
                        forge.get_pull_request_by_number(repo_id, number).await?
                    {
                        snapshots.push(ArtifactSnapshot::from_pull_request(&pull_request));
                    }
                }
            }
        }
        Ok(snapshots)
    }

    /// Explicit all-history reconciliation for deep audits and compatibility tests.
    pub async fn reconcile_deep_audit<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        let entries = journal.list().await?;
        let snapshots = self.load_deep_audit_snapshots(forge, repo_id).await?;
        Ok(self
            .reconcile_loaded_snapshots(
                forge,
                repo_id,
                snapshots,
                &entries,
                now,
                None,
                ReconciliationDetailCacheStats::default(),
            )
            .await)
    }

    async fn load_deep_audit_snapshots<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
    ) -> Result<Vec<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let issues = forge.list_issues(repo_id, IssueQuery::default()).await?;
        let pull_requests = forge
            .list_pull_requests(repo_id, PullRequestQuery::default())
            .await?;
        let mut snapshots: Vec<ArtifactSnapshot> =
            issues.iter().map(ArtifactSnapshot::from_issue).collect();
        let pull_request_snapshots = pull_requests
            .iter()
            .map(ArtifactSnapshot::from_pull_request);
        snapshots.extend(pull_request_snapshots);
        normalize_snapshots(&mut snapshots);
        Ok(snapshots)
    }

    async fn reconcile_loaded_snapshots<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        mut snapshots: Vec<ArtifactSnapshot>,
        entries: &[CommandRecord],
        now: chrono::DateTime<chrono::Utc>,
        state_index: Option<&DependencyStateIndex>,
        cache_stats: ReconciliationDetailCacheStats,
    ) -> ReconcileReport {
        normalize_snapshots(&mut snapshots);
        let snapshot_count = snapshots.len();
        let deps = self
            .dependency_status(forge, repo_id, &snapshots, state_index)
            .await;
        let mut report = self.scan(&snapshots, entries, &deps, now);
        report.snapshot_count = snapshot_count;
        report.cache_stats = cache_stats;
        report
    }

    async fn dependency_status<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        snapshots: &[ArtifactSnapshot],
        state_index: Option<&DependencyStateIndex>,
    ) -> DependencyStatus {
        let classifier = Classifier::new(self.workflow);
        let artifacts = snapshots
            .iter()
            .filter(|snapshot| snapshot_is_dependency_visible(snapshot))
            .filter_map(|snapshot| {
                classifier
                    .classify_snapshot_with_dependencies(
                        snapshot.source,
                        &snapshot.labels,
                        &snapshot.body,
                        &snapshot.dependencies,
                    )
                    .ok()
            })
            .collect::<Vec<_>>();
        dependency_state::status_for_artifacts_with_index(forge, repo_id, &artifacts, state_index)
            .await
    }
}

fn snapshot_is_dependency_visible(snapshot: &ArtifactSnapshot) -> bool {
    !crate::requires_human_attention(&snapshot.labels)
        && !crate::parse_metadata_block(&snapshot.body)
            .ok()
            .flatten()
            .is_some_and(|metadata| metadata.staged)
}

fn normalize_snapshots(snapshots: &mut Vec<ArtifactSnapshot>) {
    snapshots.sort_by_key(|snapshot| artifact_source_sort_key(snapshot.source));
    snapshots.dedup_by_key(|snapshot| snapshot.source);
}

fn sort_artifact_sources(sources: &mut [ArtifactSource]) {
    sources.sort_by_key(|source| artifact_source_sort_key(*source));
}

fn artifact_source_sort_key(source: ArtifactSource) -> (ItemNumber, u8) {
    match source {
        ArtifactSource::Issue { number } => (number, 0),
        ArtifactSource::PullRequest { number } => (number, 1),
    }
}
