use temper_engine_domain_tasks::{
    Active, Class, Contract, End, Ending, Event, Hold, Key, Party, Phase, Refusal, Result, Stage, Was,
};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};
fn refused(reply: &Reply, why: Refusal) {
    match reply {
        Reply::Refused(problem) => assert_eq!(problem.why, why),
        Reply::Made(_) | Reply::Done | Reply::Acknowledged(_) => panic!("expected refusal {why:?}"),
    }
}
#[test]
fn batch_is_atomic_and_cycles_and_limits_refuse_at_entrance() {
    for (limits, why) in [
        (temper_engine_domain_tasks::Limits { batch: 1, ..LIMITS }, Refusal::Batch),
        (temper_engine_domain_tasks::Limits { tasks: 1, ..LIMITS }, Refusal::Live),
        (temper_engine_domain_tasks::Limits { project_tasks: 1, ..LIMITS }, Refusal::Project),
    ] {
        let mut w = World::new(1, limits);
        refused(&w.make(Party::Person(1), vec![task(1, &[]), task(2, &[])]), why);
        assert_eq!(w.live(), 0);
    }
    let mut w = World::new(1, LIMITS);
    refused(&w.make(Party::Person(1), vec![task(1, &[2]), task(2, &[1])]), Refusal::Cycle);
    assert_eq!(w.live(), 0);
    refused(&w.make(Party::Person(1), vec![task(1, &[]), task(1, &[])]), Refusal::Duplicate);
    refused(&w.make(Party::Person(1), vec![]), Refusal::Empty);
    for why in [
        Refusal::Spec,
        Refusal::Contract,
        Refusal::Executor,
        Refusal::Dependencies,
        Refusal::AuthorityShape,
        Refusal::Inputs,
    ] {
        let mut new = task(2, &[]);
        match why {
            Refusal::Spec => new.spec.words = vec![1; 65].into_boxed_slice(),
            Refusal::Contract => new.contract = Contract::Report { words: 33 },
            Refusal::Executor => new.executor = temper_engine_domain_tasks::Executor::Agent { charter: 99 },
            Refusal::Dependencies => new.dependencies = Box::new([99]),
            Refusal::AuthorityShape => {
                new.authority.delegation.kinds =
                    vec![temper_engine_domain_tasks::AuthorityExecutor::Role(1); 4].into_boxed_slice();
            }
            Refusal::Inputs => new.spec.inputs = Box::new([99]),
            Refusal::NotReady
            | Refusal::Unknown
            | Refusal::Busy
            | Refusal::Duplicate
            | Refusal::Empty
            | Refusal::Batch
            | Refusal::Live
            | Refusal::Project
            | Refusal::Tree
            | Refusal::Depth
            | Refusal::Delegates
            | Refusal::Cycle
            | Refusal::State
            | Refusal::Attempt
            | Refusal::LiveDelegates
            | Refusal::Unheld
            | Refusal::Reason
            | Refusal::Restore => unreachable!(),
        }
        refused(&w.make(Party::Person(1), vec![task(1, &[]), new]), why);
        assert_eq!(w.live(), 0);
    }
}
#[test]
fn tree_depth_delegates_and_stub_reservation_are_atomic() {
    for (limits, why) in [
        (temper_engine_domain_tasks::Limits { tree_tasks: 1, ..LIMITS }, Refusal::Tree),
        (temper_engine_domain_tasks::Limits { depth: 0, ..LIMITS }, Refusal::Depth),
        (temper_engine_domain_tasks::Limits { delegates: 0, ..LIMITS }, Refusal::Delegates),
    ] {
        let mut w = World::new(2, limits);
        w.make(Party::Person(1), vec![task(1, &[])]);
        refused(&w.make(Party::Task(1), vec![task(2, &[])]), why);
        assert!(w.record(1).delegates.is_empty());
        assert_eq!(w.live(), 1);
    }
    let limits = temper_engine_domain_tasks::Limits { tasks: 2, stubs: 2, ..LIMITS };
    let mut w = World::new(2, limits);
    let reply_to = w.to();
    w.send(Event::RememberStub {
        reply_to,
        stub: temper_engine_domain_tasks::Stub {
            number: 99,
            project: 1,
            status: temper_engine_domain_tasks::Status::Done,
            attempt: 0,
            last_answer: None,
        },
    });
    refused(&w.make(Party::Person(1), vec![task(1, &[]), task(2, &[])]), Refusal::Busy);
    assert_eq!(w.live(), 0);
}
#[test]
fn dependency_order_negative_verdict_starts_but_failure_holds() {
    for fail in [false, true] {
        let mut w = World::new(3, LIMITS);
        let mut first = task(1, &[]);
        first.contract =
            Contract::Verdict { choices: Box::new([temper_engine_domain_tasks::Verdict { code: 0, words: 32 }]) };
        w.make(Party::Person(1), vec![first, task(2, &[1])]);
        assert!(!w.activations.contains(&2));
        w.claim(1, 1);
        let result = if fail {
            Result::Failure { reason: Box::new([1]) }
        } else {
            Result::Verdict { code: 0, words: Box::new([]) }
        };
        w.terminal(1, End::Finished { result, cancel_delegates: false });
        assert!(!w.activations.contains(&2));
        w.settle(1);
        if fail {
            assert_eq!(w.record(2).phase, Phase::Held { was: Was::Waiting, why: Hold::Dependency(1) });
            let reply_to = w.to();
            w.send(Event::Release { reply_to, task: 2 });
            assert_eq!(w.record(2).phase, Phase::Held { was: Was::Waiting, why: Hold::Dependency(1) });
        } else {
            w.claim(2, 2);
            w.finish(2);
            w.settle(2);
        }
    }
}
#[test]
fn every_failure_class_holds_after_tries_and_release_resets_tries_only() {
    for class in [Class::Transient, Class::Permanent, Class::Run, Class::Agent, Class::Lost, Class::Invalid] {
        let mut w = World::new(4, LIMITS);
        w.make(Party::Person(1), vec![task(1, &[])]);
        for attempt in 1..=3 {
            w.claim(1, attempt);
            w.terminal(1, End::Failed(class));
            if attempt < 3 {
                w.advance();
            }
        }
        assert_eq!(w.record(1).phase, Phase::Held { was: Was::Active(Active::Due), why: Hold::Failures(class) });
        assert_eq!(w.record(1).tries.of(class), 3);
        let reply_to = w.to();
        w.send(Event::Release { reply_to, task: 1 });
        assert_eq!(w.record(1).tries.of(class), 0);
        assert_eq!(w.record(1).attempt, 3);
        w.claim(1, 4);
        w.finish(1);
        w.settle(1);
    }
}
#[test]
fn refusals_and_preparation_failure_do_not_spend_tries() {
    let mut w = World::new(5, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    let reply_to = w.to();
    w.send(Event::Prepare { reply_to, task: 1 });
    w.send(Event::PreparationFailed { task: 1 });
    assert_eq!(w.record(1).attempt, 0);
    w.advance();
    for attempt in 1..=5 {
        w.claim(1, attempt);
        w.terminal(1, End::Refused);
        w.advance();
        assert_eq!(w.record(1).tries, temper_engine_domain_tasks::Tries::NONE);
    }
    w.claim(1, 6);
    w.finish(1);
    w.settle(1);
}
#[test]
fn held_running_task_waits_for_terminal_and_retains_closing() {
    let mut w = World::new(6, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.send(Event::Hold { task: 1, why: Hold::Stopped });
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    assert!(
        matches!(w.replies.last_key_value().expect("reply").1, Reply::Refused(problem) if problem.why == Refusal::Busy)
    );
    w.terminal(1, End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false });
    assert!(w.closing.is_empty());
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    assert!(w.closing.contains(&1));
    w.send(Event::Hold { task: 1, why: Hold::Effects });
    w.settle(1);
    assert!(w.records.contains_key(&Key::Live(1)));
    assert!(matches!(
        w.record(1).phase,
        Phase::Held { was: Was::Closing(temper_engine_domain_tasks::Closing { stage: Stage::Settled, .. }), .. }
    ));
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    assert!(matches!(w.results.get(&1), Some(Ending::Done(_))));
}
#[test]
fn finish_with_live_delegates_can_be_corrected_and_cancel_is_deepest_first() {
    let mut w = World::new(7, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 2);
    w.make(Party::Task(2), vec![task(3, &[])]);
    w.claim(3, 3);
    refused(
        &w.terminal(1, End::Finished { result: Result::Report { words: Box::new([]) }, cancel_delegates: false }),
        Refusal::LiveDelegates,
    );
    assert!(w.runs.contains_key(&1));
    w.cancel(1, b"stop");
    assert!(w.closing.is_empty());
    w.terminal(3, End::Finished { result: Result::Report { words: Box::new([9]) }, cancel_delegates: false });
    w.terminal(2, End::Parked);
    w.terminal(1, End::Parked);
    assert_eq!(w.closing.iter().copied().collect::<Vec<_>>(), [3]);
    w.settle(3);
    assert!(w.closing.contains(&2));
    w.settle(2);
    assert!(w.closing.contains(&1));
    w.settle(1);
    assert!(
        matches!(w.results.get(&3), Some(Ending::Cancelled { result: Some(Result::Report { words }), .. }) if words.as_ref() == [9])
    );
    w.referee.assert_passed(7);
}
#[test]
fn lifetime_tree_limit_survives_delegate_ending() {
    let mut w = World::new(8, temper_engine_domain_tasks::Limits { tree_tasks: 2, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 1);
    w.finish(2);
    w.settle(2);
    assert_eq!(w.record(1).made, 2);
    refused(&w.make(Party::Task(1), vec![task(3, &[])]), Refusal::Tree);
}
#[test]
fn invalid_results_spend_invalid_tries_and_corrected_finish_cancels_delegates() {
    let mut w = World::new(9, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.terminal(1, End::Finished { result: Result::Verdict { code: 0, words: Box::new([]) }, cancel_delegates: false });
    assert_eq!(w.record(1).tries.invalid, 1);
    assert!(w.closing.is_empty());
    w.advance();
    w.claim(1, 2);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(2, 3);
    refused(
        &w.terminal(1, End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: false }),
        Refusal::LiveDelegates,
    );
    w.terminal(1, End::Finished { result: Result::Report { words: Box::new([1]) }, cancel_delegates: true });
    assert!(w.stops.contains(&(2, 3)));
    assert!(!w.closing.contains(&1));
    w.terminal(2, End::Parked);
    w.settle(2);
    assert!(w.closing.contains(&1));
    w.settle(1);
    assert!(matches!(w.results.get(&1), Some(Ending::Done(_))));
    assert!(matches!(w.results.get(&2), Some(Ending::Cancelled { .. })));
}
#[test]
fn held_closing_refuses_new_delegates_atomically_and_restores_through_release() {
    let mut w = World::new(10, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    w.finish(1);
    w.send(Event::Hold { task: 1, why: Hold::Effects });
    let before = w.records.clone();
    let problem = w.make(Party::Task(1), vec![task(2, &[]), task(3, &[2])]);
    assert_eq!(problem, Reply::Refused(temper_engine_domain_tasks::Problem { task: Some(1), why: Refusal::State }));
    assert_eq!(w.records, before, "refused batch writes no task or ancestor record");
    assert_eq!(w.live(), 1);
    assert!(!w.activations.contains(&2) && !w.activations.contains(&3));
    w.restart();
    assert_eq!(w.records, before, "held closing remains a valid durable state");
    w.settle(1);
    assert!(w.results.is_empty(), "settlement preserves the hold");
    let settled = w.records.clone();
    refused(&w.make(Party::Task(1), vec![task(4, &[])]), Refusal::State);
    assert_eq!(w.records, settled, "settled prior state cannot gain a delegate either");
    w.restart();
    assert_eq!(w.records, settled, "settled held state restores before release");
    let reply_to = w.to();
    w.send(Event::Release { reply_to, task: 1 });
    assert_eq!(w.live(), 0);
    assert_eq!(w.results.len(), 1);
    assert_eq!(w.results.get(&1), Some(&Ending::Done(Result::Report { words: Box::new([1]) })));
    w.restart();
    assert_eq!(w.results.len(), 1, "requester result remains committed once");
    w.referee.assert_passed(10);
}
