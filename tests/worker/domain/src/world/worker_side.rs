//! The worker's stage, and what it asks of the world: the channel to the
//! engine, its agents' process trees, and io's git; with the worker's
//! contracts checked as it goes.

use std::collections::BTreeSet;

use skein_lib::Token;
use temper_legacy_engine_domain as engine;
use temper_legacy_engine_domain_world::codec;
use temper_legacy_engine_domain_world::deployment::{CUE, MAIN};
use temper_legacy_engine_domain_world::referee as stories;
use temper_worker_agent_world::script::{self, Said};
use temper_worker_agent_world::tree::{self, Tree};
use temper_worker_checkout_world::translate as io;
use temper_worker_domain::agent::channel::{Ask, Down, Finish, Reply, Up};
use temper_worker_domain::checkout::git::Place;
use temper_worker_domain::{self as worker, Domain, Event, Hello, Phase, Request, agent, host};
use temper_world::Stage;

use super::git::{files, tree as forge_tree};
use super::{Agent, Attempt, Content, Delivery, Garbled, Lane, RELEASE, Repository, World, gone, sizes};
use crate::protocol::{self, Names};
use crate::referee;
use crate::translate;

impl World {
    pub(super) fn run_worker(&mut self) {
        // Its ready list first, then its events, then its alarms, while it
        // has room for what one more may produce.
        while self.stage.has_room() && self.worker.is_ready() {
            self.log("worker resume");
            worker::resume(&mut self.worker, &self.stage.env, &mut self.stage.out);
        }
        while let Some(event) = self.stage.next_event() {
            self.log(format!("worker <- {event:?}"));
            self.take(&event);
            worker::step(&mut self.worker, &self.stage.env, event, &mut self.stage.out);
        }
        while self.stage.has_room() && self.worker.is_due(self.now) {
            self.log("worker alarm");
            worker::fire(&mut self.worker, &self.stage.env, &mut self.stage.out);
        }
        // The facts, drained as the shell would write them out; the run's,
        // sent to the engine best effort.
        while self.worker.pop_fact().is_some() {
            self.stats.facts += 1;
        }
        while let Some(told) = self.worker.pop_told() {
            self.stats.told += 1;
            self.send_up(move |channel| protocol::told(channel, told));
        }
        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.log(format!("worker -> {request:?}"));
            self.route(request);
            self.routed += 1;
        }
        let following: Vec<&Names> = self.following.iter().collect();
        assert!(following.is_empty(), "the answers a hello lists as held follow it: {following:?} did not");
        self.worker.reclaim();
        let hosted = self.worker.host().hosted();
        assert!(hosted <= self.stage.env.limits.host.slots, "runs stay within their slots");
        self.stats.peak = self.stats.peak.max(hosted);
        let abandoned = self.worker.abandoned();
        if abandoned > self.abandoned {
            assert!(self.shut && !self.up, "only a worker shutting down out of reach gives answers up");
            assert!(self.graced.is_some(), "past its grace");
            assert_eq!(self.worker.host().unanswered(), 0, "with no run left, whose answer could go with them");
            self.abandoned = abandoned;
        }
        if self.shut && self.worker.is_done() {
            self.stop();
        }
    }

    /// The protocol boundary turns raw wire replies and cancellation races
    /// into one terminal per emitted relay. Late duplicates are filtered here,
    /// before they enter the domain's event stream.
    pub(super) fn push_worker_event(&mut self, event: Event) {
        if let Event::Relayed { run, attempt, call, .. } = &event {
            let Some((names, _)) = self.relay_waits.get(call) else { return };
            if *names != (*run, *attempt) {
                return;
            }
            self.relay_waits.remove(call);
        }
        if let Event::RelayCancelled { call } = &event
            && self.relay_waits.remove(call).is_none()
        {
            return;
        }
        self.stage.push(event);
    }

    /// Notes what the worker takes as `event`, before it takes it.
    fn take(&mut self, event: &Event) {
        match event {
            temper_worker_domain::Event::AcknowledgeTurn { .. } | temper_worker_domain::Event::TurnBusy { .. } => {
                unreachable!("this system world runs version one")
            }

            temper_worker_domain::Event::ConnectedV2
            | temper_worker_domain::Event::RelayedV2 { .. }
            | temper_worker_domain::Event::AssignV2 { .. } => unreachable!("this system world runs version one"),

            Event::Connected => {
                if let Some(since) = self.down_since.take()
                    && !self.open.is_empty()
                {
                    let outage = self.now.saturating_since(since);
                    self.stats.longest_outage =
                        Some(self.stats.longest_outage.map_or(outage, |longest| longest.max(outage)));
                }
                self.up = true;
            }
            Event::Lost => {
                if self.up {
                    self.up = false;
                    self.down_since = Some(self.now);
                }
            }
            Event::Shutdown => self.shut = true,
            Event::Assign { assignment } => {
                let names = (assignment.run, assignment.attempt);
                if let Some(branch) = &assignment.save {
                    self.save_branches.insert(branch.to_vec());
                }
                let repositories = assignment.workspace.repositories.iter().map(|repository| {
                    let saved = match &repository.start {
                        temper_worker_domain::host::Start::Merge { .. } => {
                            unreachable!("this system world runs version one")
                        }

                        host::Start::Saved { branch } => Some(branch.to_vec()),
                        host::Start::Base { .. } | host::Start::Branch { .. } | host::Start::Commit { .. } => None,
                    };
                    let push = match &repository.access {
                        temper_worker_domain::host::Access::WritableV2 { .. } => {
                            unreachable!("this system world runs version one")
                        }

                        host::Access::Writable { push } => Some(push.to_vec()),
                        host::Access::ReadOnly => None,
                    };
                    Repository { name: repository.name.to_vec(), remote: repository.remote.to_vec(), saved, push }
                });
                let record = Attempt {
                    at: self.now,
                    charter: assignment.charter.clone(),
                    repositories: repositories.collect(),
                    snapshot: assignment.snapshot.clone(),
                    agent: None,
                    answer: None,
                    refused: false,
                };
                // The engine places an attempt again once its worker refused
                // it, which the worker forgot; the attempt hosted, assigned
                // again, is dropped.
                match self.attempts.get(&names) {
                    Some(before) if before.refused => {}
                    Some(_) => return,
                    None => {}
                }
                self.attempts.insert(names, record);
                self.open.insert(names);
            }
            Event::Acknowledged { run, attempt } => {
                let names = (*run, *attempt);
                // The worker forgets the answer: no hello lists it again.
                if self.attempts.get(&names).is_some_and(|record| record.answer.is_some()) {
                    self.open.remove(&names);
                }
                self.stats.acknowledgements += 1;
            }
            Event::Cancel { run, attempt } => {
                let names = (*run, *attempt);
                let live = self.attempts.get(&names).is_some_and(|record| record.answer.is_none());
                if live && !self.cancelled.contains_key(&names) {
                    let first = self.routed + u64::from(self.stage.out.len());
                    self.cancelled.insert(names, first);
                }
            }
            Event::Inbound { .. }
            | Event::Grant { .. }
            | Event::Relayed { .. }
            | Event::RelayCancelled { .. }
            | Event::Spawned { .. }
            | Event::Unspawned { .. }
            | Event::Sent { .. }
            | Event::Unsent { .. }
            | Event::Received { .. }
            | Event::Malformed { .. }
            | Event::Hangup { .. }
            | Event::Signalled { .. }
            | Event::Exited { .. }
            | Event::Reaped { .. }
            | Event::Done { .. } => {}
        }
    }

    /// One of the worker's requests, to the world that carries it out.
    fn route(&mut self, request: Request) {
        match request {
            temper_worker_domain::Request::RelayV2 { .. }
            | temper_worker_domain::Request::HelloV2 { .. }
            | temper_worker_domain::Request::Turn { .. }
            | temper_worker_domain::Request::AnswerV2 { .. } => unreachable!("this system world runs version one"),

            Request::Dial => self.dial(),
            Request::Hello { hello } => {
                self.hello(&hello);
                self.send_up(move |channel| protocol::up(Request::Hello { hello }, channel));
            }
            Request::Answer { run, attempt, answer } => {
                self.answered((run, attempt), &answer);
                self.send_up(move |channel| protocol::up(Request::Answer { run, attempt, answer }, channel));
            }
            Request::Relay { run, attempt, call, body } => {
                assert!(self.up, "a relay goes on a channel open");
                self.stats.relays += 1;
                assert!(
                    self.relay_waits.insert(call, ((run, attempt), false)).is_none(),
                    "relay wait names are unique"
                );
                if protocol::call_of(&body).is_none() {
                    // Nothing the engine reads: the protocol layer answers it
                    // itself.
                    self.end("undecoded");
                    let answer = protocol::undecoded();
                    let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
                    self.send(at, Delivery::Worker(self.lives, Event::Relayed { run, attempt, call, answer }));
                    return;
                }
                self.send_up(move |channel| protocol::up(Request::Relay { run, attempt, call, body }, channel));
            }
            Request::CancelRelay { call } => {
                // A response already handed to the inbox wins its race with
                // cancellation. Otherwise cancellation competes with any raw
                // wire response still in flight; only the first is delivered.
                if let Some((_, cancelled)) = self.relay_waits.get_mut(&call) {
                    assert!(!*cancelled, "a relay delivery is cancelled once");
                    *cancelled = true;
                    let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
                    self.send(at, Delivery::Worker(self.lives, Event::RelayCancelled { call }));
                }
            }
            Request::Bounced { name, run, attempt, bounce } => {
                assert!(self.up, "a bounce goes on a channel open");
                self.stats.bounces += 1;
                self.send_up(move |channel| protocol::up(Request::Bounced { run, attempt, name, bounce }, channel));
            }
            Request::Rejected { run, attempt, account, generation } => {
                self.send_up(move |channel| {
                    protocol::up(Request::Rejected { run, attempt, account, generation }, channel)
                });
            }
            Request::Exhausted { run, attempt, account, retry_after } => {
                self.send_up(move |channel| {
                    protocol::up(Request::Exhausted { run, attempt, account, retry_after }, channel)
                });
            }
            Request::Spawn { owner, workspace, deadline } => {
                self.spawn(owner, workspace);
                self.tree_take(agent::Request::Spawn { owner, workspace, deadline });
            }
            Request::Send { owner, process, message } => {
                self.down(owner, &message);
                self.tree_take(agent::Request::Send { owner, process, message });
            }
            Request::Read { owner, process } => self.tree_take(agent::Request::Read { owner, process }),
            Request::Signal { owner, process, signal } => {
                self.tree_take(agent::Request::Signal { owner, process, signal });
            }
            Request::Wait { owner, process } => self.tree_take(agent::Request::Wait { owner, process }),
            Request::Reap { owner, process } => self.tree_take(agent::Request::Reap { owner, process }),
            Request::Io { owner, op, deadline } => self.start_op(owner, op, deadline),
            Request::CancelIo { owner } => self.cancel_op(owner),
        }
    }

    /// The worker's hello: on a channel it holds open, listing exactly the
    /// runs it was given and has not answered, those whose answers it holds
    /// followed by them.
    fn hello(&mut self, hello: &Hello) {
        assert!(self.up, "a hello goes on a channel open");
        let slots = if self.shut { 0 } else { self.stage.env.limits.host.slots };
        assert_eq!(hello.slots, slots, "the hello says the worker's slots, none once it is shutting down");
        let listed: BTreeSet<Names> = hello.hosting.iter().map(|hosted| (hosted.run, hosted.attempt)).collect();
        assert_eq!(listed.len(), hello.hosting.len(), "a hello lists each run once");
        // Of the runs it was given and has not answered, it lists all but those
        // whose answers it gave up, shutting down out of reach past its grace.
        let unlisted: Vec<Names> = self.open.difference(&listed).copied().collect();
        assert!(listed.is_subset(&self.open), "a hello lists only runs the worker was given and has not answered");
        let given_up = u64::try_from(self.given_up.len() + unlisted.len()).expect("fits");
        assert_eq!(
            given_up,
            self.worker.abandoned(),
            "a hello lists every run the worker has not answered or given up"
        );
        for names in unlisted {
            self.open.remove(&names);
            self.given_up.insert(names);
        }
        for hosted in &hello.hosting {
            match hosted.phase {
                Phase::Answered => {
                    self.following.insert((hosted.run, hosted.attempt));
                    self.stats.held += 1;
                    if self.shut && self.graced.is_some() {
                        self.stats.kept_past_grace += 1;
                    }
                }
                Phase::Preparing | Phase::Starting | Phase::Active | Phase::Waiting | Phase::Ending => {}
            }
        }
        self.stats.hellos += 1;
    }

    /// The worker's answer for the attempt `names`: once, on a channel it
    /// holds open, for an attempt it was given, once its agent has gone and
    /// nothing of git runs for it; and cancelled only by whoever may cancel
    /// it.
    fn answered(&mut self, names: Names, answer: &host::Answer) {
        assert!(self.up, "an answer goes on a channel open");
        let record = self.attempts.get_mut(&names).expect("an answer is for an attempt the worker was given");
        let said = format!("{answer:?}");
        if record.refused && record.answer.as_ref() == Some(&said) {
            // The same assignment again, a copy that came behind the one it
            // refused before it could answer, refused the same way.
            self.stats.answers_sent += 1;
            return;
        }
        assert!(self.open.contains(&names), "an answer is for a run neither acknowledged nor given up");
        self.following.remove(&names);
        self.stats.answers_sent += 1;
        if let Some(first) = &record.answer {
            // Sent again after a hello, the engine's acknowledgement not heard.
            assert_eq!(*first, said, "an answer sent again is the same answer");
            self.stats.resent += 1;
            return;
        }
        record.answer = Some(said);
        let assigned = record.at;
        let agent = record.agent;
        // Another party may delete a push branch before the next attempt.
        let pushes: Vec<(Vec<u8>, Vec<u8>)> = record
            .repositories
            .iter()
            .filter_map(|repository| Some((repository.remote.clone(), repository.push.clone()?)))
            .collect();
        if !pushes.is_empty() && self.rng.chance(self.settings.git.deletes) {
            let index = usize::try_from(self.rng.below(u64::try_from(pushes.len()).expect("fits"))).expect("fits");
            let (remote, branch) = pushes[index].clone();
            let at = self.now.saturating_add(self.settings.git.advance_after.draw(&mut self.rng));
            self.send(at, Delivery::Delete { remote, branch });
        }
        if let host::Answer::Ended { outcome, .. } = answer
            && codec::outcome_of(outcome).is_none()
        {
            // An outcome no engine reads: the protocol layer says the agent
            // failed.
            self.end("undecodable");
        }
        self.answered_copies(names);
        let record = self.attempts.get_mut(&names).expect("looked up above");
        *self.stats.answers.entry(translate::answer_kind(answer)).or_default() += 1;
        match answer {
            host::Answer::Failed { failure: host::Failure::Cancelled(reason), .. } => match reason {
                host::Reason::Engine => {
                    assert!(
                        self.cancelled.contains_key(&names),
                        "a run is cancelled by the engine once it cancelled it"
                    );
                }
                host::Reason::Contact => assert!(
                    self.graced.is_some_and(|at| at >= assigned),
                    "a run is cancelled for contact once the worker was out of reach past its grace"
                ),
                host::Reason::Shutdown => assert!(self.shut, "a run is cancelled for a shutdown once told to"),
            },
            // A refusal goes once, and holds no slot: the worker forgets it.
            host::Answer::Refused(_) => {
                record.refused = true;
                self.open.remove(&names);
            }
            host::Answer::Ended { .. } | host::Answer::Parked { .. } | host::Answer::Failed { .. } => {}
        }
        let Some(owner) = agent else {
            return;
        };
        assert!(self.gone(owner), "a run answers once its agent has gone");
        let record = self.agents.remove(&owner).expect("an agent started is known");
        let space = self.spaces.get_mut(&record.workspace).expect("an agent runs in a workspace");
        if space.agent == Some(owner) {
            space.agent = None;
        }
        if let Some(hold) = space.hold {
            assert!(!self.ops.contains(hold), "a run answers once nothing of git runs in its workspace");
            self.closed.insert(hold);
        }
    }

    fn spawn(&mut self, owner: Token, workspace: Token) {
        assert!(self.disk.exists(&io::dir(workspace)), "an agent is spawned in a workspace io has");
        let busy = self.busy(workspace);
        assert!(!busy, "an agent is spawned in a workspace nothing of git runs in");
        let space = self.spaces.get_mut(&workspace).expect("an agent is spawned in a workspace prepared");
        if let Some(before) = space.agent {
            let record = self.agents.get(&before).expect("an agent is known until its run answers");
            assert!(record.attempt.is_none() && gone(&self.tree, record), "one agent at a time in a workspace");
        }
        space.agent = Some(owner);
        let agent = Agent {
            owner,
            workspace,
            attempt: None,
            spawned: false,
            unspawned: false,
            pushes: BTreeSet::new(),
            content: None,
            garbled: Garbled::default(),
        };
        assert!(self.agents.insert(owner, agent).is_none(), "an agent is spawned once");
        self.stats.spawns += 1;
    }

    /// What the worker sends down to its agent `owner`.
    fn down(&mut self, owner: Token, message: &Down) {
        match message {
            temper_worker_domain::agent::channel::Down::StartV2 { .. } => {
                unreachable!("this system world runs version one")
            }

            Down::Start { repositories: _, grants: _, charter, snapshot } => {
                self.start(owner, charter, snapshot.as_deref());
            }
            Down::Event { name, event } => {
                let agent = self.agents.get(&owner).expect("an event goes to an agent spawned");
                let names = agent.attempt.expect("an inbound follows its Start");
                let expected =
                    self.inbound_names.get(&(names, *name)).expect("the referee knows this attempt's named inbound");
                assert!(
                    expected.iter().any(|body| body.as_ref() == event.as_ref()),
                    "the named inbound reaches its attempt unchanged"
                );
                self.stats.events += 1;
                // A person's message reaches a run where its agent hears it.
                let (item, comment) = (protocol::item(names.0), protocol::event_comment(event));
                self.stories.observe(self.now, stories::Seen::Inbound { item, comment }, &mut Vec::new());
                self.stories.assert_holding(self.settings.seed);
            }
            Down::Answer { call, reply } => {
                let agent = self.agents.get_mut(&owner).expect("an answer goes to an agent spawned");
                agent.pushes.remove(&call.raw());
                let attempt = agent.attempt.expect("an agent calls once started");
                match reply {
                    Reply::Relayed { .. } => {
                        let stopped = self.stopped.contains(&attempt);
                        assert!(!stopped, "a cancelled run's relayed calls are answered unavailable");
                        self.stats.relayed += 1;
                    }
                    Reply::Pushed(push) => *self.stats.pushed.entry(push_kind(push)).or_default() += 1,
                    Reply::Unavailable => self.stats.unavailable += 1,
                    Reply::Busy => self.stats.busy += 1,
                    Reply::Withdrawn => self.stats.withdrawn += 1,
                    Reply::TooLarge => {}
                }
            }
            Down::Grant { .. } => {}
            Down::Cancel => {
                let agent = self.agents.get(&owner).expect("a cancel goes to an agent spawned");
                if let Some(attempt) = agent.attempt
                    && self.cancelled.get(&attempt).is_some_and(|first| self.routed >= *first)
                {
                    self.stopped.insert(attempt);
                }
                self.stats.cancels += 1;
            }
        }
    }

    /// The agent `owner` starts on `charter`: for an attempt not answered,
    /// with its snapshot, in the tree each repository was checked out at.
    /// What its words will say is drawn from its charter, as the forge shows
    /// its item now.
    fn start(&mut self, owner: Token, charter: &[u8], snapshot: Option<&[u8]>) {
        let expected: Vec<Names> = self
            .attempts
            .iter()
            .filter_map(|(names, record)| {
                (record.charter.as_ref() == charter
                    && !record.refused
                    && record.answer.is_none()
                    && record.agent.is_none())
                .then_some(*names)
            })
            .collect();
        assert_eq!(expected.len(), 1, "the referee identifies one assigned charter without changing its bytes");
        let names = expected[0];
        let charter = protocol::charter_of(charter);
        let base = if self.settings.release { RELEASE } else { MAIN };
        let content = Content::new(protocol::item(names.0), &charter, snapshot, &self.mirror, base);
        self.tree.plot(owner, content.plot.clone());
        let calls = self.tree.view(owner).fate == script::Fate::Garbage;
        let garbled = Garbled { calls, outcome: self.rng.chance(self.settings.garbled) };
        let agent = self.agents.get_mut(&owner).expect("a start goes to an agent spawned");
        assert_eq!(agent.attempt, None, "an agent starts once");
        agent.attempt = Some(names);
        agent.content = Some(content);
        agent.garbled = garbled;
        let workspace = agent.workspace;
        let record = self.attempts.get_mut(&names).expect("an agent starts for an attempt the worker was given");
        assert!(record.answer.is_none(), "no agent starts for a run answered");
        assert_eq!(record.agent, None, "an attempt starts one agent");
        record.agent = Some(owner);
        assert_eq!(snapshot, record.snapshot.as_deref(), "an agent starts from its attempt's snapshot");
        self.stats.starts += 1;
        if snapshot.is_some() {
            self.stats.resumed += 1;
        }
        let space = self.spaces.get(&workspace).expect("an agent runs in a workspace prepared");
        for repository in &record.repositories {
            let commit =
                *space.checked.get(&repository.name).expect("every repository is checked out before its run starts");
            let files = files(&self.disk, workspace, &repository.name);
            assert_eq!(files, forge_tree(&self.forge, commit), "an agent starts in the tree it was checked out at");
            if let Some(branch) = &repository.saved {
                let key = (repository.remote.clone(), branch.clone());
                let saved = self.saves.get(&key).expect("a run starts from saved work only where a save landed");
                assert_eq!(commit, *saved, "a run started from saved work starts from the last save that landed");
                self.stats.from_saved += 1;
            }
        }
        // Another party may move a push branch while the run works.
        let pushes: Vec<(Vec<u8>, Vec<u8>)> = record
            .repositories
            .iter()
            .filter_map(|repository| Some((repository.remote.clone(), repository.push.clone()?)))
            .collect();
        if !pushes.is_empty() && self.rng.chance(self.settings.git.advance) {
            let index = usize::try_from(self.rng.below(u64::try_from(pushes.len()).expect("fits"))).expect("fits");
            let (remote, branch) = pushes[index].clone();
            let at = self.now.saturating_add(self.settings.git.advance_after.draw(&mut self.rng));
            self.send(at, Delivery::Advance { remote, branch });
        }
    }

    fn tree_take(&mut self, request: agent::Request) {
        let outs = self.tree.take(self.now, request);
        self.tree_outs(outs);
    }

    pub(super) fn tree_outs(&mut self, outs: Vec<tree::Out>) {
        for out in outs {
            match out {
                tree::Out::Domain { after, event } => {
                    let lane = match &event {
                        temper_worker_domain::agent::Event::SpawnV2 { .. }
                        | temper_worker_domain::agent::Event::TurnCredit { .. } => {
                            unreachable!("this system world runs version one")
                        }

                        agent::Event::Received { .. }
                        | agent::Event::Malformed { .. }
                        | agent::Event::Hangup { .. } => Some(Lane::Reads),
                        agent::Event::Exited { .. } | agent::Event::Reaped { .. } => Some(Lane::Exits),
                        agent::Event::Spawned { owner, .. } => {
                            self.agents.get_mut(owner).expect("a spawn is the world's").spawned = true;
                            None
                        }
                        agent::Event::Unspawned { owner, .. } => {
                            self.agents.get_mut(owner).expect("a spawn is the world's").unspawned = true;
                            None
                        }
                        agent::Event::Sent { .. } | agent::Event::Unsent { .. } | agent::Event::Signalled { .. } => {
                            None
                        }
                        agent::Event::Spawn { .. }
                        | agent::Event::Deliver { .. }
                        | agent::Event::Answer { .. }
                        | agent::Event::Stop { .. }
                        | agent::Event::Grant { .. } => unreachable!("io ends requests"),
                    };
                    let at = match lane {
                        Some(lane) => self.lane(lane, after),
                        None => {
                            self.now.saturating_add(after).saturating_add(self.settings.network.hop.draw(&mut self.rng))
                        }
                    };
                    let event = self.content_of(event);
                    self.send(at, Delivery::Worker(self.lives, translate::from_agent_io(event)));
                }
                tree::Out::Due { after, due } => {
                    self.send(self.now.saturating_add(after), Delivery::Tree(self.lives, due));
                }
                tree::Out::Wrote { owner, said } => self.wrote(owner, &said),
            }
        }
    }

    /// What the agent's message says: its relayed calls and its outcome are
    /// its run's, unless they are left as its script wrote them.
    fn content_of(&mut self, event: agent::Event) -> agent::Event {
        let agent::Event::Received { owner, message } = event else {
            return event;
        };
        let Some(agent) = self.agents.get_mut(&owner) else {
            return agent::Event::Received { owner, message };
        };
        let garbled = agent.garbled;
        let Some(content) = agent.content.as_mut() else {
            return agent::Event::Received { owner, message };
        };
        let message = match message {
            temper_worker_domain::agent::channel::Up::Call {
                ask: temper_worker_domain::agent::channel::Ask::PushV2 { .. },
                ..
            } => unreachable!("this system world runs version one"),

            temper_worker_domain::agent::channel::Up::Turn { .. }
            | temper_worker_domain::agent::channel::Up::FinishV2 { .. } => {
                unreachable!("this system world runs version one")
            }

            Up::Call { call, ask: Ask::Relay { .. } } if !garbled.calls => {
                Up::Call { call, ask: Ask::Relay { body: protocol::call(content.call()).into_boxed_slice() } }
            }
            Up::Finish { finish: Finish::Ended { .. } } if !garbled.outcome => {
                let outcome = codec::outcome(&content.outcome(&self.mirror)).into_boxed_slice();
                Up::Finish { finish: Finish::Ended { outcome } }
            }
            Up::Fact { fact } => Up::Fact {
                fact: temper_engine_protocol::payload::encode_fact(
                    engine::views::Kind::Progress,
                    fact,
                    &protocol::SIZES,
                )
                .expect("bounded fixture fact"),
            },
            Up::Call { ask: Ask::Relay { .. } | Ask::Push { .. }, .. }
            | Up::Finish { finish: Finish::Ended { .. } | Finish::Parked { .. } | Finish::Failed { .. } }
            | Up::Withdraw { .. }
            | Up::Long { .. }
            | Up::LongDone
            | Up::Waiting { .. }
            | Up::Rejected { .. }
            | Up::Exhausted { .. } => message,
        };
        agent::Event::Received { owner, message }
    }

    /// The agent `owner` wrote `said`: before it asks to push, and now and
    /// then as it tells a fact, it edits its working trees, while no push of
    /// its own is under way. Before it pushes, a change's run writes what
    /// its run says into the file CI reads.
    fn wrote(&mut self, owner: Token, said: &Said) {
        match said {
            Said::Call { name, push: true, .. } => {
                self.stats.push_calls += 1;
                if self.may_edit(owner) {
                    if self.rng.chance(self.settings.edits) {
                        self.edit(owner);
                    }
                    self.cue(owner);
                }
                let agent = self.agents.get_mut(&owner).expect("an agent that writes was spawned");
                agent.pushes.insert(*name);
                let workspace = agent.workspace;
                let attempt = agent.attempt.expect("an agent writes once started");
                let record = self.attempts.get(&attempt).expect("an attempt started");
                let space = self.spaces.get_mut(&workspace).expect("an agent runs in a workspace prepared");
                for repository in &record.repositories {
                    space.asked.insert(repository.name.clone(), files(&self.disk, workspace, &repository.name));
                }
            }
            Said::Call { push: false, .. } => self.stats.relay_calls += 1,
            Said::Fact { .. } => {
                if self.may_edit(owner) && self.rng.chance(self.settings.scribbles) {
                    self.edit(owner);
                }
            }
            Said::Withdraw { .. } => self.stats.withdraws += 1,
            Said::Long { .. }
            | Said::LongDone
            | Said::Waiting { .. }
            | Said::Ended { .. }
            | Said::Parked { .. }
            | Said::Failed { .. }
            | Said::Garbage => {}
        }
    }

    fn may_edit(&self, owner: Token) -> bool {
        let agent = self.agents.get(&owner).expect("an agent that writes was spawned");
        agent.attempt.is_some() && agent.pushes.is_empty() && !self.busy(agent.workspace)
    }

    /// The agent `owner` writes a file in one of its repositories.
    fn edit(&mut self, owner: Token) {
        let agent = self.agents.get(&owner).expect("an agent that writes was spawned");
        let workspace = agent.workspace;
        let record = self.attempts.get(&agent.attempt.expect("started")).expect("an attempt started");
        let count = u64::try_from(record.repositories.len()).expect("fits");
        let index = usize::try_from(self.rng.below(count)).expect("fits");
        let name = record.repositories[index].name.clone();
        self.edits += 1;
        let file =
            if self.rng.chance(500) { b"README".to_vec() } else { format!("notes/{}", self.edits % 4).into_bytes() };
        let content = format!("edit {}", self.edits);
        self.write(workspace, &name, &file, content.as_bytes());
        self.stats.edits += 1;
    }

    /// A change's agent `owner` writes what its run says into the file CI
    /// reads, in each repository it may push to.
    fn cue(&mut self, owner: Token) {
        let agent = self.agents.get(&owner).expect("an agent that writes was spawned");
        let Some(cue) = agent.content.as_ref().and_then(|content| content.cue.clone()) else { return };
        let workspace = agent.workspace;
        let record = self.attempts.get(&agent.attempt.expect("started")).expect("an attempt started");
        let names: Vec<Vec<u8>> = record
            .repositories
            .iter()
            .filter(|repository| repository.push.is_some())
            .map(|repository| repository.name.clone())
            .collect();
        for name in names {
            self.write(workspace, &name, CUE, &cue);
        }
    }

    /// Writes `content` to `file` of the repository `name` of `workspace`,
    /// and notes what its agent left there.
    fn write(&mut self, workspace: Token, name: &[u8], file: &[u8], content: &[u8]) {
        let at = Place { workspace, repository: name.into() };
        let path = [io::path(&at).as_slice(), b"/", file].concat();
        self.disk.write(&path, content);
        let left = files(&self.disk, workspace, name);
        let space = self.spaces.get_mut(&workspace).expect("an agent runs in a workspace prepared");
        space.left.insert(name.to_vec(), left);
    }

    /// Whether the agent `owner` has gone: it could not be spawned, or its
    /// process tree is empty.
    fn gone(&self, owner: Token) -> bool {
        gone(&self.tree, self.agents.get(&owner).expect("an agent is known until its run answers"))
    }

    /// A new worker starts where the last, shut down, left the disk: cold,
    /// with no run, and dials in. What the world kept of the last is
    /// checked, and forgotten.
    pub(super) fn come_back(&mut self) {
        assert!(self.done, "a worker starts again once the last is done");
        self.assert_worker_settled();
        // What it neither had acknowledged nor listed again, it gave up as it
        // stopped.
        let given_up: Vec<Names> = self.given_up.union(&self.open).copied().collect();
        self.hosting.observe(self.now, referee::Seen::Stopped { given_up }, &mut Vec::new());
        self.hosting.assert_holding(self.settings.seed);
        self.stats.comebacks += 1;
        self.stats.abandoned += self.worker.abandoned();
        self.stats.facts_lost += self.worker.facts_lost();
        self.stats.told_lost += self.worker.told_lost();
        let limits = self.settings.upgrade.unwrap_or(self.settings.worker);
        self.worker = Domain::new(&limits, self.rng.next_u64());
        let max_out = worker::max_out(&limits);
        self.stage = Stage::new(limits, max_out, max_out + super::SPARE);
        self.stage.tick(self.now);
        self.tree = Tree::new(self.settings.tree, self.settings.script, sizes(&limits), self.rng.next_u64());
        self.shut = false;
        self.done = false;
        self.lives += 1;
        self.channel = super::Channel::Idle;
        self.up = false;
        self.down_since = None;
        self.graced = None;
        self.abandoned = 0;
        self.attempts.clear();
        self.open.clear();
        self.given_up.clear();
        self.reached.clear();
        self.following.clear();
        self.cancelled.clear();
        self.stopped.clear();
        self.agents.clear();
        self.spaces.clear();
        self.closed.clear();
        self.routed = 0;
        self.log("a new worker starts");
    }
}

fn push_kind(push: &agent::channel::Push) -> &'static str {
    match push {
        temper_worker_domain::agent::channel::Push::Conflicted { .. } => {
            unreachable!("this system world runs version one")
        }

        agent::channel::Push::Done => "done",
        agent::channel::Push::Moved => "moved",
        agent::channel::Push::Failed { .. } => "failed",
        agent::channel::Push::Nothing => "nothing",
    }
}
