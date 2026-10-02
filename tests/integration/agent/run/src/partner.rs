//! A scripted conversation partner: what the run's conversations would be,
//! the session sub-model and the LLM behind it, played from a seed.
//!
//! It speaks the run's conversation vocabulary, as the top level will once it
//! translates the session's, and plays the conversations' contract:
//!
//! - An opening is refused (`Ended` as busy or invalid, with no `Started`) or
//!   started at once.
//! - A started conversation takes turns, each a `Used` once its latency has
//!   passed. After each, the script draws what the LLM does next: fail
//!   (`Ended` with a fault), yield (`Yielded`, then wait for `Say` or
//!   `Close`), or carry on.
//! - It keeps to its share of the budget as a session keeps to its ceilings:
//!   before each turn it ends, out of budget, when no turn, input or output
//!   is left, or once a cache part is past its share; and it expires, out of
//!   time, when its time runs out, yielded or not.
//! - `Say` comes only while it is yielded; `Close` at any time. Closed, it
//!   settles for a while, and a turn in flight may win the race with the
//!   close and be spent; then it sends its one `Ended`. A `Say` or `Close`
//!   for a conversation that has ended is dropped, as a stale handle is.
//!
//! What it does at a later time it asks the world to wake it for. Each wake
//! names the conversation and which of its wakes it is, so a wake that a
//! later one replaced is ignored.

use std::collections::BTreeMap;

use temper_agent_model_run::{Budget, End, Event, Exhausted, Fault, Opening, Spend, Stop};
use temper_lib::{Duration, Rng, Time, Token};

use crate::world::Span;

/// How the partner behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// Conversations at once. An opening beyond them is refused as busy.
    pub conversations: u32,
    /// The chance, per mille, that an opening is refused as invalid.
    pub invalid: u32,
    /// How long a turn takes.
    pub turn: Span,
    /// The tokens a turn spends: input and output drawn from `0..=` the
    /// largest of each, cache reads and writes each from `0..=` the largest.
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    /// After each turn, the chance, per mille, that the conversation fails,
    /// and that the LLM yields.
    pub faults: u32,
    pub yields: u32,
    /// The chance, per mille, that a yield stops for something other than the
    /// end of a turn.
    pub odd_stops: u32,
    /// How long a closed conversation takes to settle.
    pub settle: Span,
    /// The chance, per mille, that a turn in flight wins the race with a close.
    pub races: u32,
}

/// What the partner asks of the world.
#[derive(Debug)]
pub enum Out {
    /// An event for the run.
    Event(Event),
    /// Wake the partner at `at` for `peer`'s wake numbered `wake`.
    Wake { at: Time, peer: Token, wake: u64 },
}

/// What the partner counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub opened: u32,
    pub refused: u32,
    pub turns: u32,
    pub yields: u32,
    pub nudged: u32,
    pub faults: u32,
    /// Conversations that ended at a ceiling of their share, or out of time.
    pub ceilings: u32,
    pub expired: u32,
    pub closed: u32,
    /// Turns in flight that won the race with a close.
    pub races: u32,
    /// `Say` and `Close` for conversations that had ended.
    pub stale: u32,
}

pub struct Partner {
    script: Script,
    rng: Rng,
    talks: BTreeMap<Token, Talk>,
    /// Names for conversations.
    serial: u64,
    /// What every conversation has spent, by its `Used`.
    spent: Spend,
    tally: Tally,
}

/// A started conversation.
struct Talk {
    /// The run's name for it.
    conversation: Token,
    budget: Budget,
    expires: Time,
    spent: Spend,
    phase: Phase,
    /// The wake that counts; earlier ones are stale.
    wake: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// A turn is in flight: its wake ends it.
    Turning,
    /// Waiting for `Say` or `Close`: its wake is the expiry.
    Yielded,
    /// Closed, settling: its wake ends it. `in_flight` says a turn was.
    Closing { in_flight: bool },
}

impl Partner {
    #[must_use]
    pub fn new(script: Script, seed: u64) -> Partner {
        Partner {
            script,
            rng: Rng::new(seed),
            talks: BTreeMap::new(),
            serial: 0,
            spent: Spend::ZERO,
            tally: Tally::default(),
        }
    }

    /// Conversations started and not ended.
    #[must_use]
    pub fn live(&self) -> usize {
        self.talks.len()
    }

    /// What every conversation has spent.
    #[must_use]
    pub fn spent(&self) -> Spend {
        self.spent
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// The most a turn may spend.
    #[must_use]
    pub fn turn_max(&self) -> Spend {
        let Script { input, output, cache, .. } = self.script;
        Spend { turns: 1, input, output, cache_read: cache, cache_write: cache }
    }

    pub fn open(&mut self, now: Time, conversation: Token, opening: &Opening, out: &mut Vec<Out>) {
        let full = self.talks.len() >= usize::try_from(self.script.conversations).expect("a u32 fits");
        if full || self.rng.chance(self.script.invalid) {
            let end = if full { End::Busy } else { End::Invalid };
            out.push(Out::Event(Event::Ended { conversation, end, spend: Spend::ZERO }));
            self.tally.refused += 1;
            return;
        }
        self.serial += 1;
        let peer = Token::new(self.serial);
        let expires = now.saturating_add(opening.budget.time);
        let talk =
            Talk { conversation, budget: opening.budget, expires, spent: Spend::ZERO, phase: Phase::Turning, wake: 0 };
        self.talks.insert(peer, talk);
        out.push(Out::Event(Event::Started { conversation, peer }));
        self.tally.opened += 1;
        self.carry_on(now, peer, out);
    }

    pub fn say(&mut self, now: Time, peer: Token, out: &mut Vec<Out>) {
        let Some(talk) = self.talks.get(&peer) else {
            self.tally.stale += 1;
            return;
        };
        assert_eq!(talk.phase, Phase::Yielded, "the run says something only to a yielded conversation");
        self.tally.nudged += 1;
        self.carry_on(now, peer, out);
    }

    pub fn close(&mut self, now: Time, peer: Token, out: &mut Vec<Out>) {
        let settle = self.draw(self.script.settle);
        let Some(talk) = self.talks.get_mut(&peer) else {
            self.tally.stale += 1;
            return;
        };
        talk.phase = match talk.phase {
            Phase::Turning => Phase::Closing { in_flight: true },
            Phase::Yielded => Phase::Closing { in_flight: false },
            Phase::Closing { .. } => panic!("the run closes a conversation once"),
        };
        self.wake(now.saturating_add(settle), peer, out);
    }

    /// The wake numbered `wake` for `peer` has come.
    pub fn woken(&mut self, now: Time, peer: Token, wake: u64, out: &mut Vec<Out>) {
        let Some(talk) = self.talks.get(&peer) else { return };
        if talk.wake != wake {
            return;
        }
        match talk.phase {
            Phase::Turning if now >= talk.expires => self.end(peer, End::Budget(Exhausted::Time), out),
            Phase::Turning => self.turn(now, peer, out),
            Phase::Yielded => {
                assert!(now >= talk.expires, "a yielded conversation wakes only when it expires");
                self.end(peer, End::Budget(Exhausted::Time), out);
            }
            Phase::Closing { in_flight } => {
                if in_flight && self.rng.chance(self.script.races) {
                    self.spend(peer, out);
                    self.tally.races += 1;
                }
                self.tally.closed += 1;
                self.end(peer, End::Closed, out);
            }
        }
    }

    /// A turn in flight completes; the script draws what comes next.
    fn turn(&mut self, now: Time, peer: Token, out: &mut Vec<Out>) {
        self.spend(peer, out);
        let roll = u32::try_from(self.rng.below(1000)).expect("below 1000");
        if roll < self.script.faults {
            let fault = if self.rng.chance(500) { Fault::Provider } else { Fault::ContextFull };
            self.tally.faults += 1;
            self.end(peer, End::Fault(fault), out);
        } else if roll < self.script.faults.saturating_add(self.script.yields) {
            let stop = if self.rng.chance(self.script.odd_stops) {
                match self.rng.below(3) {
                    0 => Stop::MaxTokens,
                    1 => Stop::Refusal,
                    _ => Stop::NoCalls,
                }
            } else {
                Stop::EndTurn
            };
            let talk = self.talks.get_mut(&peer).expect("a turn is of a live conversation");
            talk.phase = Phase::Yielded;
            let (conversation, expires) = (talk.conversation, talk.expires);
            out.push(Out::Event(Event::Yielded { conversation, stop, text: b"I have looked into it."[..].into() }));
            self.tally.yields += 1;
            self.wake(expires, peer, out);
        } else {
            self.carry_on(now, peer, out);
        }
    }

    /// The LLM goes on: another turn, unless the share leaves no room for one.
    fn carry_on(&mut self, now: Time, peer: Token, out: &mut Vec<Out>) {
        let latency = self.draw(self.script.turn);
        let talk = self.talks.get_mut(&peer).expect("a live conversation");
        if let Some(exhausted) = ceiling(&talk.budget, talk.spent) {
            self.tally.ceilings += 1;
            self.end(peer, End::Budget(exhausted), out);
            return;
        }
        talk.phase = Phase::Turning;
        // A turn that would outlast the conversation's time is cut off when it
        // expires.
        let at = now.saturating_add(latency).min(talk.expires);
        self.wake(at, peer, out);
    }

    fn spend(&mut self, peer: Token, out: &mut Vec<Out>) {
        let Script { input, output, cache, .. } = self.script;
        let spend = Spend {
            turns: 1,
            input: self.rng.between(1, input),
            output: self.rng.between(1, output),
            cache_read: self.rng.below(cache.saturating_add(1)),
            cache_write: self.rng.below(cache.saturating_add(1)),
        };
        let talk = self.talks.get_mut(&peer).expect("a live conversation spends");
        talk.spent = talk.spent.saturating_add(spend);
        self.spent = self.spent.saturating_add(spend);
        self.tally.turns += 1;
        out.push(Out::Event(Event::Used { conversation: talk.conversation, spend }));
    }

    fn end(&mut self, peer: Token, end: End, out: &mut Vec<Out>) {
        let talk = self.talks.remove(&peer).expect("a conversation ends once");
        if end == End::Budget(Exhausted::Time) {
            self.tally.expired += 1;
        }
        out.push(Out::Event(Event::Ended { conversation: talk.conversation, end, spend: talk.spent }));
    }

    /// Replaces `peer`'s wake with one at `at`.
    fn wake(&mut self, at: Time, peer: Token, out: &mut Vec<Out>) {
        let talk = self.talks.get_mut(&peer).expect("a live conversation is woken");
        talk.wake += 1;
        out.push(Out::Wake { at, peer, wake: talk.wake });
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }
}

/// The part of `budget` that leaves no room for another turn after `spent`:
/// no turn, input or output left, or a cache part gone past (a turn need not
/// cache).
fn ceiling(budget: &Budget, spent: Spend) -> Option<Exhausted> {
    if spent.turns >= budget.turns {
        Some(Exhausted::Turns)
    } else if spent.input >= budget.input {
        Some(Exhausted::Input)
    } else if spent.output >= budget.output {
        Some(Exhausted::Output)
    } else if spent.cache_read > budget.cache_read {
        Some(Exhausted::CacheRead)
    } else if spent.cache_write > budget.cache_write {
        Some(Exhausted::CacheWrite)
    } else {
        None
    }
}
