//! The agent processes, each a fresh agent domain driven as its process's
//! shell would, and what lies beneath each: its protocol layer toward the
//! worker ([`crate::channel`]), the LLM provider, and io, running the tools'
//! file operations and commands and the run's own looks and checks on the
//! disk, in the working trees the worker prepared.

use skein_lib::{Duration, ReplyTo, Time, Token};
use temper_agent_domain::run::facts::{self as run_facts, Return};
use temper_agent_domain::run::outcome::{Child, Declared, Field, Verdict};
use temper_agent_domain::run::{self, Spend};
use temper_agent_domain::tools::{Done, Fault, Op};
use temper_agent_domain::{self as agent, Event, Fact, Request, session, tools};
use temper_agent_domain_tools_tests::translate as io;
use temper_checkout_fake as fake;
use temper_llm_domain as provider;

use super::{Call, Checking, Checks, Delivery, Owner, Pending, Process, Work, World, answer_kind, files};
use crate::channel::{self, Toward};
use crate::referee::Seen;
use crate::script;
use crate::{fixture, translate};

impl Process {
    /// Steps the agent with `event`, keeping what it tells in its place,
    /// before what the step asks for.
    fn step(&mut self, event: Event) {
        let before = self.submitted + u64::from(self.stage.out.len());
        let domain = self.agent.as_mut().expect("a process that runs has its agent");
        agent::step(domain, &self.stage.env, event, &mut self.stage.out);
        while let Some(fact) = domain.pop_fact() {
            self.facts.push_back((before, fact));
        }
    }

    fn resume(&mut self) {
        let before = self.submitted + u64::from(self.stage.out.len());
        let domain = self.agent.as_mut().expect("a process that runs has its agent");
        agent::resume(domain, &self.stage.env, &mut self.stage.out);
        while let Some(fact) = domain.pop_fact() {
            self.facts.push_back((before, fact));
        }
    }

    fn fire(&mut self) {
        let before = self.submitted + u64::from(self.stage.out.len());
        let domain = self.agent.as_mut().expect("a process that runs has its agent");
        agent::fire(domain, &self.stage.env, &mut self.stage.out);
        while let Some(fact) = domain.pop_fact() {
            self.facts.push_back((before, fact));
        }
    }

    fn is_ready(&self) -> bool {
        self.agent.as_ref().is_some_and(agent::Domain::is_ready)
    }

    fn is_due(&self, now: Time) -> bool {
        self.agent.as_ref().is_some_and(|agent| agent.is_due(now))
    }

    /// Notes when the start reached the agent, and a cancel its run.
    fn heard_event(&mut self, event: &Event, now: Time, cancels: &mut u32) {
        match event {
            Event::Start { .. } => self.run.as_mut().expect("a run starts once known").started = Some(now),
            Event::Cancel { .. } => {
                self.run.as_mut().expect("a cancel is for a run started").cancelled = true;
                *cancels += 1;
            }
            Event::Pushed { .. }
            | Event::HostCancelled { .. }
            | Event::Completed { .. }
            | Event::Failed { .. }
            | Event::Cancelled { .. }
            | Event::Done { .. }
            | Event::Read { .. }
            | Event::Probed { .. }
            | Event::Checked { .. }
            | Event::Aborted { .. } => {}
        }
    }
}

impl World {
    /// The agent of `process` takes what came down its channel, then what its
    /// neighbours sent, then its alarms, as its shell would, while it has room.
    pub(super) fn drive(&mut self, id: u64) {
        let now = self.now;
        let World { processes, trace, stats, .. } = self;
        let process = processes.get_mut(&id).expect("a process driven was spawned");
        while process.stage.has_room() && process.is_ready() {
            trace.log(now, format!("agent {id} ready"));
            process.resume();
        }
        // The protocol layer reads the channel's messages in order, each once
        // the agent has taken the one before.
        while process.stage.has_room()
            && let Some(down) = process.heard.pop_front()
        {
            let link = process.link.as_ref().expect("the start comes down first");
            let Some(event) = channel::down(down, link) else {
                continue;
            };
            trace.log(now, format!("agent {id} <- {}", describe_event(&event)));
            process.heard_event(&event, now, &mut stats.cancels);
            let start = is_start(&event);
            process.step(event);
            // The run's name, which the start's step gives, addresses a
            // cancel that comes down after it.
            if start {
                let admitted = process.stage.out.iter().find_map(admitted);
                process.link.as_mut().expect("known above").run = admitted;
            }
        }
        while let Some(event) = process.stage.next_event() {
            trace.log(now, format!("agent {id} <- {}", describe_event(&event)));
            process.heard_event(&event, now, &mut stats.cancels);
            process.step(event);
        }
        while process.stage.has_room() && process.is_due(now) {
            trace.log(now, format!("agent {id} alarm"));
            process.fire();
        }
    }

    /// What the agent of `process` asked for and told in this iteration, in
    /// order: each step's facts go up before its requests.
    pub(super) fn submit(&mut self, id: u64) {
        loop {
            let process = self.processes.get_mut(&id).expect("a process submits once spawned");
            if let Some(&(before, _)) = process.facts.front()
                && before <= process.submitted
            {
                let (_, fact) = process.facts.pop_front().expect("looked at above");
                self.tell(id, fact);
                continue;
            }
            let Some(request) = process.stage.out.pop() else {
                break;
            };
            process.submitted += 1;
            self.agent_request(id, request);
        }
        while let Some((_, fact)) = self.process_mut(id).facts.pop_front() {
            self.tell(id, fact);
        }
    }

    /// An event for the agent of `process`, unless it has exited.
    pub(super) fn agent_event(&mut self, id: u64, event: Event) {
        let process = self.process_mut(id);
        if process.agent.is_some() {
            process.stage.push(event);
        }
    }

    fn process_mut(&mut self, id: u64) -> &mut Process {
        self.processes.get_mut(&id).expect("a process io spawned")
    }

    /// The agent's requests, carried out the way its protocol layer and its
    /// neighbours would.
    fn agent_request(&mut self, id: u64, request: Request) {
        self.log(&format!("agent {id} -> {}", describe_request(&request)));
        let link = self.processes[&id].link.as_ref().expect("an agent asks once started").worker;
        match &request {
            Request::Admitted { worker, run } => {
                assert_eq!(*worker, link, "the agent admits the run it was started with");
                let state = self.run_of(id);
                assert!(state.run.is_none() && state.answer.is_none(), "a run is admitted once, before its answer");
                state.run = Some(*run);
                self.stats.admitted += 1;
            }
            Request::Answer { to, answer } => {
                assert_eq!(*to, ReplyTo::new(link), "the run answers whoever started it");
                self.answered(id, answer);
                self.observe(Seen::Answered { process: id, answer: copy(answer) });
                let now = self.now;
                let state = self.run_of(id);
                (state.answer, state.answered) = (Some(copy(answer)), Some(now));
                self.stats.answers += 1;
                // Its last word: the process exits next.
                let at = self.now.saturating_add(self.draw(self.settings.pipe));
                self.schedule(at, Delivery::Exit { process: id });
            }
            Request::Checking { worker, .. } => {
                assert_eq!(*worker, link, "checks run for the run started");
                assert!(self.run_of(id).answer.is_none(), "checks run for a run that has not answered");
                self.stats.checking += 1;
            }
            Request::Push { worker, owner, change } => {
                assert_eq!(*worker, link, "a run pushes to whoever started it");
                let passed = self.passed.contains(&(id, *owner));
                let state = self.run_of(id);
                assert!(state.answer.is_none(), "a run pushes before it answers");
                assert!(state.allowed.change, "a run pushes only a change its charter allows");
                assert_eq!(&*change.title, script::TITLE, "the change pushed is the one the LLM declared");
                if state.found {
                    assert!(state.allowed.checks, "a run looks for checks only if its change must pass them");
                    assert!(passed, "a change is pushed only once its checks passed");
                }
                self.pushes.open((id, *owner), ());
                self.stats.pushes += 1;
                // What the worker is to commit: the tree each repository it
                // may write holds now.
                let process = &self.processes[&id];
                let workspace = process.workspace;
                let link = process.link.as_ref().expect("checked above");
                let trees = link
                    .checkout
                    .repositories
                    .iter()
                    .filter(|repository| repository.writable)
                    .map(|repository| (repository.name.to_vec(), files(&self.disk, workspace, &repository.name)))
                    .collect();
                self.observe(Seen::Asked { process: id, trees });
            }
            Request::CancelHost { .. } => self.stats.host_cancels += 1,
            Request::Complete { .. }
            | Request::Cancel { .. }
            | Request::Io { .. }
            | Request::CancelIo { .. }
            | Request::Read { .. }
            | Request::Probe { .. }
            | Request::Check { .. }
            | Request::Abort { .. } => {}
        }
        match channel::up(request, self.now) {
            Toward::Worker(up) => self.write_up(id, up),
            Toward::Admitted { run } => {
                let link = self.processes[&id].link.as_ref().expect("checked above");
                assert_eq!(link.run, Some(run), "the protocol layer keeps the run's name as its start gives it");
            }
            Toward::Below(request) => self.below(id, request),
        }
    }

    /// The run of the agent of `process`.
    fn run_of(&mut self, id: u64) -> &mut super::Run {
        self.process_mut(id).run.as_mut().expect("an agent at work carries a run")
    }

    /// What the agent of `process` asks of the provider and of io.
    fn below(&mut self, id: u64, request: Request) {
        match request {
            Request::Complete { owner, prompt, timeout } => {
                let call = self.wire.name();
                let deadline = self.schedule(self.now.saturating_add(timeout), Delivery::Deadline { call });
                self.calls_out.open(call, Call { owner: (id, owner), deadline });
                assert!(self.calling.insert((id, owner), call).is_none(), "a session has one call in flight");
                offers(&prompt);
                self.count_results(id, owner, &prompt);
                let query = translate::query(prompt);
                self.send(Delivery::Query { call, query });
                self.stats.calls += 1;
            }
            Request::Cancel { owner } => {
                // A call that has already ended has its terminal event on the
                // way: the cancel lost the race and changes nothing. One still
                // in flight may end of itself all the same.
                let Some(&call) = self.calling.get(&(id, owner)) else {
                    self.stats.cancels_crossed += 1;
                    return;
                };
                if self.rng.chance(self.settings.cancels_lost) {
                    self.cancel_lost.insert((id, owner));
                    self.stats.cancels_lost += 1;
                } else {
                    self.end_call(call);
                    self.stats.cancelled += 1;
                    self.send(Delivery::Agent { process: id, event: Event::Cancelled { owner } });
                }
            }
            Request::Io { owner, op, deadline } => self.start_op((id, owner), op, deadline),
            Request::CancelIo { owner } => self.cancel_op((id, owner)),
            Request::Read { owner, at, max, deadline } => self.read(id, owner, &at, max, deadline),
            Request::Probe { owner, at, deadline } => self.probe(id, owner, &at, deadline),
            Request::Check { owner, program, deadline, tail } => self.check(id, owner, &program, deadline, tail),
            Request::Abort { owner } => self.abort((id, owner)),
            request @ (Request::Admitted { .. }
            | Request::Answer { .. }
            | Request::Checking { .. }
            | Request::Push { .. }
            | Request::CancelHost { .. }) => unreachable!("the worker's, not io's: {request:?}"),
        }
    }

    /// Checks the answer of the agent of `process` against what happened to
    /// its run.
    fn answered(&mut self, id: u64, answer: &run::Answer) {
        let lost = self.processes[&id].agent.as_ref().map_or(0, agent::Domain::facts_lost);
        let live = u32::try_from(self.processes[&id].live.len()).expect("a few conversations");
        let pushing = self.pushing(id);
        let state = self.processes[&id].run.as_ref().expect("an agent answers the run it carries");
        assert!(state.answer.is_none(), "one answer per start");
        if state.run.is_some() && lost == 0 {
            assert_eq!(live, 0, "a run answers once its conversations have all ended");
        }
        assert!(!pushing, "a run answers once its pushes have ended");
        let spent = match answer {
            run::Answer::Refused(_) => {
                assert!(state.used == Spend::ZERO, "a refused run spends nothing");
                return;
            }
            run::Answer::Accepted { outcome: Declared::Change(change), spent } => {
                assert!(state.allowed.change, "a change is accepted only if the charter allows one");
                assert_eq!(state.pushes.last(), Some(&run::Push::Done), "a change is accepted once it is pushed");
                assert_eq!(&*change.title, script::TITLE, "the change accepted is the one the LLM declared");
                spent
            }
            run::Answer::Accepted { outcome: Declared::Verdict(_), spent } => {
                assert!(state.allowed.verdicts, "a verdict is accepted only if the charter allows one");
                assert!(state.pushes.is_empty(), "a verdict pushes nothing");
                spent
            }
            run::Answer::Failed { failure, spent } => {
                match failure {
                    run::Failure::Stale => {
                        assert!(state.pushes.contains(&run::Push::Moved), "a run is stale once a push found it so");
                    }
                    run::Failure::Cancelled => assert!(state.cancelled, "a run is cancelled only by the worker"),
                    run::Failure::Model(_) | run::Failure::Budget(_) | run::Failure::Policy(_) => {}
                }
                spent
            }
        };
        if lost == 0 {
            assert_eq!(*spent, state.used, "a run's answer adds up what its conversations used");
            if let Some((live, after)) = state.crossed {
                assert!(after <= live + 1, "past its budget, a run's conversations finish what they had started");
            }
        }
    }

    /// Whether the agent of `process` has a push in flight.
    pub(super) fn pushing(&self, id: u64) -> bool {
        self.pushes.keys().any(|(process, _)| *process == id)
    }

    /// io reads the file at `at` for a run, its first `max` bytes as text.
    fn read(&mut self, id: u64, owner: Token, at: &run::Place, max: u32, deadline: Time) {
        let (ends, found) = self.look(id, at, deadline);
        let read = match found {
            Some(Ok(content)) => text(&content, max),
            Some(Err(fake::Failure::Missing | fake::Failure::NotFile | fake::Failure::NotDirectory)) => {
                run::Read::Missing
            }
            Some(Err(_)) | None => run::Read::Failed,
        };
        self.stats.reads += 1;
        self.looked((id, owner), ends, Event::Read { owner, read });
    }

    /// io finds out for a run whether an executable is at `at`.
    fn probe(&mut self, id: u64, owner: Token, at: &run::Place, deadline: Time) {
        let (ends, found) = self.look(id, at, deadline);
        let executable = match found {
            Some(Ok(content)) => fixture::executable(&content),
            Some(Err(_)) | None => false,
        };
        self.stats.probes += 1;
        self.run_of(id).found |= executable;
        self.looked((id, owner), ends, Event::Probed { owner, executable });
    }

    /// io runs a run's checks, `program`, to end by `deadline`: after a draw,
    /// on the working tree as it is then, or stopped at the deadline.
    fn check(&mut self, id: u64, owner: Token, program: &run::Place, deadline: Time, tail: u32) {
        let root = program.root.raw();
        assert_eq!(self.roots.get(&root), Some(&id), "checks run in the run's checkout");
        assert!(self.run_of(id).answer.is_none(), "checks run for a run that has not answered");
        assert_eq!(&*program.path, fixture::CHECKS, "the run runs the checks it probed for");
        let owner = (id, owner);
        self.passed.remove(&owner);
        let ends = self.now.saturating_add(self.draw(self.settings.check));
        let (at, work) = if ends > deadline {
            let ran = run::Ran { exit: run::Exit::TimedOut, output: Box::new([]), cut: 0 };
            let noticed = deadline.saturating_add(self.draw(self.settings.network));
            self.stats.check_timeouts += 1;
            (noticed, Checks::Ending(Event::Checked { owner: owner.1, ran }))
        } else {
            (ends, Checks::Running { root, tail })
        };
        let delivery = self.schedule(at, Delivery::Checked { owner });
        self.checks.open(owner, Checking { delivery, work });
        self.stats.checks += 1;
    }

    /// io is asked to stop the checks of `owner`: they end aborted after a
    /// network draw, unless they end of themselves first.
    fn abort(&mut self, owner: Owner) {
        let Some(checking) = self.checks.get(owner) else {
            self.stats.cancels_crossed += 1;
            return;
        };
        let ending = match checking.work {
            Checks::Running { .. } => false,
            Checks::Ending(_) => true,
        };
        if ending || self.rng.chance(self.settings.cancels_lost) {
            self.abort_lost.insert(owner);
            self.stats.cancels_lost += 1;
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.network));
        let delivery = self.schedule(at, Delivery::Checked { owner });
        let checking = self.checks.get_mut(owner).expect("looked up above");
        self.wire.withdraw(checking.delivery).expect("checks in flight have their end on the way");
        (checking.delivery, checking.work) = (delivery, Checks::Ending(Event::Aborted { owner: owner.1 }));
        self.stats.aborts += 1;
    }

    /// io looks at `at` for the run of `process`, by `deadline`: when it ends,
    /// and the content of the file there or why there is none; or nothing, if
    /// io failed, or the deadline passed first (which io says a moment after).
    fn look(&mut self, id: u64, at: &run::Place, deadline: Time) -> (Time, Option<Result<Vec<u8>, fake::Failure>>) {
        let root = at.root.raw();
        assert_eq!(self.roots.get(&root), Some(&id), "a run looks in its own checkout");
        assert!(self.run_of(id).answer.is_none(), "a run looks in its checkout before it answers");
        let ends = self.now.saturating_add(self.draw(self.settings.look));
        if ends > deadline {
            return (deadline.saturating_add(self.draw(self.settings.network)), None);
        }
        if self.rng.chance(self.settings.io_errors) {
            return (ends, None);
        }
        (ends, Some(self.disk.load(root, &at.path, u64::MAX).map(|(content, _)| content)))
    }

    /// The look of `owner` ends with `event` at `at`.
    fn looked(&mut self, owner: Owner, at: Time, event: Event) {
        let delivery = self.schedule(at, Delivery::Looked { owner, event });
        self.looks.open(owner, delivery);
    }

    /// io starts the tools' operation `op` for `owner`, to end by
    /// `deadline`: after a draw, or at the deadline if that comes first.
    fn start_op(&mut self, owner: Owner, op: Op, deadline: Time) {
        // A call's operations, one after another, carry its token.
        self.owners.insert(owner);
        let at = match &op {
            Op::Load { at, .. } | Op::Scan { at, .. } | Op::Store { at, .. } | Op::Search { at, .. } => at.root,
            Op::Spawn { cwd, .. } => cwd.root,
        };
        assert_eq!(self.roots.get(&at.raw()), Some(&owner.0), "an operation is in its run's checkout");
        assert!(self.run_of(owner.0).answer.is_none(), "the tools work in a run's checkout before it answers");
        let mut ends = self.now.saturating_add(self.draw(self.settings.tool));
        let mut work = if self.rng.chance(self.settings.io_errors) {
            self.stats.op_faults += 1;
            Work::Ending(Done::Failed { fault: Fault::Other })
        } else {
            match op {
                Op::Spawn { cwd, command, env, roots, head, tail } => {
                    match io::spawn(&self.disk, &cwd, &command, &env, &roots, (head, tail)) {
                        Ok(started) => {
                            let runs = Duration::from_nanos(
                                u64::try_from(started.process.program.duration.as_nanos()).unwrap_or(u64::MAX),
                            );
                            ends = ends.saturating_add(runs);
                            Work::Command(started)
                        }
                        Err(done) => Work::Ending(done),
                    }
                }
                op @ (Op::Load { .. } | Op::Scan { .. } | Op::Store { .. } | Op::Search { .. }) => Work::File(op),
            }
        };
        // io runs the race with the deadline, and says it lost a moment after
        // the deadline passes: a command killed then tells what it wrote by
        // then (nothing, here).
        if ends > deadline {
            let done = match work {
                Work::Command(started) => io::exited(None, &[], started.head, started.tail),
                Work::File(_) | Work::Ending(_) => Done::TimedOut,
            };
            let noticed = deadline.saturating_add(self.draw(self.settings.network));
            (work, ends) = (Work::Ending(done), noticed);
            self.stats.op_timeouts += 1;
        }
        let delivery = self.schedule(ends, Delivery::Ran { owner });
        self.ops.open(owner, Pending { delivery, work });
        self.stats.ops += 1;
    }

    /// io is asked to cancel the operation of `owner`: it ends cancelled
    /// after a network draw, unless it ends of itself first.
    fn cancel_op(&mut self, owner: Owner) {
        assert!(self.owners.contains(&owner), "a cancel names an operation io was asked for");
        if !self.ops.contains(owner) {
            // It ended in the iteration the cancel was sent.
            self.stats.cancels_crossed += 1;
            return;
        }
        if self.rng.chance(self.settings.cancels_lost) {
            self.op_cancel_lost.insert(owner);
            self.stats.cancels_lost += 1;
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.network));
        let delivery = self.schedule(at, Delivery::Ran { owner });
        let pending = self.ops.get_mut(owner).expect("looked up above");
        self.wire.withdraw(pending.delivery).expect("an operation in flight has its end on the way");
        (pending.delivery, pending.work) = (delivery, Work::Ending(Done::Cancelled));
        self.stats.op_cancels += 1;
    }

    /// The operation of `owner` ends, and io tells the tools how.
    pub(super) fn ran(&mut self, owner: Owner) {
        let pending = self.ops.end(owner);
        let done = match pending.work {
            Work::File(op) => io::perform(&mut self.disk, op),
            Work::Command(started) => {
                self.disk.finish(&started.process);
                let program = &started.process.program;
                io::exited(Some(program.exit), &program.output, started.head, started.tail)
            }
            Work::Ending(done) => done,
        };
        self.op_cancel_lost.remove(&owner);
        self.agent_event(owner.0, Event::Done { owner: owner.1, done });
    }

    /// The checks of `owner` end, and io tells the run how.
    pub(super) fn checked(&mut self, owner: Owner) {
        let checking = self.checks.end(owner);
        self.abort_lost.remove(&owner);
        let event = match checking.work {
            Checks::Running { root, tail } => {
                let program = self.disk.load(root, fixture::CHECKS, u64::MAX).expect("the checks probed for").0;
                let (passed, output) = fixture::check(&self.disk, root, &program);
                let keep = output.len().min(usize::try_from(tail).expect("a small tail"));
                let cut = u64::try_from(output.len() - keep).expect("a small output");
                let output = output[output.len() - keep..].into();
                let exit = run::Exit::Code { code: u8::from(!passed) };
                if passed {
                    self.passed.insert(owner);
                    self.stats.checks_passed += 1;
                } else {
                    self.stats.checks_failed += 1;
                }
                self.run_of(owner.0).checked.push(passed);
                Event::Checked { owner: owner.1, ran: run::Ran { exit, output, cut } }
            }
            Checks::Ending(event) => event,
        };
        self.agent_event(owner.0, event);
    }

    /// The provider's answer to `call` arrives back at the agent's side.
    pub(super) fn answer(&mut self, call: u64, result: Result<provider::api::Answer, provider::api::Error>) {
        // The fake refuses a transcript a real provider would: a call without
        // its result, a result without its call.
        assert!(result != Err(provider::api::Error::InvalidRequest), "the agent sends well-formed queries");
        let Some(Call { owner, .. }) = self.end_call(call) else {
            self.stats.late_answers += 1;
            return;
        };
        self.cancel_lost.remove(&owner);
        let (id, owner) = owner;
        let event = match result {
            Ok(answer) => {
                self.stats.completed += 1;
                Event::Completed { owner, completion: translate::completion(answer) }
            }
            Err(error) => {
                self.stats.failed += 1;
                Event::Failed { owner, failure: translate::failure(error) }
            }
        };
        self.agent_event(id, event);
    }

    /// The provider's requests, carried back the way its protocol layer would.
    pub(super) fn provider_request(&mut self, request: provider::Request) {
        match request {
            provider::Request::Reply { to, result } => {
                let call = to.into_token().raw();
                self.serving.end(call);
                self.send(Delivery::Answer { call, result });
            }
        }
    }

    /// Ends the agent's call `call` if it is still in flight, withdrawing its
    /// deadline, and returns it.
    pub(super) fn end_call(&mut self, call: u64) -> Option<Call> {
        let ended = self.calls_out.take(call)?;
        self.calling.remove(&ended.owner);
        self.wire.withdraw(ended.deadline);
        Some(ended)
    }

    /// Abandons what io and the provider do for the agent of `process`, which
    /// a signal ended: its calls, its operations and commands, its looks and
    /// checks, and its pushes, whose answers find no one to read them.
    pub(super) fn abandon(&mut self, id: u64) {
        let calls: Vec<u64> =
            self.calling.iter().filter(|((process, _), _)| *process == id).map(|(_, call)| *call).collect();
        for call in calls {
            self.end_call(call);
        }
        self.cancel_lost.retain(|(process, _)| *process != id);
        for owner in self.ops.keys().filter(|(process, _)| *process == id).copied().collect::<Vec<_>>() {
            let pending = self.ops.end(owner);
            self.wire.withdraw(pending.delivery).expect("an operation in flight has its end on the way");
        }
        self.op_cancel_lost.retain(|(process, _)| *process != id);
        for owner in self.looks.keys().filter(|(process, _)| *process == id).copied().collect::<Vec<_>>() {
            let delivery = self.looks.end(owner);
            self.wire.withdraw(delivery).expect("a look in flight has its end on the way");
        }
        for owner in self.checks.keys().filter(|(process, _)| *process == id).copied().collect::<Vec<_>>() {
            let checking = self.checks.end(owner);
            self.wire.withdraw(checking.delivery).expect("checks in flight have their end on the way");
        }
        self.abort_lost.retain(|(process, _)| *process != id);
        self.passed.retain(|(process, _)| *process != id);
        for owner in self.pushes.keys().filter(|(process, _)| *process == id).copied().collect::<Vec<_>>() {
            self.pushes.end(owner);
        }
    }

    /// Asserts that the agent of `process`, which exits of itself, holds
    /// nothing, and that nothing is in flight for it.
    pub(super) fn assert_quiet(&self, id: u64) {
        let process = &self.processes[&id];
        let agent = process.agent.as_ref().expect("an agent that exits ran");
        let run = agent.run();
        let sessions = agent.session();
        assert_eq!((run.runs(), run.conversations(), run.calls()), (0, 0, 0), "every run has ended, and its calls");
        assert_eq!((sessions.sessions(), sessions.runs()), (0, 0), "every session has ended, and its calls");
        assert_eq!((sessions.kits(), sessions.jobs()), (0, 0), "every kit has closed, its calls answered");
        assert_eq!((agent.peers(), agent.flights()), (0, 0), "every peer and flight is freed");
        assert_eq!(agent.tickets(), 0, "every ticket is freed");
        assert!(!agent.is_ready(), "the ready list is drained");
        assert_eq!(agent.next_deadline(), None, "no alarm outlives what it was for");
        let mine = |(process, _): &Owner| *process == id;
        assert!(!self.calling.keys().any(mine), "no call is in flight");
        assert!(!self.ops.keys().any(mine) && !self.looks.keys().any(mine), "no operation is in flight");
        assert!(!self.checks.keys().any(mine) && !self.pushing(id), "no check or push is in flight");
        if agent.facts_lost() == 0 {
            assert!(process.live.is_empty(), "every conversation has ended");
        }
    }

    /// Counts a fact of the agent of `process`, follows its run's
    /// conversations and what they use, and writes it up the channel.
    fn tell(&mut self, id: u64, fact: Fact) {
        let told = &mut self.told;
        let process = self.processes.get_mut(&id).expect("a process tells once spawned");
        match fact {
            Fact::Run { fact } => match fact {
                run_facts::Fact::Admitted { .. } => told.admitted += 1,
                run_facts::Fact::Prepared { .. } => told.prepared += 1,
                run_facts::Fact::Opened { conversation, depth, .. } => {
                    told.opened += 1;
                    told.deepest = told.deepest.max(depth);
                    process.live.insert(conversation);
                    let live = u32::try_from(process.live.len()).expect("a few conversations");
                    let state = process.run.as_mut().expect("a run opens conversations once started");
                    assert!(state.crossed.is_none(), "a run past its budget opens no conversation");
                    state.widest = state.widest.max(live);
                }
                run_facts::Fact::Ended { conversation, .. } => {
                    told.ended += 1;
                    assert!(process.live.remove(&conversation), "a conversation ends once, after it opened");
                }
                run_facts::Fact::Called { ask, .. } => match ask {
                    run_facts::Asked::Finish => told.finishes += 1,
                    run_facts::Asked::SubAgent => told.sub_agents += 1,
                },
                run_facts::Fact::Returned { result, .. } => {
                    told.returned += 1;
                    match result {
                        Return::Accepted => told.accepted += 1,
                        Return::Rejected => told.rejected += 1,
                        Return::ChecksFailed => told.checks_failed += 1,
                        Return::Answered => told.answered += 1,
                        Return::Refused => told.refused += 1,
                        Return::Cancelled => told.returns_cancelled += 1,
                        Return::Unpushed => told.unpushed += 1,
                        Return::Moved | Return::TimedOut | Return::Busy | Return::Unanswered => {}
                    }
                }
                run_facts::Fact::CheckStarted { .. } => told.checks_started += 1,
                run_facts::Fact::CheckFinished { .. } => told.checks_finished += 1,
                run_facts::Fact::Pushed { .. } => told.pushed += 1,
                run_facts::Fact::Answered { .. } => told.runs_answered += 1,
            },
            Fact::Session { fact } => match fact {
                session::Fact::Opened { .. } => told.sessions += 1,
                session::Fact::CompletionStarted { .. } => told.completions_started += 1,
                session::Fact::CompletionAnswered { .. } => told.completions_answered += 1,
                session::Fact::CompletionFailed { .. } => told.completions_failed += 1,
                session::Fact::CompletionCancelled { .. } => told.completions_cancelled += 1,
                session::Fact::CompletionRetried { .. }
                | session::Fact::DelegateAnswered { .. }
                | session::Fact::DelegateCancelled { .. } => {}
                session::Fact::Tools { opener: _, fact } => match fact {
                    tools::Fact::Started { .. } => told.tools_started += 1,
                    tools::Fact::Answered { .. } => told.tools_answered += 1,
                    tools::Fact::Opened { .. }
                    | tools::Fact::Refused { .. }
                    | tools::Fact::Closing { .. }
                    | tools::Fact::Closed { .. } => {}
                },
                session::Fact::DelegateStarted { .. } => told.delegated += 1,
                session::Fact::Yielded { .. } => told.yielded += 1,
                session::Fact::Used { opener: _, usage } => {
                    told.used += 1;
                    let live = u32::try_from(process.live.len()).expect("a few conversations");
                    let state = process.run.as_mut().expect("a run's conversations use what they use");
                    used(state, live, usage);
                }
                session::Fact::Ended { .. } => told.sessions_ended += 1,
            },
        }
        self.write_up(id, channel::fact(fact));
    }

    /// Counts the results of the run's tools in the last message of
    /// `prompt`, the first time the session of `owner` sends it.
    fn count_results(&mut self, id: u64, owner: Token, prompt: &agent::llm::Prompt) {
        let counted = self.process_mut(id).counted.entry(owner).or_default();
        if prompt.messages.len() <= *counted {
            return;
        }
        *counted = prompt.messages.len();
        let last = prompt.messages.last().expect("a prompt has a message");
        for block in &last.content {
            match block {
                agent::llm::Block::ToolResult { result: agent::llm::Returned::Served { returned, .. }, .. } => {
                    match returned {
                        run::Returned::Answered { .. } => self.stats.sub_answers += 1,
                        run::Returned::Accepted
                        | run::Returned::Rejected { .. }
                        | run::Returned::ChecksFailed { .. }
                        | run::Returned::Moved
                        | run::Returned::Unpushed
                        | run::Returned::Cancelled
                        | run::Returned::TimedOut
                        | run::Returned::Busy
                        | run::Returned::Unanswered { .. }
                        | run::Returned::Refused { .. } => self.stats.served += 1,
                    }
                }
                agent::llm::Block::Text { .. }
                | agent::llm::Block::ToolCall { .. }
                | agent::llm::Block::ToolResult { .. } => {}
            }
        }
    }

    /// What the facts must add up to when none were dropped and no agent was
    /// killed: what the world saw cross the boundary.
    pub(super) fn assert_told(&self) {
        let (told, stats) = (&self.told, &self.stats);
        assert_eq!(told.admitted, stats.admitted, "a fact for every run admitted");
        // A run refused at its own entrance was never admitted, and its answer
        // is about no run.
        let admitted = self.runs().filter(|state| state.run.is_some() && state.answer.is_some()).count();
        assert_eq!(told.runs_answered, u32::try_from(admitted).expect("a few runs"), "a fact for every answer");
        assert_eq!(told.opened, told.ended, "a conversation opened ends");
        assert_eq!(told.sessions_ended, told.opened, "each conversation's session ends, or is refused");
        assert!(told.sessions <= told.opened, "a session opens for a conversation");
        assert_eq!(told.completions_started, stats.calls, "a fact for every call");
        let ended = told.completions_answered + told.completions_failed + told.completions_cancelled;
        assert_eq!(ended, stats.completed + stats.failed + stats.cancelled, "a fact for the end of every call");
        assert_eq!(told.used, told.completions_answered, "every completion answered is used");
        assert_eq!(told.delegated, told.finishes + told.sub_agents, "every delegated call reaches the run");
        assert_eq!(told.returned, told.delegated, "every call the run took returns");
        assert_eq!(told.checks_started, stats.checks, "a fact for every run of the checks");
        assert_eq!(
            told.checks_finished,
            stats.checks_passed + stats.checks_failed + stats.check_timeouts,
            "a fact for every end of the checks"
        );
        assert_eq!(told.pushed, stats.pushed + stats.moved + stats.unpushed, "a fact for every push's end");
        assert!(told.tools_answered >= told.tools_started, "a fact for the answer to every call started");
    }
}

/// A completion of a conversation of `state`'s run, `live` of whose
/// conversations live, used `usage`: the run adds it up, and notes when it
/// first spends past its budget.
fn used(state: &mut super::Run, live: u32, usage: session::llm::Usage) {
    let session::llm::Usage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens } = usage;
    let spend = Spend {
        turns: 1,
        input: input_tokens,
        output: output_tokens,
        cache_read: cache_read_tokens,
        cache_write: cache_write_tokens,
    };
    state.used = state.used.saturating_add(spend);
    match &mut state.crossed {
        Some((_, after)) => *after += 1,
        None if overspent(&state.budget, state.used) => state.crossed = Some((live, 0)),
        None => {}
    }
}

/// Whether `event` starts a run.
fn is_start(event: &Event) -> bool {
    match event {
        Event::Start { .. } => true,
        Event::Cancel { .. }
        | Event::Pushed { .. }
        | Event::HostCancelled { .. }
        | Event::Completed { .. }
        | Event::Failed { .. }
        | Event::Cancelled { .. }
        | Event::Done { .. }
        | Event::Read { .. }
        | Event::Probed { .. }
        | Event::Checked { .. }
        | Event::Aborted { .. } => false,
    }
}

/// The run's name, if `request` admits it.
fn admitted(request: &Request) -> Option<Token> {
    match request {
        Request::Admitted { run, .. } => Some(*run),
        Request::Answer { .. }
        | Request::Checking { .. }
        | Request::Push { .. }
        | Request::CancelHost { .. }
        | Request::Complete { .. }
        | Request::Cancel { .. }
        | Request::Io { .. }
        | Request::CancelIo { .. }
        | Request::Read { .. }
        | Request::Probe { .. }
        | Request::Check { .. }
        | Request::Abort { .. } => None,
    }
}

/// A copy of `answer`, for the world to keep, as the original goes up the
/// channel.
fn copy(answer: &run::Answer) -> run::Answer {
    match answer {
        run::Answer::Refused(refusal) => run::Answer::Refused(*refusal),
        run::Answer::Accepted { outcome, spent } => {
            let outcome = match outcome {
                Declared::Change(change) => Declared::Change(change.clone()),
                Declared::Verdict(Verdict { name, body, children }) => {
                    let children = children.iter().map(|Child { kind, fields }| Child {
                        kind: kind.clone(),
                        fields: fields
                            .iter()
                            .map(|Field { name, value }| Field { name: name.clone(), value: value.clone() })
                            .collect(),
                    });
                    Declared::Verdict(Verdict { name: name.clone(), body: body.clone(), children: children.collect() })
                }
            };
            run::Answer::Accepted { outcome, spent: *spent }
        }
        run::Answer::Failed { failure, spent } => run::Answer::Failed { failure: *failure, spent: *spent },
    }
}

/// Checks what `prompt` offers against whose conversation it is, which its
/// system text tells: a sub-agent's starts with the brief its asker wrote,
/// which the scripts start with a sub-agent's cue, and main's with the
/// charter's, which starts with its step's guidance. Only main may finish;
/// an explorer only inspects; a fixer has every family of tools, which the
/// run gave it only if main had them.
fn offers(prompt: &agent::llm::Prompt) {
    let finish = prompt.served.contains(&agent::llm::Served::Finish);
    let sub_agent = [b"@explore".as_slice(), b"@fix", b"@burn"].iter().any(|cue| prompt.system.starts_with(cue));
    assert!(finish != sub_agent, "only main may finish");
    let inspect = tools::Grants { inspect: true, modify: false, shell: false };
    if prompt.system.starts_with(b"@explore") || prompt.system.starts_with(b"@burn") {
        assert_eq!(prompt.tools, inspect, "a sub-agent has the families it was asked with");
    }
    if prompt.system.starts_with(b"@fix") {
        let all = tools::Grants { inspect: true, modify: true, shell: true };
        assert_eq!(prompt.tools, all, "a sub-agent has the families it was asked with");
    }
}

/// Whether `spent` is past `budget` in any part but time.
fn overspent(budget: &run::Budget, spent: Spend) -> bool {
    spent.turns > budget.turns
        || spent.input > budget.input
        || spent.output > budget.output
        || spent.cache_read > budget.cache_read
        || spent.cache_write > budget.cache_write
}

/// What a run's read finds in `content`: its first characters in at most
/// `max` bytes, cut where a character ends, if it is text.
fn text(content: &[u8], max: u32) -> run::Read {
    let Ok(text) = std::str::from_utf8(content) else {
        return run::Read::NotText;
    };
    let mut end = text.len().min(usize::try_from(max).expect("a small read"));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    run::Read::Text { text: content[..end].into(), whole: end == content.len() }
}

fn describe_event(event: &Event) -> String {
    match event {
        Event::Start { worker, charter, .. } => format!("start {} {:?}", worker.raw(), charter.grants.tools),
        Event::Cancel { run } => format!("cancel {}", run.raw()),
        Event::Pushed { owner, push } => format!("pushed {} {push:?}", owner.raw()),
        Event::HostCancelled { owner } => format!("host cancelled {}", owner.raw()),
        Event::Completed { owner, completion } => {
            format!("completed {} {:?} with {} parts", owner.raw(), completion.stop, completion.content.len())
        }
        Event::Failed { owner, failure } => format!("failed {} {failure:?}", owner.raw()),
        Event::Cancelled { owner } => format!("cancelled {}", owner.raw()),
        Event::Done { owner, done } => format!("done {} {}", owner.raw(), done_kind(done)),
        Event::Read { owner, read } => match read {
            run::Read::Text { text, whole } => format!("read {} {} bytes, whole {whole}", owner.raw(), text.len()),
            run::Read::Missing | run::Read::NotText | run::Read::Failed => format!("read {} {read:?}", owner.raw()),
        },
        Event::Probed { owner, executable } => format!("probed {} {executable}", owner.raw()),
        Event::Checked { owner, ran } => format!("checked {} {:?}", owner.raw(), ran.exit),
        Event::Aborted { owner } => format!("aborted {}", owner.raw()),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Admitted { worker, run } => format!("admitted {} as {}", worker.raw(), run.raw()),
        Request::Answer { to, answer } => format!("answer {to:?} {}", answer_kind(answer)),
        Request::Checking { worker, deadline } => format!("checking {} until {}", worker.raw(), deadline.as_nanos()),
        Request::Push { worker, owner, .. } => format!("push {} for {}", owner.raw(), worker.raw()),
        Request::CancelHost { owner } => format!("cancel host {}", owner.raw()),
        Request::Complete { owner, prompt, .. } => {
            format!("complete {} with {} messages, {} served", owner.raw(), prompt.messages.len(), prompt.served.len())
        }
        Request::Cancel { owner } => format!("cancel {}", owner.raw()),
        Request::Io { owner, op, .. } => format!("io {} {}", owner.raw(), op_kind(op)),
        Request::CancelIo { owner } => format!("cancel io {}", owner.raw()),
        Request::Read { owner, at, .. } => format!("read {} {}", owner.raw(), String::from_utf8_lossy(&at.path)),
        Request::Probe { owner, at, .. } => format!("probe {} {}", owner.raw(), String::from_utf8_lossy(&at.path)),
        Request::Check { owner, .. } => format!("check {}", owner.raw()),
        Request::Abort { owner } => format!("abort {}", owner.raw()),
    }
}

fn op_kind(op: &Op) -> &'static str {
    match op {
        Op::Load { .. } => "load",
        Op::Scan { .. } => "scan",
        Op::Store { .. } => "store",
        Op::Search { .. } => "search",
        Op::Spawn { .. } => "spawn",
    }
}

fn done_kind(done: &Done) -> String {
    match done {
        Done::Loaded { content, .. } => format!("loaded {} bytes", content.len()),
        Done::Scanned { entries, .. } => format!("scanned {} entries", entries.len()),
        Done::Found { hits, .. } => format!("found {} hits", hits.len()),
        Done::Exited { exit, .. } => format!("exited {exit:?}"),
        done @ (Done::Stored { .. }
        | Done::Missing
        | Done::NotDirectory
        | Done::Escapes
        | Done::Failed { .. }
        | Done::Conflict { .. }
        | Done::NotFile
        | Done::Linked
        | Done::TooLarge { .. }
        | Done::TimedOut
        | Done::Cancelled) => format!("{done:?}"),
    }
}
