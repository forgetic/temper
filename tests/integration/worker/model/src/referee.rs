//! What the world expects of the engine and the worker between them, held by
//! a referee (testing-pyramid.md, 5.2) that sees only what crosses the
//! channel, at the protocol layers: what the engine assigned, acknowledged
//! and answered, and what the worker's answers and relays brought it; and
//! the records the engine writes on the forge. Never a model's state.
//! Safety, on every observation:
//!
//! - an answer reaches the engine only for an attempt the engine assigned:
//!   nothing for an attempt it never made;
//! - the engine acknowledges only an answer that reached it; a call
//!   reaches it once, and it answers only a call that was relayed to it,
//!   for the same attempt, and once;
//! - the engine takes each attempt's answer once, however often it came:
//!   an item's record counts one more failure, in one class, for each of
//!   its attempts that failed, and never two for one.
//!
//! And liveness: an answer that reached the engine, but a refusal, is
//! acknowledged within a bound, unless the worker that sent it gave it up
//! as it shut down. What the engine's stories expect of it (what lands, what
//! is made once, one live run per item) the engine world's referee holds it
//! to, beside this one.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use temper_engine_model::work::{Failures, Lifecycle};
use temper_lib::{Duration, Token};
use temper_world::{Expectations, Judge};

use crate::protocol::Names;

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The engine assigned the attempt.
    Assigned {
        names: Names,
    },
    /// An answer for the attempt reached the engine: a refusal, or not.
    Answered {
        names: Names,
        refused: bool,
    },
    /// The engine acknowledged the attempt's answer.
    Acknowledged {
        names: Names,
    },
    /// A call of the attempt's reached the engine; the engine answered one.
    Relay {
        names: Names,
        call: Token,
    },
    Relayed {
        names: Names,
        call: Token,
    },
    /// The engine heard the channel it held lost.
    Lost,
    /// The engine wrote the record of the item `number` of `repository`,
    /// which says this of its lifecycle.
    Recorded {
        repository: Vec<u8>,
        number: u64,
        lifecycle: Lifecycle,
    },
    /// The worker stopped, having given up the answers of `given_up`; a new
    /// one starts cold.
    Stopped {
        given_up: Vec<Names>,
    },
    /// The world settled.
    Settled,
}

/// What the referee expects to happen: the attempt's answer acknowledged.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Acknowledged(pub Names);

/// The expectations between the engine and the worker.
#[derive(Debug)]
pub struct Hosting {
    /// Within which an answer that reached the engine is acknowledged.
    bound: Duration,
    assigned: BTreeSet<Names>,
    answered: BTreeSet<Names>,
    relays: BTreeSet<(Names, Token)>,
    /// Each item's failures as its last record said, and the attempts its
    /// records counted a failure for.
    failures: BTreeMap<(Vec<u8>, u64), Failures>,
    failed: BTreeSet<(Vec<u8>, u64, u64)>,
    pub lost: u32,
}

impl Hosting {
    #[must_use]
    pub fn new(bound: Duration) -> Hosting {
        Hosting {
            bound,
            assigned: BTreeSet::new(),
            answered: BTreeSet::new(),
            relays: BTreeSet::new(),
            failures: BTreeMap::new(),
            failed: BTreeSet::new(),
            lost: 0,
        }
    }

    /// The item's record says `lifecycle`: a failure more than its last said
    /// is its last claim's attempt's, the one failure that attempt has.
    fn recorded(&mut self, item: (Vec<u8>, u64), lifecycle: Lifecycle, judge: &mut Judge<Acknowledged, Infallible>) {
        let before = self.failures.insert(item.clone(), lifecycle.failures).unwrap_or(Failures::NONE);
        let after = lifecycle.failures;
        let more = [
            (before.transient, after.transient),
            (before.permanent, after.permanent),
            (before.run, after.run),
            (before.agent, after.agent),
            (before.lost, after.lost),
            (before.invalid, after.invalid),
        ]
        .into_iter()
        .map(|(before, after)| after.saturating_sub(before))
        .sum::<u32>();
        if more == 0 {
            return;
        }
        let (repository, number) = item;
        let attempt = lifecycle.attempts;
        judge.check(
            more == 1,
            format_args!("a record counts one failure at a time: {more} more for attempt {attempt} of {number}"),
        );
        judge.check(
            self.failed.insert((repository, number, attempt)),
            format_args!("an attempt fails once, however often its answer came: attempt {attempt} of {number}"),
        );
    }
}

impl Expectations for Hosting {
    type Seen = Seen;
    type Name = Acknowledged;
    type Stimulus = Infallible;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Acknowledged, Infallible>) {
        match seen {
            Seen::Assigned { names } => {
                self.assigned.insert(names);
            }
            Seen::Answered { names, refused } => {
                judge.check(
                    self.assigned.contains(&names),
                    format_args!("an answer reaches the engine only for an attempt it assigned: {names:?}"),
                );
                if !refused && self.answered.insert(names) {
                    judge.expect(Acknowledged(names), self.bound);
                }
            }
            Seen::Acknowledged { names } => {
                judge.check(
                    self.answered.contains(&names),
                    format_args!("the engine acknowledges only an answer that reached it: {names:?}"),
                );
                judge.meet(&Acknowledged(names));
            }
            Seen::Relay { names, call } => judge.check(
                self.relays.insert((names, call)),
                format_args!("a call reaches the engine once: {names:?} {call:?}"),
            ),
            Seen::Relayed { names, call } => judge.check(
                self.relays.remove(&(names, call)),
                format_args!("the engine answers only a call relayed to it, and once: {names:?} {call:?}"),
            ),
            Seen::Lost => self.lost += 1,
            Seen::Recorded { repository, number, lifecycle } => self.recorded((repository, number), lifecycle, judge),
            Seen::Stopped { given_up } => {
                for names in given_up {
                    judge.withdraw(&Acknowledged(names));
                }
            }
            Seen::Settled => {}
        }
    }
}
