//! What the work hub's scenarios expect, held by a referee
//! (testing-pyramid.md, 5.2) that sees what the fakes see (the records on the
//! forge and the writes it refuses, its keyed creations, the plan asked, the
//! runs on the workers) and people's answers, never the hub's state:
//!
//! - one run per item at a time: a worker takes an item's attempt only while
//!   no other attempt of it runs;
//! - a run starts only after its claim is written: the item's record names
//!   the attempt, claimed, as a worker takes it;
//! - attempts only grow, across restarts: each one taken past the last, and
//!   no record counts fewer than the last one did (but for a record a person
//!   mangled, read back counting from what its outcomes name);
//! - an outcome is applied at most once: the parent is asked to apply it
//!   only while the item's record says it is being applied, and never once a
//!   record has moved past it, its commit; no keyed creation is made twice
//!   (its outcome comment, its writes, an action's); an engine action is
//!   made only between runs, its record neither claiming, applying nor
//!   holding;
//! - an answer a worker forgets by the engine's word is on the forge: the
//!   item's record has moved past the claim of its attempt, or a later one
//!   fenced it (unless a person mangled the record, which loses it);
//! - a held item is never due until released: the plan is never asked about
//!   an item its record holds, or held since a write of its record was
//!   refused for good;
//! - the record's update comes last: an item's record says done only once
//!   the item is closed;
//! - and liveness: every item taken in ends, done or held, within a bound
//!   the world sets, and again within it once a person releases it or a
//!   restart forgets a hold the record does not show (a restart may also
//!   forget a release whose record it had not written yet: the item is held
//!   again).
//!
//! The referee injects the engine's restarts, at moments the scenario sets.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_domain_work::{Item, Lifecycle, Phase};
use temper_lib::Duration;
use temper_world::{Expectations, Judge};

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The engine took the item in.
    Taken { item: Item },
    /// The forge holds the item's record as written now.
    Recorded { item: Item, lifecycle: Lifecycle },
    /// The forge refused a write of the item's record for good: the engine
    /// holds the item, though its record does not say so.
    Refused { item: Item },
    /// The plan was asked what is due for the item.
    Asked { item: Item },
    /// A worker took the item's attempt: it runs.
    Started { item: Item, attempt: u64 },
    /// The item's attempt stopped running on its worker: it answered, or
    /// died with its worker.
    Finished { item: Item, attempt: u64 },
    /// The forge made the keyed creation `key`.
    Created { key: Key },
    /// The forge closed the item.
    Closed { item: Item },
    /// A person's release of the item was answered: it is released.
    Released { item: Item },
    /// The parent is asked to apply the outcome of the item's attempt
    /// `attempt`, posted as the comment `outcome`.
    Apply { item: Item, attempt: u64, outcome: u64 },
    /// The parent is asked to make an engine action's writes for the item.
    Act { item: Item },
    /// A worker forgot the answer of the item's attempt, as the engine said.
    Forgot { item: Item, attempt: u64 },
    /// A person mangled the item's record: what it said is lost, and the
    /// item's outcomes name attempts up to `attempts`.
    Mangled { item: Item, attempts: u64 },
    /// The engine restarted, as the referee injected: what it held in memory
    /// is gone.
    Restarted,
}

/// What a creation is keyed by (seams: a key derived from what causes it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Key {
    /// The comment an attempt's outcome is posted as.
    Outcome {
        item: Item,
        attempt: u64,
    },
    /// A write of an attempt's outcome, by its place among them.
    Write {
        item: Item,
        attempt: u64,
        index: u32,
    },
    /// The writes of an item's `nth` engine action, or of its last.
    Action {
        item: Item,
        nth: u32,
    },
    Done {
        item: Item,
    },
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The item ends, done or held.
    End(Item),
}

/// What the referee injects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    /// The engine's process stops and a new one starts, cold.
    Restart,
}

/// The expectations of the work hub's world.
#[derive(Debug)]
pub struct Work {
    /// How long an item may take to end.
    within: Duration,
    items: BTreeMap<Item, Tracked>,
    created: BTreeSet<Key>,
    /// Runs started, plan decisions asked, and applications and forgotten
    /// answers judged.
    pub starts: u64,
    pub asks: u64,
    pub applies: u64,
    pub forgets: u64,
}

/// What the referee knows of an item.
#[derive(Debug, Default)]
struct Tracked {
    /// Its record, as the forge holds it.
    record: Option<Lifecycle>,
    /// A write of its record was refused for good, and the engine holds it
    /// since.
    refused: bool,
    /// The attempt running on a worker, and the last one taken.
    running: Option<u64>,
    last: u64,
    closed: bool,
    /// The attempts whose outcomes were committed: a record moved past them.
    committed: BTreeSet<u64>,
    /// A person mangled its record, which no write has replaced since; and
    /// the highest attempt its outcomes name then.
    mangled: bool,
    bound: u64,
}

impl Tracked {
    fn held(&self) -> bool {
        let recorded = match self.record {
            Some(Lifecycle { phase: Phase::Held { .. }, .. }) => true,
            Some(Lifecycle {
                phase:
                    Phase::Waiting
                    | Phase::Parked
                    | Phase::Retrying(_)
                    | Phase::Claimed
                    | Phase::Applying { .. }
                    | Phase::Done,
                ..
            })
            | None => false,
        };
        recorded || self.refused
    }

    fn done(&self) -> bool {
        match self.record {
            Some(Lifecycle { phase: Phase::Done, .. }) => true,
            Some(Lifecycle {
                phase:
                    Phase::Waiting
                    | Phase::Parked
                    | Phase::Retrying(_)
                    | Phase::Claimed
                    | Phase::Applying { .. }
                    | Phase::Held { .. },
                ..
            })
            | None => false,
        }
    }
}

impl Work {
    /// Expectations under which every item ends `within` its hand-in or its
    /// release.
    #[must_use]
    pub fn new(within: Duration) -> Work {
        Work { within, items: BTreeMap::new(), created: BTreeSet::new(), starts: 0, asks: 0, applies: 0, forgets: 0 }
    }

    fn item(&mut self, item: Item) -> &mut Tracked {
        self.items.entry(item).or_default()
    }

    fn recorded(&mut self, item: Item, lifecycle: Lifecycle, judge: &mut Judge<Expected, Stimulus>) {
        let tracked = self.item(item);
        if let Some(earlier) = tracked.record {
            judge.check(
                lifecycle.attempts >= earlier.attempts || tracked.mangled,
                format_args!("attempts only grow: {item:?} recorded {lifecycle:?} after {earlier:?}"),
            );
            // A record that moves past an outcome being applied commits it,
            // unless it is held keeping it.
            if let Phase::Applying { outcome } = earlier.phase {
                let kept = match lifecycle.phase {
                    Phase::Applying { outcome: now } | Phase::Held { outcome: Some(now), .. } => now == outcome,
                    Phase::Waiting
                    | Phase::Parked
                    | Phase::Retrying(_)
                    | Phase::Claimed
                    | Phase::Held { outcome: None, .. }
                    | Phase::Done => false,
                };
                if !kept && !tracked.mangled {
                    tracked.committed.insert(earlier.attempts);
                }
            }
        }
        tracked.record = Some(lifecycle);
        tracked.refused = false;
        tracked.mangled = false;
        let closed = tracked.closed;
        if tracked.done() {
            judge.check(closed, format_args!("the record's update comes last: {item:?} is done before it is closed"));
        }
        if tracked.done() || tracked.held() {
            judge.meet(&Expected::End(item));
        }
    }

    /// The parent is asked to apply an outcome: only as the record says, and
    /// never once committed.
    fn apply(&mut self, item: Item, attempt: u64, outcome: u64, judge: &mut Judge<Expected, Stimulus>) {
        self.applies += 1;
        let tracked = self.item(item);
        let applying = match tracked.record {
            Some(Lifecycle { phase: Phase::Applying { outcome: recorded }, attempts, .. }) => {
                recorded == outcome && attempts == attempt
            }
            Some(Lifecycle {
                phase:
                    Phase::Waiting | Phase::Parked | Phase::Retrying(_) | Phase::Claimed | Phase::Held { .. } | Phase::Done,
                ..
            })
            | None => false,
        };
        judge.check(
            applying,
            format_args!("an outcome is applied as its record says: {item:?} {attempt} on {:?}", tracked.record),
        );
        let fresh = !tracked.committed.contains(&attempt);
        judge.check(fresh, format_args!("an outcome is applied at most once: {item:?} {attempt} committed"));
    }

    /// The parent is asked to make an engine action's writes: only between
    /// runs.
    fn act(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        let tracked = self.item(item);
        let between = match tracked.record {
            Some(Lifecycle { phase: Phase::Waiting | Phase::Parked | Phase::Retrying(_), .. }) | None => true,
            Some(Lifecycle {
                phase: Phase::Claimed | Phase::Applying { .. } | Phase::Held { .. } | Phase::Done,
                ..
            }) => false,
        };
        judge.check(between, format_args!("an engine action is made between runs: {item:?} on {:?}", tracked.record));
    }

    /// A worker forgot an answer, as the engine said: only once it is on the
    /// forge, or fenced.
    fn forgot(&mut self, item: Item, attempt: u64, judge: &mut Judge<Expected, Stimulus>) {
        self.forgets += 1;
        let tracked = self.item(item);
        let durable = match tracked.record {
            Some(Lifecycle { phase: Phase::Claimed, attempts, .. }) => attempts > attempt,
            Some(Lifecycle {
                phase:
                    Phase::Waiting
                    | Phase::Parked
                    | Phase::Retrying(_)
                    | Phase::Applying { .. }
                    | Phase::Held { .. }
                    | Phase::Done,
                attempts,
                ..
            }) => attempts >= attempt,
            None => false,
        };
        judge.check(
            durable || tracked.mangled,
            format_args!(
                "an answer is forgotten only once it is on the forge, or fenced: {item:?} {attempt} on {:?}",
                tracked.record
            ),
        );
    }

    fn started(&mut self, item: Item, attempt: u64, judge: &mut Judge<Expected, Stimulus>) {
        self.starts += 1;
        let tracked = self.item(item);
        let claimed = match tracked.record {
            Some(Lifecycle { phase: Phase::Claimed, attempts, .. }) => attempts == attempt,
            Some(Lifecycle {
                phase:
                    Phase::Waiting
                    | Phase::Parked
                    | Phase::Retrying(_)
                    | Phase::Applying { .. }
                    | Phase::Held { .. }
                    | Phase::Done,
                ..
            })
            | None => false,
        };
        judge.check(
            claimed,
            format_args!("a run starts only after its claim is written: {item:?} {attempt} on {:?}", tracked.record),
        );
        judge.check(
            tracked.running.is_none(),
            format_args!("one run per item: {item:?} {attempt} while {:?} runs", tracked.running),
        );
        judge.check(
            attempt > tracked.last,
            format_args!("attempts only grow: {item:?} {attempt} after {}", tracked.last),
        );
        tracked.running = Some(attempt);
        tracked.last = tracked.last.max(attempt);
    }
}

impl Expectations for Work {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Taken { item } => {
                let tracked = self.item(item);
                let ended = tracked.done() || tracked.held();
                if !ended && !judge.is_pending(&Expected::End(item)) {
                    judge.expect(Expected::End(item), self.within);
                }
            }
            Seen::Recorded { item, lifecycle } => self.recorded(item, lifecycle, judge),
            Seen::Refused { item } => {
                self.item(item).refused = true;
                judge.meet(&Expected::End(item));
            }
            Seen::Asked { item } => {
                self.asks += 1;
                let tracked = self.item(item);
                let (held, done) = (tracked.held(), tracked.done());
                judge.check(!held, format_args!("a held item is never due until released: {item:?}"));
                judge.check(!done, format_args!("a done item is not asked about: {item:?}"));
            }
            Seen::Started { item, attempt } => self.started(item, attempt, judge),
            Seen::Finished { item, attempt } => {
                let tracked = self.item(item);
                if tracked.running == Some(attempt) {
                    tracked.running = None;
                }
            }
            Seen::Created { key } => {
                let fresh = self.created.insert(key);
                judge.check(fresh, format_args!("an outcome is applied at most once: {key:?} made twice"));
            }
            Seen::Closed { item } => self.item(item).closed = true,
            Seen::Released { item } => {
                self.item(item).refused = false;
                if !judge.is_pending(&Expected::End(item)) {
                    judge.expect(Expected::End(item), self.within);
                }
            }
            Seen::Apply { item, attempt, outcome } => self.apply(item, attempt, outcome, judge),
            Seen::Act { item } => self.act(item, judge),
            Seen::Forgot { item, attempt } => self.forgot(item, attempt, judge),
            Seen::Mangled { item, attempts } => {
                let tracked = self.item(item);
                tracked.mangled = true;
                tracked.bound = attempts;
            }
            Seen::Restarted => {
                // The new process goes on from the records: a hold only the
                // engine's memory knew is gone, and a release whose record
                // had not been written yet with it, so the item is held again.
                // An item with a record is taken in again at once: it is to
                // end. One without is new again, and may wait at the entrance
                // behind held work: it is to end once it is taken in.
                // A record that does not decode holds its item.
                for (item, tracked) in &mut self.items {
                    tracked.refused = tracked.mangled;
                    if tracked.mangled {
                        // Read mangled, its attempts are counted again from
                        // what its outcomes name, and what the workers list.
                        tracked.last = tracked.bound;
                    }
                    let ended = tracked.done() || tracked.held() || tracked.record.is_none();
                    if ended {
                        judge.meet(&Expected::End(*item));
                    } else if !judge.is_pending(&Expected::End(*item)) {
                        judge.expect(Expected::End(*item), self.within);
                    }
                }
            }
        }
    }
}
