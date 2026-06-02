//! Portable webhook/poll trigger primitives.
//!
//! Triggering stays above the [`temper_forge::Forge`] trait: a [`ChangeHint`]
//! is only a wake-up hint that tells a worker to run its normal
//! pull/classify/plan/execute path.

use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::ItemNumber;
pub use temper_forge::{ChangeHint, ChangeKind};
use temper_workflow::RoleId;

/// Worker endpoint selected for a wake.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WakeTarget {
    Role(RoleId),
    Mechanical,
}

/// Broad, safe routing helper for the first webhook implementation.
///
/// It wakes every configured role plus the mechanical worker. This may do extra
/// work, but correctness still comes from fresh Forge scans.
pub fn broad_targets<I>(roles: I) -> BTreeSet<WakeTarget>
where
    I: IntoIterator<Item = RoleId>,
{
    let mut targets: BTreeSet<WakeTarget> = roles.into_iter().map(WakeTarget::Role).collect();
    targets.insert(WakeTarget::Mechanical);
    targets
}

/// Debounced, per-worker coalescing scheduler.
#[derive(Clone, Debug)]
pub struct TriggerScheduler {
    debounce: Duration,
    workers: BTreeMap<WakeTarget, WorkerState>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct WorkerState {
    due_at: Option<DateTime<Utc>>,
    in_flight: bool,
    dirty: bool,
    pending_keys: BTreeSet<HintKey>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HintKey {
    owner: String,
    name: String,
    item: Option<u64>,
    kind: ChangeKind,
}

impl From<&ChangeHint> for HintKey {
    fn from(hint: &ChangeHint) -> Self {
        Self {
            owner: hint.repo.owner.clone(),
            name: hint.repo.name.clone(),
            item: hint.item.map(ItemNumber::get),
            kind: hint.kind,
        }
    }
}

impl TriggerScheduler {
    /// Creates a scheduler with a positive debounce window.
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            workers: BTreeMap::new(),
        }
    }

    /// Records a hint routed to the supplied targets.
    pub fn submit<I>(&mut self, hint: &ChangeHint, targets: I, now: DateTime<Utc>)
    where
        I: IntoIterator<Item = WakeTarget>,
    {
        let key = HintKey::from(hint);
        let due = now + self.debounce;
        for target in targets {
            let state = self.workers.entry(target).or_default();
            if state.in_flight {
                state.dirty = true;
            } else {
                state.due_at = Some(state.due_at.map_or(due, |old| old.min(due)));
                state.pending_keys.insert(key.clone());
            }
        }
    }

    /// Returns targets whose debounced wake is due, marking each in-flight.
    pub fn due_targets(&mut self, now: DateTime<Utc>) -> Vec<WakeTarget> {
        let mut due = Vec::new();
        for (target, state) in &mut self.workers {
            if state.due_at.is_some_and(|due_at| due_at <= now) && !state.in_flight {
                state.due_at = None;
                state.pending_keys.clear();
                state.in_flight = true;
                due.push(target.clone());
            }
        }
        due
    }

    /// Marks a worker tick complete.
    ///
    /// If hints arrived during the tick, schedules at most one follow-up wake.
    pub fn finish(&mut self, target: &WakeTarget, now: DateTime<Utc>) {
        if let Some(state) = self.workers.get_mut(target) {
            state.in_flight = false;
            if state.dirty {
                state.dirty = false;
                state.due_at = Some(now + self.debounce);
            }
        }
    }

    /// Returns whether the target currently has a queued wake.
    pub fn is_pending(&self, target: &WakeTarget) -> bool {
        self.workers
            .get(target)
            .is_some_and(|state| state.due_at.is_some())
    }

    /// Returns the target's next scheduled wake time, if any.
    pub fn next_due_at(&self, target: &WakeTarget) -> Option<DateTime<Utc>> {
        self.workers.get(target).and_then(|state| state.due_at)
    }

    /// Returns whether the target has a tick in progress.
    pub fn is_in_flight(&self, target: &WakeTarget) -> bool {
        self.workers
            .get(target)
            .is_some_and(|state| state.in_flight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::{ItemNumber, RepositoryPath};

    fn t(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    fn hint() -> ChangeHint {
        ChangeHint::item(
            RepositoryPath::new("acme", "service"),
            ItemNumber::new(7),
            ChangeKind::Label,
        )
    }

    fn engineer() -> WakeTarget {
        WakeTarget::Role(RoleId::new("engineer"))
    }

    #[test]
    fn duplicate_burst_coalesces_to_one_wake() {
        let mut scheduler = TriggerScheduler::new(Duration::milliseconds(500));
        scheduler.submit(&hint(), [engineer()], t(0));
        scheduler.submit(&hint(), [engineer()], t(0));
        scheduler.submit(&hint(), [engineer()], t(0));

        assert!(scheduler.due_targets(t(0)).is_empty());
        assert_eq!(scheduler.due_targets(t(1)), vec![engineer()]);
        assert!(scheduler.due_targets(t(1)).is_empty());
    }

    #[test]
    fn hints_during_tick_schedule_one_follow_up() {
        let mut scheduler = TriggerScheduler::new(Duration::milliseconds(250));
        scheduler.submit(&hint(), [engineer()], t(0));
        assert_eq!(scheduler.due_targets(t(1)), vec![engineer()]);
        assert!(scheduler.is_in_flight(&engineer()));

        scheduler.submit(&hint(), [engineer()], t(1));
        scheduler.submit(&hint(), [engineer()], t(1));
        assert!(!scheduler.is_pending(&engineer()));
        scheduler.finish(&engineer(), t(2));

        assert!(scheduler.is_pending(&engineer()));
        assert_eq!(scheduler.due_targets(t(3)), vec![engineer()]);
    }

    #[test]
    fn broad_routing_includes_roles_and_mechanical() {
        let targets = broad_targets([RoleId::new("architect"), RoleId::new("owner")]);
        assert!(targets.contains(&WakeTarget::Role(RoleId::new("architect"))));
        assert!(targets.contains(&WakeTarget::Role(RoleId::new("owner"))));
        assert!(targets.contains(&WakeTarget::Mechanical));
    }
}
