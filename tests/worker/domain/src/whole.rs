//! Whole worker stories: the Smith process boundary, real checkout, fake disk
//! and forge. The referee reads remote trees and records crossing boundaries.
use std::collections::VecDeque;
use std::fmt::Write;

use skein_lib::{Duration, Env, List, Queue, Time, Token, Wall, Writer};
use smith_channel::{CEILINGS, DeliverAsk, DeliverAskParts, Field, FieldParts};
use temper_fake_checkout::{
    Checkout,
    git::{Remote, Tree},
};
use temper_worker_checkout_world::{forge::Forge, translate};
use temper_worker_domain::{
    Domain, Event, Limits, Request, agent, checkout::git::Op, fire, max_out, resume, step, wire, worst_case,
};

const RUN: Token = Token::new(81);
const ATTEMPT: Token = Token::new(13);
const PROCESS: Token = Token::new(707);
const REMOTE: &[u8] = b"org/app";
const PATH: &[u8] = b"code";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Delivered,
    CancelledDelivery,
    ContactWithin,
    ContactPast,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub tree: Tree,
    pub trace: Vec<String>,
    pub cancellations: u32,
    pub saves: u32,
    pub stop_bound: Duration,
}

struct World {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    owed: VecDeque<Event>,
    forge: Forge,
    disk: Checkout,
    first: u64,
    workspace: Option<Token>,
    process: Option<Token>,
    reading: bool,
    hold_push: bool,
    held: Option<(Token, Op)>,
    delivered: bool,
    answer: Option<wire::Answer>,
    report: Report,
}

impl World {
    fn new(seed: u64) -> Self {
        let mut limits = crate::LIMITS;
        limits.host.slots = 1;
        limits.host.turn_bytes = 64;
        limits.host.turns = 1;
        limits.host.turn_queue_bytes = 64;
        limits.checkout.workspaces = 1;
        limits.checkout.repositories = 1;
        limits.agent.agents = 1;
        limits.agent.turn_bytes = 64;
        limits.agent.turns = 1;
        limits.agent.unacknowledged_bytes = 64;
        limits.agent.no_progress = Duration::from_secs(120);
        assert!(worst_case(&limits).is_some());
        let mut forge = Forge::new(seed);
        let first = forge.repository(REMOTE, b"main", [(PATH.to_vec(), b"original\n".to_vec())].into());
        forge.create_branch(REMOTE, b"topic", first).expect("topic");
        Self {
            domain: Domain::new(&limits, seed),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(max_out(&limits)),
            owed: VecDeque::new(),
            forge,
            disk: Checkout::new(),
            first,
            workspace: None,
            process: None,
            reading: false,
            hold_push: false,
            held: None,
            delivered: false,
            answer: None,
            report: Report {
                tree: Tree::new(),
                trace: Vec::new(),
                cancellations: 0,
                saves: 0,
                stop_bound: Duration::ZERO,
            },
        }
    }

    fn event(&mut self, event: Event) {
        self.report.trace.push(format!("{:?} -> {event:?}", self.env.now));
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.settle();
    }

    fn fire(&mut self) {
        for _ in 0..16 {
            if !self.domain.is_due(self.env.now) {
                return;
            }
            fire(&mut self.domain, &self.env, &mut self.out);
            self.settle();
        }
        panic!("bounded alarm burst");
    }

    fn settle(&mut self) {
        for _ in 0..256 {
            while let Some(request) = self.out.pop() {
                self.report.trace.push(format!("{:?} <- {request:?}", self.env.now));
                self.take(request);
            }
            self.domain.reclaim();
            if self.domain.is_ready() {
                resume(&mut self.domain, &self.env, &mut self.out);
                continue;
            }
            let Some(event) = self.owed.pop_front() else { return };
            self.report.trace.push(format!("{:?} -> {event:?}", self.env.now));
            step(&mut self.domain, &self.env, event, &mut self.out);
        }
        panic!("bounded script failed to settle");
    }

    fn say(&mut self, message: agent::Up) {
        assert!(self.reading, "one terminal for the granted read");
        self.reading = false;
        self.event(Event::Received { owner: self.process.expect("spawned"), message });
    }

    fn take(&mut self, request: Request) {
        match request {
            Request::Dial => {}
            Request::HelloV2 { hello, graces, push_deadline } => {
                assert_eq!(graces, temper_worker_domain::declared_graces(&self.env.limits).expect("bound"));
                assert_eq!(push_deadline, temper_worker_domain::push_deadline(&self.env.limits).expect("push bound"));
                assert!(hello.hosting.len() <= 1);
                assert!(graces > self.env.limits.grace, "stop bound includes stopping and saving");
                self.report.stop_bound = graces;
            }
            Request::Io { owner, op, deadline } => {
                assert!(deadline > self.env.now);
                if let Op::Make { workspace } = &op {
                    self.workspace = Some(*workspace);
                }
                if let Op::Commit { title, body, .. } = &op {
                    if title.as_ref() == b"Save unfinished work" {
                        self.report.saves += 1;
                    } else {
                        assert_eq!(title.as_ref(), b"Delivery");
                        assert_eq!(body.as_ref(), b"Worker story");
                    }
                }
                if self.hold_push && matches!(op, Op::Push { .. }) {
                    assert!(self.held.replace((owner, op)).is_none());
                } else {
                    self.forge.at(self.env.now);
                    let done = translate::perform(&mut self.forge, &mut self.disk, op);
                    self.owed.push_back(Event::Done { owner, done });
                }
            }
            Request::Spawn { owner, deadline, .. } => {
                assert!(deadline > self.env.now);
                assert!(self.process.replace(owner).is_none());
                self.owed.push_back(Event::Spawned { owner, process: PROCESS });
            }
            Request::Send { owner, process, message } => {
                assert_eq!((Some(owner), process), (self.process, PROCESS));
                match message {
                    agent::Down::Start { start, .. } => {
                        assert_eq!(start.activation, ATTEMPT.raw());
                        assert_eq!(start.directories.len(), 1);
                        assert_eq!(start.directories[0].name.as_ref(), b"app");
                        assert_eq!(start.charter.as_ref(), b"charter");
                    }
                    agent::Down::Answer { call, reply } => {
                        assert_eq!(call, Token::new(44));
                        let agent::Reply::Delivery(agent::Delivery::Delivered(receipts)) = reply else {
                            panic!("the complete edited tree is delivered: {reply:?}");
                        };
                        assert_eq!(receipts.receipts().len(), 1);
                        assert_eq!(receipts.receipts()[0].directory(), 0);
                        let landed = self.forge.branch(REMOTE, b"topic").expect("delivered branch");
                        let mut encoded = String::with_capacity(64);
                        for byte in translate::commit(landed).raw() {
                            write!(&mut encoded, "{byte:02x}").expect("String write");
                        }
                        assert_eq!(
                            receipts.receipts()[0].text(),
                            encoded.as_bytes(),
                            "the receipt identifies the pushed head"
                        );
                        self.delivered = true;
                    }
                    agent::Down::Cancel => {
                        self.report.cancellations += 1;
                    }
                    agent::Down::Acknowledge { .. } | agent::Down::Grant { .. } | agent::Down::Message { .. } => {
                        panic!("unscripted process record")
                    }
                }
                self.owed.push_back(Event::Sent { owner });
            }
            Request::Read { owner, process } => {
                assert_eq!((Some(owner), process), (self.process, PROCESS));
                assert!(!self.reading);
                self.reading = true;
            }
            Request::Wait { owner, process } | Request::Reap { owner, process } => {
                assert_eq!((Some(owner), process), (self.process, PROCESS));
            }
            Request::Answer { run, attempt, answer } => {
                assert_eq!((run, attempt), (RUN, ATTEMPT));
                assert!(self.held.is_none(), "delivery settles before the final answer");
                assert!(self.answer.replace(answer).is_none());
            }
            Request::Turn { .. }
            | Request::Relay { .. }
            | Request::Bounced { .. }
            | Request::Rejected { .. }
            | Request::Exhausted { .. }
            | Request::Signal { .. }
            | Request::CancelRelay { .. }
            | Request::CancelIo { .. } => panic!("unscripted request"),
        }
    }

    fn start(&mut self) {
        self.fire();
        self.event(Event::ConnectedV2);
        self.event(Event::Assign {
            assignment: wire::Assignment {
                assignment: wire::RunAssignment {
                    run: RUN,
                    attempt: ATTEMPT,
                    workspace: wire::Workspace {
                        key: Box::from(&b"stream"[..]),
                        repositories: Box::from([wire::Repository {
                            tag: 1,
                            name: Box::from(&b"app"[..]),
                            remote: Box::from(REMOTE),
                            identity: 0,
                            start: wire::Start::Base { branch: Box::from(&b"main"[..]) },
                            access: wire::Access::WritableV2 {
                                push: Box::from(&b"topic"[..]),
                                expected: Some(translate::commit(self.first).raw()),
                            },
                        }]),
                    },
                    save: Some(Box::from(&b"saved"[..])),
                    charter: Box::from(&b"charter"[..]),
                    grants: Box::new([]),
                },
                turns: Box::new([]),
                answered: Box::new([]),
            },
        });
        self.say(agent::Up::Admitted);
        let place = temper_worker_domain::checkout::git::Place {
            workspace: self.workspace.expect("prepared"),
            repository: Box::from(&b"app"[..]),
        };
        self.disk.write(&[translate::path(&place).as_slice(), b"/code"].concat(), b"edited\n");
    }

    fn deliver(&mut self) {
        let mut fields = List::with_capacity(2);
        for (name, text) in [(b"title".as_slice(), b"Delivery".as_slice()), (b"body", b"Worker story")] {
            fields
                .push(
                    Field::new(&CEILINGS, FieldParts { name: Box::from(name), text: Box::from(text) }).expect("field"),
                )
                .expect("two fields");
        }
        let ask = DeliverAsk::new(&CEILINGS, DeliverAskParts { fields }).expect("fields");
        let mut writer = Writer::new(usize::try_from(ask.measure()).expect("fits"));
        ask.encode(&mut writer).expect("sized");
        self.say(agent::Up::Call {
            call: Token::new(44),
            name: agent::CallName { activation: ATTEMPT.raw(), completion: 1, position: 0 },
            deadline: self.env.now.saturating_add(Duration::from_secs(60)),
            ask: agent::Ask::Deliver { fields: writer.finish() },
        });
    }

    fn gone(&mut self) {
        let owner = self.process.expect("spawned");
        self.event(Event::Exited { owner });
        assert!(self.reading);
        self.reading = false;
        self.event(Event::Hangup { owner });
        self.event(Event::Reaped { owner, detail: Box::new([]) });
    }

    fn run(mut self, case: Case) -> Report {
        self.start();
        match case {
            Case::Delivered => {
                self.deliver();
                assert!(self.delivered);
                self.say(agent::Up::Answer {
                    answer: agent::Answer {
                        turns: 0,
                        spent: 0,
                        result: agent::RunResult::Accepted { outcome: Box::from(&b"done"[..]) },
                    },
                });
                self.gone();
            }
            Case::CancelledDelivery => {
                self.hold_push = true;
                self.deliver();
                assert!(self.held.is_some());
                assert_eq!(self.forge.branch(REMOTE, b"topic"), Some(self.first));
                self.event(Event::Cancel { run: RUN, attempt: ATTEMPT });
                assert_eq!(self.report.cancellations, 1);
                self.gone();
                assert!(self.answer.is_none());
                self.hold_push = false;
                let (owner, op) = self.held.take().expect("push held");
                let done = translate::perform(&mut self.forge, &mut self.disk, op);
                self.event(Event::Done { owner, done });
            }
            Case::ContactWithin | Case::ContactPast => {
                self.event(Event::Lost);
                self.env.now = self.env.now.saturating_add(if case == Case::ContactWithin {
                    Duration::from_secs(1)
                } else {
                    self.env.limits.grace
                });
                self.fire();
                if case == Case::ContactWithin {
                    assert_eq!(self.report.cancellations, 0);
                    self.event(Event::ConnectedV2);
                    self.deliver();
                    self.say(agent::Up::Answer {
                        answer: agent::Answer {
                            turns: 0,
                            spent: 0,
                            result: agent::RunResult::Accepted { outcome: Box::from(&b"done"[..]) },
                        },
                    });
                    self.gone();
                } else {
                    assert_eq!(self.report.cancellations, 1);
                    self.gone();
                    assert_eq!(self.report.saves, 1, "save before the cancellation answer reaches the engine");
                    assert!(self.answer.is_none());
                    self.event(Event::ConnectedV2);
                }
            }
        }
        assert_eq!(self.domain.agent().agents(), 0);
        assert_eq!(self.domain.workspaces(), 0);
        let answer = self.answer.take().expect("a terminal answer");
        let branch = if case == Case::ContactPast { b"saved".as_slice() } else { b"topic".as_slice() };
        let commit = self.forge.branch(REMOTE, branch).expect("worker pushed");
        let work = match (case, answer.ending) {
            (Case::Delivered | Case::ContactWithin, wire::Ending::Ended { work, .. })
            | (
                Case::CancelledDelivery,
                wire::Ending::Failed { failure: wire::Failure::Cancelled(wire::Reason::Engine), work, .. },
            )
            | (
                Case::ContactPast,
                wire::Ending::Failed { failure: wire::Failure::Cancelled(wire::Reason::Contact), work, .. },
            ) => work,
            (_, ending) => panic!("ending did not follow the story: {ending:?}"),
        };
        if case == Case::ContactPast {
            let saved = work.saved.expect("saved work");
            let [wire::Landing::Landed { commit: saved, .. }] = saved.as_ref() else {
                panic!("one saved head");
            };
            assert_eq!(*saved, translate::commit(commit).raw());
        } else {
            assert_eq!(work.landed.as_ref(), [wire::Landed { tag: 1, commit: translate::commit(commit).raw() }]);
        }
        self.report.tree = self.forge.tree(commit);
        assert_eq!(self.report.tree, [(PATH.to_vec(), b"edited\n".to_vec())].into(), "the entire edited tree landed");
        assert!(self.forge.is_ancestor(self.first, commit));
        self.event(Event::Acknowledged { run: RUN, attempt: ATTEMPT });
        assert_eq!(self.domain.held(), 0);
        self.event(Event::Shutdown);
        assert!(self.domain.is_done());
        self.report
    }
}

#[must_use]
pub fn run(seed: u64, case: Case) -> Report {
    World::new(seed).run(case)
}
