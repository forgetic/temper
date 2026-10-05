//! The agent child domain's client, scripted: it stands in for the parent and,
//! through it, for the host, and speaks the agent child domain's vocabulary as
//! the top level will once it translates the host's. Played from a seed:
//!
//! - It spawns agents now and then, some beyond the limits, some resuming
//!   from a snapshot.
//! - Once an agent has started, it delivers inbound events at random moments,
//!   some in a burst, some larger than the limits allow, each beginning with
//!   its place in the order sent, and stops some agents at a random moment.
//! - It answers each host call once, after a while (sometimes slowly, past
//!   the no-progress deadline): a push with how it went, a relayed call with
//!   an answer, sometimes one too large to go down. A relayed call the run
//!   withdraws it answers at once, as withdrawn; a push, when it settles.
//! - Once told how an agent's run finishes or how the agent failed, it
//!   mostly stops it, as the host does when a run leaves live.
//! - What it sends after an agent has gone is sent all the same, as a stale
//!   handle is, and must change nothing.
//!
//! It checks what it hears as it goes: an agent is named before anything
//! else is said of it, hears at most one finish or fault, and has gone once,
//! last; and a spawn beyond the limits is refused for it, once.

use std::collections::BTreeMap;

use skein_lib::{Duration, Rng, Token};
use temper_worker_domain_agent::channel::{Ask, Push, Reply};
use temper_worker_domain_agent::{Bounce, End, Event, Invalid, Limits, Request, Spawn};
use temper_world::Span;

/// How the client behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// How many agents it spawns, and the time between spawns; the chance,
    /// per mille, that a spawn is beyond the limits, and that it resumes
    /// from a snapshot.
    pub spawns: u32,
    pub spacing: Span,
    pub invalid: u32,
    pub snapshots: u32,
    /// The most inbound events it delivers to an agent, the time between
    /// them, and the chance, per mille, that one is beyond the limits.
    pub events: u32,
    pub event_gap: Span,
    pub large: u32,
    /// The chance, per mille, that an event follows the one before at once.
    pub burst: u32,
    /// How long it takes to answer a call; the chance, per mille, that it
    /// takes `slow_answer` instead; and the chance that a relayed answer is
    /// beyond the limits.
    pub answer: Span,
    pub slow: u32,
    pub slow_answer: Span,
    pub oversized: u32,
    /// The chance, per mille, that it stops an agent at a moment of its own,
    /// and when; and that it stops one once told how it finishes or failed.
    pub stops: u32,
    pub stop_after: Span,
    pub stop_on_end: u32,
}

/// What the client does next.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "fixed diagnostic tails keep boundary records bounded without allocation"
)]
pub enum Out {
    /// An event for the domain, after a hop.
    Domain(Event),
    /// A plan of its own, `after` from now.
    Later { after: Duration, plan: Plan },
}

/// What the client plans to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Plan {
    Spawn,
    Deliver {
        client: Token,
        place: u64,
    },
    Stop {
        client: Token,
    },
    /// Answer the call `call`, the client's `serial`th: a later call that
    /// reuses the name is another.
    Answer {
        client: Token,
        call: Token,
        serial: u64,
        push: bool,
    },
}

/// What the client counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub spawns: u32,
    pub invalid: u32,
    pub snapshots: u32,
    pub started: u32,
    pub events: u32,
    pub large: u32,
    pub calls: u32,
    pub slow: u32,
    pub oversized: u32,
    pub waiting: u32,
    pub withdrawn: u32,
    pub told: u32,
    pub stops: u32,
    /// Deliveries, answers and stops sent to an agent that had gone.
    pub stale: u32,
}

#[derive(Debug)]
struct Spawned {
    /// Whether the spawn was beyond the limits.
    invalid: bool,
    agent: Option<Token>,
    /// How it was told the run finishes or the agent failed, if it was.
    told: bool,
    gone: bool,
    /// Its calls not yet answered, each with its serial and whether it is a
    /// push.
    calls: BTreeMap<Token, (u64, bool)>,
}

pub struct Client {
    rng: Rng,
    script: Script,
    limits: Limits,
    spawned: BTreeMap<Token, Spawned>,
    tally: Tally,
}

impl Client {
    #[must_use]
    pub fn new(script: Script, limits: Limits, seed: u64) -> Client {
        Client { rng: Rng::new(seed), script, limits, spawned: BTreeMap::new(), tally: Tally::default() }
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// The client's first plan.
    pub fn begin(&mut self) -> Vec<Out> {
        if self.script.spawns == 0 {
            return Vec::new();
        }
        vec![Out::Later { after: self.script.spacing.draw(&mut self.rng), plan: Plan::Spawn }]
    }

    /// Carries out `plan`.
    pub fn plan(&mut self, plan: Plan) -> Vec<Out> {
        match plan {
            Plan::Spawn => self.spawn(),
            Plan::Deliver { client, place } => {
                let agent = self.agent(client);
                let large = self.rng.chance(self.script.large);
                let len =
                    if large { self.limits.event_bytes + 1 } else { self.rng.between(8, self.limits.event_bytes) };
                let mut event = place.to_be_bytes().to_vec();
                event.resize(usize::try_from(len).expect("fits"), b'v');
                self.tally.events += 1;
                if large {
                    self.tally.large += 1;
                }
                vec![Out::Domain(Event::Deliver {
                    name: Token::new(place + 1),
                    agent,
                    event: event.into_boxed_slice(),
                })]
            }
            Plan::Stop { client } => {
                let agent = self.agent(client);
                self.tally.stops += 1;
                vec![Out::Domain(Event::Stop { agent })]
            }
            Plan::Answer { client, call, serial, push } => {
                let spawned = self.spawned.get_mut(&client).expect("a call of a spawned agent");
                // A relay the run withdrew was answered as it did.
                if spawned.calls.get(&call) != Some(&(serial, push)) {
                    return Vec::new();
                }
                spawned.calls.remove(&call);
                let agent = self.agent(client);
                let reply = if push {
                    let outcomes = [
                        Push::Done,
                        Push::Moved,
                        Push::Failed {
                            failure: temper_worker_domain_agent::PushFailure::new(
                                temper_worker_domain_agent::PushReason::Unknown,
                            ),
                        },
                        Push::Nothing,
                    ];
                    Reply::Pushed(outcomes[usize::try_from(self.rng.below(4)).expect("fits")].clone())
                } else if self.rng.chance(self.script.oversized) {
                    self.tally.oversized += 1;
                    Reply::Relayed {
                        answer: vec![b'a'; usize::try_from(self.limits.answer_bytes + 1).expect("fits")]
                            .into_boxed_slice(),
                    }
                } else if self.rng.chance(100) {
                    Reply::Unavailable
                } else {
                    let len = self.rng.between(1, self.limits.answer_bytes);
                    Reply::Relayed { answer: vec![b'a'; usize::try_from(len).expect("fits")].into_boxed_slice() }
                };
                vec![Out::Domain(Event::Answer { agent, call, reply })]
            }
        }
    }

    fn spawn(&mut self) -> Vec<Out> {
        self.tally.spawns += 1;
        let client = Token::new(u64::from(self.tally.spawns));
        let invalid = self.rng.chance(self.script.invalid);
        let charter_len =
            if invalid { self.limits.charter_bytes + 1 } else { self.rng.between(1, self.limits.charter_bytes) };
        let snapshot = if self.rng.chance(self.script.snapshots) {
            self.tally.snapshots += 1;
            let len = self.rng.between(1, self.limits.snapshot_bytes);
            Some(vec![b's'; usize::try_from(len).expect("fits")].into_boxed_slice())
        } else {
            None
        };
        if invalid {
            self.tally.invalid += 1;
        }
        let charter = vec![b'c'; usize::try_from(charter_len).expect("fits")].into_boxed_slice();
        let spawn = Spawn {
            repositories: Box::new([]),
            grants: Box::new([]),
            workspace: Token::new(1000 + client.raw()),
            charter,
            snapshot,
        };
        let spawned = Spawned { invalid, agent: None, told: false, gone: false, calls: BTreeMap::new() };
        self.spawned.insert(client, spawned);
        let mut outs = vec![Out::Domain(Event::Spawn { client, spawn })];
        if self.tally.spawns < self.script.spawns {
            outs.push(Out::Later { after: self.script.spacing.draw(&mut self.rng), plan: Plan::Spawn });
        }
        outs
    }

    /// Takes a record of the domain's for the client.
    #[expect(clippy::too_many_lines, reason = "the frozen version-one boundary dispatch remains exhaustive")]
    pub fn take(&mut self, request: Request) -> Vec<Out> {
        let mut outs = Vec::new();
        match request {
            temper_worker_domain_agent::Request::Turn { .. }
            | temper_worker_domain_agent::Request::FinishedV2 { .. } => unreachable!("this script runs version one"),

            Request::Started { client, agent } => {
                let spawned = self.live(client);
                assert!(spawned.agent.is_none(), "an agent is named once");
                assert!(!spawned.invalid, "a spawn beyond the limits never starts");
                spawned.agent = Some(agent);
                self.tally.started += 1;
                let events = self.rng.below(u64::from(self.script.events) + 1);
                let mut after = Duration::ZERO;
                for place in 0..events {
                    if !self.rng.chance(self.script.burst) {
                        after = after.saturating_add(self.script.event_gap.draw(&mut self.rng));
                    }
                    outs.push(Out::Later { after, plan: Plan::Deliver { client, place } });
                }
                if self.rng.chance(self.script.stops) {
                    let after = self.script.stop_after.draw(&mut self.rng);
                    outs.push(Out::Later { after, plan: Plan::Stop { client } });
                }
            }
            Request::Called { client, call, ask } => {
                let push = match ask {
                    temper_worker_domain_agent::channel::Ask::PushV2 { .. } => {
                        unreachable!("this script runs version one")
                    }

                    Ask::Push { .. } => true,
                    Ask::Relay { .. } => false,
                };
                let slow = self.rng.chance(self.script.slow);
                self.tally.calls += 1;
                let serial = u64::from(self.tally.calls);
                let spawned = self.started(client);
                let fresh = spawned.calls.insert(call, (serial, push)).is_none();
                assert!(fresh, "a call's name is not in flight twice");
                let after = if slow {
                    self.tally.slow += 1;
                    self.script.slow_answer.draw(&mut self.rng)
                } else {
                    self.script.answer.draw(&mut self.rng)
                };
                outs.push(Out::Later { after, plan: Plan::Answer { client, call, serial, push } });
            }
            Request::Withdrawn { client, call } => {
                self.tally.withdrawn += 1;
                let spawned = self.started(client);
                let (_, push) = *spawned.calls.get(&call).expect("a call withdrawn is one the client has to answer");
                // A relay is answered at once; a push when it has settled, as
                // planned.
                if !push {
                    spawned.calls.remove(&call);
                    let agent = spawned.agent.expect("started");
                    outs.push(Out::Domain(Event::Answer { agent, call, reply: Reply::Withdrawn }));
                }
            }
            Request::Waiting { client } => {
                self.started(client);
                self.tally.waiting += 1;
            }
            Request::Rejected { client, .. }
            | Request::Exhausted { client, .. }
            | Request::Told { client, fact: _ } => {
                self.started(client);
            }
            Request::Finished { client, .. } | Request::Faulted { client, .. } => outs.extend(self.told(client)),
            Request::Bounced { name: _, client, bounce } => {
                self.started(client);
                match bounce {
                    Bounce::TooLarge | Bounce::Full | Bounce::Ending => {}
                }
            }
            Request::Gone { client, end, detail } => {
                let len = u64::try_from(detail.len()).expect("fits");
                assert!(len <= u64::from(self.limits.detail_bytes), "detail is cut to its limit");
                let spawned = self.live(client);
                spawned.gone = true;
                // A spawn beyond the limits is refused as invalid, or as busy
                // when there is no room to look at it.
                match end {
                    temper_worker_domain_agent::End::Invalid(temper_worker_domain_agent::Invalid::Transcript) => {
                        unreachable!("this script runs version one")
                    }

                    End::Stopped => {
                        assert!(spawned.agent.is_some(), "an agent that ran was named");
                        assert!(!spawned.invalid, "a spawn beyond the limits never runs");
                    }
                    End::Invalid(Invalid::Charter | Invalid::Snapshot | Invalid::Repositories | Invalid::Grants) => {
                        assert!(spawned.invalid, "a spawn is refused as invalid only if it is beyond the limits");
                    }
                    End::Unspawned => {
                        assert!(spawned.agent.is_none(), "it never started");
                        assert!(!spawned.invalid, "a spawn beyond the limits is never spawned");
                    }
                    End::Busy => assert!(spawned.agent.is_none(), "it never started"),
                }
            }
            Request::Spawn { .. }
            | Request::Send { .. }
            | Request::Read { .. }
            | Request::Signal { .. }
            | Request::Wait { .. }
            | Request::Reap { .. } => unreachable!("io's requests go to the tree"),
        }
        outs
    }

    /// The client is told how the run finishes or how the agent failed:
    /// mostly, it stops the agent.
    fn told(&mut self, client: Token) -> Vec<Out> {
        let stop = self.rng.chance(self.script.stop_on_end);
        self.tally.told += 1;
        let spawned = self.started(client);
        assert!(!spawned.told, "a client hears at most one finish or fault");
        spawned.told = true;
        let agent = spawned.agent.expect("started");
        if stop {
            self.tally.stops += 1;
            return vec![Out::Domain(Event::Stop { agent })];
        }
        Vec::new()
    }

    /// The agent's name, for a plan: counted as stale if it has gone.
    fn agent(&mut self, client: Token) -> Token {
        let spawned = self.spawned.get(&client).expect("a plan for a spawned agent");
        if spawned.gone {
            self.tally.stale += 1;
        }
        spawned.agent.expect("plans follow the start")
    }

    fn live(&mut self, client: Token) -> &mut Spawned {
        let spawned = self.spawned.get_mut(&client).expect("records are of spawned agents");
        assert!(!spawned.gone, "nothing is said of an agent after it has gone");
        spawned
    }

    fn started(&mut self, client: Token) -> &mut Spawned {
        let spawned = self.live(client);
        assert!(spawned.agent.is_some(), "an agent is named before anything else is said of it");
        spawned
    }

    /// Checks that every agent has gone and every call was answered.
    pub fn assert_settled(&self) {
        assert_eq!(u32::try_from(self.spawned.len()).expect("fits"), self.script.spawns, "every spawn was made");
        for (client, spawned) in &self.spawned {
            assert!(spawned.gone, "agent {client:?} has gone");
            assert!(spawned.calls.is_empty(), "agent {client:?}'s calls were answered");
        }
    }
}
