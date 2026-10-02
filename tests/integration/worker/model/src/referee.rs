//! What the world expects of the engine and the worker between them, held by
//! a referee (testing-pyramid.md, 5.2) that sees only what crosses the
//! channel, at the protocol layers: what the engine assigned, acknowledged
//! and answered, and what the worker's answers and relays brought it. Never
//! a model's state. Safety, on every observation:
//!
//! - an answer reaches the engine only for an attempt the engine assigned:
//!   nothing for an attempt it never made;
//! - the engine acknowledges only an answer that reached it, and answers
//!   only a call that was relayed to it, for the same attempt.
//!
//! And liveness: an answer that reached the engine, but a refusal, is
//! acknowledged within a bound, unless the worker that sent it gave it up
//! as it shut down. What the engine's stories expect of it (what lands, what
//! is made once, one live run per item) the engine world's referee holds it
//! to, beside this one.

use std::collections::BTreeSet;
use std::convert::Infallible;

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
    pub lost: u32,
}

impl Hosting {
    #[must_use]
    pub fn new(bound: Duration) -> Hosting {
        Hosting { bound, assigned: BTreeSet::new(), answered: BTreeSet::new(), relays: BTreeSet::new(), lost: 0 }
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
            Seen::Relay { names, call } => {
                self.relays.insert((names, call));
            }
            Seen::Relayed { names, call } => judge.check(
                self.relays.contains(&(names, call)),
                format_args!("the engine answers only a call relayed to it: {names:?} {call:?}"),
            ),
            Seen::Lost => self.lost += 1,
            Seen::Stopped { given_up } => {
                for names in given_up {
                    judge.withdraw(&Acknowledged(names));
                }
            }
            Seen::Settled => {}
        }
    }
}
