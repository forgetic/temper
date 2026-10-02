//! Scripted workers (engine-model.md, section 8; worker-model.md, sections 2
//! and 4): each dials in with its slots and the workstreams it holds, hosts
//! the runs it is assigned on their charters (as bytes, decoded by the
//! codec), and plays each run's script ([`crate::script`]): facts, relayed
//! calls, pushes through the fake forge's git, waits for inbound events,
//! and an answer, which it keeps, with its slot, until the engine
//! acknowledges it, sending it again after every hello. It refuses an
//! assignment it has no slot for, or whose workstream another run holds,
//! as busy. A cancelled run is answered as a transient failure: the worker
//! stopped it. When its channel drops, its runs go on, and what they would
//! send waits for the next channel; a call in flight is answered as
//! failed; past its grace, it cancels its runs itself and keeps their
//! answers.
//!
//! The world carries what it says up its channel and what comes down, and
//! does its pushes; a worker takes liberties a real one does not, reading
//! the forge as observed ([`Mirror`]) to choose what its runs do.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_engine_model::fleet::{Bounce, Phase};
use temper_engine_model::views::Kind;
use temper_engine_model::{Call, Charter, Failure, Hosted, Inbound, Item, Landed, Served, Start, Unserved, Workspace};
use temper_lib::{Duration, Rng, Time, Token};
use temper_world::Span;

use crate::codec;
use crate::mirror::Mirror;
use crate::script::{self, Act, End};

/// How the workers behave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    pub slots: u32,
    /// Between a run's acts.
    pub pace: Span,
    /// How long a worker keeps its runs going without contact.
    pub grace: Duration,
    /// How long a run waits for the answer to its call, as its agent's
    /// tools bound every wait.
    pub call: Duration,
}

/// An assignment as it comes down a channel: the charter as bytes.
#[derive(Debug)]
pub struct Assigned {
    pub item: Item,
    pub attempt: u64,
    pub workspace: Workspace,
    pub charter: Vec<u8>,
    pub snapshot: Option<Box<[u8]>>,
}

/// What comes down a worker's channel.
#[derive(Debug)]
pub enum Down {
    Assign(Assigned),
    Inbound { item: Item, attempt: u64, event: Inbound },
    Cancel { item: Item, attempt: u64 },
    Relayed { item: Item, attempt: u64, call: Token, served: Served },
    Acknowledge { item: Item, attempt: u64 },
}

/// A run's answer, as a worker says it: an outcome as bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Said {
    Busy,
    Ended { outcome: Vec<u8>, landed: Vec<Landed> },
    Parked { snapshot: Option<Vec<u8>>, landed: Vec<Landed> },
    Failed { failure: Failure, landed: Vec<Landed> },
}

/// What goes up a worker's channel.
#[derive(Debug)]
pub enum Up {
    Hello { slots: u32, workstreams: Vec<Box<[u8]>>, hosting: Vec<Hosted> },
    Answer { item: Item, attempt: u64, said: Said },
    Relay { item: Item, attempt: u64, call: Token, body: Call },
    Bounced { item: Item, attempt: u64, bounce: Bounce },
    Told { item: Item, attempt: u64, kind: Kind, content: Vec<u8> },
}

/// What a worker asks of the world.
#[derive(Debug)]
pub enum Effect {
    /// Send up the worker's channel.
    Up(Up),
    /// The run's next act, at `at`, if its `serial` is still the run's.
    Wake { at: Time, item: Item, attempt: u64, serial: u64 },
    /// Push `content`, in the file CI reads, onto `start` of `repository`,
    /// to `branch`.
    Push { item: Item, attempt: u64, repository: u32, start: Start, branch: Box<[u8]>, content: Vec<u8> },
}

/// What a run is waiting for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wait {
    /// Its next act, woken under its serial.
    Act,
    /// The answer to its call.
    Call(Token),
    /// Its push's end.
    Push,
    /// An inbound event, or its serial's wake.
    Inbound,
    /// A channel, to send what it says next.
    Contact,
}

impl Wait {
    fn is_call(self) -> bool {
        match self {
            Wait::Call(_) => true,
            Wait::Act | Wait::Push | Wait::Inbound | Wait::Contact => false,
        }
    }
}

#[derive(Debug)]
struct Run {
    workspace: Workspace,
    acts: VecDeque<Act>,
    wait: Wait,
    serial: u64,
    landed: Vec<Landed>,
    /// Its push in flight, and whether it was refused once already.
    pushing: Option<(u32, Box<[u8]>, Vec<u8>)>,
    refused: bool,
    /// Inbound events that came while it did not wait for one.
    inbound: u32,
}

/// What a worker did, counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub assigned: u32,
    pub busy: u32,
    pub ended: u32,
    pub parked: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub expired: u32,
    pub calls: u32,
    pub served: u32,
    pub unserved: u32,
    pub inbound: u32,
    pub bounced: u32,
    pub pushes: u32,
}

#[derive(Debug)]
pub struct Worker {
    script: Script,
    rng: Rng,
    channel: Option<Token>,
    lost: Option<Time>,
    runs: BTreeMap<(Item, u64), Run>,
    /// Answers kept until the engine acknowledges them.
    answers: BTreeMap<(Item, u64), Said>,
    workstreams: BTreeSet<Box<[u8]>>,
    calls: u64,
    serials: u64,
    tally: Tally,
}

impl Worker {
    #[must_use]
    pub fn new(script: Script, seed: u64) -> Worker {
        Worker {
            script,
            rng: Rng::new(seed),
            channel: None,
            lost: None,
            runs: BTreeMap::new(),
            answers: BTreeMap::new(),
            workstreams: BTreeSet::new(),
            calls: 0,
            serials: 0,
            tally: Tally::default(),
        }
    }

    #[must_use]
    pub fn channel(&self) -> Option<Token> {
        self.channel
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Whether the worker hosts nothing and keeps no answer.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.runs.is_empty() && self.answers.is_empty()
    }

    /// The runs it hosts, and those whose answers it keeps.
    #[must_use]
    pub fn hosting(&self) -> Vec<(Item, u64)> {
        self.runs.keys().chain(self.answers.keys()).copied().collect()
    }

    /// The runs it hosts, not yet answered.
    #[must_use]
    pub fn running(&self) -> Vec<(Item, u64)> {
        self.runs.keys().copied().collect()
    }

    /// When contact was lost, if it is.
    #[must_use]
    pub fn lost(&self) -> Option<Time> {
        self.lost
    }

    /// A channel is open: hello first, then every answer kept, then what
    /// waited for contact.
    pub fn hello(&mut self, channel: Token, now: Time, out: &mut Vec<Effect>) {
        self.channel = Some(channel);
        self.lost = None;
        let mut hosting: Vec<Hosted> =
            self.runs.keys().map(|&(item, attempt)| Hosted { item, attempt, phase: Phase::Active }).collect();
        hosting.extend(self.answers.keys().map(|&(item, attempt)| Hosted { item, attempt, phase: Phase::Answered }));
        let workstreams = self.workstreams.iter().cloned().collect();
        out.push(Effect::Up(Up::Hello { slots: self.script.slots, workstreams, hosting }));
        for (&(item, attempt), said) in &self.answers {
            out.push(Effect::Up(Up::Answer { item, attempt, said: said.clone() }));
        }
        let waiting: Vec<(Item, u64)> =
            self.runs.iter().filter(|(_, run)| run.wait == Wait::Contact).map(|(key, _)| *key).collect();
        for (item, attempt) in waiting {
            self.wake(item, attempt, now, out);
        }
    }

    /// The channel closed: runs go on; a call in flight fails.
    pub fn lose(&mut self, now: Time, out: &mut Vec<Effect>) {
        if self.channel.take().is_some() {
            self.lost = Some(now);
        }
        let calling: Vec<(Item, u64)> =
            self.runs.iter().filter(|(_, run)| run.wait.is_call()).map(|(key, _)| *key).collect();
        for (item, attempt) in calling {
            self.tally.unserved += 1;
            self.wake(item, attempt, now, out);
        }
    }

    /// Past its grace without contact, the worker cancels its runs itself,
    /// and keeps their answers for the next channel.
    pub fn expire(&mut self, now: Time) {
        let Some(lost) = self.lost else { return };
        if now < lost.saturating_add(self.script.grace) {
            return;
        }
        let keys: Vec<(Item, u64)> = self.runs.keys().copied().collect();
        for (item, attempt) in keys {
            let run = self.runs.remove(&(item, attempt)).expect("a run hosted");
            self.tally.expired += 1;
            self.answers.insert((item, attempt), Said::Failed { failure: Failure::Transient, landed: run.landed });
        }
    }

    /// What came down the channel.
    pub fn down(&mut self, down: Down, now: Time, mirror: &Mirror, out: &mut Vec<Effect>) {
        match down {
            Down::Assign(assigned) => self.assign(assigned, now, mirror, out),
            Down::Inbound { item, attempt, event: _ } => {
                self.tally.inbound += 1;
                if let Some(run) = self.runs.get_mut(&(item, attempt)) {
                    if run.wait == Wait::Inbound {
                        self.wake(item, attempt, now, out);
                    } else {
                        run.inbound += 1;
                    }
                } else {
                    self.tally.bounced += 1;
                    out.push(Effect::Up(Up::Bounced { item, attempt, bounce: Bounce::Ending }));
                }
            }
            Down::Cancel { item, attempt } => {
                if let Some(run) = self.runs.remove(&(item, attempt)) {
                    self.tally.cancelled += 1;
                    self.answer(item, attempt, Said::Failed { failure: Failure::Transient, landed: run.landed }, out);
                }
            }
            Down::Relayed { item, attempt, call, served } => {
                let Some(run) = self.runs.get(&(item, attempt)) else { return };
                if run.wait != Wait::Call(call) {
                    return;
                }
                match served {
                    Served::Unserved(_) => self.tally.unserved += 1,
                    Served::Read(_) | Served::Recalled { .. } | Served::Noted(_) | Served::Posted { .. } => {
                        self.tally.served += 1;
                    }
                }
                self.wake(item, attempt, now, out);
            }
            Down::Acknowledge { item, attempt } => {
                self.answers.remove(&(item, attempt));
            }
        }
    }

    fn assign(&mut self, assigned: Assigned, now: Time, mirror: &Mirror, out: &mut Vec<Effect>) {
        let Assigned { item, attempt, workspace, charter, snapshot } = assigned;
        let key = (item, attempt);
        if self.runs.contains_key(&key) || self.answers.contains_key(&key) {
            return;
        }
        let taken = u32::try_from(self.runs.len() + self.answers.len()).expect("few");
        let busy_workstream = self.runs.values().any(|run| run.workspace.key == workspace.key);
        if taken >= self.script.slots || busy_workstream {
            self.tally.busy += 1;
            out.push(Effect::Up(Up::Answer { item, attempt, said: Said::Busy }));
            return;
        }
        self.tally.assigned += 1;
        let charter: Charter = codec::charter_of(&charter).expect("a charter decodes as it was encoded");
        let acts = script::acts(item, &charter, snapshot.as_deref(), mirror);
        self.workstreams.insert(workspace.key.clone());
        let run = Run {
            workspace,
            acts: acts.into(),
            wait: Wait::Act,
            serial: 0,
            landed: Vec::new(),
            pushing: None,
            refused: false,
            inbound: 0,
        };
        self.runs.insert(key, run);
        self.wake(item, attempt, now, out);
    }

    /// The run's next act, after the worker's pace.
    fn wake(&mut self, item: Item, attempt: u64, now: Time, out: &mut Vec<Effect>) {
        self.serials += 1;
        let serial = self.serials;
        let at = now.saturating_add(self.script.pace.draw(&mut self.rng));
        let Some(run) = self.runs.get_mut(&(item, attempt)) else { return };
        run.wait = Wait::Act;
        run.serial = serial;
        out.push(Effect::Wake { at, item, attempt, serial });
    }

    /// The run's wake under `serial`: its next act, if it still waits for
    /// it, or for an inbound event that did not come.
    pub fn act(&mut self, item: Item, attempt: u64, serial: u64, now: Time, out: &mut Vec<Effect>) {
        let connected = self.channel.is_some();
        let Some(run) = self.runs.get_mut(&(item, attempt)) else { return };
        if run.serial != serial || !(run.wait == Wait::Act || run.wait == Wait::Inbound || run.wait.is_call()) {
            return;
        }
        if run.wait.is_call() {
            // Its call was not answered in time: it goes on without.
            self.tally.unserved += 1;
        }
        let Some(act) = run.acts.pop_front() else { unreachable!("a script ends with its answer") };
        match act {
            Act::Tell { kind, content } => {
                if connected {
                    out.push(Effect::Up(Up::Told { item, attempt, kind, content }));
                }
                self.wake(item, attempt, now, out);
            }
            Act::Call(body) => {
                if !connected {
                    run.acts.push_front(Act::Call(body));
                    run.wait = Wait::Contact;
                    return;
                }
                self.calls += 1;
                let call = Token::new(self.calls);
                run.wait = Wait::Call(call);
                self.serials += 1;
                run.serial = self.serials;
                self.tally.calls += 1;
                out.push(Effect::Up(Up::Relay { item, attempt, call, body }));
                let at = now.saturating_add(self.script.call);
                out.push(Effect::Wake { at, item, attempt, serial: run.serial });
            }
            Act::Push { repository, content } => {
                let checkout = run.workspace.repositories.iter().find(|checkout| checkout.repository == repository);
                let Some(checkout) = checkout else { unreachable!("a change's run is given its repository") };
                let Some(branch) = checkout.push.clone() else { unreachable!("a change's run may push") };
                let start = checkout.start.clone();
                run.wait = Wait::Push;
                run.pushing = Some((repository, branch.clone(), content.clone()));
                self.tally.pushes += 1;
                out.push(Effect::Push { item, attempt, repository, start, branch, content });
            }
            Act::Await { within } => {
                if run.inbound > 0 {
                    run.inbound -= 1;
                    self.wake(item, attempt, now, out);
                    return;
                }
                self.serials += 1;
                run.serial = self.serials;
                run.wait = Wait::Inbound;
                out.push(Effect::Wake { at: now.saturating_add(within), item, attempt, serial: run.serial });
            }
            Act::End(end) => {
                let run = self.runs.remove(&(item, attempt)).expect("the run acting");
                let said = match end {
                    End::Ended(outcome) => {
                        self.tally.ended += 1;
                        Said::Ended { outcome: codec::outcome(&outcome), landed: run.landed }
                    }
                    End::Parked(snapshot) => {
                        self.tally.parked += 1;
                        Said::Parked { snapshot, landed: run.landed }
                    }
                    End::Failed(failure) => {
                        self.tally.failed += 1;
                        Said::Failed { failure, landed: run.landed }
                    }
                };
                self.answer(item, attempt, said, out);
            }
        }
    }

    /// The run's push ended: what it landed, if it did.
    pub fn pushed(&mut self, item: Item, attempt: u64, landed: Option<Landed>, now: Time, out: &mut Vec<Effect>) {
        let Some(run) = self.runs.get_mut(&(item, attempt)) else { return };
        if run.wait != Wait::Push {
            return;
        }
        if let Some(landed) = landed {
            run.landed.push(landed);
        } else if !run.refused
            && let Some((repository, branch, content)) = run.pushing.take()
        {
            // Refused: the branch moved since the run started, by a push
            // of an earlier attempt that landed late, say. The run is told,
            // works again from where the branch is, and pushes once more.
            run.refused = true;
            let start = Start::Branch { branch: branch.clone() };
            self.tally.pushes += 1;
            out.push(Effect::Push { item, attempt, repository, start, branch, content });
            return;
        } else {
            // Rejected: the run fails, as a push that does not land.
            run.acts.clear();
            run.acts.push_back(Act::End(End::Failed(Failure::Transient)));
        }
        self.wake(item, attempt, now, out);
    }

    /// Keeps `said` until it is acknowledged, and sends it if in contact.
    fn answer(&mut self, item: Item, attempt: u64, said: Said, out: &mut Vec<Effect>) {
        self.answers.insert((item, attempt), said.clone());
        if self.channel.is_some() {
            out.push(Effect::Up(Up::Answer { item, attempt, said }));
        }
    }
}

/// Whether a call's answer served it.
#[must_use]
pub fn is_served(served: &Served) -> bool {
    match served {
        Served::Unserved(
            Unserved::Ungranted | Unserved::Busy | Unserved::Invalid | Unserved::Refused | Unserved::Failed,
        ) => false,
        Served::Read(_) | Served::Recalled { .. } | Served::Noted(_) | Served::Posted { .. } => true,
    }
}
