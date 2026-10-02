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
//!   (`Ended` with a fault), call `finish` (`Delegated`, then wait for its
//!   `Return`), yield (`Yielded`, then wait for `Say` or `Close`), or carry
//!   on. A finish declares an outcome that fits the fake worker's charters
//!   or one that breaks them, a change or a verdict.
//! - It keeps to its share of the budget as a session keeps to its ceilings
//!   (seams.md B): it starts a turn only while turns, input and output each
//!   have some left; after a turn that went past any part of its share, it
//!   settles that turn's call, if it made one, and ends out of budget. It
//!   expires, out of time, when its time runs out.
//! - A finish in flight past its deadline is withdrawn, and the LLM carries
//!   on once it returns.
//! - `Say` comes only while it is yielded; `Close` at any time. Closed, it
//!   withdraws its finish in flight and waits for it to return, settles for
//!   a while (a turn in flight may win the race with the close and be spent),
//!   then sends its one `Ended`. A `Say` or `Close` for a conversation that
//!   has ended is dropped, as a stale handle is.
//!
//! What it does at a later time it asks the world to wake it for. Each wake
//! names the conversation and which of its wakes it is, so a wake that a
//! later one replaced is ignored.

use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model_run::outcome::{Change, Child, Declared, Field, Verdict};
use temper_agent_model_run::{Ask, Budget, End, Event, Exhausted, Fault, Opening, Returned, Spend, Stop};
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
    /// The tokens a turn spends: input and output drawn from `1..=` the
    /// largest of each, cache reads and writes each from `0..=` the largest.
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    /// After each turn, the chance, per mille, that the conversation fails,
    /// that the LLM calls `finish`, and that it yields; otherwise it carries
    /// on.
    pub faults: u32,
    pub finishes: u32,
    pub yields: u32,
    /// Of finishes, the chance, per mille, that the outcome is a change
    /// rather than a verdict, and that it fits the fake worker's charters.
    pub changes: u32,
    pub good: u32,
    /// How long a finish may take before the conversation withdraws it.
    pub finish_deadline: Span,
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
    /// Finishes called, and how they returned.
    pub finishes: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub checks_failed: u32,
    pub moved: u32,
    pub unpushed: u32,
    pub cancelled: u32,
    pub busy: u32,
    /// Finishes withdrawn: past their deadline, or as the conversation closed.
    pub withdrawn: u32,
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
    /// Conversations that have ended, which a stale `Say` or `Close` may name.
    ended: BTreeSet<Token>,
    /// The conversation each finish in flight is of.
    calls: BTreeMap<Token, Token>,
    /// Names for conversations and calls.
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
    /// Its finish `call` is in flight: its wake is the call's deadline or the
    /// expiry. `over` is the part of the share the turn that called it went
    /// past, if any.
    Finishing { call: Token, over: Option<Exhausted> },
    /// It withdrew its finish `call`, and waits for its return; then `then`.
    Withdrawn { call: Token, then: Then },
    /// Closed, settling: its wake ends it. `in_flight` says a turn was.
    Closing { in_flight: bool },
}

/// What a conversation does once its withdrawn finish returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Then {
    /// The LLM carries on, or the conversation ends past its share.
    CarryOn { over: Option<Exhausted> },
    /// It ends, out of time.
    Expire,
    /// It settles, closed.
    Close,
}

impl Partner {
    #[must_use]
    pub fn new(script: Script, seed: u64) -> Partner {
        Partner {
            script,
            rng: Rng::new(seed),
            talks: BTreeMap::new(),
            ended: BTreeSet::new(),
            calls: BTreeMap::new(),
            serial: 0,
            spent: Spend::ZERO,
            tally: Tally::default(),
        }
    }

    /// Conversations started and not ended, and finishes in flight.
    #[must_use]
    pub fn live(&self) -> usize {
        self.talks.len() + self.calls.len()
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
        assert!(opening.finish, "a main conversation may finish");
        let peer = self.mint();
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
            assert!(self.ended.contains(&peer), "a stale say names a conversation that ended");
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
            assert!(self.ended.contains(&peer), "a stale close names a conversation that ended");
            self.tally.stale += 1;
            return;
        };
        let conversation = talk.conversation;
        let in_flight = match talk.phase {
            Phase::Turning => true,
            Phase::Yielded => false,
            // It withdraws its finish in flight and waits for it to return,
            // with no wake.
            Phase::Finishing { call, over: _ } => {
                talk.phase = Phase::Withdrawn { call, then: Then::Close };
                talk.wake += 1;
                out.push(Out::Event(Event::Withdraw { conversation, call }));
                self.tally.withdrawn += 1;
                return;
            }
            Phase::Withdrawn { call, then: _ } => {
                talk.phase = Phase::Withdrawn { call, then: Then::Close };
                return;
            }
            Phase::Closing { .. } => panic!("the run closes a conversation once"),
        };
        talk.phase = Phase::Closing { in_flight };
        self.wake(now.saturating_add(settle), peer, out);
    }

    /// The run's answer to the finish `call`.
    pub fn returned(&mut self, now: Time, call: Token, result: &Returned, out: &mut Vec<Out>) {
        let peer = self.calls.remove(&call).expect("a return names a finish in flight");
        match result {
            Returned::Accepted => self.tally.accepted += 1,
            Returned::Rejected { .. } => self.tally.rejected += 1,
            Returned::ChecksFailed { .. } => self.tally.checks_failed += 1,
            Returned::Moved => self.tally.moved += 1,
            Returned::Unpushed => self.tally.unpushed += 1,
            Returned::Cancelled => self.tally.cancelled += 1,
            Returned::Busy => self.tally.busy += 1,
        }
        let talk = self.talks.get_mut(&peer).expect("a conversation outlives its calls");
        let then = match talk.phase {
            Phase::Finishing { call: finishing, over } => {
                assert_eq!(finishing, call, "a conversation has one finish in flight");
                Then::CarryOn { over }
            }
            Phase::Withdrawn { call: withdrawn, then } => {
                assert_eq!(withdrawn, call, "a conversation has one finish in flight");
                then
            }
            Phase::Turning | Phase::Yielded | Phase::Closing { .. } => {
                panic!("a return comes while its call is in flight")
            }
        };
        let expires = talk.expires;
        match then {
            Then::CarryOn { over: Some(exhausted) } => self.end(peer, End::Budget(exhausted), out),
            Then::CarryOn { over: None } if now >= expires => self.end(peer, End::Budget(Exhausted::Time), out),
            Then::CarryOn { over: None } => self.carry_on(now, peer, out),
            Then::Expire => self.end(peer, End::Budget(Exhausted::Time), out),
            Then::Close => {
                let settle = self.draw(self.script.settle);
                let talk = self.talks.get_mut(&peer).expect("a live conversation");
                talk.phase = Phase::Closing { in_flight: false };
                self.wake(now.saturating_add(settle), peer, out);
            }
        }
    }

    /// The wake numbered `wake` for `peer` has come.
    pub fn woken(&mut self, now: Time, peer: Token, wake: u64, out: &mut Vec<Out>) {
        let Some(talk) = self.talks.get_mut(&peer) else { return };
        if talk.wake != wake {
            return;
        }
        let (phase, expires, conversation) = (talk.phase, talk.expires, talk.conversation);
        match phase {
            Phase::Turning if now >= expires => self.end(peer, End::Budget(Exhausted::Time), out),
            Phase::Turning => self.turn(now, peer, out),
            Phase::Yielded => {
                assert!(now >= expires, "a yielded conversation wakes only when it expires");
                self.end(peer, End::Budget(Exhausted::Time), out);
            }
            Phase::Finishing { call, over } => {
                // Past the call's deadline, or the conversation's.
                let then = if now >= expires { Then::Expire } else { Then::CarryOn { over } };
                talk.phase = Phase::Withdrawn { call, then };
                out.push(Out::Event(Event::Withdraw { conversation, call }));
                self.tally.withdrawn += 1;
            }
            Phase::Withdrawn { .. } => unreachable!("a withdrawn finish waits without a wake"),
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
        let talk = self.talks.get(&peer).expect("a turn is of a live conversation");
        let over = overspent(&talk.budget, talk.spent);
        let roll = u32::try_from(self.rng.below(1000)).expect("below 1000");
        let Script { faults, finishes, yields, .. } = self.script;
        if roll < faults {
            let fault = if self.rng.chance(500) { Fault::Provider } else { Fault::ContextFull };
            self.tally.faults += 1;
            self.end(peer, End::Fault(fault), out);
        } else if roll < faults.saturating_add(finishes) {
            self.finish(now, peer, over, out);
        } else if let Some(exhausted) = over {
            // Past its share with no call to settle: it ends.
            self.tally.ceilings += 1;
            self.end(peer, End::Budget(exhausted), out);
        } else if roll < faults.saturating_add(finishes).saturating_add(yields) {
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

    /// The LLM calls `finish`, with an outcome drawn from the script.
    fn finish(&mut self, now: Time, peer: Token, over: Option<Exhausted>, out: &mut Vec<Out>) {
        let outcome = self.outcome();
        let call = self.mint();
        let deadline = self.draw(self.script.finish_deadline);
        let talk = self.talks.get_mut(&peer).expect("a live conversation finishes");
        talk.phase = Phase::Finishing { call, over };
        let (conversation, expires) = (talk.conversation, talk.expires);
        self.calls.insert(call, peer);
        out.push(Out::Event(Event::Delegated { conversation, call, ask: Ask::Finish { outcome } }));
        self.tally.finishes += 1;
        self.wake(now.saturating_add(deadline).min(expires), peer, out);
    }

    /// An outcome to declare: a change or a verdict, one that fits the fake
    /// worker's charters or one that does not.
    fn outcome(&mut self) -> Declared {
        let change = self.rng.chance(self.script.changes);
        let good = self.rng.chance(self.script.good);
        if change {
            let title = if good { b"Fix the parser"[..].into() } else { Box::default() };
            return Declared::Change(Change { title, body: b"It accepts tabs now."[..].into() });
        }
        let comment = |kind: &[u8], fields: &[&[u8]]| Child {
            kind: kind.into(),
            fields: fields.iter().map(|name| Field { name: (*name).into(), value: b"...".as_slice().into() }).collect(),
        };
        let (name, children): (&[u8], Box<[Child]>) = match (good, self.rng.below(3)) {
            (true, 0) => (b"approve", Box::new([])),
            (true, _) => {
                let count = self.rng.between(1, 3);
                (b"request-changes", (0..count).map(|_| comment(b"nit", &[b"path", b"body"])).collect())
            }
            (false, 0) => (b"reject", Box::new([])),
            (false, 1) => (b"request-changes", Box::new([])),
            (false, _) => (b"request-changes", Box::new([comment(b"praise", &[b"path"])])),
        };
        Declared::Verdict(Verdict { name: name.into(), body: b"See the comments."[..].into(), children })
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
        self.ended.insert(peer);
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

    fn mint(&mut self) -> Token {
        self.serial += 1;
        Token::new(self.serial)
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }
}

/// The part of `budget` that leaves no room to start another turn after
/// `spent`: no turn, input or output left.
fn ceiling(budget: &Budget, spent: Spend) -> Option<Exhausted> {
    if spent.turns >= budget.turns {
        Some(Exhausted::Turns)
    } else if spent.input >= budget.input {
        Some(Exhausted::Input)
    } else if spent.output >= budget.output {
        Some(Exhausted::Output)
    } else {
        None
    }
}

/// The first part of `budget` that `spent` has gone past, if any.
fn overspent(budget: &Budget, spent: Spend) -> Option<Exhausted> {
    if spent.turns > budget.turns {
        Some(Exhausted::Turns)
    } else if spent.input > budget.input {
        Some(Exhausted::Input)
    } else if spent.output > budget.output {
        Some(Exhausted::Output)
    } else if spent.cache_read > budget.cache_read {
        Some(Exhausted::CacheRead)
    } else if spent.cache_write > budget.cache_write {
        Some(Exhausted::CacheWrite)
    } else {
        None
    }
}
