//! Pure, deterministic reconciliation scanning.
//!
//! This is the side-effect-free half of the reconciler: it walks snapshots,
//! journal records, dependency status, and time, and produces one
//! (finding, action) pair per detected problem. It never touches a backend. The
//! backend loaders that gather snapshots before calling [`Reconciler::scan`]
//! live in the sibling [`load`](super::load) module.

use super::finding::{ReconcileFinding, ReconcileReport, RecoveryPolicy};
use super::{ArtifactSnapshot, Reconciler};
use crate::classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact, Classifier,
};
use crate::ids::TransitionId;
use crate::journal::CommandRecord;
use crate::metadata::parse_metadata_block;
use crate::plan::{DependencyStatus, GateSignals, Planner, Postcondition, WorkflowEffect};
use crate::relation::RelationKind;
use crate::validated::{GateCondition, ValidatedTransition};
use std::collections::HashSet;

impl<P: RecoveryPolicy> Reconciler<'_, P> {
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
            // A fan-out child remains completely mechanically inert until the
            // create intent clears staging after all sibling/parent wiring.
            if snapshot_is_staged(snapshot) {
                continue;
            }
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
        if snapshot.is_some_and(snapshot_is_staged) {
            return;
        }

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
}

fn snapshot_is_staged(snapshot: &ArtifactSnapshot) -> bool {
    parse_metadata_block(&snapshot.body)
        .ok()
        .flatten()
        .is_some_and(|metadata| metadata.staged)
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

fn dependency_relation_count(artifact: &ClassifiedArtifact) -> usize {
    artifact
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::Dependency)
        .count()
}
