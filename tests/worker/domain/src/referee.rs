//! What the world expects of the engine and the worker between them, held by
//! a referee (testing.md, 5.2) that sees only what crosses the
//! channel, at the protocol layers: what the engine acknowledged and
//! answered, what the worker's hellos, answers and relays brought it, and
//! the channel lost; and the records the engine writes on the forge. Never a
//! domain's state. Safety, on every observation:
//!
//! - the engine acknowledges only an answer that reached it (a refusal
//!   among them: one that comes again, an assignment sent again and
//!   refused again, is for an attempt the engine has finished with, which it
//!   acknowledges again, and the worker, which keeps nothing of a refusal,
//!   ignores); a call
//!   reaches it once, and it answers only a call that was relayed to it,
//!   for the same attempt, and once;
//! - the engine takes each attempt's answer once, however often it came:
//!   an item's record counts one more failure, in one class, for each of
//!   its attempts that failed, and never two for one.
//!
//! And liveness: an answer that reached the engine, but a refusal, is
//! acknowledged within a bound of minutes while the channel it came on
//! stays open. The channel lost, or the engine restarting, withdraws the
//! expectation, and the next hello that lists the answer arms it again; a
//! worker that gave the answer up as it shut down withdraws it for good.
//! What the engine's stories expect of it (what lands, what is made once,
//! one live run per item) the engine world's referee holds it to, beside
//! this one.
//!
//! It injects what belongs to neither domain: a channel the world says drops
//! dropping, a drawn while after it opened (how long each lives is drawn
//! before the run starts); the worker told to shut down; and the engine
//! restarting, at moments drawn before the run starts, and at once as it
//! posts an outcome the world says it restarts after, so that it restarts
//! while it applies it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use skein_lib::{Duration, Token};
use temper_engine_domain::work::{Failures, Lifecycle};
use temper_world::{Expectations, Judge};

use crate::protocol::Names;

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The channel `epoch` opened; it is one that drops, if the next life
    /// drawn is left.
    Opened {
        epoch: u64,
        drops: bool,
    },
    /// A hello reached the engine, listing the attempts whose answers the
    /// worker keeps, `answered`.
    Hello {
        answered: Vec<Names>,
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
    /// The engine heard the channel it held lost; the engine restarted.
    Lost,
    Restarted,
    /// The engine posted a run's outcome, which it now applies; the world
    /// says whether the engine is one that restarts then.
    Posted {
        restarts: bool,
    },
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

/// What the referee injects: the channel `epoch` drops; the worker is told
/// to shut down; the engine restarts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    Drop { epoch: u64 },
    Shutdown,
    Restart,
}

/// The expectations between the engine and the worker.
#[derive(Debug)]
pub struct Hosting {
    /// Within which an answer that reached the engine is acknowledged, its
    /// channel open.
    bound: Duration,
    /// How long each channel that drops lives, in the order they open; and
    /// how often more the engine restarts as soon as it posts an outcome.
    lives: VecDeque<Duration>,
    applying: u32,
    /// The attempts whose answers reached the engine, refusals among them;
    /// those whose answers, but a refusal, did; and those it acknowledged.
    reached: BTreeSet<Names>,
    answered: BTreeSet<Names>,
    acknowledged: BTreeSet<Names>,
    relays: BTreeSet<(Names, Token)>,
    /// Each item's failures as its last record said, and the attempts its
    /// records counted a failure for.
    failures: BTreeMap<(Vec<u8>, u64), Failures>,
    failed: BTreeSet<(Vec<u8>, u64, u64)>,
}

impl Hosting {
    /// Expectations with answers acknowledged within `bound`, channels that
    /// drop each the next of `lives` after they open, and an engine that
    /// restarts as it applies an outcome up to `applying` times.
    #[must_use]
    pub fn new(bound: Duration, lives: Vec<Duration>, applying: u32) -> Hosting {
        Hosting {
            bound,
            lives: lives.into(),
            applying,
            reached: BTreeSet::new(),
            answered: BTreeSet::new(),
            acknowledged: BTreeSet::new(),
            relays: BTreeSet::new(),
            failures: BTreeMap::new(),
            failed: BTreeSet::new(),
        }
    }

    /// The answers that reached the engine and are not acknowledged yet.
    fn unacknowledged(&self) -> Vec<Names> {
        self.answered.difference(&self.acknowledged).copied().collect()
    }

    /// The item's record says `lifecycle`: a failure more than its last said
    /// is its last claim's attempt's, the one failure that attempt has.
    fn recorded(&mut self, item: (Vec<u8>, u64), lifecycle: Lifecycle, judge: &mut Judge<Acknowledged, Stimulus>) {
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
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Acknowledged, Stimulus>) {
        match seen {
            Seen::Opened { epoch, drops } => {
                if let Some(life) = drops.then(|| self.lives.pop_front()).flatten() {
                    judge.inject(judge.now().saturating_add(life), Stimulus::Drop { epoch });
                }
            }
            Seen::Hello { answered } => {
                for names in answered {
                    let unacknowledged = self.answered.contains(&names) && !self.acknowledged.contains(&names);
                    if unacknowledged && !judge.is_pending(&Acknowledged(names)) {
                        judge.expect(Acknowledged(names), self.bound);
                    }
                }
            }
            Seen::Answered { names, refused } => {
                self.reached.insert(names);
                if !refused && self.answered.insert(names) {
                    judge.expect(Acknowledged(names), self.bound);
                }
            }
            Seen::Acknowledged { names } => {
                judge.check(
                    self.reached.contains(&names),
                    format_args!("the engine acknowledges only an answer that reached it: {names:?}"),
                );
                self.acknowledged.insert(names);
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
            Seen::Posted { restarts } => {
                if restarts && self.applying > 0 {
                    self.applying -= 1;
                    judge.inject_now(Stimulus::Restart);
                }
            }
            Seen::Lost | Seen::Restarted => {
                for names in self.unacknowledged() {
                    judge.withdraw(&Acknowledged(names));
                }
            }
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
