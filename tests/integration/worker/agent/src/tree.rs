//! Process trees, standing in for io and the protocol layer below the agent
//! sub-model: each spawn is a contained tree of a scripted agent process
//! ([`crate::script`]) and the children it started, with a channel over its
//! pipes. It speaks the agent sub-model's io vocabulary, through the world's
//! translations of the channel ([`crate::translate`]), and plays io's
//! contracts:
//!
//! - A spawn takes a while, and may fail. A send reaches the agent through
//!   the pipe after a while, and fails at once if the agent no longer reads
//!   its channel. A read is a demand: it ends with the next message the agent
//!   wrote once it is through the pipe, as malformed if it is garbage, or
//!   with a hangup once every writer (the agent, and the children that hold
//!   its channel open) has gone and all it wrote has been read.
//! - A terminate asks every member to exit: the agent and most children do,
//!   after a while, and some ignore it; a kill ends every member at once.
//! - A wait ends when the agent's process exits; a reap once every member
//!   has gone, after the exit's. What the agent wrote is still read to the
//!   channel's end, and a request on a process that has gone ends at once.
//!
//! Children live a while, or outlive the agent until they are signalled;
//! some hold the agent's channel open as long as they live.

use std::collections::{BTreeMap, VecDeque};

use temper_lib::{Duration, Rng, Time, Token};
use temper_worker_model_agent::{Event, Request, Signal};
use temper_world::Span;

use crate::script::{self, Act, Agent, Heard, Said, Sizes, View};
use crate::translate;

/// How process trees behave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// How long a spawn takes, and the chance, per mille, that it fails.
    pub spawn: Span,
    pub unspawned: u32,
    /// How long bytes take through a pipe, either way.
    pub pipe: Span,
    /// The most children an agent starts; for each, the chance, per mille,
    /// that it outlives the agent until signalled, that it holds the channel
    /// open, and that it ignores a terminate; and how long it lives
    /// otherwise.
    pub children: u32,
    pub lingering: u32,
    pub holding: u32,
    pub stubborn: u32,
    pub child_life: Span,
    /// How long a child takes to exit on a terminate.
    pub term: Span,
    /// The most bytes of error output a process leaves.
    pub detail: u64,
}

/// What the tree does next.
#[derive(Debug)]
pub enum Out {
    /// An event for the model, `after` from now, and then a hop.
    Model { after: Duration, event: Event },
    /// Something of the tree's own falls due `after` from now.
    Due { after: Duration, due: Due },
}

/// What falls due in the tree.
#[derive(Debug)]
pub enum Due {
    /// The agent of `process` wakes, for its wake `serial`.
    Wake { process: u64, serial: u64 },
    /// What the model sent reaches the agent of `process`.
    Heard { process: u64, heard: Heard },
    /// A member of `process` exits.
    Exit { process: u64, member: Member },
    /// What the agent of `process` wrote may be through the pipe now.
    Readable { process: u64 },
}

/// A member of a tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Member {
    /// The agent's process.
    Main,
    Child(usize),
}

/// What the tree counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub spawns: u32,
    pub unspawned: u32,
    pub children: u32,
    /// Children alive after the agent's process exited.
    pub orphans: u32,
    pub terminates: u32,
    pub kills: u32,
    /// Members that ignored a terminate.
    pub ignored: u32,
    pub sends: u32,
    pub unsent: u32,
    pub hangups: u32,
    pub malformed: u32,
}

#[derive(Debug)]
struct Child {
    alive: bool,
    holds: bool,
    stubborn: bool,
}

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "what io knows of a process, each fact on its own")]
struct Proc {
    /// The model's name for it.
    owner: Token,
    agent: Agent,
    /// The agent's process runs.
    main: bool,
    children: Vec<Child>,
    /// What the agent wrote and the model has not read, each with when it is
    /// through the pipe.
    up: VecDeque<(Time, Said)>,
    /// The agent reads its channel; when the last message sent down is
    /// through the pipe.
    stdin: bool,
    down: Time,
    /// A read is in flight; the channel up ended as the model read it.
    reading: bool,
    ended: bool,
    /// A wait is in flight; the exit was told.
    waiting: bool,
    exited: bool,
    /// A reap is in flight; the tree's emptiness was told.
    reaping: bool,
    reaped: bool,
    detail: Vec<u8>,
}

impl Proc {
    fn is_empty(&self) -> bool {
        !self.main && self.children.iter().all(|child| !child.alive)
    }

    fn writers(&self) -> bool {
        self.main || self.children.iter().any(|child| child.alive && child.holds)
    }
}

pub struct Tree {
    rng: Rng,
    script: Script,
    agents: script::Script,
    sizes: Sizes,
    procs: BTreeMap<u64, Proc>,
    names: u64,
    tally: Tally,
}

impl Tree {
    #[must_use]
    pub fn new(script: Script, agents: script::Script, sizes: Sizes, seed: u64) -> Tree {
        Tree { rng: Rng::new(seed), script, agents, sizes, procs: BTreeMap::new(), names: 0, tally: Tally::default() }
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// What the world reads of the agent the model names `owner`.
    #[must_use]
    pub fn view(&self, owner: Token) -> View {
        self.find(owner).agent.view()
    }

    /// Whether the agent the model names `owner` has stopped talking: its
    /// process exited, it stopped reading its channel, or the channel up
    /// ended.
    #[must_use]
    pub fn quiet(&self, owner: Token) -> bool {
        let proc = self.find(owner);
        !proc.main || !proc.stdin || proc.ended
    }

    /// Whether the agent the model names `owner` has exited and its tree is
    /// empty.
    #[must_use]
    pub fn is_gone(&self, owner: Token) -> bool {
        self.find(owner).is_empty()
    }

    fn find(&self, owner: Token) -> &Proc {
        self.procs.values().find(|proc| proc.owner == owner).expect("the model names a spawned agent")
    }

    /// Takes one of the model's io requests.
    pub fn take(&mut self, now: Time, request: Request) -> Vec<Out> {
        match request {
            Request::Spawn { owner, workspace: _ } => self.spawn(owner),
            Request::Send { owner, process, message } => {
                self.tally.sends += 1;
                let pipe = self.script.pipe.draw(&mut self.rng);
                let proc = self.proc(owner, process);
                if !proc.main || !proc.stdin {
                    self.tally.unsent += 1;
                    return vec![Out::Model { after: Duration::ZERO, event: Event::Unsent { owner } }];
                }
                // Bytes come through a pipe in the order written.
                let through = now.saturating_add(pipe).max(proc.down);
                proc.down = through;
                let heard = translate::down(message);
                vec![
                    Out::Due {
                        after: through.saturating_since(now),
                        due: Due::Heard { process: process.raw(), heard },
                    },
                    Out::Model { after: Duration::ZERO, event: Event::Sent { owner } },
                ]
            }
            Request::Read { owner, process } => {
                let proc = self.proc(owner, process);
                assert!(!proc.reading && !proc.ended, "one read at a time, until the channel ends");
                proc.reading = true;
                self.try_read(now, process.raw())
            }
            Request::Signal { owner, process, signal } => self.signal(now, owner, process, signal),
            Request::Wait { owner, process } => {
                let proc = self.proc(owner, process);
                assert!(!proc.waiting && !proc.exited, "a process is waited for once");
                if proc.main {
                    proc.waiting = true;
                    return Vec::new();
                }
                proc.exited = true;
                vec![Out::Model { after: Duration::ZERO, event: Event::Exited { owner } }]
            }
            Request::Reap { owner, process } => {
                let proc = self.proc(owner, process);
                assert!(!proc.reaping && !proc.reaped, "a process is reaped once");
                if !proc.is_empty() {
                    proc.reaping = true;
                    return Vec::new();
                }
                assert!(proc.exited, "a reap follows its process's wait");
                proc.reaped = true;
                let detail = proc.detail.clone().into_boxed_slice();
                vec![Out::Model { after: Duration::ZERO, event: Event::Reaped { owner, detail } }]
            }
            Request::Started { .. }
            | Request::Called { .. }
            | Request::Waiting { .. }
            | Request::Told { .. }
            | Request::Finished { .. }
            | Request::Faulted { .. }
            | Request::Bounced { .. }
            | Request::Gone { .. } => unreachable!("the client's records go to the client"),
        }
    }

    /// Something of the tree's own falls due.
    pub fn due(&mut self, now: Time, due: Due) -> Vec<Out> {
        match due {
            Due::Wake { process, serial } => {
                let proc = self.procs.get_mut(&process).expect("a wake is for a spawned agent");
                if !proc.main {
                    return Vec::new();
                }
                let acts = proc.agent.woken(now, serial);
                self.act(now, process, acts)
            }
            Due::Heard { process, heard } => {
                let proc = self.procs.get_mut(&process).expect("a message is for a spawned agent");
                // Lost if it no longer reads its channel.
                if !proc.main || !proc.stdin {
                    return Vec::new();
                }
                let acts = proc.agent.hear(now, heard);
                self.act(now, process, acts)
            }
            Due::Exit { process, member } => self.exit(now, process, member),
            Due::Readable { process } => self.try_read(now, process),
        }
    }

    fn spawn(&mut self, owner: Token) -> Vec<Out> {
        self.tally.spawns += 1;
        let after = self.script.spawn.draw(&mut self.rng);
        let len = self.rng.below(self.script.detail + 1);
        let detail = vec![b'e'; usize::try_from(len).expect("fits")];
        if self.rng.chance(self.script.unspawned) {
            self.tally.unspawned += 1;
            let event = Event::Unspawned { owner, detail: detail.into_boxed_slice() };
            return vec![Out::Model { after, event }];
        }
        self.names += 1;
        let process = self.names;
        let agent = Agent::new(self.agents, self.sizes, self.rng.next_u64());
        let mut outs = vec![Out::Model { after, event: Event::Spawned { owner, process: Token::new(process) } }];
        let mut children = Vec::new();
        let count = self.rng.below(u64::from(self.script.children) + 1);
        for index in 0..usize::try_from(count).expect("fits") {
            self.tally.children += 1;
            let holds = self.rng.chance(self.script.holding);
            let stubborn = self.rng.chance(self.script.stubborn);
            if !self.rng.chance(self.script.lingering) {
                let life = after.saturating_add(self.script.child_life.draw(&mut self.rng));
                outs.push(Out::Due { after: life, due: Due::Exit { process, member: Member::Child(index) } });
            }
            children.push(Child { alive: true, holds, stubborn });
        }
        let proc = Proc {
            owner,
            agent,
            main: true,
            children,
            up: VecDeque::new(),
            stdin: true,
            down: Time::ZERO,
            reading: false,
            ended: false,
            waiting: false,
            exited: false,
            reaping: false,
            reaped: false,
            detail,
        };
        self.procs.insert(process, proc);
        outs
    }

    fn signal(&mut self, now: Time, owner: Token, process: Token, signal: Signal) -> Vec<Out> {
        let term = self.script.term.draw(&mut self.rng);
        let mut outs = vec![Out::Model { after: Duration::ZERO, event: Event::Signalled { owner } }];
        match signal {
            Signal::Terminate => self.tally.terminates += 1,
            Signal::Kill => self.tally.kills += 1,
        }
        let proc = self.proc(owner, process);
        let mut exits = Vec::new();
        let mut ignored = 0;
        match signal {
            Signal::Terminate => {
                if proc.main {
                    let acts = proc.agent.terminate();
                    if acts.is_empty() {
                        ignored += 1;
                    }
                    outs.extend(self.act(now, process.raw(), acts));
                }
                let proc = self.proc(owner, process);
                for (index, child) in proc.children.iter().enumerate() {
                    if !child.alive {
                        continue;
                    }
                    if child.stubborn {
                        ignored += 1;
                    } else {
                        let due = Due::Exit { process: process.raw(), member: Member::Child(index) };
                        outs.push(Out::Due { after: term, due });
                    }
                }
            }
            Signal::Kill => {
                if proc.main {
                    exits.push(Member::Main);
                }
                for (index, child) in proc.children.iter().enumerate() {
                    if child.alive {
                        exits.push(Member::Child(index));
                    }
                }
            }
        }
        self.tally.ignored += ignored;
        for member in exits {
            outs.extend(self.exit(now, process.raw(), member));
        }
        outs
    }

    /// A member exits: the wait ends with the agent's process, the reap with
    /// the last member, and the channel up once its last writer has gone.
    fn exit(&mut self, now: Time, process: u64, member: Member) -> Vec<Out> {
        let proc = self.procs.get_mut(&process).expect("a member is of a spawned tree");
        match member {
            Member::Main => {
                if !proc.main {
                    return Vec::new();
                }
                proc.main = false;
                proc.stdin = false;
                let orphans = proc.children.iter().filter(|child| child.alive).count();
                self.tally.orphans += u32::try_from(orphans).expect("few");
            }
            Member::Child(index) => {
                let child = proc.children.get_mut(index).expect("a child of the tree");
                if !child.alive {
                    return Vec::new();
                }
                child.alive = false;
            }
        }
        let proc = self.procs.get_mut(&process).expect("looked up above");
        let owner = proc.owner;
        let mut outs = Vec::new();
        if member == Member::Main && proc.waiting {
            proc.waiting = false;
            proc.exited = true;
            outs.push(Out::Model { after: Duration::ZERO, event: Event::Exited { owner } });
        }
        if proc.is_empty() && proc.reaping {
            assert!(proc.exited, "the exit is told before the reap");
            proc.reaping = false;
            proc.reaped = true;
            let detail = proc.detail.clone().into_boxed_slice();
            outs.push(Out::Model { after: Duration::ZERO, event: Event::Reaped { owner, detail } });
        }
        outs.extend(self.try_read(now, process));
        outs
    }

    /// Ends the read in flight, if it can be: with the next message through
    /// the pipe, or a hangup once every writer has gone and all is read.
    fn try_read(&mut self, now: Time, process: u64) -> Vec<Out> {
        let proc = self.procs.get_mut(&process).expect("a read is of a spawned agent");
        if !proc.reading {
            return Vec::new();
        }
        let owner = proc.owner;
        match proc.up.front() {
            Some((ready, _)) if *ready > now => {
                let after = ready.saturating_since(now);
                vec![Out::Due { after, due: Due::Readable { process } }]
            }
            Some(_) => {
                let (_, said) = proc.up.pop_front().expect("looked at above");
                proc.reading = false;
                if said == Said::Garbage {
                    // The rest is no message either: the protocol layer
                    // drops it to the channel's end.
                    proc.ended = true;
                    proc.up.clear();
                    self.tally.malformed += 1;
                }
                vec![Out::Model { after: Duration::ZERO, event: translate::up(owner, said) }]
            }
            None if !proc.writers() => {
                proc.reading = false;
                proc.ended = true;
                self.tally.hangups += 1;
                vec![Out::Model { after: Duration::ZERO, event: Event::Hangup { owner } }]
            }
            None => Vec::new(),
        }
    }

    /// Carries out what the agent of `process` does.
    fn act(&mut self, now: Time, process: u64, acts: Vec<Act>) -> Vec<Out> {
        let mut outs = Vec::new();
        for act in acts {
            match act {
                Act::Write(said) => {
                    let pipe = self.script.pipe.draw(&mut self.rng);
                    let proc = self.procs.get_mut(&process).expect("an agent of a spawned tree");
                    // Bytes come through a pipe in the order written.
                    let last = proc.up.back().map_or(now, |(ready, _)| *ready);
                    let ready = now.saturating_add(pipe).max(last);
                    proc.up.push_back((ready, said));
                    if proc.reading {
                        outs.push(Out::Due { after: ready.saturating_since(now), due: Due::Readable { process } });
                    }
                }
                Act::Wake { after, serial } => outs.push(Out::Due { after, due: Due::Wake { process, serial } }),
                Act::Exit { after } => outs.push(Out::Due { after, due: Due::Exit { process, member: Member::Main } }),
                Act::Deaf => {
                    let proc = self.procs.get_mut(&process).expect("an agent of a spawned tree");
                    proc.stdin = false;
                }
            }
        }
        outs
    }

    fn proc(&mut self, owner: Token, process: Token) -> &mut Proc {
        let proc = self.procs.get_mut(&process.raw()).expect("io's name for a spawned process");
        assert_eq!(proc.owner, owner, "a request on a process names its agent");
        proc
    }

    /// Checks that every tree has gone, and was read to its end.
    pub fn assert_settled(&self) {
        for (process, proc) in &self.procs {
            assert!(proc.is_empty(), "process {process}'s tree is empty");
            assert!(proc.exited && proc.reaped, "process {process} was waited for and reaped");
            assert!(proc.ended && !proc.reading, "process {process}'s channel was read to its end");
            assert!(!proc.waiting && !proc.reaping, "nothing is asked of process {process}");
        }
    }
}
