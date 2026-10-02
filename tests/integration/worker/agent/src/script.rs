//! A scripted agent process: the agent's side of the channel, in its own
//! terms, played from a seed. It shares no type with the agent sub-model: the
//! world translates what it writes and reads ([`crate::translate`]), as a
//! protocol layer would encode and decode them.
//!
//! Once it hears its start, a run takes a number of steps, each after a
//! while: a fact, a host call (some of which it waits for), a long operation
//! (silent for up to its span), or a wait for an inbound event (parking if
//! none comes within its idle time). Then it meets its fate: it ends, parks or
//! fails as it says, and exits after a while (sometimes slowly, past the
//! grace); or it misbehaves: it crashes (exits without a word), hangs
//! (silence), overruns (progress until its wall time), writes garbage, reuses
//! a call's name before its answer, says more after its finish, says more
//! than the limits allow, or stops reading its channel. A cancel winds it down
//! to a cancelled failure, unless it ignores cancels; a terminate makes it
//! exit after a while, unless it ignores that too, and a kill always does.
//!
//! It checks what it hears as it goes: the start first and once, inbound
//! events once each, in the order sent, and an answer only to a call it made.

use std::collections::BTreeMap;

use temper_lib::{Duration, Rng, Time};
use temper_world::Span;

/// What a scripted agent writes up its channel.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Said {
    /// A host call named `name`: a push, or a relayed call.
    Call {
        name: u64,
        push: bool,
        body: Vec<u8>,
    },
    Fact {
        text: Vec<u8>,
    },
    /// It started an operation that may run for `span`.
    Long {
        span: Duration,
    },
    /// It waits for its next inbound event.
    Waiting,
    Ended {
        outcome: Vec<u8>,
    },
    Parked {
        snapshot: Option<Vec<u8>>,
    },
    Failed {
        why: Why,
    },
    /// Bytes that are no message.
    Garbage,
}

/// What a scripted agent reads down its channel.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Heard {
    Start {
        charter: Vec<u8>,
        snapshot: Option<Vec<u8>>,
    },
    /// An inbound event; its first eight bytes are its place in the order
    /// the client sent them.
    Event {
        event: Vec<u8>,
    },
    Answer {
        name: u64,
        answer: Answer,
    },
    Cancel,
}

/// An answer, as the scripted agent reads it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Answer {
    Relayed { answer: Vec<u8> },
    Pushed { done: bool },
    Unavailable,
    Busy,
    TooLarge,
}

/// Why a scripted run failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    Model,
    Budget,
    Policy,
    Cancelled,
    Stale,
}

/// The limits the agent writes within, or one byte past when it misbehaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sizes {
    pub call: u64,
    pub fact: u64,
    pub outcome: u64,
    pub snapshot: u64,
}

/// How scripted agents behave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// The most steps a run takes before its fate, and the time between them.
    pub steps: u32,
    pub step: Span,
    /// The chance, per mille, that a step is a call, a long operation or a
    /// wait for an inbound event; otherwise it is a fact.
    pub calls: u32,
    pub longs: u32,
    pub waits: u32,
    /// The chance, per mille, that a call is a push, and that the run waits
    /// for its answer.
    pub pushes: u32,
    pub blocking: u32,
    /// The span a long operation announces; it ends within it.
    pub long: Span,
    /// How long a waiting run waits for an inbound event before it parks.
    pub idle: Span,
    /// How runs end, by weight.
    pub fates: Fates,
    /// How long an agent takes to exit after its finish, and the chance,
    /// per mille, that it takes `slow_exit` instead.
    pub exit: Span,
    pub slow_exits: u32,
    pub slow_exit: Span,
    /// The chance, per mille, that it ignores a cancel, and how long it takes
    /// to wind down when it does not.
    pub deaf_to_cancel: u32,
    pub wind: Span,
    /// The chance, per mille, that it ignores a terminate, and how long it
    /// takes to exit when it does not.
    pub stubborn: u32,
    pub term: Span,
}

/// The weights of a run's fates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fates {
    pub ended: u32,
    pub parked: u32,
    pub failed: u32,
    pub crash: u32,
    pub hang: u32,
    pub overrun: u32,
    pub garbage: u32,
    pub duplicate: u32,
    pub trailing: u32,
    pub oversized: u32,
    pub deaf: u32,
}

/// How a run ends, as its script has it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fate {
    Ended,
    Parked,
    Failed,
    Crash,
    Hang,
    Overrun,
    Garbage,
    Duplicate,
    Trailing,
    Oversized,
    Deaf,
}

impl Fates {
    fn draw(self, rng: &mut Rng) -> Fate {
        let weights = [
            (self.ended, Fate::Ended),
            (self.parked, Fate::Parked),
            (self.failed, Fate::Failed),
            (self.crash, Fate::Crash),
            (self.hang, Fate::Hang),
            (self.overrun, Fate::Overrun),
            (self.garbage, Fate::Garbage),
            (self.duplicate, Fate::Duplicate),
            (self.trailing, Fate::Trailing),
            (self.oversized, Fate::Oversized),
            (self.deaf, Fate::Deaf),
        ];
        let total: u64 = weights.iter().map(|(weight, _)| u64::from(*weight)).sum();
        assert!(total > 0, "some fate has weight");
        let mut roll = rng.below(total);
        for (weight, fate) in weights {
            if roll < u64::from(weight) {
                return fate;
            }
            roll -= u64::from(weight);
        }
        unreachable!("the roll is below the total")
    }
}

/// What the agent does: for the process tree to carry out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Act {
    /// It writes `said` up its channel.
    Write(Said),
    /// Wake it, `after` from now, for its wake `serial`.
    Wake { after: Duration, serial: u64 },
    /// Its process exits, `after` from now.
    Exit { after: Duration },
    /// It stops reading its channel.
    Deaf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// It has not heard its start.
    Unstarted,
    /// It takes its steps.
    Working,
    /// It waits for the answer to its call `name`.
    Blocked { name: u64 },
    /// In a long operation, until its wake.
    Long,
    /// It waits for an inbound event, and parks at its wake.
    Waiting,
    /// Silent, for good.
    Hung,
    /// Progress, for good.
    Overrunning,
    /// Cancelled: it finishes as cancelled at its wake.
    Winding,
    /// It has said all it will, and exits.
    Done,
}

/// What the world reads of an agent for its checks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "what the world reads, one check each")]
pub struct View {
    pub fate: Fate,
    /// When it last wrote.
    pub last_write: Time,
    /// When its last long operation's span runs out.
    pub long_until: Time,
    /// It waits for a call's answer, or for an inbound event.
    pub blocked: bool,
    pub waiting: bool,
    /// It stopped reading its channel.
    pub deaf: bool,
    /// It wrote its finish.
    pub finished: bool,
}

/// A scripted agent's run.
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "a script's traits, drawn once, and what the world reads")]
pub struct Agent {
    rng: Rng,
    script: Script,
    sizes: Sizes,
    fate: Fate,
    steps: u32,
    phase: Phase,
    /// The serial of its wake armed now; an older one is stale.
    serial: u64,
    names: u64,
    /// Its calls without an answer, by name: a name it reused counts twice.
    awaiting: BTreeMap<u64, u32>,
    /// Inbound events heard and not yet taken.
    events: u32,
    last_event: Option<u64>,
    obeys_cancel: bool,
    obeys_terminate: bool,
    slow_exit: bool,
    last_write: Time,
    long_until: Time,
    deaf: bool,
    finished: bool,
}

impl Agent {
    #[must_use]
    pub fn new(script: Script, sizes: Sizes, seed: u64) -> Agent {
        let mut rng = Rng::new(seed);
        let fate = script.fates.draw(&mut rng);
        let steps = u32::try_from(rng.below(u64::from(script.steps) + 1)).expect("fits");
        let obeys_cancel = !rng.chance(script.deaf_to_cancel);
        let obeys_terminate = !rng.chance(script.stubborn);
        let slow_exit = rng.chance(script.slow_exits);
        Agent {
            rng,
            script,
            sizes,
            fate,
            steps,
            phase: Phase::Unstarted,
            serial: 0,
            names: 0,
            awaiting: BTreeMap::new(),
            events: 0,
            last_event: None,
            obeys_cancel,
            obeys_terminate,
            slow_exit,
            last_write: Time::ZERO,
            long_until: Time::ZERO,
            deaf: false,
            finished: false,
        }
    }

    #[must_use]
    pub fn view(&self) -> View {
        View {
            fate: self.fate,
            last_write: self.last_write,
            long_until: self.long_until,
            blocked: match self.phase {
                Phase::Blocked { .. } => true,
                Phase::Unstarted
                | Phase::Working
                | Phase::Long
                | Phase::Waiting
                | Phase::Hung
                | Phase::Overrunning
                | Phase::Winding
                | Phase::Done => false,
            },
            waiting: self.phase == Phase::Waiting,
            deaf: self.deaf,
            finished: self.finished,
        }
    }

    /// It hears `heard` at `now`.
    pub fn hear(&mut self, now: Time, heard: Heard) -> Vec<Act> {
        let mut acts = Vec::new();
        match heard {
            Heard::Start { .. } => {
                assert_eq!(self.phase, Phase::Unstarted, "the start comes first, once");
                self.phase = Phase::Working;
                self.next(&mut acts);
            }
            Heard::Event { event } => {
                assert!(self.phase != Phase::Unstarted, "events come after the start");
                let place: [u8; 8] = event.get(..8).expect("an event begins with its place").try_into().expect("eight");
                let place = u64::from_be_bytes(place);
                assert!(self.last_event < Some(place), "inbound events come once each, in the order sent");
                self.last_event = Some(place);
                if self.phase == Phase::Waiting {
                    self.write(now, Said::Fact { text: b"woken".to_vec() }, &mut acts);
                    self.phase = Phase::Working;
                    self.next(&mut acts);
                } else {
                    self.events += 1;
                }
            }
            Heard::Answer { name, answer: _ } => {
                let count = self.awaiting.get_mut(&name).expect("an answer is to a call the agent made");
                *count -= 1;
                if *count == 0 {
                    self.awaiting.remove(&name);
                }
                if self.phase == (Phase::Blocked { name }) {
                    self.phase = Phase::Working;
                    self.next(&mut acts);
                }
            }
            Heard::Cancel => match self.phase {
                Phase::Unstarted => unreachable!("the start comes first"),
                Phase::Done | Phase::Winding => {}
                Phase::Working
                | Phase::Blocked { .. }
                | Phase::Long
                | Phase::Waiting
                | Phase::Hung
                | Phase::Overrunning => {
                    if self.obeys_cancel {
                        self.phase = Phase::Winding;
                        let wind = self.script.wind.draw(&mut self.rng);
                        self.wake(wind, &mut acts);
                    }
                }
            },
        }
        acts
    }

    /// Its wake `serial` falls due at `now`.
    pub fn woken(&mut self, now: Time, serial: u64) -> Vec<Act> {
        let mut acts = Vec::new();
        if serial != self.serial {
            return acts;
        }
        match self.phase {
            Phase::Working => self.step(now, &mut acts),
            Phase::Long => {
                self.write(now, Said::Fact { text: b"checked".to_vec() }, &mut acts);
                self.phase = Phase::Working;
                self.next(&mut acts);
            }
            Phase::Waiting => {
                let snapshot = self.snapshot();
                self.finish(now, Said::Parked { snapshot }, &mut acts);
            }
            Phase::Overrunning => {
                self.write(now, Said::Fact { text: b"more".to_vec() }, &mut acts);
                self.next(&mut acts);
            }
            Phase::Winding => self.finish(now, Said::Failed { why: Why::Cancelled }, &mut acts),
            Phase::Unstarted | Phase::Blocked { .. } | Phase::Hung | Phase::Done => {
                unreachable!("no wake is armed in {:?}", self.phase)
            }
        }
        acts
    }

    /// It is told to terminate: it exits after a while, unless it ignores
    /// that.
    pub fn terminate(&mut self) -> Vec<Act> {
        let mut acts = Vec::new();
        if self.obeys_terminate && self.phase != Phase::Done {
            self.phase = Phase::Done;
            self.serial += 1;
            let term = self.script.term.draw(&mut self.rng);
            acts.push(Act::Exit { after: term });
        }
        acts
    }

    /// A step, or its fate once its steps are taken.
    fn step(&mut self, now: Time, acts: &mut Vec<Act>) {
        if self.steps == 0 {
            self.meet_fate(now, acts);
            return;
        }
        self.steps -= 1;
        let script = self.script;
        let roll = u32::try_from(self.rng.below(1000)).expect("fits");
        if roll < script.calls {
            let name = self.call(now, acts);
            if self.rng.chance(script.blocking) {
                self.phase = Phase::Blocked { name };
            } else {
                self.next(acts);
            }
        } else if roll < script.calls + script.longs {
            let span = script.long.draw(&mut self.rng);
            self.write(now, Said::Long { span }, acts);
            self.long_until = now.saturating_add(span);
            self.phase = Phase::Long;
            let within = Duration::from_nanos(self.rng.below(span.as_nanos()));
            self.wake(within, acts);
        } else if roll < script.calls + script.longs + script.waits {
            if self.events > 0 {
                self.events -= 1;
                self.write(now, Said::Fact { text: b"took".to_vec() }, acts);
                self.next(acts);
            } else {
                self.write(now, Said::Waiting, acts);
                self.phase = Phase::Waiting;
                let idle = script.idle.draw(&mut self.rng);
                self.wake(idle, acts);
            }
        } else {
            let text = self.bytes(self.sizes.fact);
            self.write(now, Said::Fact { text }, acts);
            self.next(acts);
        }
    }

    fn meet_fate(&mut self, now: Time, acts: &mut Vec<Act>) {
        match self.fate {
            Fate::Ended => {
                let outcome = self.bytes(self.sizes.outcome);
                self.finish(now, Said::Ended { outcome }, acts);
            }
            Fate::Deaf => {
                // It stops reading its channel, and ends a few steps later.
                self.deaf = true;
                acts.push(Act::Deaf);
                self.fate = Fate::Ended;
                self.steps = 3;
                self.next(acts);
            }
            Fate::Parked => {
                let snapshot = self.snapshot();
                self.finish(now, Said::Parked { snapshot }, acts);
            }
            Fate::Failed => {
                let why = [Why::Model, Why::Budget, Why::Policy, Why::Stale]
                    [usize::try_from(self.rng.below(4)).expect("fits")];
                self.finish(now, Said::Failed { why }, acts);
            }
            Fate::Crash => {
                self.phase = Phase::Done;
                acts.push(Act::Exit { after: Duration::ZERO });
            }
            Fate::Hang => self.phase = Phase::Hung,
            Fate::Overrun => {
                self.phase = Phase::Overrunning;
                self.next(acts);
            }
            Fate::Garbage => {
                self.write(now, Said::Garbage, acts);
                self.phase = Phase::Hung;
            }
            Fate::Duplicate => {
                // The same name twice, back to back: the second comes up
                // before the first can have been answered.
                let name = self.call(now, acts);
                let body = self.bytes(self.sizes.call);
                self.write(now, Said::Call { name, push: false, body }, acts);
                *self.awaiting.entry(name).or_default() += 1;
                self.fate = Fate::Ended;
                self.next(acts);
            }
            Fate::Trailing => {
                let outcome = self.bytes(self.sizes.outcome);
                self.finish(now, Said::Ended { outcome }, acts);
                self.write(now, Said::Fact { text: b"and one more thing".to_vec() }, acts);
            }
            Fate::Oversized => {
                let text = vec![b'o'; usize::try_from(self.sizes.fact + 1).expect("fits")];
                self.write(now, Said::Fact { text }, acts);
                self.phase = Phase::Hung;
            }
        }
    }

    /// It writes its finish, and exits after a while.
    fn finish(&mut self, now: Time, said: Said, acts: &mut Vec<Act>) {
        self.write(now, said, acts);
        self.finished = true;
        self.phase = Phase::Done;
        self.serial += 1;
        let span = if self.slow_exit { self.script.slow_exit } else { self.script.exit };
        let after = span.draw(&mut self.rng);
        acts.push(Act::Exit { after });
    }

    /// A host call with a name of its own; its name.
    fn call(&mut self, now: Time, acts: &mut Vec<Act>) -> u64 {
        self.names += 1;
        let name = self.names;
        let push = self.rng.chance(self.script.pushes);
        let body = self.bytes(self.sizes.call);
        self.write(now, Said::Call { name, push, body }, acts);
        *self.awaiting.entry(name).or_default() += 1;
        name
    }

    fn snapshot(&mut self) -> Option<Vec<u8>> {
        if self.rng.chance(500) { Some(self.bytes(self.sizes.snapshot)) } else { None }
    }

    /// Up to `most` bytes, at least one.
    fn bytes(&mut self, most: u64) -> Vec<u8> {
        let len = self.rng.between(1, most.max(1));
        vec![b'b'; usize::try_from(len).expect("fits")]
    }

    fn write(&mut self, now: Time, said: Said, acts: &mut Vec<Act>) {
        self.last_write = now;
        acts.push(Act::Write(said));
    }

    /// The next step, after a while.
    fn next(&mut self, acts: &mut Vec<Act>) {
        let step = self.script.step.draw(&mut self.rng);
        self.wake(step, acts);
    }

    fn wake(&mut self, after: Duration, acts: &mut Vec<Act>) {
        self.serial += 1;
        acts.push(Act::Wake { after, serial: self.serial });
    }
}
