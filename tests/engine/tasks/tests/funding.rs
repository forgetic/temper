use skein_lib::ReplyTo;
use temper_engine_domain_tasks::{
    self as tasks, Accepted, End, Event, Funder, Key, Numbers, Party, Refusal, Result, Stored,
};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};
fn send(world: &mut World, build: impl FnOnce(ReplyTo) -> Event) -> Reply {
    let reply_to = world.to();
    let key = reply_to.into_token().raw();
    world.send(build(ReplyTo::new(skein_lib::Token::new(key))));
    world.replies[&key].clone()
}
fn turn(world: &mut World, task: u64, attempt: u64, turn: u32, read: Option<u64>, cumulative: u64) -> Reply {
    send(world, |reply_to| Event::ChargedTurn { reply_to, task, attempt, turn, read, cumulative })
}
fn end(world: &mut World, task: u64, attempt: u64, end: End, cumulative: u64) -> Reply {
    send(world, |reply_to| Event::ChargedActivation { reply_to, task, attempt, end, cumulative })
}
fn ledger(world: &World, funder: Funder) -> tasks::FundingRecord {
    match world.records[&Key::Ledger(funder)] {
        Stored::Ledger(record) => record,
        Stored::Live(_)
        | Stored::Ended(_)
        | Stored::Stub(_)
        | Stored::Message(_)
        | Stored::ArchivedMessage(_)
        | Stored::Receipt(_)
        | Stored::Offer(_)
        | Stored::Question(_)
        | Stored::Subscription(_)
        | Stored::History(_)
        | Stored::Closure(_)
        | Stored::Funding { .. }
        | Stored::Admission(_) => unreachable!(),
    }
}
#[test]
fn finite_pool_carves_reserves_and_posts_to_its_original_period_atomically() {
    let mut world = World::new(71, LIMITS);
    world.open_period(1, 300);
    world.carve_pool(1, 200);
    let period = Funder::Period { project: 1, period: 1 };
    let pool = Funder::Pool { project: 1, person: 9, period: 1 };
    assert_eq!(ledger(&world, pool).parent, Some(period));
    let mut first = task(1, &[]);
    first.funder = pool;
    let mut second = task(2, &[]);
    second.funder = pool;
    world.make(Party::Person(9), vec![first.clone()]);
    assert_eq!(ledger(&world, pool).numbers.reserved, 100);
    let before = world.records.clone();
    let reply = send(&mut world, |reply_to| Event::Make {
        reply_to,
        creator: Party::Person(9),
        batch: vec![second.clone(), {
            let mut third = second;
            third.number = 3;
            third
        }]
        .into_boxed_slice(),
    });
    assert!(matches!(reply, Reply::Refused(problem) if problem.why == Refusal::Funding));
    assert_eq!(world.records, before);
    world.claim(1, 1);
    assert_eq!(turn(&mut world, 1, 1, 1, None, 20), Reply::Turn(Accepted::New));
    world.open_period(2, 300);
    world.restart();
    assert_eq!(
        end(
            &mut world,
            1,
            1,
            End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false },
            30
        ),
        Reply::Acknowledged(Accepted::New)
    );
    world.settle(1);
    assert_eq!(ledger(&world, pool).numbers, Numbers { budget: 200, spent: 0, spent_below: 30, reserved: 0 });
    assert_eq!(ledger(&world, period).numbers.reserved, 200, "original pool carve remains reserved");
    assert_eq!(ledger(&world, Funder::Period { project: 1, period: 2 }).numbers.reserved, 0);
}
#[test]
fn charged_admissions_refuse_without_spending_and_replay_exactly_after_durable_cuts() {
    let mut world = World::new(72, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    for (attempt, read, why) in [(2, None, Refusal::Attempt), (1, Some(999), Refusal::Read)] {
        let before = world.records.clone();
        assert!(matches!(turn(&mut world, 1, attempt, 1, read, 20), Reply::Refused(problem) if problem.why == why));
        assert_eq!(world.records, before);
    }
    assert_eq!(turn(&mut world, 1, 1, 1, None, 20), Reply::Turn(Accepted::New));
    world.restart();
    let before = world.records.clone();
    assert_eq!(turn(&mut world, 1, 1, 1, None, 20), Reply::Turn(Accepted::Already));
    assert_eq!(world.records, before);
    assert!(
        matches!(turn(&mut world, 1, 1, 1, None, 21), Reply::Refused(problem) if problem.why == Refusal::KeyConflict)
    );
    let finish = End::Finished { result: Result::Report { words: Box::new([2]) }, cancel_delegates: false };
    let before = world.records.clone();
    assert!(
        matches!(end(&mut world, 1, 1, finish.clone(), 19), Reply::Refused(problem) if problem.why == Refusal::Turn)
    );
    assert_eq!(world.records, before);
    assert_eq!(end(&mut world, 1, 1, finish.clone(), 30), Reply::Acknowledged(Accepted::New));
    world.settle(1);
    world.restart();
    let before = world.records.clone();
    assert_eq!(end(&mut world, 1, 1, finish, 30), Reply::Acknowledged(Accepted::Already));
    assert_eq!(world.records, before);
}
#[test]
fn charged_terminals_normalize_invalid_narrowed_and_cancelled_results_and_refuse_live_delegates() {
    let mut world = World::new(73, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.make(Party::Task(1), vec![task(2, &[])]);
    world.claim(1, 1);
    let finish = End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false };
    let before = world.records.clone();
    assert!(
        matches!(end(&mut world, 1, 1, finish.clone(), 20), Reply::Refused(problem) if problem.why == Refusal::LiveDelegates)
    );
    assert_eq!(world.records, before);
    let invalid = End::Finished { result: Result::Verdict { code: 99, words: Box::new([1]) }, cancel_delegates: false };
    assert_eq!(end(&mut world, 1, 1, invalid.clone(), 20), Reply::Acknowledged(Accepted::New));
    assert_eq!(world.record(1).numbers.spent, 20);
    world.cancel(1, b"cancel");
    world.complete_cancel();
    let mut world = World::new(74, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    world.cancel(1, b"cancel");
    assert_eq!(end(&mut world, 1, 1, invalid, 7), Reply::Acknowledged(Accepted::New));
    world.settle(1);
    assert_eq!(world.results.len(), 1);
}
#[test]
fn charged_overrun_holds_and_accounting_overflow_refuses_before_acceptance() {
    let mut world = World::new(75, LIMITS);
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    assert_eq!(turn(&mut world, 1, 1, 1, None, 101), Reply::Turn(Accepted::New));
    assert!(matches!(world.record(1).phase, tasks::Phase::Held { why: tasks::Hold::Budget, .. }));
    assert_eq!(world.record(1).numbers.spent, 101);
    let mut world = World::new(76, LIMITS);
    world.open_period(1, u64::MAX);
    let mut first = task(1, &[]);
    first.funder = Funder::Period { project: 1, period: 1 };
    first.authority.budget.spend = 0;
    first.numbers.budget = 0;
    let mut second = first.clone();
    second.number = 2;
    world.make(Party::Person(9), vec![first, second]);
    world.claim(1, 1);
    world.claim(2, 2);
    assert_eq!(turn(&mut world, 1, 1, 1, None, u64::MAX - 5), Reply::Turn(Accepted::New));
    let before = world.records.clone();
    assert!(matches!(turn(&mut world, 2, 2, 1, None, 6), Reply::Refused(problem) if problem.why == Refusal::Funding));
    assert_eq!(world.records, before);
}
#[test]
fn admission_pressure_and_oversized_terminal_do_not_charge_or_copy() {
    let mut world = World::new(77, tasks::Limits { admissions: 1, ..LIMITS });
    world.make(Party::Person(9), vec![task(1, &[])]);
    world.claim(1, 1);
    let before = world.records.clone();
    assert!(
        matches!(end(&mut world, 1, 1, End::Finished { result: Result::Report { words: vec![0; 1024].into_boxed_slice() }, cancel_delegates: false }, 2), Reply::Refused(problem) if problem.why == Refusal::Contract)
    );
    assert_eq!(world.records, before);
    assert_eq!(turn(&mut world, 1, 1, 1, None, 1), Reply::Turn(Accepted::New));
    let before = world.records.clone();
    assert!(matches!(turn(&mut world, 1, 1, 2, None, 2), Reply::Refused(problem) if problem.why == Refusal::Busy));
    assert_eq!(world.records, before);
}

#[test]
fn external_referee_detects_completely_omitted_expected_saves() {
    use temper_engine_tasks_world::accounting_referee::Accounting;
    let mut world = World::new(78, LIMITS);
    let before = world.records.clone();
    world.make(Party::Person(9), vec![task(1, &[])]);
    let source = Funder::Period { project: 1, period: 0 };
    let mut judge = Accounting::default();
    judge.reset(&before);
    judge.emitted(source);
    assert_eq!(judge.committed(&world.records), Ok(()));
    for entirely in [false, true] {
        let mut bad = world.records.clone();
        bad.insert(Key::Funding(source), before[&Key::Funding(source)].clone());
        if entirely {
            bad.insert(Key::Ledger(source), before[&Key::Ledger(source)].clone());
        }
        let mut judge = Accounting::default();
        judge.reset(&before);
        assert!(judge.committed(&bad).is_err(), "omitted external saves entirely={entirely}");
    }
}

#[test]
fn narrowed_obsolete_terminal_charges_then_parks_and_corrupt_receipts_refuse_restore() {
    let mut world = World::new(79, LIMITS);
    let mut new = task(1, &[]);
    new.authority.tools = tasks::Tools(3);
    world.make(Party::Person(9), vec![new]);
    world.claim(1, 1);
    let before = world.record(1).authority.clone();
    let mut after = before.clone();
    after.tools = tasks::Tools(1);
    let message = world.number();
    assert_eq!(
        send(&mut world, |reply_to| Event::Amend {
            reply_to,
            task: 1,
            authorization: tasks::Authorization::Person { person: 9, project: 1 },
            amendment: tasks::Amendment {
                message,
                spec: None,
                policy: None,
                dependencies: None,
                tracked: None,
                authorities: Box::new([tasks::AuthorityChange {
                    task: 1,
                    before,
                    after,
                    budget: 100,
                    stop_run: true,
                    message
                }]),
                balances: Box::new([]),
                reason: Box::new([])
            }
        }),
        Reply::Done
    );
    let obsolete = End::Finished { result: Result::Verdict { code: 9, words: Box::new([1]) }, cancel_delegates: false };
    assert_eq!(end(&mut world, 1, 1, obsolete, 4), Reply::Acknowledged(Accepted::New));
    assert_eq!(world.record(1).numbers.spent, 4);
    assert!(
        matches!(world.record(1).phase, tasks::Phase::Active(tasks::Active::Idle | tasks::Active::Due)),
        "parked terminal may immediately wake for its unread amendment"
    );
    for receipt in [
        tasks::Admission::Turn { task: 0, attempt: 1, turn: 1, read: None, cumulative: 1 },
        tasks::Admission::Turn { task: 1, attempt: 0, turn: 1, read: None, cumulative: 1 },
        tasks::Admission::Turn { task: 1, attempt: 1, turn: 0, read: None, cumulative: 1 },
        tasks::Admission::Activation { task: 0, attempt: 1, end: End::Parked, cumulative: 1 },
        tasks::Admission::Activation {
            task: 1,
            attempt: 1,
            end: End::Finished {
                result: Result::Report { words: vec![1; 1024].into_boxed_slice() },
                cancel_delegates: false,
            },
            cumulative: 1,
        },
    ] {
        let mut domain = tasks::Domain::new(&LIMITS, 1, Box::new([1]));
        let mut out = skein_lib::Queue::with_capacity(tasks::max_out(&LIMITS));
        tasks::step(&mut domain, &world.env, Event::Restore { record: Stored::Admission(receipt) }, &mut out);
        assert!(matches!(out.pop(), Some(tasks::Request::RestoreRefused { .. })));
    }
}
