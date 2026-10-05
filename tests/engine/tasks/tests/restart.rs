use temper_engine_domain_tasks::{Active, End, Event, Hold, Key, Party, Phase, Refusal, Result, Status, Stored, Stub};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};
#[test]
fn make_and_claim_have_independent_before_and_after_durable_cuts() {
    let mut w = World::new(20, LIMITS);
    let reply_to = w.to();
    w.stage(Event::Make { reply_to, creator: Party::Person(1), batch: Box::new([task(1, &[])]) });
    assert_eq!(w.live(), 0);
    assert!(w.activations.is_empty());
    w.restart();
    assert_eq!(w.live(), 0);
    let reply_to = w.to();
    w.stage(Event::Make { reply_to, creator: Party::Person(1), batch: Box::new([task(2, &[])]) });
    w.durable();
    assert!(w.activations.is_empty());
    w.restart();
    assert!(w.activations.contains(&2));
    let reply_to = w.to();
    w.send(Event::Prepare { reply_to, task: 2 });
    let reply_to = w.to();
    w.stage(Event::Claim { reply_to, task: 2, attempt: 10 });
    assert!(w.runs.is_empty());
    w.restart();
    assert_eq!(w.record(2).attempt, 0);
    assert!(w.activations.contains(&2));
    let reply_to = w.to();
    w.send(Event::Prepare { reply_to, task: 2 });
    let reply_to = w.to();
    w.stage(Event::Claim { reply_to, task: 2, attempt: 11 });
    w.durable();
    assert!(w.runs.is_empty());
    w.restart();
    assert_eq!(w.runs.get(&2), Some(&11));
    assert_eq!(w.record(2).phase, Phase::Active(Active::Claimed { attempt: 11 }));
    w.finish(2);
    w.settle(2);
}
#[test]
fn closing_and_held_state_survive_restart_without_early_result() {
    let mut w = World::new(21, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.send(Event::Hold { task: 1, why: Hold::Stopped });
    w.restart();
    assert!(w.stops.contains(&(1, 1)));
    assert!(w.activations.is_empty());
    w.terminal(1, End::Parked);
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    w.cancel(1, b"stop");
    assert!(w.results.is_empty());
    w.restart();
    assert!(w.closing.contains(&1));
    assert!(w.results.is_empty());
    w.settle(1);
    w.restart();
    assert_eq!(w.results.len(), 1);
    w.referee.assert_passed(21);
}
#[test]
fn stale_claims_and_replayed_terminal_are_typed_and_do_not_mutate() {
    let mut w = World::new(22, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 4);
    let reply_to = w.to();
    w.send(Event::Claim { reply_to, task: 1, attempt: 4 });
    assert!(
        matches!(w.replies.last_key_value().expect("reply").1, Reply::Refused(problem) if problem.why == Refusal::Attempt)
    );
    w.terminal(1, End::Refused);
    let record = w.record(1).clone();
    let reply_to = w.to();
    w.send(Event::Activation {
        reply_to,
        task: 1,
        attempt: 4,
        end: End::Failed(temper_engine_domain_tasks::Class::Lost),
    });
    assert_eq!(w.record(1), &record);
    assert!(matches!(
        w.replies.last_key_value().expect("reply").1,
        Reply::Acknowledged(temper_engine_domain_tasks::Accepted::Already)
    ));
}
#[test]
fn ended_inputs_can_reload_stubs_and_forget_only_after_last_reference() {
    let mut w = World::new(23, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.finish(1);
    w.settle(1);
    assert!(!w.records.contains_key(&Key::Stub(1)));
    let reply_to = w.to();
    w.send(Event::RememberStub {
        reply_to,
        stub: Stub { number: 1, project: 1, status: Status::Done, attempt: 1, last_answer: Some(1) },
    });
    let mut next = task(2, &[]);
    next.spec.inputs = Box::new([1]);
    w.make(Party::Person(1), vec![next]);
    let reply_to = w.to();
    w.send(Event::ForgetStub { reply_to, task: 1 });
    assert!(
        matches!(w.replies.last_key_value().expect("reply").1, Reply::Refused(problem) if problem.why == Refusal::Busy)
    );
    w.restart();
    w.claim(2, 2);
    w.finish(2);
    w.settle(2);
    let reply_to = w.to();
    w.send(Event::ForgetStub { reply_to, task: 1 });
    assert!(!w.records.contains_key(&Key::Stub(1)));
}
#[test]
fn a_crash_before_finished_decision_resumes_current_attempt() {
    let mut w = World::new(24, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    let reply_to = w.to();
    w.stage(Event::Activation {
        reply_to,
        task: 1,
        attempt: 1,
        end: End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false },
    });
    assert!(w.results.is_empty());
    assert!(w.closing.is_empty());
    w.restart();
    assert_eq!(w.runs.get(&1), Some(&1));
    w.finish(1);
    w.settle(1);
}
#[test]
fn restore_rejects_corrupt_links_cycles_and_contracts_without_panicking() {
    use skein_lib::{Env, Queue, Time, Wall};
    use temper_engine_domain_tasks::{Domain, Request, max_out, step};
    let mut source = World::new(25, LIMITS);
    source.make(Party::Person(1), vec![task(1, &[]), task(2, &[])]);
    for corrupt in 0..4 {
        let mut domain = Domain::new(&LIMITS, 25, Box::new([1]));
        let env = Env { limits: LIMITS, now: Time::ZERO, wall: Wall::EPOCH };
        let mut out = Queue::with_capacity(max_out(&LIMITS));
        for number in [1, 2] {
            let mut record = source.record(number).clone();
            match corrupt {
                0 => record.root = 99,
                1 => {
                    record.phase = Phase::Waiting;
                    record.dependencies = Box::new([3 - number]);
                }
                2 => record.contract = temper_engine_domain_tasks::Contract::Report { words: 33 },
                3 => {
                    if number == 1 {
                        record.delegates = Box::new([2]);
                    } else {
                        record.requester = Party::Task(1);
                        record.root = 1;
                        record.depth = 1;
                    }
                }
                _ => unreachable!(),
            }
            step(&mut domain, &env, Event::Restore { record: Stored::Live(Box::new(record)) }, &mut out);
        }
        step(&mut domain, &env, Event::Restored, &mut out);
        assert!(!domain.ready());
        let mut rejected = false;
        while let Some(request) = out.pop() {
            if matches!(request, Request::RestoreRefused { .. }) {
                rejected = true;
            }
        }
        assert!(rejected);
    }
}
#[test]
fn terminal_and_settlement_each_have_a_durable_cut_before_delivery() {
    let mut w = World::new(26, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    let reply_to = w.to();
    w.stage(Event::Activation {
        reply_to,
        task: 1,
        attempt: 1,
        end: End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false },
    });
    w.durable();
    assert!(w.closing.is_empty());
    assert!(w.results.is_empty());
    w.restart();
    assert!(w.closing.contains(&1));
    assert!(!w.runs.contains_key(&1));
    w.observe(temper_engine_tasks_world::referee::Seen::Settled { task: 1 });
    w.stage(Event::Settled { task: 1 });
    w.durable();
    assert_eq!(w.results.len(), 1);
    assert!(!w.records.contains_key(&Key::Live(1)));
    w.restart();
    assert_eq!(w.results.len(), 1);
    assert!(w.closing.is_empty());
}
#[test]
fn cancellation_cut_after_durability_stops_adopted_runs_then_closes_deepest() {
    let mut w = World::new(27, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 2);
    w.make(Party::Task(2), vec![task(3, &[])]);
    w.claim(3, 3);
    w.observe(temper_engine_tasks_world::referee::Seen::Cancelled { task: 1 });
    let reply_to = w.to();
    w.stage(Event::Cancel { reply_to, task: 1, reason: Box::new([1]) });
    w.durable();
    assert!(w.stops.is_empty());
    w.restart();
    assert_eq!(w.stops.len(), 3);
    w.complete_cancel();
    assert_eq!(w.results.len(), 3);
}
#[test]
fn wall_correction_does_not_move_a_live_backoff_but_restore_reprojects_it() {
    use skein_lib::{Duration, Wall};
    let mut w = World::new(28, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.terminal(1, End::Refused);
    w.env.wall = Wall::from_nanos(Duration::from_secs(100).as_nanos());
    w.advance();
    assert!(w.activations.contains(&1));
    w.claim(1, 2);
    w.terminal(1, End::Refused);
    w.env.wall = Wall::from_nanos(Duration::from_secs(200).as_nanos());
    w.restart();
    w.advance();
    assert!(w.activations.contains(&1));
}
