//! The engine's store, in memory (engine-model.md, sections 6 and 11):
//! snapshots of parked runs by item, one each, and traces, appended and
//! expired. It outlives the engine's restarts, as a disk does. Each
//! operation is done when it arrives and answered after a latency drawn
//! from the seed; some fail, done or not, as a disk's can.

use std::collections::BTreeMap;

use temper_engine_model::{Item, Store as Op, Stored, Trace};
use temper_lib::{Duration, Rng, Time};
use temper_world::Span;

/// How the store behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    pub latency: Span,
    /// The chance per mille that an operation fails, and that a failed one
    /// was done all the same.
    pub failures: u32,
    pub done_anyway: u32,
}

/// What the store did, counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub puts: u32,
    pub gets: u32,
    pub found: u32,
    pub drops: u32,
    pub appended: u32,
    pub expired: u32,
    pub failed: u32,
}

#[derive(Debug)]
pub struct Store {
    script: Script,
    rng: Rng,
    snapshots: BTreeMap<Item, Box<[u8]>>,
    traces: Vec<Trace>,
    /// The most traces kept at once: the world's own bound.
    most: usize,
    tally: Tally,
}

impl Store {
    #[must_use]
    pub fn new(script: Script, seed: u64, most: usize) -> Store {
        Store {
            script,
            rng: Rng::new(seed),
            snapshots: BTreeMap::new(),
            traces: Vec::new(),
            most,
            tally: Tally::default(),
        }
    }

    /// Does `op`, and says how it ended and how long the answer takes.
    pub fn apply(&mut self, op: Op) -> (Stored, Duration) {
        let after = self.script.latency.draw(&mut self.rng);
        let failed = self.rng.chance(self.script.failures);
        if failed && !self.rng.chance(self.script.done_anyway) {
            self.tally.failed += 1;
            return (Stored::Failed, after);
        }
        let stored = match op {
            Op::Put { item, snapshot } => {
                self.tally.puts += 1;
                self.snapshots.insert(item, snapshot);
                Stored::Done
            }
            Op::Get { item } => {
                self.tally.gets += 1;
                let found = self.snapshots.get(&item).cloned();
                if found.is_some() {
                    self.tally.found += 1;
                }
                Stored::Got(found)
            }
            Op::Drop { item } => {
                self.tally.drops += 1;
                self.snapshots.remove(&item);
                Stored::Done
            }
            Op::Append { traces } => {
                self.tally.appended += u32::try_from(traces.len()).expect("few");
                self.traces.extend(traces.into_vec());
                assert!(self.traces.len() <= self.most, "the store's traces grew past the world's bound");
                Stored::Done
            }
            Op::Expire { before } => {
                let kept = self.traces.len();
                self.traces.retain(|trace| trace.at >= before);
                self.tally.expired += u32::try_from(kept - self.traces.len()).expect("few");
                Stored::Done
            }
        };
        if failed {
            self.tally.failed += 1;
            return (Stored::Failed, after);
        }
        (stored, after)
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// The snapshot kept for `item`, if any.
    #[must_use]
    pub fn snapshot(&self, item: Item) -> Option<&[u8]> {
        self.snapshots.get(&item).map(|snapshot| &**snapshot)
    }

    /// The traces kept, oldest first.
    #[must_use]
    pub fn traces(&self) -> &[Trace] {
        &self.traces
    }

    /// When the oldest trace kept was reported.
    #[must_use]
    pub fn oldest(&self) -> Option<Time> {
        self.traces.iter().map(|trace| trace.at).min()
    }
}
