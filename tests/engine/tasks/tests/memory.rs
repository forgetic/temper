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
    refused: bool,
}
impl Measured {
    fn new(l: &Limits) -> Measured {
        let l = *l;
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
            refused: false,
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
        self.refused = false;
        while let Some(request) = self.out.pop() {
            self.refused |= matches!(request, Request::Refused { .. });
            drop(request);
        }
        self.meter.check(measured, self.bound, self.env.limits);
        self.d.reclaim();
    }
    fn bootstrap(&mut self) {
        self.event(Event::Restored);
        for period in [0, 7] {
            let reply_to = self.to();
            self.event(Event::OpenPeriod { reply_to, project: 1, period, budget: 100_000 });
        }
        let reply_to = self.to();
        self.event(Event::CarvePool { reply_to, project: 1, person: 9, period: 7, budget: 100_000 });
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
        let mut m = Measured::new(&l);
        m.bootstrap();
        for number in 100..100 + u64::from(l.inputs) {
            let reply_to = m.to();
            m.event(Event::RememberStub {
                reply_to,
                stub: Stub { number, project: 1, status: Status::Failed, attempt: 0, last_answer: None },
            });
        }
        let mut batch = Vec::new();
        for number in 1..=u64::from(l.tasks) {
            batch.push(filled(&l, number));
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
    let mut m = Measured::new(&l);
    for row in source.records.values() {
        let record = row.clone();
        m.event(Event::Restore { record });
    }
    m.bootstrap();
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

fn filled(l: &Limits, number: u64) -> temper_engine_domain_tasks::New {
    let l = *l;
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
    let mut m = Measured::new(&l);
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

#[test]
fn rejected_words_questions_and_answers_are_not_copied_before_the_byte_check() {
    use temper_engine_domain_tasks::{Problem, Refusal, UserMessage};
    for kind in 0..3 {
        let l = Limits { tasks: 2, stubs: 4, ..LIMITS };
        let mut m = Measured::new(&l);
        m.bootstrap();
        let reply_to = m.to();
        m.event(Event::Make { reply_to, creator: Party::Person(1), batch: Box::new([task(1, &[]), task(2, &[])]) });
        let reply_to = m.to();
        m.event(Event::Introduce { reply_to, by: Party::Person(1), left: 1, right: 2 });
        let reply_to = m.to();
        m.event(Event::Send {
            reply_to,
            number: 1,
            task: 2,
            from: Party::Task(1),
            message: UserMessage::Question { words: Box::new([1]) },
        });
        let input_bytes = m.bound.checked_add(65_536).expect("small test bound");
        let words = vec![1; usize::try_from(input_bytes).expect("small input")].into_boxed_slice();
        let (target, from, message) = match kind {
            0 => (2, Party::Task(1), UserMessage::Words { words }),
            1 => (2, Party::Task(1), UserMessage::Question { words }),
            2 => (1, Party::Task(2), UserMessage::Answer { question: 1, words }),
            _ => unreachable!("three input shapes"),
        };
        let reply_to = m.to();
        // The caller owns this already allocated input before admission.
        // Allow it once at the measured peak, but no unbounded domain copy.
        m.meter.start();
        tasks::step(&mut m.d, &m.env, Event::Send { reply_to, number: 2, task: target, from, message }, &mut m.out);
        let measured = m.meter.end();
        let Some(Request::Refused { problem, .. }) = m.out.pop() else { panic!("oversized message refused") };
        assert_eq!(problem, Problem { task: Some(target), why: Refusal::Message });
        assert!(m.out.is_empty());
        m.meter.check(measured, m.bound.checked_add(input_bytes).expect("small peak bound"), kind);
        assert!(m.meter.held() <= m.bound, "no rejected payload retained");
    }
}
#[test]
fn dedicated_amendment_slots_and_bottom_up_move_scratch_fit_counted_memory() {
    let l = Limits { tasks: 8, project_tasks: 8, stubs: 16, tree_tasks: 8, depth: 2, delegates: 7, batch: 7, ..LIMITS };
    let mut m = Measured::new(&l);
    m.bootstrap();
    let mut root = task(1, &[]);
    root.numbers.budget = 10_000;
    root.authority.budget.spend = 10_000;
    let reply_to = m.to();
    m.event(Event::Make { reply_to, creator: Party::Person(1), batch: Box::new([root]) });
    let mut children = Vec::new();
    for number in 2..=8 {
        let mut child = task(number, &[]);
        child.spec.words = vec![1; l.spec_bytes as usize].into_boxed_slice();
        child.funder = tasks::Funder::Task(1);
        children.push(child);
    }
    let reply_to = m.to();
    m.event(Event::Make { reply_to, creator: Party::Task(1), batch: children.into_boxed_slice() });
    assert!(!m.refused, "full funding tree admitted");
    for task in 1..=8 {
        let reply_to = m.to();
        m.event(Event::Amend {
            reply_to,
            task,
            authorization: tasks::Authorization::Person { person: 9, project: 1 },
            amendment: tasks::Amendment {
                message: task,
                spec: None,
                policy: None,
                dependencies: None,
                tracked: None,
                authorities: Box::new([]),
                balances: Box::new([]),
                reason: vec![1; l.message_bytes as usize].into_boxed_slice(),
            },
        });
    }
    let source = tasks::Funder::Period { project: 1, period: 0 };
    let destination = tasks::Funder::Pool { project: 1, person: 9, period: 7 };
    let before = tasks::Numbers { budget: 100_000, spent: 0, spent_below: 0, reserved: 10_000 };
    let reply_to = m.to();
    m.event(Event::Move {
        reply_to,
        task: 1,
        authorization: tasks::Authorization::Person { person: 9, project: 1 },
        movement: tasks::Movement {
            to: Party::Person(9),
            transfers: Box::new([tasks::Transfer { task: 1, before: source, after: destination }]),
            balances: Box::new([
                tasks::Balance { funder: source, before, after: tasks::Numbers { reserved: 0, ..before } },
                tasks::Balance { funder: destination, before: tasks::Numbers { reserved: 0, ..before }, after: before },
            ]),
            reason: Box::new([1]),
        },
    });
    assert!(!m.refused, "worst-case normalization admitted");
}

#[test]
fn saturated_ordinary_and_two_immutable_control_offers_fit_counted_memory_and_restore() {
    let l = Limits { tasks: 1, stubs: 2, offers: 1, tree_tasks: 1, ..LIMITS };
    let mut w = temper_engine_tasks_world::World::new(19, l);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.mail(
        1,
        Party::Person(9),
        tasks::UserMessage::Words { words: vec![1; l.message_bytes as usize].into_boxed_slice() },
    );
    let mut m = Measured::new(&l);
    for record in w.records.values().filter(|row| !matches!(row, Stored::Funding { .. })) {
        m.event(Event::Restore { record: record.clone() });
    }
    m.event(Event::Restored);
    for message in 2..=4 {
        let reply_to = m.to();
        m.event(Event::Amend {
            reply_to,
            task: 1,
            authorization: tasks::Authorization::Person { person: 9, project: 1 },
            amendment: tasks::Amendment {
                message,
                spec: None,
                policy: None,
                dependencies: None,
                tracked: None,
                authorities: Box::new([]),
                balances: Box::new([]),
                reason: vec![1; l.message_bytes as usize].into_boxed_slice(),
            },
        });
        assert_eq!(m.refused, message == 4, "third immutable control refuses before copying");
    }
    let reply_to = m.to();
    m.event(Event::Turn { reply_to, task: 1, attempt: 1, turn: 1, read: Some(2) });
    assert!(!m.refused);
}

#[test]
fn restored_full_control_offer_partition_fits_counted_memory() {
    let l = Limits { tasks: 1, stubs: 2, offers: 1, tree_tasks: 1, ..LIMITS };
    let mut w = temper_engine_tasks_world::World::new(20, l);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.mail(
        1,
        Party::Person(9),
        tasks::UserMessage::Words { words: vec![1; l.message_bytes as usize].into_boxed_slice() },
    );
    for message in 2..=3 {
        let reply_to = w.to();
        w.send(Event::Amend {
            reply_to,
            task: 1,
            authorization: tasks::Authorization::Person { person: 9, project: 1 },
            amendment: tasks::Amendment {
                message,
                spec: None,
                policy: None,
                dependencies: None,
                tracked: None,
                authorities: Box::new([]),
                balances: Box::new([]),
                reason: vec![1; l.message_bytes as usize].into_boxed_slice(),
            },
        });
    }
    let mut m = Measured::new(&l);
    for record in w.records.values().filter(|row| !matches!(row, Stored::History(_) | Stored::Funding { .. })) {
        m.event(Event::Restore { record: record.clone() });
    }
    m.event(Event::Restored);
    assert!(!m.refused);
}

#[test]
fn saturated_charged_receipts_and_oversized_terminal_fit_before_copying() {
    let l = Limits { admissions: 4, ..LIMITS };
    let mut measured = Measured::new(&l);
    measured.bootstrap();
    let reply_to = measured.to();
    measured.event(Event::Make { reply_to, creator: Party::Person(1), batch: Box::new([task(1, &[])]) });
    measured.claim(1);
    for turn in 1..=4 {
        let reply_to = measured.to();
        measured.event(Event::ChargedTurn {
            reply_to,
            task: 1,
            attempt: 1,
            turn,
            read: None,
            cumulative: u64::from(turn),
        });
        assert!(!measured.refused);
    }
    let reply_to = measured.to();
    measured.event(Event::ChargedActivation {
        reply_to,
        task: 1,
        attempt: 1,
        end: End::Finished {
            result: Result::Report { words: vec![1; 32_768].into_boxed_slice() },
            cancel_delegates: false,
        },
        cumulative: 5,
    });
    assert!(measured.refused);
    let reply_to = measured.to();
    measured.event(Event::ChargedActivation { reply_to, task: 1, attempt: 1, end: End::Parked, cumulative: 5 });
    assert!(measured.refused);
    let l = Limits { admissions: 4, ..LIMITS };
    let mut measured = Measured::new(&l);
    for task in 1..=4 {
        measured.event(Event::Restore {
            record: Stored::Admission(tasks::Admission::Activation {
                task,
                attempt: 1,
                end: End::Finished {
                    result: Result::Report { words: vec![1; l.result_bytes as usize].into_boxed_slice() },
                    cancel_delegates: false,
                },
                cumulative: task,
            }),
        });
    }
}

#[test]
fn borrowed_stored_bytes_matches_independent_allocator_for_deep_and_malformed_rows() {
    let mut world = temper_engine_tasks_world::World::new(80, LIMITS);
    world.make(Party::Person(1), vec![task(1, &[])]);
    let mut record = world.record(1).clone();
    let new = filled(&LIMITS, 1);
    record.spec = new.spec;
    record.spec.parameters = Box::new([
        Parameter::Bytes { name: 1, value: vec![1; 4096].into_boxed_slice() },
        Parameter::Number { name: 2, value: 5 },
        Parameter::Resource { name: 3, connector: 1, resource: 2 },
    ]);
    record.authority = new.authority;
    record.authority.grants[0].pattern.segments = Box::new([Box::new([1_u8, 2]) as Box<[u8]>, Box::new([3])]);
    record.authority.grants[1].pattern.last = Last::Open(Box::new([4, 5]));
    record.contract = new.contract;
    record.dependencies = Box::new([1, 2, 3]);
    record.delegates = Box::new([4, 5]);
    record.references = Box::new([6]);
    record.results_due = Box::new([7, 8]);
    let ending = tasks::Ending::Cancelled {
        reason: Box::new([1, 2]),
        result: Some(Result::Report { words: Box::new([3, 4, 5]) }),
    };
    record.phase = tasks::Phase::Held {
        was: tasks::Was::Closing(tasks::Closing { stage: tasks::Stage::Delegates, ending: ending.clone() }),
        why: Hold::Budget,
    };
    let mut rows = world.records.values().cloned().collect::<Vec<_>>();
    rows.push(Stored::Live(Box::new(record.clone())));
    record.phase = tasks::Phase::Ended(ending.clone());
    rows.push(Stored::Ended(Box::new(record)));
    for message in [
        tasks::Message::Words { words: Box::new([1]) },
        tasks::Message::Question { words: Box::new([1, 2]) },
        tasks::Message::Answer { question: 1, words: Box::new([1, 2, 3]) },
        tasks::Message::Amendment { revision: 1, reason: Box::new([1, 2]) },
        tasks::Message::News { subscription: 1, class: tasks::NewsClass::Kept, words: Box::new([1, 2]) },
        tasks::Message::Result { task: 1, ending: ending.clone() },
        tasks::Message::Notice { subscription: 1, target: 1, notice: tasks::Notice::Ended(ending) },
        tasks::Message::Notice { subscription: 1, target: 1, notice: tasks::Notice::Held(Hold::Budget) },
        tasks::Message::Timer { subscription: 1, at: Wall::EPOCH },
    ] {
        let envelope = tasks::Envelope {
            number: 1,
            task: 1,
            from: Party::Person(1),
            message,
            at: Wall::EPOCH,
            hits: 1,
            eligible: true,
        };
        rows.push(Stored::Message(envelope.clone()));
        rows.push(Stored::ArchivedMessage(envelope.clone()));
        rows.push(Stored::Offer(tasks::Offer { attempt: 1, envelope }));
    }
    rows.push(Stored::History(tasks::History {
        task: 1,
        revision: 1,
        by: Party::Person(1),
        reason: Box::new([1, 2]),
        change: tasks::Change::Amended,
    }));
    for message in [
        tasks::UserMessage::Words { words: Box::new([1]) },
        tasks::UserMessage::Question { words: Box::new([1, 2]) },
        tasks::UserMessage::Answer { question: 1, words: Box::new([1, 2, 3]) },
    ] {
        rows.push(Stored::Receipt(tasks::Receipt { number: 1, task: 1, from: Party::Person(1), message }));
    }
    for result in [
        Result::Report { words: Box::new([1]) },
        Result::Verdict { code: 1, words: Box::new([1, 2]) },
        Result::Change { connector: 1, kind: 1, resource: 1, words: Box::new([1, 2, 3]) },
        Result::Failure { reason: vec![1; 4096].into_boxed_slice() },
    ] {
        rows.push(Stored::Admission(tasks::Admission::Activation {
            task: 1,
            attempt: 1,
            end: End::Finished { result, cancel_delegates: false },
            cumulative: 1,
        }));
    }
    for row in rows {
        let meter = Meter::new();
        let cloned = row.clone();
        let actual = meter.held();
        assert_eq!(tasks::stored_bytes(&cloned), Some(actual));
        assert_eq!(meter.held(), actual, "borrowed measurement allocates nothing");
    }
}
