use crate::referee::{Seen, Stimulus, Tasks};
use skein_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token, Wall};
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain_tasks::{
    self as tasks, Accepted, Authority, Budget, Contract, Delegation, Domain, End, Ending, Event, Executor, Fact,
    Funder, Key, Limits, New, Numbers, Party, Problem, Request, Result, Retries, Retry, Scopes, Spec, Stored, Tools,
};
use temper_world::{Referee, Trace};
pub const RETRY: Retry = Retry { retries: 2, base: Duration::from_millis(10), max: Duration::from_secs(1) };
pub const LIMITS: Limits = Limits {
    tasks: 16,
    project_tasks: 16,
    stubs: 32,
    tree_tasks: 16,
    depth: 4,
    delegates: 8,
    batch: 8,
    dependencies: 8,
    inputs: 4,
    spec_bytes: 64,
    parameters: 4,
    result_bytes: 32,
    contract_choices: 4,
    charters: 2,
    authority_grants: 4,
    authority_segments: 4,
    authority_bytes: 64,
    executor_kinds: 3,
    retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
    facts: 16,
    references: 8,
    inbox_messages: 32,
    inbox_bytes: 2048,
    message_bytes: 64,
    questions: 16,
    subscriptions: 16,
    receipts: 64,
    offers: 64,
};
#[must_use]
pub fn authority() -> Authority {
    Authority {
        tools: Tools(0),
        grants: Box::new([]),
        delegation: Delegation { kinds: Box::new([]), tasks: 16, depth: 4 },
        budget: Budget { spend: 100, deadline: None },
        notes: Scopes(0),
    }
}
#[must_use]
pub fn task(number: u64, dependencies: &[u64]) -> New {
    New {
        number,
        project: 1,
        executor: Executor::Agent { charter: 1 },
        spec: Spec { words: Box::new([1]), parameters: Box::new([]), inputs: Box::new([]) },
        contract: Contract::Report { words: 32 },
        authority: authority(),
        numbers: Numbers { budget: 100, spent: 0, spent_below: 0, reserved: 0 },
        funder: Funder::Period { project: 1, period: 0 },
        dependencies: dependencies.into(),
        policy: tasks::WakePolicy::DEFAULT,
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply {
    Made(Vec<u64>),
    Done,
    Refused(Problem),
    Acknowledged(Accepted),
    Sent(Accepted),
    Inbox(Vec<tasks::Envelope>, bool),
    Turn(Accepted),
}
#[derive(Debug)]
pub struct World {
    pub env: Env<Limits>,
    domain: Domain,
    out: Queue<Request>,
    pending: Vec<Request>,
    pub records: BTreeMap<Key, Stored>,
    pub replies: BTreeMap<u64, Reply>,
    pub activations: BTreeSet<u64>,
    pub runs: BTreeMap<u64, u64>,
    pub stops: BTreeSet<(u64, u64)>,
    pub closing: BTreeSet<u64>,
    pub results: BTreeMap<u64, Ending>,
    pub facts: Vec<Fact>,
    pub relays: BTreeMap<(u64, u64, u64), tasks::Envelope>,
    pub topics: BTreeMap<u64, tasks::Subscription>,
    message: u64,
    pub accounting_referee: crate::accounting_referee::Accounting,
    pub inbox_referee: crate::inbox_referee::Inbox,
    reads: Vec<crate::inbox_referee::Read>,
    pub consume_facts: bool,
    pub referee: Referee<Tasks>,
    pub trace: Trace,
    seed: u64,
    call: u64,
    commit: u64,
}
impl World {
    #[must_use]
    pub fn new(seed: u64, limits: Limits) -> World {
        let mut world = World {
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            domain: Domain::new(&limits, seed, Box::new([1])),
            out: Queue::with_capacity(tasks::max_out(&limits)),
            pending: Vec::new(),
            records: BTreeMap::new(),
            replies: BTreeMap::new(),
            activations: BTreeSet::new(),
            runs: BTreeMap::new(),
            stops: BTreeSet::new(),
            closing: BTreeSet::new(),
            results: BTreeMap::new(),
            facts: Vec::new(),
            relays: BTreeMap::new(),
            topics: BTreeMap::new(),
            message: 0,
            accounting_referee: crate::accounting_referee::Accounting::default(),
            inbox_referee: crate::inbox_referee::Inbox::default(),
            reads: Vec::new(),
            consume_facts: true,
            referee: Referee::new(Tasks::default()),
            trace: Trace::default(),
            seed,
            call: 0,
            commit: 0,
        };
        world.send(Event::Restored);
        world
    }
    pub fn number(&mut self) -> u64 {
        self.message += 1;
        self.message
    }
    #[must_use]
    pub fn messages(&self, task: u64) -> Vec<tasks::Envelope> {
        self.records
            .values()
            .filter_map(|row| match row {
                Stored::Message(envelope) if envelope.task == task => Some(envelope.clone()),
                Stored::Message(_)
                | Stored::Live(_)
                | Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_) => None,
            })
            .collect()
    }
    pub fn mail(&mut self, task: u64, from: Party, message: tasks::UserMessage) -> Reply {
        let number = self.number();
        self.mail_number(number, task, from, message)
    }
    pub fn mail_number(&mut self, number: u64, task: u64, from: Party, message: tasks::UserMessage) -> Reply {
        self.message = self.message.max(number);
        let reply_to = self.to();
        let call = self.call;
        self.send(Event::Send { reply_to, number, task, from, message });
        self.replies[&call].clone()
    }
    pub fn turn(&mut self, task: u64, turn: u32, read: Option<u64>) -> Reply {
        let attempt = self.record(task).attempt;
        let reply_to = self.to();
        let call = self.call;
        self.send(Event::Turn { reply_to, task, attempt, turn, read });
        self.replies[&call].clone()
    }
    fn callbacks(&mut self) {
        let mut at = 0;
        while at < self.pending.len() {
            let event = match &self.pending[at] {
                Request::Ended { task, requester: Party::Task(parent), ending } => {
                    Some((0, *parent, *task, Some(ending.clone()), None))
                }
                Request::Observe { subscription, target } => {
                    let ending = match self.records.get(&Key::Ended(*target)) {
                        Some(Stored::Ended(record)) => match &record.phase {
                            tasks::Phase::Ended(ending) => ending.clone(),
                            tasks::Phase::Waiting
                            | tasks::Phase::Active(_)
                            | tasks::Phase::Closing(_)
                            | tasks::Phase::Held { .. } => panic!("historical row ended"),
                        },
                        Some(
                            Stored::Live(_)
                            | Stored::Stub(_)
                            | Stored::Message(_)
                            | Stored::History(_)
                            | Stored::Closure(_)
                            | Stored::Funding { .. }
                            | Stored::ArchivedMessage(_)
                            | Stored::Receipt(_)
                            | Stored::Offer(_)
                            | Stored::Question(_)
                            | Stored::Subscription(_),
                        )
                        | None => panic!("root loads the authoritative ended row"),
                    };
                    Some((1, *subscription, 0, None, Some(tasks::Notice::Ended(ending))))
                }
                Request::Notify { subscription, notice } => Some((1, *subscription, 0, None, Some(notice.clone()))),
                Request::Timer { subscription } => Some((2, *subscription, 0, None, None)),
                Request::Made { .. }
                | Request::Refused { .. }
                | Request::Done { .. }
                | Request::Acknowledged { .. }
                | Request::Sent { .. }
                | Request::Inbox { .. }
                | Request::TurnAcknowledged { .. }
                | Request::Relay { .. }
                | Request::Topic { .. }
                | Request::Activate { .. }
                | Request::Stop { .. }
                | Request::Adopt { .. }
                | Request::Close { .. }
                | Request::Ended { requester: Party::Person(_) | Party::Deployment { .. }, .. }
                | Request::Save { .. }
                | Request::Erase { .. }
                | Request::RestoreRefused { .. } => None,
            };
            at += 1;
            if let Some((kind, target, delegate, ending, notice)) = event {
                let number = self.number();
                let reply_to = self.to();
                let event = match kind {
                    0 => Event::DeliverResult {
                        reply_to,
                        number,
                        task: target,
                        delegate,
                        ending: ending.expect("result callback carries ending"),
                    },
                    1 => Event::DeliverNotice {
                        reply_to,
                        number,
                        subscription: target,
                        notice: notice.expect("notice callback carries notice"),
                    },
                    2 => Event::DeliverTimer { reply_to, number, subscription: target },
                    _ => unreachable!(),
                };
                self.trace.log(self.env.now, format_args!("{event:?}"));
                tasks::step(&mut self.domain, &self.env, event, &mut self.out);
                while let Some(request) = self.out.pop() {
                    assert!(
                        !matches!(request, Request::Refused { .. }),
                        "guaranteed callback must have reserved room: {request:?}"
                    );
                    self.pending.push(request);
                }
            }
        }
    }
    pub fn to(&mut self) -> ReplyTo {
        self.call += 1;
        ReplyTo::new(Token::new(self.call))
    }
    pub fn observe(&mut self, seen: Seen) {
        self.trace.log(self.env.now, format_args!("{seen:?}"));
        let mut stimuli = Vec::new();
        self.referee.observe(self.env.now, seen, &mut stimuli);
        self.referee.assert_holding(self.seed);
        for stimulus in stimuli {
            match stimulus {
                Stimulus::Restart => self.restart(),
            }
        }
    }
    pub fn stage(&mut self, event: Event) {
        match &event {
            Event::Move { movement, .. } => self.accounting_referee.begin(&movement.balances),
            Event::Amend { amendment, .. } => self.accounting_referee.begin(&amendment.balances),
            Event::Control { .. }
            | Event::Charge { .. }
            | Event::Turn { .. }
            | Event::Make { .. }
            | Event::Prepare { .. }
            | Event::Claim { .. }
            | Event::Send { .. }
            | Event::Peek { .. }
            | Event::DeliverResult { .. }
            | Event::DeliverNotice { .. }
            | Event::DeliverTimer { .. }
            | Event::News { .. }
            | Event::ForgetReceipt { .. }
            | Event::Introduce { .. }
            | Event::ForgetReference { .. }
            | Event::Subscribe { .. }
            | Event::Unsubscribe { .. }
            | Event::Started { .. }
            | Event::Activation { .. }
            | Event::PreparationFailed { .. }
            | Event::Hold { .. }
            | Event::Release { .. }
            | Event::Cancel { .. }
            | Event::Settled { .. }
            | Event::RememberStub { .. }
            | Event::ForgetStub { .. }
            | Event::Restore { .. }
            | Event::Restored => self.accounting_referee.begin(&[]),
        }
        assert!(self.pending.is_empty(), "one parent decision at a time");
        let read = match &event {
            Event::Turn { task, attempt, turn, read, .. } => {
                Some(crate::inbox_referee::Read { task: *task, attempt: *attempt, turn: *turn, through: *read })
            }
            Event::Control { .. }
            | Event::Amend { .. }
            | Event::Move { .. }
            | Event::Charge { .. }
            | Event::Make { .. }
            | Event::Prepare { .. }
            | Event::Claim { .. }
            | Event::Send { .. }
            | Event::Peek { .. }
            | Event::DeliverResult { .. }
            | Event::DeliverNotice { .. }
            | Event::DeliverTimer { .. }
            | Event::News { .. }
            | Event::ForgetReceipt { .. }
            | Event::Introduce { .. }
            | Event::ForgetReference { .. }
            | Event::Subscribe { .. }
            | Event::Unsubscribe { .. }
            | Event::Started { .. }
            | Event::Activation { .. }
            | Event::PreparationFailed { .. }
            | Event::Hold { .. }
            | Event::Release { .. }
            | Event::Cancel { .. }
            | Event::Settled { .. }
            | Event::RememberStub { .. }
            | Event::ForgetStub { .. }
            | Event::Restore { .. }
            | Event::Restored => None,
        };
        self.trace.log(self.env.now, format_args!("{event:?}"));
        tasks::step(&mut self.domain, &self.env, event, &mut self.out);
        while let Some(request) = self.out.pop() {
            self.pending.push(request);
        }
        if let Some(read) = read
            && self
                .pending
                .iter()
                .any(|request| matches!(request, Request::TurnAcknowledged { accepted: Accepted::New, .. }))
        {
            self.reads.push(read);
        }
        if self.consume_facts {
            while let Some(fact) = self.domain.pop_fact() {
                self.facts.push(fact);
            }
        }
    }
    pub fn send(&mut self, event: Event) {
        self.stage(event);
        self.durable();
        self.deliver();
    }
    /// Commit records and requester mail together, then allow delivery later.
    pub fn durable(&mut self) {
        self.callbacks();
        self.commit += 1;
        let mut histories = Vec::new();
        let mut terminals = Vec::new();
        let mut ended = Vec::new();
        for request in &self.pending {
            match request {
                Request::Save { record } => {
                    self.records.insert(record.key(), record.clone());
                    if let Stored::Funding { funder, .. } = record {
                        self.accounting_referee.emitted(*funder);
                    }
                    if let Stored::History(history) = record {
                        histories.push(history.clone());
                    }
                }
                Request::Erase { key } => {
                    self.records.remove(key);
                }
                Request::Ended { task, ending, .. } => {
                    assert!(self.results.insert(*task, ending.clone()).is_none(), "requester mail is committed once");
                    ended.push((*task, status(ending)));
                }
                Request::Acknowledged { task, attempt, accepted: Accepted::New, .. } => {
                    terminals.push((*task, *attempt));
                }
                Request::Sent { .. }
                | Request::Inbox { .. }
                | Request::TurnAcknowledged { .. }
                | Request::Relay { .. }
                | Request::Observe { .. }
                | Request::Notify { .. }
                | Request::Timer { .. }
                | Request::Topic { .. }
                | Request::Made { .. }
                | Request::Refused { .. }
                | Request::Done { .. }
                | Request::Acknowledged { accepted: Accepted::Already, .. }
                | Request::Activate { .. }
                | Request::Stop { .. }
                | Request::Adopt { .. }
                | Request::Close { .. }
                | Request::RestoreRefused { .. } => {}
            }
        }
        if self.domain.ready() {
            self.accounting_referee.committed(&self.records).expect("independent funding conservation");
            self.inbox_referee
                .committed(&self.records, &self.env.limits, &self.reads)
                .expect("independent inbox invariants");
        }
        self.reads.clear();
        self.observe(Seen::Durable { commit: self.commit });
        let made = self
            .records
            .values()
            .filter_map(|record| match record {
                Stored::Live(record) if !self.referee.expectations().has_task(record.number) => Some(Seen::Made {
                    task: record.number,
                    parent: record.requester,
                    dependencies: record.dependencies.to_vec(),
                    depth: record.depth,
                }),
                Stored::Live(_)
                | Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::Message(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_) => None,
            })
            .collect::<Vec<_>>();
        for seen in made {
            self.observe(seen);
        }
        for history in histories {
            match history.change {
                tasks::Change::Moved { from, to } => self.observe(Seen::Moved { task: history.task, from, to }),
                tasks::Change::Amended => self.observe(Seen::Amended {
                    task: history.task,
                    dependencies: self.record(history.task).dependencies.to_vec(),
                }),
                tasks::Change::Cancelled => self.observe(Seen::Cancelled { task: history.task }),
                tasks::Change::Released => {}
            }
        }
        for (task, attempt) in terminals {
            self.runs.remove(&task);
            self.observe(Seen::Terminal { task, attempt });
        }
        for (task, status) in ended {
            self.observe(Seen::Ended { task, status, after: self.commit });
        }
    }
    pub fn deliver(&mut self) {
        let requests = std::mem::take(&mut self.pending);
        for request in requests {
            match request {
                Request::Save { .. }
                | Request::Erase { .. }
                | Request::Ended { .. }
                | Request::Observe { .. }
                | Request::Notify { .. }
                | Request::Timer { .. } => {}
                Request::Sent { reply_to, accepted, .. } => self.reply(reply_to, Reply::Sent(accepted)),
                Request::Inbox { reply_to, messages, more } => {
                    self.reply(reply_to, Reply::Inbox(messages.into_vec(), more));
                }
                Request::TurnAcknowledged { reply_to, accepted, .. } => self.reply(reply_to, Reply::Turn(accepted)),
                Request::Relay { task, attempt, envelope } => {
                    assert_eq!(self.runs.get(&task), Some(&attempt), "relay reaches actual adopted/live executor");
                    let key = (task, attempt, envelope.number);
                    assert_eq!(
                        self.records.get(&Key::Offer(envelope.key())),
                        Some(&Stored::Offer(tasks::Offer { attempt, envelope: envelope.clone() })),
                        "offered payload durable before relay"
                    );
                    if let Some(old) = self.relays.insert(key, envelope.clone()) {
                        assert_eq!(old, envelope, "replayed relay has identical payload");
                    }
                }
                Request::Topic { subscription, present } => {
                    if present {
                        self.topics.insert(subscription.number, subscription);
                    } else {
                        self.topics.remove(&subscription.number);
                    }
                }
                Request::Made { reply_to, tasks } => self.reply(reply_to, Reply::Made(tasks.into_vec())),
                Request::Refused { reply_to, problem } => self.reply(reply_to, Reply::Refused(problem)),
                Request::Done { reply_to } => self.reply(reply_to, Reply::Done),
                Request::Acknowledged { reply_to, accepted, .. } => self.reply(reply_to, Reply::Acknowledged(accepted)),
                Request::Activate { task, .. } => {
                    self.activations.insert(task);
                }
                Request::Stop { task, attempt } => {
                    self.stops.insert((task, attempt));
                }
                Request::Adopt { task, attempt } => {
                    self.runs.insert(task, attempt);
                    self.observe(Seen::Assigned { task, attempt, after: self.commit, adopted: true });
                }
                Request::Close { task, .. } => {
                    self.closing.insert(task);
                    self.observe(Seen::Closing { task });
                }
                Request::RestoreRefused { problem } => panic!("world store corrupted: {problem:?}"),
            }
        }
        self.observe(Seen::Limit { live: self.live(), cap: self.env.limits.tasks });
        let live = self
            .records
            .values()
            .filter_map(|record| match record {
                Stored::Live(record) => Some(*record.clone()),
                Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::Message(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_) => None,
            })
            .collect();
        let stubs = self.records.keys().filter(|key| matches!(key, Key::Stub(_))).count();
        self.observe(Seen::Stored { live, stubs, limits: Box::new(self.env.limits) });
        self.domain.reclaim();
    }
    fn reply(&mut self, to: ReplyTo, reply: Reply) {
        let call = to.into_token().raw();
        assert!(self.replies.insert(call, reply).is_none(), "one reply per call");
        self.observe(Seen::Replied { call, after: self.commit });
    }
    #[must_use]
    pub fn live(&self) -> usize {
        self.records.keys().filter(|key| matches!(key, Key::Live(_))).count()
    }
    #[must_use]
    pub fn record(&self, number: u64) -> &tasks::TaskRecord {
        match self.records.get(&Key::Live(number)).expect("task live") {
            Stored::Live(record) => record,
            Stored::Ended(_)
            | Stored::Stub(_)
            | Stored::Message(_)
            | Stored::History(_)
            | Stored::Closure(_)
            | Stored::Funding { .. }
            | Stored::ArchivedMessage(_)
            | Stored::Receipt(_)
            | Stored::Offer(_)
            | Stored::Question(_)
            | Stored::Subscription(_) => unreachable!("key is live"),
        }
    }
    pub fn make(&mut self, creator: Party, batch: Vec<New>) -> Reply {
        let before = self.records.keys().copied().collect::<BTreeSet<_>>();
        let members = batch.iter().map(|task| task.number).collect::<Vec<_>>();
        let reply_to = self.to();
        let call = self.call;
        self.send(Event::Make { reply_to, creator, batch: batch.into_boxed_slice() });
        let reply = self.replies.get(&call).expect("make replied").clone();
        let made = self
            .records
            .keys()
            .filter_map(|key| match key {
                Key::Live(number) if !before.contains(key) => Some(*number),
                Key::Live(_)
                | Key::Ended(_)
                | Key::Stub(_)
                | Key::Message(_)
                | Key::History { .. }
                | Key::Closure { .. }
                | Key::Funding(_)
                | Key::ArchivedMessage(_)
                | Key::Receipt(_)
                | Key::Offer(_)
                | Key::Question(_)
                | Key::Subscription(_) => None,
            })
            .collect();
        self.observe(Seen::Batch { members, accepted: matches!(reply, Reply::Made(_)), made });
        reply
    }
    /// Root builds a brief, asks the executor to prepare, commits the fresh
    /// claim, then starts actual work. Authority initially always allows.
    pub fn claim(&mut self, task: u64, attempt: u64) {
        assert!(self.activations.remove(&task), "task activated before preparation");
        let reply_to = self.to();
        self.send(Event::Prepare { reply_to, task });
        let reply_to = self.to();
        let call = self.call;
        let readable = self.messages(task).iter().map(|message| message.number).collect::<Vec<_>>().into_boxed_slice();
        self.send(Event::Claim { reply_to, task, attempt, readable });
        assert_eq!(self.replies.get(&call), Some(&Reply::Done), "fresh claim accepted");
        assert!(self.runs.insert(task, attempt).is_none(), "one physical run per task");
        self.observe(Seen::Assigned { task, attempt, after: self.commit, adopted: false });
        self.send(Event::Started { task, attempt });
    }
    pub fn terminal(&mut self, task: u64, end: End) -> Reply {
        let attempt = *self.runs.get(&task).expect("physical run exists");
        let reply_to = self.to();
        let call = self.call;
        self.send(Event::Activation { reply_to, task, attempt, end });
        self.replies.get(&call).expect("terminal replied").clone()
    }
    pub fn claim_fresh(&mut self, task: u64) {
        let attempt = self
            .records
            .values()
            .map(|record| match record {
                Stored::Live(record) | Stored::Ended(record) => record.attempt,
                Stored::Stub(stub) => stub.attempt,
                Stored::Message(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_) => 0,
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .expect("test counters fit");
        self.claim(task, attempt);
    }
    pub fn finish(&mut self, task: u64) {
        self.terminal(task, End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false });
    }
    pub fn settle(&mut self, task: u64) {
        assert!(self.closing.remove(&task), "root closing gate open");
        self.observe(Seen::Settled { task });
        self.send(Event::Settled { task });
    }
    pub fn advance(&mut self) {
        let at = self.domain.next_deadline().expect("backoff deadline pending");
        self.env.wall = Wall::from_nanos(
            self.env.wall.as_nanos().saturating_add(at.as_nanos().saturating_sub(self.env.now.as_nanos())),
        );
        self.env.now = at;
        tasks::fire(&mut self.domain, &self.env, &mut self.out);
        while let Some(request) = self.out.pop() {
            self.pending.push(request);
        }
        self.durable();
        self.deliver();
    }
    pub fn restart(&mut self) {
        self.pending.clear();
        self.reads.clear();
        self.inbox_referee.reset(&self.records);
        self.accounting_referee.reset(&self.records);
        self.domain = Domain::new(&self.env.limits, self.seed, Box::new([1]));
        self.activations.clear();
        self.stops.clear();
        self.closing.clear();
        let rows = self
            .records
            .values()
            .filter(|record| {
                !matches!(
                    record,
                    Stored::Ended(_)
                        | Stored::History(_)
                        | Stored::Closure(_)
                        | Stored::Funding { .. }
                        | Stored::ArchivedMessage(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        for record in rows {
            self.send(Event::Restore { record });
        }
        self.send(Event::Restored);
    }
    pub fn cancel(&mut self, task: u64, reason: &[u8]) {
        self.observe(Seen::Cancelled { task });
        let reply_to = self.to();
        self.send(Event::Cancel { reply_to, task, reason: reason.into() });
    }
    pub fn complete_cancel(&mut self) {
        for (task, _) in self.stops.clone() {
            if self.runs.contains_key(&task) {
                self.terminal(task, End::Parked);
            }
        }
        while let Some(task) = self.closing.iter().next().copied() {
            self.settle(task);
        }
        self.observe(Seen::Finished);
        self.referee.assert_passed(self.seed);
    }
}
fn status(ending: &Ending) -> tasks::Status {
    match ending {
        Ending::Done(_) => tasks::Status::Done,
        Ending::Failed { .. } => tasks::Status::Failed,
        Ending::Cancelled { .. } => tasks::Status::Cancelled,
    }
}
/// A dependency plan whose agent fault and restart choices are drawn from the
/// seed, with a cancel of the complete tree at the end.
#[must_use]
pub fn run_story(seed: u64) -> (Vec<String>, usize) {
    run_story_facts(seed, true)
}
#[must_use]
pub fn run_story_facts(seed: u64, consume_facts: bool) -> (Vec<String>, usize) {
    let mut rng = Rng::new(seed);
    let mut world = World::new(seed, LIMITS);
    world.consume_facts = consume_facts;
    let arrival = Duration::from_millis(rng.between(1, 20));
    world.env.now = world.env.now.saturating_add(arrival);
    world.env.wall = Wall::from_nanos(arrival.as_nanos());
    world.make(Party::Person(1), vec![task(1, &[])]);
    world.claim(1, 1);
    world.make(Party::Task(1), vec![task(2, &[]), task(3, &[2])]);
    world.claim_fresh(2);
    // Inbox decisions travel through the same durable root and real agents.
    let question = world.number();
    world.mail_number(question, 1, Party::Task(2), tasks::UserMessage::Question { words: Box::new([1]) });
    world.mail(2, Party::Task(1), tasks::UserMessage::Answer { question, words: Box::new([2]) });
    let reply_to = world.to();
    world.send(Event::Subscribe {
        reply_to,
        subscription: tasks::Subscription {
            number: 1,
            task: 1,
            kind: tasks::SubscriptionKind::Topic { connector: 1, topic: 1 },
            pending: false,
        },
    });
    for _ in 0..rng.between(1, 4) {
        let number = world.number();
        let reply_to = world.to();
        let class = match rng.below(3) {
            0 => tasks::NewsClass::Wakes,
            1 => tasks::NewsClass::Kept,
            2 => tasks::NewsClass::Dropped,
            _ => unreachable!("three news classes"),
        };
        world.send(Event::News { reply_to, number, subscription: 1, class, words: Box::new([3]) });
    }
    world.mail(1, Party::Person(1), tasks::UserMessage::Words { words: Box::new([4]) });
    let read = world.messages(1).last().expect("person words present").number;
    let reply_to = world.to();
    world.stage(Event::Turn { reply_to, task: 1, attempt: 1, turn: 1, read: Some(read) });
    if rng.chance(500) {
        world.durable();
    }
    world.restart();
    world.turn(1, 1, Some(read));
    match rng.below(4) {
        0 => {
            world.terminal(2, End::Refused);
            world.advance();
            world.claim_fresh(2);
        }
        1 => {
            let class = match rng.below(6) {
                0 => tasks::Class::Transient,
                1 => tasks::Class::Permanent,
                2 => tasks::Class::Run,
                3 => tasks::Class::Agent,
                4 => tasks::Class::Lost,
                5 => tasks::Class::Invalid,
                _ => unreachable!("draw below six"),
            };
            for failure in 0..3 {
                world.terminal(2, End::Failed(class));
                if failure < 2 {
                    world.advance();
                    world.claim_fresh(2);
                }
            }
            let reply_to = world.to();
            world.send(Event::Release { reply_to, task: 2 });
            world.claim_fresh(2);
        }
        2 | 3 => {}
        _ => unreachable!("draw below four"),
    }
    if rng.chance(500) {
        world.restart();
    }
    if rng.chance(200) {
        world.terminal(2, End::Finished { result: Result::Failure { reason: Box::new([1]) }, cancel_delegates: false });
        world.settle(2);
    } else {
        world.finish(2);
        world.settle(2);
        world.claim_fresh(3);
        world.finish(3);
        world.settle(3);
    }
    world.make(Party::Task(1), vec![task(4, &[])]);
    world.claim_fresh(4);
    world.make(Party::Task(4), vec![task(5, &[])]);
    world.claim_fresh(5);
    if rng.chance(500) {
        world.restart();
    }
    world.cancel(1, b"done");
    world.complete_cancel();
    assert_eq!(world.results.len(), 5, "all tasks have a durable result");
    (world.trace.lines().to_vec(), world.results.len())
}
