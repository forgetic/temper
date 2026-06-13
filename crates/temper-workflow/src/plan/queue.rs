//! Queue matching and activation predicates for the pure planner.

use super::{conditions::gate_condition_satisfied, signals::GateSignals};
use crate::classify::ClassifiedArtifact;
use crate::compile::QueueManifest;
use crate::ids::{ArtifactKindId, LabelId};
use crate::validated::{GateCondition, QueueLabelSet, ValidatedQueue};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;

/// A queue query: artifact kinds, required labels, and optional activation.
///
/// Implemented by both [`ValidatedQueue`] and the compiled [`QueueManifest`] so
/// the same matcher and activation predicate work from the validated model or a
/// compiled manifest.
pub trait QueueQuery {
    /// Artifact kinds the queue selects.
    fn queue_artifacts(&self) -> &[ArtifactKindId];
    /// Labels that must all be present for an artifact to match every branch.
    fn queue_labels(&self) -> &[LabelId];
    /// Alternative AND-label clauses; an empty list means no disjunction.
    fn queue_any_of(&self) -> &[QueueLabelSet];
    /// Optional depth threshold before the queue should be serviced.
    fn queue_min_depth(&self) -> Option<u32> {
        None
    }
    /// Optional oldest-member age threshold before the queue should be serviced.
    fn queue_max_age(&self) -> Option<Duration> {
        None
    }
    /// Optional native/projected condition that must hold to match the queue.
    fn queue_condition(&self) -> Option<&GateCondition> {
        None
    }
}

impl QueueQuery for ValidatedQueue {
    fn queue_artifacts(&self) -> &[ArtifactKindId] {
        &self.artifacts
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
    fn queue_any_of(&self) -> &[QueueLabelSet] {
        &self.any_of
    }
    fn queue_min_depth(&self) -> Option<u32> {
        self.min_depth
    }
    fn queue_max_age(&self) -> Option<Duration> {
        self.max_age
    }
    fn queue_condition(&self) -> Option<&GateCondition> {
        self.condition.as_ref()
    }
}

impl QueueQuery for QueueManifest {
    fn queue_artifacts(&self) -> &[ArtifactKindId] {
        &self.artifacts
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
    fn queue_any_of(&self) -> &[QueueLabelSet] {
        &self.any_of
    }
    fn queue_min_depth(&self) -> Option<u32> {
        self.min_depth
    }
    fn queue_max_age(&self) -> Option<Duration> {
        self.max_age
    }
    fn queue_condition(&self) -> Option<&GateCondition> {
        self.condition.as_ref()
    }
}

/// A matched queue member that can report the timestamp used for age activation.
pub trait QueueMember {
    /// Timestamp used to decide whether this member is old enough to activate a queue.
    fn queue_pending_since(&self) -> Option<&DateTime<Utc>>;
}

impl QueueMember for ClassifiedArtifact {
    fn queue_pending_since(&self) -> Option<&DateTime<Utc>> {
        self.updated_at.as_ref()
    }
}

impl<T: QueueMember + ?Sized> QueueMember for &T {
    fn queue_pending_since(&self) -> Option<&DateTime<Utc>> {
        (*self).queue_pending_since()
    }
}

/// Returns `true` when a classified artifact matches a queue query.
///
/// An artifact matches when its kind is one of the queue's artifact kinds, every
/// common label is present, and either no `any_of` clauses are declared or at
/// least one clause's labels are all present. Queue activation policy is
/// deliberately separate from matching.
pub fn matches_queue<Q: QueueQuery + ?Sized>(query: &Q, artifact: &ClassifiedArtifact) -> bool {
    matches_queue_with(query, artifact, &GateSignals::default())
}

/// Returns `true` when a classified artifact passes a queue's kind and label filters.
///
/// This is the cheap first stage of queue matching: it deliberately ignores the
/// optional queue condition so callers can decide whether any runtime signal
/// reads are necessary before evaluating the condition.
pub fn matches_queue_cheap<Q: QueueQuery + ?Sized>(
    query: &Q,
    artifact: &ClassifiedArtifact,
) -> bool {
    if !query.queue_artifacts().contains(&artifact.kind) {
        return false;
    }
    let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
    query
        .queue_labels()
        .iter()
        .all(|label| labels.contains(label.as_str()))
        && (query.queue_any_of().is_empty()
            || query.queue_any_of().iter().any(|label_set| {
                label_set
                    .labels
                    .iter()
                    .all(|label| labels.contains(label.as_str()))
            }))
}

/// Returns `true` when a classified artifact matches a queue's optional condition.
pub fn matches_queue_condition<Q: QueueQuery + ?Sized>(
    query: &Q,
    artifact: &ClassifiedArtifact,
    signals: &GateSignals,
) -> bool {
    let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
    query.queue_condition().is_none_or(|condition| {
        gate_condition_satisfied(condition, artifact, &labels, signals)
    })
}

/// Returns `true` when a classified artifact matches a queue query and signals.
pub fn matches_queue_with<Q: QueueQuery + ?Sized>(
    query: &Q,
    artifact: &ClassifiedArtifact,
    signals: &GateSignals,
) -> bool {
    matches_queue_cheap(query, artifact) && matches_queue_condition(query, artifact, signals)
}

/// Returns `true` when matched members make a queue eligible for servicing.
///
/// Queues without an activation policy are active whenever they have at least
/// one member. Queues with `min_depth` and/or `max_age` are active when the
/// member count reaches `min_depth` or the oldest timestamped member is at
/// least `max_age` old at `now`. Empty queues are never active.
pub fn queue_active<Q, M>(query: &Q, members: &[M], now: DateTime<Utc>) -> bool
where
    Q: QueueQuery + ?Sized,
    M: QueueMember,
{
    if members.is_empty() {
        return false;
    }

    let min_depth = query.queue_min_depth();
    let max_age = query.queue_max_age();
    if min_depth.is_none() && max_age.is_none() {
        return true;
    }

    min_depth.is_some_and(|depth| members.len() >= depth as usize)
        || max_age.is_some_and(|age| {
            members
                .iter()
                .filter_map(QueueMember::queue_pending_since)
                .min()
                .is_some_and(|oldest| now.signed_duration_since(*oldest) >= age)
        })
}
