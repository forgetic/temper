//! The worker's neighbours: the engine, over a channel that keeps its order
//! and never drops, with the whole worker's world's translation of the fake
//! engine's api ([`temper_worker_model_tests::translate`]); io's agent
//! processes, each the home of an agent model, and their pipes; and io's git,
//! on the forge and the disk, through the checkout world's translation. And
//! where the worker and the agent meet: what the engine records of each run.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model::run::charter::{Checkout, Repository as Placed};
use temper_agent_model::run::{self, Spend};
use temper_agent_model::{self as agent};
use temper_checkout_fake::git::Move;
use temper_fake_engine_model::{self as engine, BASE, IDENTITY, api};
use temper_lib::{Time, Token};
use temper_worker_model::agent::channel::{Down, Reply, Up};
use temper_worker_model::checkout::git::{Commit, Done, Op, Place, Want};
use temper_worker_model::{self as worker, host};
use temper_worker_model_checkout_tests::translate as io;
use temper_worker_model_tests::translate;
use temper_world::Stage;

use super::{Allowed, Attempt, Delivery, Demands, NAME, Process, Repository, Run, SLACK, World, files};
use crate::channel::{self, Link};
use crate::fixture;
use crate::referee::{Report, Seen};
use crate::script::Job;

impl World {
    /// Sends `event` up the channel to the engine, after what went before it.
    pub(super) fn send_up(&mut self, event: engine::Event) {
        let at = self.now.saturating_add(self.draw(self.settings.network)).max(self.up_lane);
        self.up_lane = at;
        self.schedule(at, Delivery::Engine(event));
    }

    /// Sends the engine's `request` down the channel to the worker, after
    /// what went before it, as the protocol layers translate it.
    pub(super) fn send_down(&mut self, request: engine::Request) {
        match &request {
            engine::Request::Assign { worker, assignment } => {
                assert_eq!(*worker, NAME, "the engine assigns to the one worker there is");
                self.assigned(assignment);
            }
            engine::Request::Cancel { attempt, .. } => self.log(&format!("engine cancels {}", attempt.raw())),
            engine::Request::Inbound { .. } | engine::Request::Relayed { .. } | engine::Request::Acknowledge { .. } => {
            }
        }
        let event = translate::down(request, self.commit, &mut self.places);
        let at = self.now.saturating_add(self.draw(self.settings.network)).max(self.down_lane);
        self.down_lane = at;
        self.schedule(at, Delivery::Worker(event));
    }

    /// Keeps what the world follows of the engine's `assignment`.
    fn assigned(&mut self, assignment: &api::Assignment) {
        let repositories = assignment.workspace.repositories.iter().map(|repository| Repository {
            name: repository.name.to_vec(),
            remote: repository.remote.to_vec(),
            push: match &repository.access {
                api::Access::Writable { push, .. } => Some(push.to_vec()),
                api::Access::ReadOnly => None,
            },
        });
        // The first repository of a job cues the script its guide is read in.
        let job = assignment.workspace.repositories.iter().find_map(|repository| fixture::job(&repository.name));
        let job = job.unwrap_or(Job::Wandering);
        if let Some(save) = &assignment.save {
            self.save_branches.insert(save.to_vec());
        }
        let repositories: Vec<Repository> = repositories.collect();
        self.observe(Seen::Assigned { attempt: assignment.attempt, repositories: repositories.clone() });
        let attempt = Attempt { job, repositories, process: None, answered: false };
        assert!(self.attempts.insert(assignment.attempt, attempt).is_none(), "the engine names its attempts apart");
        self.stats.assigned += 1;
        self.log(&format!("engine assigns {} as {job:?}", assignment.attempt.raw()));
    }

    /// The worker's requests, carried the way its protocol layer and io would.
    pub(super) fn worker_request(&mut self, request: worker::Request) {
        self.log(&format!("worker -> {}", describe_request(&request)));
        match request {
            worker::Request::Dial => {
                assert!(!self.dialled, "the worker dials once: the channel never drops");
                self.dialled = true;
                let at = self.now.saturating_add(self.draw(self.settings.network));
                self.schedule(at, Delivery::Worker(worker::Event::Connected));
            }
            worker::Request::Hello { hello } => {
                self.send_up(engine::Event::Hello { worker: NAME, hello: translate::hello(hello) });
            }
            worker::Request::Answer { run, attempt, answer } => {
                self.reported(attempt, &answer);
                let answer = translate::answer(answer);
                self.send_up(engine::Event::Answered { worker: NAME, run, attempt, answer });
            }
            worker::Request::Relay { .. } => unreachable!("the agent's run relays no calls"),
            worker::Request::Bounced { run, attempt, bounce } => {
                let bounce = translate::bounce(bounce);
                self.send_up(engine::Event::Bounced { worker: NAME, run, attempt, bounce });
            }
            worker::Request::Spawn { owner, workspace, deadline } => self.spawn(owner, workspace, deadline),
            worker::Request::Send { owner, process, message } => {
                let id = self.process_of(owner, process);
                let at = self.now.saturating_add(self.draw(self.settings.pipe));
                self.schedule(at, Delivery::Down { process: id, message });
            }
            worker::Request::Read { owner, process } => {
                let id = self.process_of(owner, process);
                let process = self.processes.get_mut(&id).expect("looked up above");
                assert!(!process.demands.read, "one read at a time");
                process.demands.read = true;
                self.serve_read(id);
            }
            worker::Request::Signal { owner, process, signal: _ } => {
                let id = self.process_of(owner, process);
                if !self.processes[&id].finish_read {
                    self.observe(Seen::Stopped { process: id });
                }
                let at = self.now.saturating_add(self.draw(self.settings.pipe));
                self.schedule(at, Delivery::Signal { process: id });
            }
            worker::Request::Wait { owner, process } => {
                let id = self.process_of(owner, process);
                self.processes.get_mut(&id).expect("looked up above").demands.wait = true;
                self.serve_exit(id);
            }
            worker::Request::Reap { owner, process } => {
                let id = self.process_of(owner, process);
                self.processes.get_mut(&id).expect("looked up above").demands.reap = true;
                self.serve_exit(id);
            }
            worker::Request::Io { owner, op, deadline } => self.start_git(owner, op, deadline),
            // The cancel of an aborted prepare always loses its race here:
            // the operation ends as it would have.
            worker::Request::CancelIo { owner: _ } => self.stats.git_cancels += 1,
        }
    }

    /// io's name for the process the worker's `owner` names `process`.
    fn process_of(&self, owner: Token, process: Token) -> u64 {
        let id = process.raw();
        let found = self.processes.get(&id).expect("the worker names a process io spawned");
        assert_eq!(found.owner, owner, "a process is the agent's io spawned it for");
        id
    }

    /// io spawns an agent process for the worker's `owner` in `workspace`.
    fn spawn(&mut self, owner: Token, workspace: Token, deadline: Time) {
        assert!(self.disk.exists(&io::dir(workspace)), "an agent is spawned in a workspace io has");
        assert!(!self.git.values().any(|op| io::workspace(op) == workspace), "nothing of git runs where it is spawned");
        if let Some(before) = self.spaces.get(&workspace).and_then(|space| space.process) {
            assert!(self.processes[&before].exited.is_some(), "one agent at a time in a workspace");
        }
        let at = self.now.saturating_add(self.draw(self.settings.spawn));
        assert!(at <= deadline, "io spawns within the deadline in this world");
        self.schedule(at, Delivery::Spawned { owner, workspace });
    }

    /// The agent process is spawned: a fresh agent model, sized for one run.
    pub(super) fn spawned(&mut self, owner: Token, workspace: Token) {
        let id = self.wire.name();
        let limits = self.settings.limits;
        let max_out = agent::max_out(&limits);
        let mut stage = Stage::new(limits, max_out, max_out + SLACK);
        stage.tick(self.now);
        let process = Process {
            owner,
            workspace,
            agent: Some(agent::Model::new(&limits, self.agent_rng.next_u64())),
            stage,
            heard: VecDeque::new(),
            link: None,
            run: None,
            facts: VecDeque::new(),
            submitted: 0,
            written: VecDeque::new(),
            demands: Demands::default(),
            finish_read: false,
            exited: None,
            told_exit: None,
            reaped: false,
            lost: 0,
            live: BTreeSet::new(),
            counted: BTreeMap::new(),
        };
        self.processes.insert(id, process);
        self.spaces.entry(workspace).or_default().process = Some(id);
        self.stats.spawns += 1;
        self.log(&format!("io spawns agent {id} in workspace {}", workspace.raw()));
        self.worker_stage.push(worker::Event::Spawned { owner, process: Token::new(id) });
    }

    /// `message` comes down the pipe of `process`: the agent's protocol layer
    /// takes it, unless the agent has exited.
    pub(super) fn heard(&mut self, id: u64, message: Down) {
        let process = &self.processes[&id];
        let owner = process.owner;
        if process.agent.is_none() {
            self.worker_stage.push(worker::Event::Unsent { owner });
            return;
        }
        let message = match message {
            Down::Start { charter, snapshot } => {
                assert!(process.link.is_none(), "the start comes down first, once");
                assert!(snapshot.is_none(), "the engine never parks a run of the agent's, so it never resumes one");
                // The frame is the world's, which the protocol layer takes off.
                let attempt = translate::charter_attempt(&charter);
                let frame = usize::try_from(translate::CHARTER_FRAME).expect("a small frame");
                let charter: Box<[u8]> = charter[frame..].into();
                self.start(id, attempt, &charter);
                Down::Start { charter, snapshot }
            }
            Down::Answer { call, reply } => {
                self.push_answered(id, call, &reply);
                Down::Answer { call, reply }
            }
            message @ (Down::Event { .. } | Down::Cancel) => message,
        };
        self.processes.get_mut(&id).expect("looked up above").heard.push_back(message);
        self.worker_stage.push(worker::Event::Sent { owner });
    }

    /// The agent of `process` starts on the attempt `attempt`'s `charter`: its
    /// protocol layer adds where io put each repository, a root of its own.
    fn start(&mut self, id: u64, attempt: Token, charter: &[u8]) {
        let record = self.attempts.get_mut(&attempt).expect("an agent starts for an attempt the engine made");
        assert!(!record.answered && record.process.is_none(), "an attempt not answered starts one agent");
        record.process = Some(id);
        let job = record.job;
        let named: Vec<(Vec<u8>, bool)> =
            record.repositories.iter().map(|repository| (repository.name.clone(), repository.push.is_some())).collect();
        let first_writable = record
            .repositories
            .iter()
            .find_map(|repository| Some((repository.remote.clone(), repository.push.clone()?)));
        self.observe(Seen::Started { process: id, attempt });
        let workspace = self.processes[&id].workspace;
        let mut placed = Vec::new();
        for (name, writable) in named {
            let at = Place { workspace, repository: name.clone().into() };
            let root = self.disk.root(&io::path(&at));
            self.roots.insert(root, id);
            placed.push(Placed { name: name.into(), root: Token::new(root), writable });
        }
        let checkout = Checkout { repositories: placed.into() };
        let decoded = channel::charter(charter, checkout.clone());
        let writable = checkout.repositories.iter().any(|repository| repository.writable);
        let change = decoded.outcome.change.as_ref();
        let allowed = Allowed {
            change: change.is_some(),
            checks: change.is_some_and(|spec| spec.checks) && writable,
            verdicts: !decoded.outcome.verdicts.is_empty(),
        };
        let run = Run {
            job,
            attempt,
            allowed,
            budget: decoded.budget,
            run: None,
            cancelled: false,
            found: false,
            checked: Vec::new(),
            pushes: Vec::new(),
            answer: None,
            started: None,
            answered: None,
            used: Spend::ZERO,
            crossed: None,
            widest: 0,
            killed: false,
            reported: None,
            landed: Vec::new(),
        };
        let process = self.processes.get_mut(&id).expect("an agent starts once spawned");
        process.link = Some(Link { worker: Token::new(id), checkout, run: None });
        process.run = Some(run);
        self.stats.starts += 1;
        self.log(&format!("agent {id} starts attempt {} as {job:?}", attempt.raw()));
        // Another party may move the branch the run pushes to while it works.
        if let Some((remote, branch)) = first_writable
            && self.rng.chance(self.settings.moved)
        {
            let at = self.now.saturating_add(self.draw(self.settings.move_after));
            self.schedule(at, Delivery::Advance { remote, branch });
        }
    }

    /// The worker answers the push `call` of the agent of `process` with
    /// `reply`, which the run hears as its push's end.
    fn push_answered(&mut self, id: u64, call: Token, reply: &Reply) {
        self.pushes.end((id, call));
        let push = match reply {
            Reply::Pushed(push) => channel::push(*push),
            Reply::Unavailable | Reply::Busy | Reply::TooLarge => run::Push::Failed,
            Reply::Withdrawn => {
                self.stats.pushes_cancelled += 1;
                return;
            }
            Reply::Relayed { .. } => unreachable!("the run relays no calls"),
        };
        match push {
            run::Push::Done => self.stats.pushed += 1,
            run::Push::Moved => self.stats.moved += 1,
            run::Push::Failed => self.stats.unpushed += 1,
        }
        let run = self.processes.get_mut(&id).and_then(|process| process.run.as_mut());
        run.expect("a run pushes once started").pushes.push(push);
    }

    /// The agent of `process` writes `up` up its pipe.
    pub(super) fn write_up(&mut self, id: u64, up: Up) {
        let at = self.now.saturating_add(self.draw(self.settings.pipe));
        self.processes.get_mut(&id).expect("an agent writes once spawned").written.push_back((at, up));
        self.serve_read(id);
    }

    /// Ends the worker's read of `process`, if it waits: with the next message
    /// once it is through the pipe, or with the pipe's end once the agent has
    /// exited and everything it wrote has been read.
    fn serve_read(&mut self, id: u64) {
        let now = self.now;
        let process = self.processes.get_mut(&id).expect("io reads a process it spawned");
        if !process.demands.read {
            return;
        }
        let owner = process.owner;
        if let Some((at, message)) = process.written.pop_front() {
            process.demands.read = false;
            // What is through the pipe already is read at once, as io reads
            // a pipe that holds data.
            if at <= now {
                self.read_up(id, message);
            } else {
                self.schedule(at, Delivery::Read { process: id, message });
            }
        } else if process.exited.is_some() {
            process.demands.read = false;
            let at = now.saturating_add(self.draw(self.settings.pipe));
            self.schedule(at, Delivery::Worker(worker::Event::Hangup { owner }));
        }
    }

    /// The worker reads `message` from the pipe of `process`.
    pub(super) fn read_up(&mut self, id: u64, message: Up) {
        let process = self.processes.get_mut(&id).expect("io reads a process it spawned");
        process.finish_read |= is_finish(&message);
        let owner = process.owner;
        self.worker_stage.push(worker::Event::Received { owner, message });
    }

    /// Ends the worker's wait for `process` to exit, once it has, and then its
    /// reap: the tree is empty as the agent exits, as the commands of its
    /// tools die with it.
    fn serve_exit(&mut self, id: u64) {
        let now = self.now;
        let process = self.processes.get_mut(&id).expect("io waits for a process it spawned");
        let owner = process.owner;
        if process.demands.wait
            && let Some(exited) = process.exited
        {
            process.demands.wait = false;
            let at = exited.max(now).saturating_add(self.settings.pipe.draw(&mut self.rng));
            process.told_exit = Some(at);
            self.schedule(at, Delivery::Worker(worker::Event::Exited { owner }));
        }
        let process = self.processes.get_mut(&id).expect("looked up above");
        if process.demands.reap
            && let Some(told) = process.told_exit
        {
            process.demands.reap = false;
            process.reaped = true;
            let at = told.max(now).saturating_add(self.settings.pipe.draw(&mut self.rng));
            self.schedule(at, Delivery::Worker(worker::Event::Reaped { owner, detail: Box::new([]) }));
        }
    }

    /// A signal reaches the tree of `process`: a terminate or a kill ends its
    /// agent, if it still runs.
    pub(super) fn signalled(&mut self, id: u64) {
        let process = &self.processes[&id];
        let owner = process.owner;
        if process.agent.is_some() {
            self.stats.kills += 1;
            self.end(id, true);
        }
        self.worker_stage.push(worker::Event::Signalled { owner });
    }

    /// The agent of `process` exits: of itself once its run has answered, or
    /// `killed` by a signal, which abandons what it had in flight. What it
    /// wrote is still read, up to the pipe's end.
    pub(super) fn end(&mut self, id: u64, killed: bool) {
        if killed {
            self.abandon(id);
        } else {
            self.assert_quiet(id);
            self.stats.exits += 1;
        }
        let now = self.now;
        let process = self.processes.get_mut(&id).expect("a process ends once spawned");
        let agent = process.agent.take().expect("a process ends once");
        process.lost = agent.facts_lost();
        process.stage.inbox.clear();
        process.heard.clear();
        assert!(process.stage.out.is_empty() && process.facts.is_empty(), "everything it asked for was submitted");
        process.exited = Some(now);
        if let Some(run) = &mut process.run {
            run.killed = killed && run.answer.is_none();
        }
        let workspace = process.workspace;
        let names: Vec<Vec<u8>> = process
            .link
            .iter()
            .flat_map(|link| link.checkout.repositories.iter().map(|repository| repository.name.to_vec()))
            .collect();
        let left = names.into_iter().map(|name| {
            let files = files(&self.disk, workspace, &name);
            (name, files)
        });
        self.observe(Seen::Gone { process: id, left: left.collect() });
        self.log(&format!("agent {id} {}", if killed { "is killed" } else { "exits" }));
        self.serve_read(id);
        self.serve_exit(id);
    }

    /// io starts the git operation `op` for the worker's `owner`.
    fn start_git(&mut self, owner: Token, op: Op, deadline: Time) {
        let limits = self.settings.worker.checkout;
        let timeout = if op.is_remote() { limits.remote_timeout } else { limits.local_timeout };
        assert_eq!(deadline, self.now.saturating_add(timeout), "an operation's deadline is by where it runs");
        if let Some(identity) = io::identity(&op) {
            assert_eq!(identity, IDENTITY, "an operation acts as its repository's identity");
        }
        let workspace = io::workspace(&op);
        if let Some(id) = self.spaces.get(&workspace).and_then(|space| space.process)
            && self.processes[&id].exited.is_none()
        {
            let pushing = self.pushing(id);
            let part = match &op {
                Op::Commit { .. } | Op::Fetch { want: Want::Branch { .. }, .. } => pushing,
                Op::Push { branch, .. } => pushing && !self.save_branches.contains(&**branch),
                Op::Make { .. } | Op::Clone { .. } | Op::Fetch { .. } | Op::Create { .. } | Op::CheckOut { .. } => {
                    false
                }
            };
            assert!(
                part,
                "no git operation touches a workspace while its agent runs, but the push it asked for: {op:?}"
            );
        }
        let at = self.now.saturating_add(self.draw(self.settings.git));
        assert!(at <= deadline, "git ends within its deadline in this world");
        self.git.open(owner, op);
        self.schedule(at, Delivery::Git { owner });
        self.stats.git += 1;
    }

    /// The git operation of `owner` ends, run on the forge and the disk.
    pub(super) fn ran_git(&mut self, owner: Token) {
        let op = self.git.end(owner);
        let workspace = io::workspace(&op);
        let committed = match &op {
            Op::Commit { at, .. } => Some(at.repository.to_vec()),
            Op::Make { .. }
            | Op::Clone { .. }
            | Op::Fetch { .. }
            | Op::Create { .. }
            | Op::CheckOut { .. }
            | Op::Push { .. } => None,
        };
        let pushed = match &op {
            Op::Push { branch, .. } => Some(branch.to_vec()),
            Op::Make { .. }
            | Op::Clone { .. }
            | Op::Fetch { .. }
            | Op::Create { .. }
            | Op::CheckOut { .. }
            | Op::Commit { .. } => None,
        };
        let before = self.forge.moves().len();
        let done = io::perform(&mut self.forge, &mut self.disk, op);
        let moved = self.forge.moves()[before..].to_vec();
        for Move { remote, branch, from, to } in moved {
            if let Some(from) = from {
                assert!(
                    self.forge.is_ancestor(from, to),
                    "{}: {} moved only by a fast-forward",
                    String::from_utf8_lossy(&remote),
                    String::from_utf8_lossy(&branch)
                );
            }
            let tree = self.forge.object(to).tree.clone();
            self.observe(Seen::Moved { remote, branch, commit: to, tree });
        }
        if let Some(repository) = committed
            && let Done::Committed { commit } = &done
        {
            let id = self.spaces.get(&workspace).and_then(|space| space.process).expect("a change is an agent's");
            let tree = self.forge.object(io::fake(*commit)).tree.clone();
            self.observe(Seen::Committed { process: id, repository, tree });
        }
        if let Some(branch) = pushed
            && done == Done::Succeeded
        {
            if self.save_branches.contains(&branch) {
                self.stats.saves += 1;
            } else {
                self.stats.pushes_landed += 1;
            }
        }
        self.worker_stage.push(worker::Event::Done { owner, done });
    }

    /// Another party moves `branch` of `remote`, making it first if it is
    /// nowhere yet, from the base branch, unless the forge refuses.
    pub(super) fn advance(&mut self, remote: &[u8], branch: &[u8]) {
        if self.forge.branch(remote, branch).is_none() {
            let base = self.forge.branch(remote, BASE).expect("every repository has the base branch");
            if self.forge.create(remote, branch, base).is_err() {
                return;
            }
        }
        self.stats.advanced += 1;
        let content = format!("another party, {}\n", self.stats.advanced);
        self.forge.advance(remote, branch, b"OTHER.md", content.as_bytes());
        self.log(&format!("another party moves {}", String::from_utf8_lossy(branch)));
    }

    /// The worker answers the engine for `attempt`: the engine hears it, and
    /// the referee judges it where the worker and the agent meet.
    fn reported(&mut self, attempt: Token, answer: &host::Answer) {
        let kind = translate::answer_kind(answer);
        self.log(&format!("worker answers {}: {kind}", attempt.raw()));
        let record = self.attempts.get_mut(&attempt).expect("the worker answers attempts the engine made");
        assert!(!record.answered, "the worker answers each attempt once");
        record.answered = true;
        let process = record.process;
        self.stats.reported += 1;
        let (landed, ends): (&[host::Landed], bool) = match answer {
            host::Answer::Refused(_) => (&[], false),
            host::Answer::Ended { work, .. } => (&work.landed, true),
            host::Answer::Parked { work, .. } | host::Answer::Failed { work, .. } => (&work.landed, false),
        };
        self.stats.landed += u32::try_from(landed.len()).expect("a few repositories");
        self.stats.ended += u32::from(ends);
        let unprepared = match answer {
            host::Answer::Failed { failure, .. } => match failure {
                host::Failure::Unprepared(_) => true,
                host::Failure::Run(_) | host::Failure::Agent(_) | host::Failure::Cancelled(_) => false,
            },
            host::Answer::Refused(_) | host::Answer::Ended { .. } | host::Answer::Parked { .. } => false,
        };
        self.stats.unprepared += u32::from(unprepared);
        let landed: Vec<(usize, u64)> = landed
            .iter()
            .map(|landed| {
                let index = usize::try_from(landed.repository).expect("a small place");
                (index, io::fake(Commit::new(landed.commit)))
            })
            .collect();
        let commits = landed.iter().map(|(_, commit)| *commit).collect();
        self.observe(Seen::Reported { attempt, kind, report: Report::of(answer), landed });
        if let Some(id) = process {
            let run = self.processes.get_mut(&id).and_then(|process| process.run.as_mut());
            let run = run.expect("an agent started carries a run");
            run.reported = Some(kind);
            run.landed = commits;
        }
    }
}

/// Whether `message` says how the run finishes.
fn is_finish(message: &Up) -> bool {
    match message {
        Up::Finish { .. } => true,
        Up::Call { .. }
        | Up::Withdraw { .. }
        | Up::Fact { .. }
        | Up::Long { .. }
        | Up::LongDone
        | Up::Waiting { .. } => false,
    }
}

/// The worker's `event`, for the trace.
pub(super) fn describe_event(event: &worker::Event) -> String {
    match event {
        worker::Event::Connected => "connected".to_owned(),
        worker::Event::Lost => "lost".to_owned(),
        worker::Event::Assign { assignment } => format!("assign {}", assignment.attempt.raw()),
        worker::Event::Inbound { attempt, .. } => format!("inbound for {}", attempt.raw()),
        worker::Event::Cancel { attempt, .. } => format!("cancel {}", attempt.raw()),
        worker::Event::Relayed { attempt, call, .. } => format!("relayed {} for {}", call.raw(), attempt.raw()),
        worker::Event::Acknowledged { attempt, .. } => format!("acknowledged {}", attempt.raw()),
        worker::Event::Shutdown => "shutdown".to_owned(),
        worker::Event::Spawned { owner, process } => format!("spawned {} as {}", owner.raw(), process.raw()),
        worker::Event::Unspawned { owner, .. } => format!("unspawned {}", owner.raw()),
        worker::Event::Sent { owner } => format!("sent {}", owner.raw()),
        worker::Event::Unsent { owner } => format!("unsent {}", owner.raw()),
        worker::Event::Received { owner, message } => format!("received {} {}", owner.raw(), describe_up(message)),
        worker::Event::Malformed { owner } => format!("malformed {}", owner.raw()),
        worker::Event::Hangup { owner } => format!("hangup {}", owner.raw()),
        worker::Event::Signalled { owner } => format!("signalled {}", owner.raw()),
        worker::Event::Exited { owner } => format!("exited {}", owner.raw()),
        worker::Event::Reaped { owner, .. } => format!("reaped {}", owner.raw()),
        worker::Event::Done { owner, done } => format!("done {} {done:?}", owner.raw()),
    }
}

fn describe_request(request: &worker::Request) -> String {
    match request {
        worker::Request::Dial => "dial".to_owned(),
        worker::Request::Hello { hello } => format!("hello hosting {}", hello.hosting.len()),
        worker::Request::Answer { attempt, answer, .. } => {
            format!("answer {} {}", attempt.raw(), translate::answer_kind(answer))
        }
        worker::Request::Relay { attempt, call, .. } => format!("relay {} for {}", call.raw(), attempt.raw()),
        worker::Request::Bounced { attempt, bounce, .. } => format!("bounced {bounce:?} for {}", attempt.raw()),
        worker::Request::Spawn { owner, workspace, .. } => format!("spawn {} in {}", owner.raw(), workspace.raw()),
        worker::Request::Send { owner, message, .. } => format!("send {} {}", owner.raw(), describe_down(message)),
        worker::Request::Read { owner, .. } => format!("read {}", owner.raw()),
        worker::Request::Signal { owner, signal, .. } => format!("signal {} {signal:?}", owner.raw()),
        worker::Request::Wait { owner, .. } => format!("wait {}", owner.raw()),
        worker::Request::Reap { owner, .. } => format!("reap {}", owner.raw()),
        worker::Request::Io { owner, op, .. } => format!("git {} {:?}", owner.raw(), op.kind()),
        worker::Request::CancelIo { owner } => format!("cancel git {}", owner.raw()),
    }
}

fn describe_down(message: &Down) -> String {
    match message {
        Down::Start { charter, .. } => format!("start, {} bytes", charter.len()),
        Down::Event { event } => format!("event, {} bytes", event.len()),
        Down::Answer { call, reply } => format!("answer {} {reply:?}", call.raw()),
        Down::Cancel => "cancel".to_owned(),
    }
}

fn describe_up(message: &Up) -> String {
    match message {
        Up::Call { call, .. } => format!("call {}", call.raw()),
        Up::Withdraw { call } => format!("withdraw {}", call.raw()),
        Up::Fact { fact } => format!("fact {}", String::from_utf8_lossy(fact)),
        Up::Long { span } => format!("long {span:?}"),
        Up::LongDone => "long done".to_owned(),
        Up::Waiting { heard } => format!("waiting, {heard} heard"),
        Up::Finish { finish } => match finish {
            worker::agent::channel::Finish::Ended { outcome } => format!("ended, {} bytes", outcome.len()),
            worker::agent::channel::Finish::Parked { .. } => "parked".to_owned(),
            worker::agent::channel::Finish::Failed { failure } => format!("failed {failure:?}"),
        },
    }
}
