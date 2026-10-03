//! A scripted agent process: the agent's side of the channel, in its own
//! terms, played from a seed. It shares no type with the agent child domain:
//! the world translates what it writes and reads ([`crate::translate`]), as a
//! protocol layer would encode and decode them.
//!
//! Once it hears its start, a run takes a number of steps, each after a
//! while: a fact, a host call (some of which it waits for, withdrawing one
//! whose answer is later than its own deadline), a long operation (silent for
//! up to its span, then done), or a wait for an inbound event (parking if
//! none comes within its idle time). Then it meets its fate: it ends, parks
//! or fails as it says, and exits after a while (sometimes slowly, past the
//! grace); or it misbehaves: it crashes (exits without a word), hangs
//! (silence), overruns (progress until its wall time), writes garbage, reuses
//! a call's name or withdraws a call twice before its answer, says more after
//! its finish, says more than the limits allow, stops reading its channel, or
//! closes its output and exits. A cancel winds it down to a cancelled failure,
//! unless it ignores cancels or closes its output at one; a terminate makes it
//! exit after a while, unless it ignores that too, and a kill always does.
//!
//! A world may give a run a plot ([`Plot`]) before it starts: the beats its
//! story calls for (calls it waits for, pushes among them, and waits for an
//! inbound event that it gives up on after a while), which it takes in order
//! before its own steps, and the ending its story calls for (ended, or parked
//! with a snapshot), which it meets when its fate would end or park it. A
//! plotted run gives up a wait of its own after its idle time, and goes on,
//! rather than parking. Its other fates, misbehaviour among them, it meets as
//! drawn. A run with no plot draws exactly as it would without plots.
//!
//! It checks what it hears as it goes: the start first and once, inbound
//! events once each, in the order sent, and an answer only to a call it made.

use std::collections::{BTreeMap, VecDeque};

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
    /// It withdraws its call `name`, past its own deadline for it.
    Withdraw {
        name: u64,
    },
    Fact {
        text: Vec<u8>,
    },
    /// It started an operation that may run for `span`.
    Long {
        span: Duration,
    },
    LongDone,
    /// It waits for its next inbound event, having read `heard`.
    Waiting {
        heard: u64,
    },
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
    Withdrawn,
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

/// The limits the agent writes within, or just past when it misbehaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sizes {
    pub call: u64,
    pub fact: u64,
    pub outcome: u64,
    pub snapshot: u64,
    pub long: Duration,
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
    /// for its answer; and its own deadline for a call it waits for, past
    /// which it withdraws it.
    pub pushes: u32,
    pub blocking: u32,
    pub call_deadline: Span,
    /// The span a long operation announces; it is done within it.
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
    /// The chance, per mille, that it ignores a cancel, and that it closes
    /// its output at one and exits; and how long it takes to wind down.
    pub deaf_to_cancel: u32,
    pub mute: u32,
    pub wind: Span,
    /// The chance, per mille, that it ignores a terminate, and how long it
    /// takes to exit when it does not.
    pub stubborn: u32,
    pub term: Span,
}

/// What a world's story calls for of a run: the beats it takes before its
/// own steps, and how it ends when its fate is to end or park.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plot {
    pub beats: VecDeque<Beat>,
    pub ending: Ending,
}

/// One thing a story calls for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Beat {
    /// A host call it waits for: a push, or a relayed call.
    Call { push: bool },
    /// A wait for an inbound event, given up after `within`.
    Await { within: Duration },
}

/// How a story ends a run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ending {
    Ended,
    Parked { snapshot: Option<Vec<u8>> },
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
    pub mute: u32,
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
    Mute,
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
            (self.mute, Fate::Mute),
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
    /// It closes its end of the channel up.
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// It has not heard its start.
    Unstarted,
    /// It takes its steps.
    Working,
    /// It waits for the answer to its call `name`, which it has withdrawn
    /// once its deadline passed.
    Blocked { name: u64, withdrawn: bool },
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
    /// When its last long operation's span runs out, or when it said it was
    /// done.
    pub long_until: Time,
    /// It waits for a call's answer, or for an inbound event.
    pub blocked: bool,
    pub waiting: bool,
    /// It stopped reading its channel.
    pub deaf: bool,
    /// It wrote its finish.
    pub finished: bool,
    /// When it hung, if it did with no call waiting for an answer: its
    /// silence must end with the watchdog.
    pub hung: Option<Time>,
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
    /// Inbound events heard and not yet taken, and read in all.
    events: u32,
    heard: u64,
    last_event: Option<u64>,
    obeys_cancel: bool,
    mute: bool,
    obeys_terminate: bool,
    slow_exit: bool,
    last_write: Time,
    long_until: Time,
    deaf: bool,
    finished: bool,
    hung: Option<Time>,
    /// What its world's story calls for, if it gave one.
    plot: Option<Plot>,
}

impl Agent {
    #[must_use]
    pub fn new(script: Script, sizes: Sizes, seed: u64) -> Agent {
        let mut rng = Rng::new(seed);
        let fate = script.fates.draw(&mut rng);
        let steps = u32::try_from(rng.below(u64::from(script.steps) + 1)).expect("fits");
        let obeys_cancel = !rng.chance(script.deaf_to_cancel);
        let mute = rng.chance(script.mute);
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
            heard: 0,
            last_event: None,
            obeys_cancel,
            mute,
            obeys_terminate,
            slow_exit,
            last_write: Time::ZERO,
            long_until: Time::ZERO,
            deaf: false,
            finished: false,
            hung: None,
            plot: None,
        }
    }

    /// Its world's story calls for `plot`: given before it starts.
    pub fn plot(&mut self, plot: Plot) {
        assert_eq!(self.phase, Phase::Unstarted, "a plot is given before the start");
        self.plot = Some(plot);
    }

    #[must_use]
    pub fn view(&self) -> View {
        let blocked = match self.phase {
            Phase::Blocked { .. } => true,
            Phase::Unstarted
            | Phase::Working
            | Phase::Long
            | Phase::Waiting
            | Phase::Hung
            | Phase::Overrunning
            | Phase::Winding
            | Phase::Done => false,
        };
        View {
            fate: self.fate,
            last_write: self.last_write,
            long_until: self.long_until,
            blocked,
            waiting: self.phase == Phase::Waiting,
            deaf: self.deaf,
            finished: self.finished,
            hung: self.hung,
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
                self.heard += 1;
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
                let unblocked = match self.phase {
                    Phase::Blocked { name: blocked, .. } => blocked == name,
                    Phase::Unstarted
                    | Phase::Working
                    | Phase::Long
                    | Phase::Waiting
                    | Phase::Hung
                    | Phase::Overrunning
                    | Phase::Winding
                    | Phase::Done => false,
                };
                if unblocked {
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
                    if self.mute {
                        // It closes its output, and exits after a while.
                        self.phase = Phase::Done;
                        self.serial += 1;
                        acts.push(Act::Close);
                        acts.push(Act::Exit { after: self.script.wind.draw(&mut self.rng) });
                    } else if self.obeys_cancel {
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
            // Its own deadline for the call passed: it withdraws it, and
            // waits for the answer all the same.
            Phase::Blocked { name, withdrawn: false } => {
                self.write(now, Said::Withdraw { name }, &mut acts);
                self.phase = Phase::Blocked { name, withdrawn: true };
            }
            Phase::Long => {
                self.write(now, Said::LongDone, &mut acts);
                self.long_until = now;
                self.phase = Phase::Working;
                self.next(&mut acts);
            }
            Phase::Waiting if self.plot.is_some() => {
                // A plotted run gives up its wait, and goes on.
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
            Phase::Unstarted | Phase::Blocked { withdrawn: true, .. } | Phase::Hung | Phase::Done => {
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
        if let Some(beat) = self.plot.as_mut().and_then(|plot| plot.beats.pop_front()) {
            self.beat(now, beat, acts);
            return;
        }
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
                self.phase = Phase::Blocked { name, withdrawn: false };
                let deadline = script.call_deadline.draw(&mut self.rng);
                self.wake(deadline, acts);
            } else {
                self.next(acts);
            }
        } else if roll < script.calls + script.longs {
            let span = script.long.draw(&mut self.rng).min(self.sizes.long);
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
                self.write(now, Said::Waiting { heard: self.heard }, acts);
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

    /// A beat of its plot.
    fn beat(&mut self, now: Time, beat: Beat, acts: &mut Vec<Act>) {
        match beat {
            Beat::Call { push } => {
                self.names += 1;
                let name = self.names;
                let body = self.bytes(self.sizes.call);
                self.write(now, Said::Call { name, push, body }, acts);
                *self.awaiting.entry(name).or_default() += 1;
                self.phase = Phase::Blocked { name, withdrawn: false };
                let deadline = self.script.call_deadline.draw(&mut self.rng);
                self.wake(deadline, acts);
            }
            Beat::Await { within } => {
                if self.events > 0 {
                    self.events -= 1;
                    self.write(now, Said::Fact { text: b"took".to_vec() }, acts);
                    self.next(acts);
                } else {
                    self.write(now, Said::Waiting { heard: self.heard }, acts);
                    self.phase = Phase::Waiting;
                    self.wake(within, acts);
                }
            }
        }
    }

    fn meet_fate(&mut self, now: Time, acts: &mut Vec<Act>) {
        if let Some(plot) = &self.plot
            && (self.fate == Fate::Ended || self.fate == Fate::Parked)
        {
            let said = match plot.ending.clone() {
                Ending::Ended => Said::Ended { outcome: vec![b'b'] },
                Ending::Parked { snapshot } => Said::Parked { snapshot },
            };
            self.finish(now, said, acts);
            return;
        }
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
            Fate::Mute => {
                self.phase = Phase::Done;
                acts.push(Act::Close);
                acts.push(Act::Exit { after: self.script.exit.draw(&mut self.rng) });
            }
            Fate::Hang => {
                self.phase = Phase::Hung;
                if self.awaiting.is_empty() {
                    self.hung = Some(now);
                }
            }
            Fate::Overrun => {
                self.phase = Phase::Overrunning;
                self.next(acts);
            }
            Fate::Garbage => {
                self.write(now, Said::Garbage, acts);
                self.phase = Phase::Hung;
            }
            Fate::Duplicate => {
                // The same name twice back to back, or a withdraw twice: the
                // second comes up before the first can have been answered.
                let name = self.call(now, acts);
                if self.rng.chance(500) {
                    let body = self.bytes(self.sizes.call);
                    self.write(now, Said::Call { name, push: false, body }, acts);
                    *self.awaiting.entry(name).or_default() += 1;
                } else {
                    self.write(now, Said::Withdraw { name }, acts);
                    self.write(now, Said::Withdraw { name }, acts);
                }
                self.fate = Fate::Ended;
                self.next(acts);
            }
            Fate::Trailing => {
                let said = match self.plot.as_ref().map(|plot| plot.ending.clone()) {
                    Some(Ending::Parked { snapshot }) => Said::Parked { snapshot },
                    Some(Ending::Ended) => Said::Ended { outcome: vec![b'b'] },
                    None => Said::Ended { outcome: self.bytes(self.sizes.outcome) },
                };
                self.finish(now, said, acts);
                self.write(now, Said::Fact { text: b"and one more thing".to_vec() }, acts);
            }
            Fate::Oversized => {
                if self.rng.chance(500) {
                    let text = vec![b'o'; usize::try_from(self.sizes.fact + 1).expect("fits")];
                    self.write(now, Said::Fact { text }, acts);
                } else {
                    let span = self.sizes.long.saturating_add(Duration::from_secs(1));
                    self.write(now, Said::Long { span }, acts);
                }
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
