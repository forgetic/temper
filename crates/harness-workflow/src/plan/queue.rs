//! Queue matching and activation predicates for the pure planner.

use crate::classify::ClassifiedArtifact;
use crate::compile::QueueManifest;
use crate::ids::{ArtifactKindId, LabelId};
use crate::validated::ValidatedQueue;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;

/// A queue query: an artifact kind, required labels, and optional activation.
///
/// Implemented by both [`ValidatedQueue`] and the compiled [`QueueManifest`] so
/// the same matcher and activation predicate work from the validated model or a
/// compiled manifest.
pub trait QueueQuery {
    /// Artifact kind the queue selects.
    fn queue_artifact(&self) -> &ArtifactKindId;
    /// Labels that must all be present for an artifact to match.
    fn queue_labels(&self) -> &[LabelId];
    /// Optional depth threshold before the queue should be serviced.
    fn queue_min_depth(&self) -> Option<u32> {
        None
    }
    /// Optional oldest-member age threshold before the queue should be serviced.
    fn queue_max_age(&self) -> Option<Duration> {
        None
    }
}

impl QueueQuery for ValidatedQueue {
    fn queue_artifact(&self) -> &ArtifactKindId {
        &self.artifact
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
    fn queue_min_depth(&self) -> Option<u32> {
        self.min_depth
    }
    fn queue_max_age(&self) -> Option<Duration> {
        self.max_age
    }
}

impl QueueQuery for QueueManifest {
    fn queue_artifact(&self) -> &ArtifactKindId {
        &self.artifact
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
    fn queue_min_depth(&self) -> Option<u32> {
        self.min_depth
    }
    fn queue_max_age(&self) -> Option<Duration> {
        self.max_age
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
/// An artifact matches when its kind equals the queue's artifact kind and every
/// label the queue requires is present on the artifact. Queue activation policy
/// is deliberately separate so existing matching semantics stay unchanged.
pub fn matches_queue<Q: QueueQuery>(query: &Q, artifact: &ClassifiedArtifact) -> bool {
    if query.queue_artifact() != &artifact.kind {
        return false;
    }
    let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
    query
        .queue_labels()
        .iter()
        .all(|label| labels.contains(label.as_str()))
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
