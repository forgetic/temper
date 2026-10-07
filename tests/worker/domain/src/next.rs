//! Version-two runtime world. The real root, host, checkout and agent child
//! domains meet a scripted engine and process, with deterministic git
//! terminals. The v1 system world's scripts and random draws remain frozen.

use std::collections::VecDeque;
use std::fmt::{self, Write};

use skein_lib::{Duration, Env, Queue, Rng, Time, Token, Wall};
use temper_worker_domain::agent::channel::{self, Ask, Down, FinishV2, Up};
use temper_worker_domain::checkout::git::{Commit, Done, Op, Want};
use temper_worker_domain::wire::{self as host, Access, Assignment, AssignmentV2, Repository, Start, Workspace};
use temper_worker_domain::{Domain, Event, Limits, Request, fire, max_out, step, worst_case};
use temper_world::heap::Meter;

const RUN: Token = Token::new(31);
const ATTEMPT: Token = Token::new(932);
const PROCESS: Token = Token::new(777);
const HEAD: [u8; 32] = [11; 32];
const BASE: [u8; 32] = [23; 32];
const LANDED: [u8; 32] = [47; 32];
const TRANSCRIPT: &[u8] = b"turn-0;call-51:committed;call-tail";
const PATH: &[u8] = b"src/merge.rs";
const TURN_BYTES: u64 = 24;
const CAPACITY: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub seed: u64,
    pub turns: u32,
    pub byte_slots: u32,
    pub merging: bool,
    pub abandon: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    pub turns: u32,
    pub copies: u32,
    pub busy: u32,
    pub reconnects: u32,
    pub commits: u32,
    pub saves: u32,
    pub peak_retained: u32,
    pub abandoned: u64,
    pub peak_heap: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Observed {
    number: u32,
    spent: u64,
    length: usize,
    digest: u64,
}

/// A formatting sink with no allocation; full boundary records and facts
/// enter the trace, including byte bodies. No trace block enters the meter.
struct Digest(u64);
impl Write for Digest {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
        }
        Ok(())
    }
}
fn digest(value: &impl fmt::Debug) -> u64 {
    let mut writer = Digest(14_695_981_039_346_656_037);
    write!(writer, "{value:?}").expect("the digest accepts every byte");
    writer.0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bodies {
    Varied,
    Full,
}

pub struct World {
    settings: Settings,
    env: Env<Limits>,
    domain: Domain,
    out: Queue<Request>,
    owed: VecDeque<Event>,
    trace: Vec<u64>,
    boundaries: Vec<u64>,
    observed: Vec<Observed>,
    meter: Meter,
    bound: u64,
    rng: Rng,
    agent: Option<Token>,
    reading: bool,
    dialling: bool,
    unresolved: bool,
    bodies: Bodies,
    answer: Option<(u32, u64, u64)>,
    stats: Stats,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self::with_facts(settings, true)
    }

    /// Runs the same records with facts and forwarded run facts dropped.
    #[must_use]
    pub fn without_facts(settings: Settings) -> Self {
        Self::with_facts(settings, false)
    }

    fn with_facts(settings: Settings, keep: bool) -> Self {
        assert!(settings.turns > 0 && settings.byte_slots > 0);
        let mut limits = crate::LIMITS;
        limits.host.slots = 1;
        limits.checkout.repositories = 1;
        limits.host.transcript_bytes = u64::try_from(TRANSCRIPT.len()).expect("bounded");
        limits.host.turn_bytes = TURN_BYTES;
        limits.checkout.conflicts = 1;
        limits.checkout.path_bytes = u32::try_from(PATH.len()).expect("bounded");
        limits.agent.transcript_bytes = limits.host.transcript_bytes;
        limits.agent.turn_bytes = TURN_BYTES;
        limits.agent.conflicts = limits.checkout.conflicts;
        limits.agent.path_bytes = limits.checkout.path_bytes;
        limits.host.turns = settings.turns;
        limits.host.turn_queue_bytes = u64::from(settings.byte_slots) * TURN_BYTES;
        limits.turn_backoff = Duration::from_millis(10);
        if !keep {
            limits.host.facts = 0;
            limits.checkout.facts = 0;
            limits.agent.facts = 0;
            limits.host.told = 0;
        }
        let bound = worst_case(&limits).expect("world limits fit");
        let out = Queue::with_capacity(max_out(&limits));
        let owed = VecDeque::with_capacity(CAPACITY);
        let trace = Vec::with_capacity(CAPACITY);
        let boundaries = Vec::with_capacity(CAPACITY);
        let observed = Vec::with_capacity(64);
        let meter = Meter::new();
        let domain = Domain::new(&limits, settings.seed);
        World {
            settings,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            domain,
            out,
            owed,
            trace,
            boundaries,
            observed,
            meter,
            bound,
            rng: Rng::new(settings.seed),
            agent: None,
            reading: false,
            dialling: false,
            unresolved: settings.merging,
            bodies: Bodies::Varied,
            answer: None,
            stats: Stats::default(),
        }
    }

    /// Saturates every retained body, transcript and conflict path in the
    /// counted-memory scenarios; the random worlds keep varied body sizes.
    pub fn fill_turns(&mut self) {
        self.bodies = Bodies::Full;
    }

    #[must_use]
    pub const fn stats(&self) -> Stats {
        self.stats
    }

    #[must_use]
    pub fn trace(&self) -> &[u64] {
        &self.trace
    }

    #[must_use]
    pub fn boundaries(&self) -> &[u64] {
        &self.boundaries
    }

    fn remember(&mut self, record: &impl fmt::Debug) {
        assert!(self.boundaries.len() < CAPACITY, "world boundary trace is bounded");
        self.boundaries.push(digest(record));
        self.remember_fact(record);
    }

    fn remember_fact(&mut self, record: &impl fmt::Debug) {
        assert!(self.trace.len() < CAPACITY, "world trace is bounded");
        self.trace.push(digest(record));
    }

    fn send(&mut self, event: Event) {
        self.remember(&event);
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain();
        self.settle();
    }

    fn fire(&mut self) {
        assert!(self.domain.is_due(self.env.now));
        let now = self.env.now;
        self.remember(&now);
        self.meter.start();
        fire(&mut self.domain, &self.env, &mut self.out);
        self.drain();
        self.settle();
    }

    fn drain(&mut self) {
        let measured = self.meter.end();
        for _ in 0..max_out(&self.env.limits) {
            let Some(request) = self.out.pop() else { break };
            self.remember(&request);
            self.take(request);
        }
        assert!(self.out.is_empty(), "max_out covers a replay burst");
        for _ in 0..1024 {
            let Some(fact) = self.domain.pop_fact() else { break };
            self.remember_fact(&fact);
        }
        for _ in 0..1024 {
            let Some(told) = self.domain.pop_told() else { break };
            self.remember_fact(&told);
        }
        self.meter.check(measured, self.bound, self.settings);
        self.stats.peak_heap = self.stats.peak_heap.max(self.meter.held());
        self.stats.peak_retained = self.stats.peak_retained.max(self.domain.retained_turns());
        self.domain.reclaim();
    }

    fn settle(&mut self) {
        for _ in 0..128 {
            let Some(event) = self.owed.pop_front() else { return };
            self.remember(&event);
            self.meter.start();
            step(&mut self.domain, &self.env, event, &mut self.out);
            self.drain();
        }
        panic!("the world owes at most 128 immediate terminals");
    }

    #[expect(clippy::too_many_lines, reason = "one exhaustive boundary referee")]
    fn take(&mut self, request: Request) {
        match request {
            Request::Dial => {
                assert!(!self.dialling);
                self.dialling = true;
            }
            Request::HelloV2 { hello, graces, push_deadline } => {
                assert_eq!(graces, temper_worker_domain::declared_graces(&self.env.limits).expect("checked"));
                assert_eq!(push_deadline, temper_worker_domain::push_deadline(&self.env.limits).expect("checked"));
                assert!(hello.hosting.len() <= 1);
            }
            Request::Turn { run, attempt, turn } => {
                assert_eq!((run, attempt), (RUN, ATTEMPT));
                let received = Observed {
                    number: turn.turn,
                    spent: turn.spent,
                    length: turn.body.len(),
                    digest: digest(&turn.body),
                };
                if let Some(previous) = self.observed.iter().find(|old| old.number == turn.turn) {
                    assert_eq!(previous, &received, "replay preserves the full body and spend");
                } else {
                    assert_eq!(turn.turn, u32::try_from(self.observed.len()).expect("bounded") + 1, "no turn gap");
                    self.observed.push(received);
                    self.stats.turns += 1;
                }
                self.stats.copies += 1;
            }
            Request::AnswerV2 { run, attempt, answer } => {
                assert_eq!((run, attempt), (RUN, ATTEMPT));
                assert_eq!(answer.turns, u32::try_from(self.observed.len()).expect("bounded"));
                let ending = match &answer.ending {
                    host::EndingV2::Parked { work } => {
                        if self.settings.merging {
                            assert!(work.saved.is_none());
                        } else {
                            assert!(work.saved.is_some());
                        }
                        digest(&answer)
                    }
                    host::EndingV2::Ended { .. } | host::EndingV2::Failed { .. } | host::EndingV2::Refused(_) => {
                        panic!("script only parks: {answer:?}")
                    }
                };
                let received = (answer.turns, answer.spent, ending);
                if let Some(old) = self.answer {
                    assert_eq!(old, received, "answer replay is identical");
                }
                self.answer = Some(received);
            }
            Request::Io { owner, op, deadline } => {
                assert!(deadline > self.env.now);
                let done = self.git(op);
                self.owed.push_back(Event::Done { owner, done });
            }
            Request::Spawn { owner, workspace: _, deadline } => {
                assert!(deadline > self.env.now);
                assert!(self.agent.is_none());
                self.agent = Some(owner);
                self.owed.push_back(Event::Spawned { owner, process: PROCESS });
            }
            Request::Send { owner, process, message } => {
                assert_eq!(Some(owner), self.agent);
                assert_eq!(process, PROCESS);
                match message {
                    Down::StartV2 { charter, transcript, repositories, grants } => {
                        assert_eq!(&*charter, b"charter");
                        assert_eq!(transcript.as_deref(), Some(TRANSCRIPT));
                        assert_eq!(repositories.len(), 1);
                        assert!(grants.is_empty());
                        if self.settings.merging {
                            assert_eq!(&*repositories[0].conflicts, [Box::<[u8]>::from(PATH)]);
                        } else {
                            assert!(repositories[0].conflicts.is_empty());
                        }
                    }
                    Down::Answer { call, reply } => {
                        assert_eq!(call, Token::new(71));
                        match reply {
                            channel::Reply::Pushed(channel::Push::Conflicted { repository, files }) => {
                                assert_eq!(repository, 0);
                                assert_eq!(&*files, [Box::<[u8]>::from(PATH)]);
                            }
                            channel::Reply::Pushed(channel::Push::Done) => {}
                            channel::Reply::Pushed(
                                channel::Push::Moved | channel::Push::Failed { .. } | channel::Push::Nothing,
                            )
                            | channel::Reply::Relayed { .. }
                            | channel::Reply::Busy
                            | channel::Reply::Unavailable
                            | channel::Reply::Withdrawn
                            | channel::Reply::TooLarge => panic!("unexpected push result: {reply:?}"),
                        }
                    }
                    Down::Start { .. } | Down::Event { .. } | Down::Grant { .. } | Down::Cancel => {
                        panic!("unexpected v1 or unscripted down record")
                    }
                }
                self.owed.push_back(Event::Sent { owner });
            }
            Request::Read { owner, process } => {
                assert_eq!(Some(owner), self.agent);
                assert_eq!(process, PROCESS);
                assert!(!self.reading);
                self.reading = true;
            }
            Request::Wait { owner, process } | Request::Reap { owner, process } => {
                assert_eq!(Some(owner), self.agent);
                assert_eq!(process, PROCESS);
            }
            Request::Hello { .. }
            | Request::Answer { .. }
            | Request::Relay { .. }
            | Request::RelayV2 { .. }
            | Request::Bounced { .. }
            | Request::Rejected { .. }
            | Request::Exhausted { .. }
            | Request::Signal { .. }
            | Request::CancelRelay { .. }
            | Request::CancelIo { .. } => panic!("unscripted output"),
        }
    }

    fn git(&mut self, op: Op) -> Done {
        match op {
            Op::Make { .. } | Op::Clone { .. } | Op::Create { .. } | Op::CheckOut { .. } => Done::Succeeded,
            Op::Fetch { want, .. } => {
                let commit = match want {
                    Want::Branch { branch } => {
                        if branch.as_ref() == b"main" {
                            BASE
                        } else {
                            HEAD
                        }
                    }
                    Want::Commit { commit } => commit.raw(),
                    Want::Default => BASE,
                };
                Done::Fetched { commit: Commit::new(commit) }
            }
            Op::Merge { theirs, .. } => {
                assert_eq!(theirs.raw(), BASE);
                Done::Conflicted { files: Box::new([Box::from(PATH)]) }
            }
            Op::Commit { title, body, merging, .. } => {
                self.stats.commits += 1;
                if title.as_ref() == b"Save unfinished work" {
                    self.stats.saves += 1;
                    assert!(!self.settings.merging);
                } else {
                    assert_eq!(title.as_ref(), b"short title");
                    assert_eq!(body.as_ref(), b"a separate, longer body");
                }
                if self.settings.merging {
                    assert_eq!(merging, Some(Commit::new(BASE)));
                }
                if self.unresolved {
                    Done::Conflicted { files: Box::new([Box::from(PATH)]) }
                } else {
                    Done::Committed { commit: Commit::new(LANDED) }
                }
            }
            Op::Push { branch, expected, .. } => {
                if branch.as_ref() == b"topic" {
                    assert_eq!(expected, Some(Commit::new(HEAD)));
                } else {
                    assert_eq!(branch.as_ref(), b"saved");
                    assert_eq!(expected, None);
                }
                Done::Succeeded
            }
        }
    }

    fn say(&mut self, message: Up) {
        assert!(self.reading, "the agent may speak only after a granted read");
        self.reading = false;
        let owner = self.agent.expect("agent starts first");
        self.send(Event::Received { owner, message });
    }

    fn ack(&mut self, turn: u32) {
        self.send(Event::AcknowledgeTurn { run: RUN, attempt: ATTEMPT, turn });
    }

    fn reconnect(&mut self) {
        self.send(Event::Lost);
        self.env.now = self.domain.next_deadline().expect("redial armed");
        self.fire();
        assert!(self.dialling);
        self.dialling = false;
        self.send(Event::ConnectedV2);
        self.stats.reconnects += 1;
    }

    /// Runs a complete scenario with bounded turns and deterministic losses,
    /// Busy replies, duplicate or wrong ACKs, and a crossed answer ACK.
    #[expect(clippy::too_many_lines, reason = "one bounded runtime scenario with its ordered IO terminals")]
    pub fn run(&mut self) {
        self.fire();
        assert!(self.dialling);
        self.dialling = false;
        self.send(Event::ConnectedV2);
        let repository = Repository {
            tag: 19,
            name: Box::from(&b"app"[..]),
            remote: Box::from(&b"org/app"[..]),
            identity: 0,
            start: if self.settings.merging {
                Start::Merge { branch: Box::from(&b"topic"[..]), base: BASE }
            } else {
                Start::Base { branch: Box::from(&b"main"[..]) }
            },
            access: Access::WritableV2 { push: Box::from(&b"topic"[..]), expected: Some(HEAD) },
        };
        let assignment = AssignmentV2 {
            assignment: Assignment {
                run: RUN,
                attempt: ATTEMPT,
                workspace: Workspace { key: Box::from(&b"stream"[..]), repositories: Box::new([repository]) },
                save: Some(Box::from(&b"saved"[..])),
                charter: Box::from(&b"charter"[..]),
                snapshot: None,
                grants: Box::new([]),
            },
            transcript: Some(Box::from(TRANSCRIPT)),
        };
        self.send(Event::AssignV2 { assignment });
        self.say(Up::Call {
            call: Token::new(71),
            ask: Ask::PushV2 {
                title: Box::from(&b"short title"[..]),
                body: Box::from(&b"a separate, longer body"[..]),
            },
        });
        if self.settings.merging {
            self.unresolved = false;
            self.say(Up::Call {
                call: Token::new(71),
                ask: Ask::PushV2 {
                    title: Box::from(&b"short title"[..]),
                    body: Box::from(&b"a separate, longer body"[..]),
                },
            });
        }
        let count = u32::try_from(self.rng.between(3, 7)).expect("bounded turns");
        let mut committed = 0;
        for number in 1..=count {
            let length =
                usize::try_from(if self.bodies == Bodies::Full { TURN_BYTES } else { self.rng.between(1, TURN_BYTES) })
                    .expect("bounded body");
            self.say(Up::Turn {
                turn: channel::Turn {
                    turn: number,
                    spent: u64::from(number) * 17,
                    read: None,
                    body: vec![u8::try_from(number).expect("bounded"); length].into_boxed_slice(),
                },
            });
            let retained = self.domain.retained_turns();
            self.send(Event::AcknowledgeTurn { run: RUN, attempt: Token::new(999), turn: number });
            assert_eq!(self.domain.retained_turns(), retained, "wrong attempt cannot release credit");
            self.send(Event::TurnBusy { run: RUN, attempt: ATTEMPT, turn: number });
            self.env.now = self.env.now.saturating_add(self.env.limits.turn_backoff);
            self.fire();
            self.stats.busy += 1;
            self.reconnect();
            assert!(retained <= self.settings.turns);
            for _ in 0..self.settings.turns {
                if self.reading {
                    break;
                }
                committed += 1;
                self.ack(committed);
                self.ack(committed);
            }
            assert!(self.reading, "committing enough bytes restores maximum-turn credit");
        }
        if !self.reading {
            committed += 1;
            self.ack(committed);
        }
        self.say(Up::FinishV2 { turns: count, spent: u64::from(count) * 17 + 9, finish: FinishV2::Parked });
        let agent = self.agent.expect("live agent");
        self.send(Event::Exited { owner: agent });
        assert!(self.reading);
        self.reading = false;
        self.send(Event::Hangup { owner: agent });
        self.send(Event::Reaped { owner: agent, detail: Box::new([]) });
        assert!(self.answer.is_some());
        assert_eq!(self.domain.agent().agents(), 0);
        assert_eq!(self.domain.workspaces(), 0);
        self.reconnect();
        self.send(Event::Acknowledged { run: RUN, attempt: ATTEMPT });
        let retained = self.domain.retained_turns();
        assert_eq!(self.domain.held(), u32::from(retained > 0), "answer ACK waits for its retained turns");
        if self.settings.abandon {
            self.send(Event::Shutdown);
            self.send(Event::Lost);
            for _ in 0..16 {
                if self.domain.is_done() {
                    break;
                }
                self.env.now = self.domain.next_deadline().expect("contact grace or redial");
                self.fire();
                if self.dialling {
                    self.dialling = false;
                    self.send(Event::Lost);
                }
            }
            assert!(self.domain.is_done());
            self.stats.abandoned = self.domain.abandoned();
            assert_eq!(self.stats.abandoned, u64::from(retained) + u64::from(retained > 0));
        } else {
            for number in committed + 1..=count {
                self.ack(number);
            }
            assert_eq!(self.domain.held(), 0);
            assert_eq!(self.domain.retained_turns(), 0);
            self.send(Event::Shutdown);
            assert!(self.domain.is_done());
        }
        assert_eq!(self.stats.turns, count);
        assert!(self.stats.copies >= count * 3);
    }
}

#[cfg(test)]
mod referee_tests {
    use super::{ATTEMPT, RUN, Settings, World, host};
    use temper_worker_domain::Request;

    fn turn(body: &[u8]) -> Request {
        Request::Turn {
            run: RUN,
            attempt: ATTEMPT,
            turn: host::Turn { turn: 1, spent: 17, read: None, body: Box::from(body) },
        }
    }

    #[test]
    #[should_panic(expected = "replay preserves the full body and spend")]
    fn referee_rejects_a_duplicate_turn_with_changed_opaque_bytes() {
        let settings = Settings { seed: 0, turns: 1, byte_slots: 1, merging: false, abandon: false };
        let mut world = World::new(settings);
        world.take(turn(b"original"));
        world.take(turn(b"different"));
    }

    #[test]
    #[should_panic(expected = "no turn gap")]
    fn referee_rejects_an_unseen_prefix() {
        let settings = Settings { seed: 0, turns: 1, byte_slots: 1, merging: false, abandon: false };
        let mut world = World::new(settings);
        world.take(Request::Turn {
            run: RUN,
            attempt: ATTEMPT,
            turn: host::Turn { turn: 2, spent: 17, read: None, body: Box::new([]) },
        });
    }
}
