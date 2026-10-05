use skein_lib::{Env, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain_tasks::{
    self as tasks, Active, AuthorityExecutor, Contract, Domain, End, Event, Grant, Hold, Last, Limits, Parameter,
    Party, Pattern, Request, Result, Status, Stored, Stub, Verdict,
};
use temper_engine_tasks_world::{LIMITS, task};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
struct Measured {
    d: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    serial: u64,
}
impl Measured {
    fn new(l: Limits) -> Measured {
        let out = Queue::with_capacity(tasks::max_out(&l));
        let meter = Meter::new();
        let d = Domain::new(&l, 1, Box::new([1]));
        Measured {
            d,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: l },
            out,
            meter,
            bound: tasks::worst_case(&l).expect("admitted bounds"),
            serial: 0,
        }
    }
    fn to(&mut self) -> ReplyTo {
        self.serial += 1;
        ReplyTo::new(Token::new(self.serial))
    }
    fn event(&mut self, event: Event) {
        self.meter.start();
        tasks::step(&mut self.d, &self.env, event, &mut self.out);
        let measured = self.meter.end();
        while let Some(request) = self.out.pop() {
            drop(request);
        }
        self.meter.check(measured, self.bound, self.env.limits);
        self.d.reclaim();
    }
    fn fire(&mut self) {
        self.env.now = self.d.next_deadline().expect("due");
        self.meter.start();
        tasks::fire(&mut self.d, &self.env, &mut self.out);
        let measured = self.meter.end();
        while let Some(request) = self.out.pop() {
            drop(request);
        }
        self.meter.check(measured, self.bound, self.env.limits);
        self.d.reclaim();
    }
    fn claim(&mut self, number: u64) {
        let reply_to = self.to();
        self.event(Event::Prepare { reply_to, task: number });
        let reply_to = self.to();
        self.event(Event::Claim { reply_to, task: number, attempt: number, readable: Box::new([]) });
        self.event(Event::Started { task: number, attempt: number });
    }
}
#[test]
fn saturated_payloads_graph_backoff_held_closing_retirement_and_restore_fit() {
    for l in [
        Limits {
            tasks: 4,
            stubs: 8,
            tree_tasks: 4,
            depth: 2,
            delegates: 3,
            batch: 4,
            dependencies: 3,
            inputs: 4,
            parameters: 4,
            ..LIMITS
        },
        Limits {
            tasks: 1,
            stubs: 2,
            tree_tasks: 1,
            depth: 0,
            delegates: 0,
            batch: 1,
            dependencies: 0,
            inputs: 1,
            parameters: 1,
            facts: 0,
            ..LIMITS
        },
    ] {
        let mut m = Measured::new(l);
        m.event(Event::Restored);
        for number in 100..100 + u64::from(l.inputs) {
            let reply_to = m.to();
            m.event(Event::RememberStub {
                reply_to,
                stub: Stub { number, project: 1, status: Status::Failed, attempt: 0, last_answer: None },
            });
        }
        let mut batch = Vec::new();
        for number in 1..=u64::from(l.tasks) {
            batch.push(filled(l, number));
        }
        let reply_to = m.to();
        m.event(Event::Make { reply_to, creator: Party::Person(1), batch: batch.into_boxed_slice() });
        for number in 1..=u64::from(l.tasks) {
            m.claim(number);
            let reply_to = m.to();
            m.event(Event::Activation { reply_to, task: number, attempt: number, end: End::Refused });
            m.fire();
            let reply_to = m.to();
            m.event(Event::Prepare { reply_to, task: number });
            let reply_to = m.to();
            m.event(Event::Claim { reply_to, task: number, attempt: number + 10, readable: Box::new([]) });
            let reply_to = m.to();
            m.event(Event::Activation {
                reply_to,
                task: number,
                attempt: number + 10,
                end: End::Finished {
                    result: Result::Verdict {
                        code: 0,
                        words: vec![1; usize::try_from(l.result_bytes).expect("small bound")].into_boxed_slice(),
                    },
                    cancel_delegates: false,
                },
            });
            let reply_to = m.to();
            m.event(Event::Cancel {
                reply_to,
                task: number,
                reason: vec![1; usize::try_from(l.result_bytes).expect("small bound")].into_boxed_slice(),
            });
            m.event(Event::Hold { task: number, why: Hold::Effects });
            m.event(Event::Settled { task: number });
            let reply_to = m.to();
            m.event(Event::Release { reply_to, task: number });
        }
    }
}
#[test]
fn full_delegate_tree_dependency_edges_and_cold_restored_claims_fit() {
    let l = Limits { tasks: 4, stubs: 8, tree_tasks: 4, delegates: 3, batch: 3, dependencies: 3, inputs: 0, ..LIMITS };
    let mut source = temper_engine_tasks_world::World::new(2, l);
    source.make(Party::Person(1), vec![task(1, &[])]);
    source.claim(1, 1);
    source.make(Party::Task(1), vec![task(2, &[]), task(3, &[2]), task(4, &[2, 3])]);
    source.claim(2, 2);
    let mut m = Measured::new(l);
    for row in source.records.values() {
        let record = row.clone();
        m.event(Event::Restore { record });
    }
    m.event(Event::Restored);
    let reply_to = m.to();
    m.event(Event::Cancel { reply_to, task: 1, reason: Box::new([1]) });
    for task in [2, 1] {
        let reply_to = m.to();
        m.event(Event::Activation { reply_to, task, attempt: task, end: End::Parked });
    }
    for task in [3, 4, 2, 1] {
        m.event(Event::Settled { task });
    }
    assert!(tasks::worst_case(&Limits { tasks: u32::MAX, ..l }).is_none());
    // Historical rows remain records, never live entities after restoration.
    let mut w = temper_engine_tasks_world::World::new(3, l);
    w.make(Party::Person(1), vec![task(10, &[])]);
    w.claim(10, 10);
    w.finish(10);
    w.settle(10);
    assert!(
        matches!(w.records.get(&tasks::Key::Ended(10)), Some(Stored::Ended(record)) if record.phase != tasks::Phase::Active(Active::Due))
    );
}

fn filled(l: Limits, number: u64) -> temper_engine_domain_tasks::New {
    let mut new = task(number, &[]);
    new.spec.words = vec![1; usize::try_from(l.spec_bytes).expect("small bound")].into_boxed_slice();
    new.spec.parameters =
        vec![Parameter::Number { name: 1, value: 1 }; usize::try_from(l.parameters).expect("small bound")]
            .into_boxed_slice();
    new.spec.inputs = (100..100 + u64::from(l.inputs)).collect::<Vec<_>>().into_boxed_slice();
    new.contract = Contract::Verdict {
        choices: (0..l.contract_choices)
            .map(|code| Verdict { code, words: l.result_bytes })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    };
    new.authority.grants = (0..l.authority_grants)
        .map(|kind| Grant {
            connector: 0,
            kind: u16::try_from(kind).expect("small bound"),
            pattern: Pattern {
                segments: (0..l.authority_segments)
                    .map(|_| Box::new([]) as Box<[u8]>)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                last: Last::Exact(if kind == 0 {
                    vec![1; usize::try_from(l.authority_bytes).expect("small bound")].into_boxed_slice()
                } else {
                    Box::new([])
                }),
            },
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    new.authority.delegation.kinds =
        vec![AuthorityExecutor::Charter(1); usize::try_from(l.executor_kinds).expect("small bound")].into_boxed_slice();
    new
}
#[test]
fn saturated_inboxes_offers_receipts_questions_subscriptions_and_restore_fit() {
    use temper_engine_domain_tasks::{Interest, NewsClass, Subscription, SubscriptionKind, UserMessage};
    let l = Limits {
        tasks: 2,
        stubs: 4,
        inbox_messages: 4,
        inbox_bytes: 256,
        message_bytes: 64,
        subscriptions: 2,
        questions: 1,
        receipts: 4,
        offers: 4,
        references: 1,
        ..LIMITS
    };
    let mut w = temper_engine_tasks_world::World::new(9, l);
    w.make(Party::Person(1), vec![task(1, &[]), task(2, &[])]);
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Person(1), left: 1, right: 2 });
    w.claim(1, 1);
    w.claim(2, 2);
    w.mail(2, Party::Task(1), UserMessage::Question { words: vec![1; 64].into_boxed_slice() });
    w.mail(1, Party::Task(2), UserMessage::Words { words: vec![2; 64].into_boxed_slice() });
    let reply_to = w.to();
    w.send(Event::Subscribe {
        reply_to,
        subscription: Subscription {
            number: 1,
            task: 1,
            kind: SubscriptionKind::Topic { connector: 1, topic: 1 },
            pending: false,
        },
    });
    let reply_to = w.to();
    w.send(Event::Subscribe {
        reply_to,
        subscription: Subscription {
            number: 2,
            task: 2,
            kind: SubscriptionKind::Task { target: 1, interest: Interest::StateAndResult },
            pending: false,
        },
    });
    for _ in 0..2 {
        let number = w.number();
        let reply_to = w.to();
        w.send(Event::News {
            reply_to,
            number,
            subscription: 1,
            class: NewsClass::Wakes,
            words: vec![3; 64].into_boxed_slice(),
        });
    }
    w.mail(2, Party::Person(1), UserMessage::Words { words: vec![4; 64].into_boxed_slice() });
    let rows = w.records.values().cloned().collect::<Vec<_>>();
    // The world's parent/store allocations predate this child's meter.
    let mut m = Measured::new(l);
    for record in &rows {
        m.event(Event::Restore { record: record.clone() });
    }
    m.event(Event::Restored);
    let reply_to = m.to();
    m.event(Event::Turn { reply_to, task: 1, attempt: 1, turn: 1, read: Some(3) });
    m.event(Event::Hold { task: 1, why: Hold::Stopped });
    let reply_to = m.to();
    m.event(Event::Activation { reply_to, task: 1, attempt: 1, end: End::Parked });
    let reply_to = m.to();
    m.event(Event::Release { reply_to, task: 1 });
}
