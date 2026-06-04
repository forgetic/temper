//! Reconciliation of Forge artifacts and command journals (Phase 7).
//!
//! The runtime cannot assume its own commands always finish: a worker may crash
//! mid-transition, a lease may expire, or a human may edit labels into an
//! impossible combination. The reconciler is the authority that periodically
//! inspects durable state — Forge artifacts plus the [command
//! journal](crate::journal) — and decides what to repair or escalate.
//!
//! # Decide, then apply
//!
//! [`Reconciler::scan`] is pure and deterministic over snapshots, journal
//! records, dependency status, and time. [`Reconciler::reconcile`] is the
//! bounded loader for exact journal targets plus workflow-labelled candidates.
//! [`Reconciler::reconcile_deep_audit`] is the explicit all-history loader.
//!
//! Applying the chosen actions is the job of
//! [`recover::Applier`](crate::recover::Applier), which routes each action
//! through the existing [`Executor`](crate::execute::Executor),
//! [`LeaseManager`](crate::lease), and [`CommandJournal`](crate::journal)
//! runtime layers. The reconciler itself only decides, so a caller can still
//! review or filter actions before handing the report to the applier.
//!
//! # Recovery policy hooks
//!
//! [`RecoveryPolicy`] has one defaulted hook per finding class, so a workflow
//! can override how it handles expired leases, partial transitions, impossible
//! states, classification drift, stale commands, or resolved dependencies by
//! implementing only the hooks it cares about. [`DefaultRecoveryPolicy`] uses
//! the safe defaults: requeue expired leases, escalate ambiguous drift, repair
//! partial transitions, mark already-realized commands reconciled, and
//! mechanically unblock dependency-gated work once its prerequisites land.

use crate::classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact, Classifier,
};
use crate::dependency_state;
use crate::ids::TransitionId;
use crate::journal::{CommandJournal, CommandRecord};
use crate::metadata::parse_metadata_block;
use crate::plan::{DependencyStatus, GateSignals, Planner, Postcondition, WorkflowEffect};
use crate::relation::RelationKind;
use crate::validated::{GateCondition, ValidatedTransition, ValidatedWorkflow};
use std::collections::HashSet;
use temper_forge::{
    Forge, Issue, IssueQuery, ItemNumber, PullRequest, PullRequestQuery, RepositoryId,
};

/// A point-in-time view of a Forge artifact used for reconciliation.
///
/// Holds only what reconciliation reads — the source, raw labels, and body — so
/// the pure [`Reconciler::scan`] needs no backend handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSnapshot {
    /// Where the artifact lives in the Forge.
    pub source: ArtifactSource,
    /// Raw Forge labels present on the artifact.
    pub labels: Vec<String>,
    /// The artifact body, which may carry a workflow metadata block.
    pub body: String,
    /// Native dependency links read from the Forge artifact record.
    pub dependencies: Vec<ItemNumber>,
}

impl ArtifactSnapshot {
    /// Builds a snapshot from a Forge issue.
    pub fn from_issue(issue: &Issue) -> Self {
        Self {
            source: ArtifactSource::Issue {
                number: issue.number,
            },
            labels: issue.labels.clone(),
            body: issue.body.clone(),
            dependencies: issue.dependencies.clone(),
        }
    }

    /// Builds a snapshot from a Forge pull request.
    pub fn from_pull_request(pull_request: &PullRequest) -> Self {
        Self {
            source: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            labels: pull_request.labels.clone(),
            body: pull_request.body.clone(),
            dependencies: pull_request.dependencies.clone(),
        }
    }
}

/// How a runtime loads reconciliation snapshots before calling [`Reconciler::scan`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationMode {
    /// Load bounded runtime inputs.
    Bounded,
    /// Load every visible issue and pull request with full details. This is for
    /// rare operator audits and compatibility tests, not normal hot-path ticks.
    DeepAudit,
}

mod candidate;
mod finding;

pub use candidate::{reconciliation_candidate_query_plan, ReconciliationCandidateQueryPlan};
pub use finding::{
    DefaultRecoveryPolicy, ReconcileError, ReconcileFinding, ReconcileReport, RecoveryAction,
    RecoveryPolicy,
};

/// Scans Forge artifacts and the command journal for recovery work.
pub struct Reconciler<'a, P: RecoveryPolicy> {
    workflow: &'a ValidatedWorkflow,
    policy: &'a P,
}

impl<'a, P: RecoveryPolicy> Reconciler<'a, P> {
    /// Creates a reconciler bound to a validated workflow and a policy.
    pub fn new(workflow: &'a ValidatedWorkflow, policy: &'a P) -> Self {
        Self { workflow, policy }
    }

    /// Deterministically scans snapshots and journal entries for recovery work.
    ///
    /// Produces one (finding, action) pair per detected problem, in a stable
    /// order: for each snapshot in order, its expired lease then either its
    /// classification problems (when it fails to classify) or its mechanical
    /// dependency unblocks (when it classifies cleanly), followed by each
    /// incomplete journal command in journal order. `deps` carries which
    /// prerequisite item numbers have landed (see [`DependencyStatus`]); it is
    /// supplied by the runtime, like the CI signal behind `ci_gate`. Pure and
    /// backend-free.
    pub fn scan(
        &self,
        snapshots: &[ArtifactSnapshot],
        journal: &[CommandRecord],
        deps: &DependencyStatus,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let classifier = Classifier::new(self.workflow);

        for snapshot in snapshots {
            self.scan_lease(snapshot, now, &mut report);
            match classifier.classify_snapshot_with_dependencies(
                snapshot.source,
                &snapshot.labels,
                &snapshot.body,
                &snapshot.dependencies,
            ) {
                Ok(artifact) => self.scan_dependency_unblocks(&artifact, deps, &mut report),
                Err(error) => self.scan_classification(snapshot.source, &error, &mut report),
            }
        }

        for record in journal.iter().filter(|record| record.state.is_incomplete()) {
            self.scan_command(record, snapshots, &mut report);
        }

        report
    }

    /// Detects an expired lease on a single snapshot.
    fn scan_lease(
        &self,
        snapshot: &ArtifactSnapshot,
        now: chrono::DateTime<chrono::Utc>,
        report: &mut ReconcileReport,
    ) {
        let Some(lease) = parse_metadata_block(&snapshot.body)
            .ok()
            .flatten()
            .and_then(|metadata| metadata.lease)
        else {
            return;
        };
        if lease.is_expired(now) {
            let action = self.policy.on_expired_lease(snapshot.source, &lease);
            report.push(
                ReconcileFinding::ExpiredLease {
                    target: snapshot.source,
                    lease,
                },
                action,
            );
        }
    }

    /// Detects impossible states and other classification drift for a snapshot
    /// that failed to classify.
    fn scan_classification(
        &self,
        source: ArtifactSource,
        error: &ClassificationError,
        report: &mut ReconcileReport,
    ) {
        let mut drift = Vec::new();
        for diagnostic in error.diagnostics() {
            match diagnostic {
                ClassificationDiagnostic::ExclusiveStateConflict { dimension, states } => {
                    let action = self.policy.on_impossible_state(source, dimension, states);
                    report.push(
                        ReconcileFinding::ImpossibleState {
                            target: source,
                            dimension: dimension.clone(),
                            states: states.clone(),
                        },
                        action,
                    );
                }
                other => drift.push(other.clone()),
            }
        }

        if !drift.is_empty() {
            let action = self.policy.on_classification_drift(source, &drift);
            report.push(
                ReconcileFinding::ClassificationDrift {
                    target: source,
                    diagnostics: drift,
                },
                action,
            );
        }
    }

    /// Detects mechanical dependency unblocks available for a classified
    /// artifact under the supplied dependency status.
    fn scan_dependency_unblocks(
        &self,
        artifact: &ClassifiedArtifact,
        deps: &DependencyStatus,
        report: &mut ReconcileReport,
    ) {
        let planner = Planner::new(self.workflow);
        for transition in self.blocked_without_dependency_transitions(artifact, &planner) {
            let dependency_count = dependency_relation_count(artifact);
            let relation_count = artifact.relations.len();
            let action = self.policy.on_blocked_without_dependencies(
                artifact.source,
                &transition,
                dependency_count,
                relation_count,
            );
            report.push(
                ReconcileFinding::BlockedWithoutDependencies {
                    target: artifact.source,
                    transition,
                    dependency_count,
                    relation_count,
                },
                action,
            );
        }
        for unblock in planner.dependency_unblocks(artifact, deps) {
            let action = self.policy.on_resolved_dependencies(
                artifact.source,
                &unblock.transition,
                &unblock.effects,
            );
            report.push(
                ReconcileFinding::DependenciesResolved {
                    target: artifact.source,
                    transition: unblock.transition,
                },
                action,
            );
        }
    }

    fn blocked_without_dependency_transitions(
        &self,
        artifact: &ClassifiedArtifact,
        planner: &Planner<'_>,
    ) -> Vec<TransitionId> {
        if dependency_relation_count(artifact) > 0 {
            return Vec::new();
        }
        self.workflow
            .transitions()
            .iter()
            .filter(|transition| transition.artifact == artifact.kind)
            .filter(|transition| self.requires_dependency_gate(transition))
            .filter_map(|transition| {
                let role = transition.roles.first()?;
                planner
                    .plan_transition_with(&transition.id, role, artifact, &GateSignals::default())
                    .is_ok()
                    .then(|| transition.id.clone())
            })
            .collect()
    }

    fn requires_dependency_gate(&self, transition: &ValidatedTransition) -> bool {
        transition.requires_gates.iter().any(|gate_id| {
            self.workflow.gates().iter().any(|gate| {
                &gate.id == gate_id
                    && matches!(gate.condition, Some(GateCondition::DependenciesResolved))
            })
        })
    }

    /// Classifies an incomplete journal command against current artifact state.
    fn scan_command(
        &self,
        record: &CommandRecord,
        snapshots: &[ArtifactSnapshot],
        report: &mut ReconcileReport,
    ) {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.source == record.target);

        // No snapshot means the target vanished, so nothing can be re-applied.
        let pending: Vec<WorkflowEffect> = match snapshot {
            None => Vec::new(),
            Some(snapshot) => {
                let labels: HashSet<&str> = snapshot.labels.iter().map(String::as_str).collect();
                record
                    .effects
                    .iter()
                    .filter(|effect| is_pending(effect, &labels))
                    .cloned()
                    .collect()
            }
        };

        if pending.is_empty() {
            // Either the effects already landed or the target is gone; only the
            // journal status lags behind reality.
            let action = self
                .policy
                .on_stale_command(&record.id, record.target, record.state);
            report.push(
                ReconcileFinding::StaleCommand {
                    command: record.id.clone(),
                    target: record.target,
                    state: record.state,
                },
                action,
            );
        } else {
            let action = self
                .policy
                .on_partial_transition(&record.id, record.target, &pending);
            let postconditions = pending.iter().filter_map(label_postcondition).collect();
            report.push(
                ReconcileFinding::PartialTransition {
                    command: record.id.clone(),
                    target: record.target,
                    pending: postconditions,
                },
                action,
            );
        }
    }

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
        let entries = journal.list().await?;
        let mut snapshots = self
            .load_incomplete_journal_snapshots(forge, repo_id, &entries)
            .await?;
        snapshots.extend(
            self.load_bounded_candidate_snapshots(forge, repo_id)
                .await?,
        );
        Ok(self
            .reconcile_loaded_snapshots(forge, repo_id, snapshots, &entries, now)
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
            .reconcile_loaded_snapshots(forge, repo_id, snapshots, &entries, now)
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

    /// Explicit all-history reconciliation for deep audits and compatibility
    /// tests, not normal bounded paths.
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
            .reconcile_loaded_snapshots(forge, repo_id, snapshots, &entries, now)
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
        snapshots.extend(
            pull_requests
                .iter()
                .map(ArtifactSnapshot::from_pull_request),
        );
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
    ) -> ReconcileReport {
        normalize_snapshots(&mut snapshots);
        let deps = self.dependency_status(forge, repo_id, &snapshots).await;
        self.scan(&snapshots, entries, &deps, now)
    }

    async fn dependency_status<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        snapshots: &[ArtifactSnapshot],
    ) -> DependencyStatus {
        let classifier = Classifier::new(self.workflow);
        let artifacts = snapshots
            .iter()
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
        dependency_state::status_for_artifacts(forge, repo_id, &artifacts).await
    }
}

/// Returns `true` when an effect's result is not yet visible in `labels`.
///
/// Only label effects are verifiable today; any other effect variant is treated
/// as not pending because the reconciler cannot yet confirm it from labels.
fn is_pending(effect: &WorkflowEffect, labels: &HashSet<&str>) -> bool {
    match effect {
        WorkflowEffect::AddLabel(label) => !labels.contains(label.as_str()),
        WorkflowEffect::RemoveLabel(label) => labels.contains(label.as_str()),
        _ => false,
    }
}

/// Derives the postcondition a pending label effect implies, if any.
fn label_postcondition(effect: &WorkflowEffect) -> Option<Postcondition> {
    match effect {
        WorkflowEffect::AddLabel(label) => Some(Postcondition::LabelPresent(label.clone())),
        WorkflowEffect::RemoveLabel(label) => Some(Postcondition::LabelAbsent(label.clone())),
        _ => None,
    }
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

fn dependency_relation_count(artifact: &ClassifiedArtifact) -> usize {
    artifact
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::Dependency)
        .count()
}

impl ValidatedWorkflow {
    /// Returns a [`Reconciler`] bound to this workflow and a recovery policy.
    pub fn reconciler<'a, P: RecoveryPolicy>(&'a self, policy: &'a P) -> Reconciler<'a, P> {
        Reconciler::new(self, policy)
    }
}
