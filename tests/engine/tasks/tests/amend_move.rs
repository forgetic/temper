use skein_lib::ReplyTo;
use temper_engine_domain_tasks::{
    self as tasks, Amendment, AuthorityChange, Authorization, Balance, Control, End, Event, Funder, Key, Movement,
    Numbers, Party, Refusal, Spec, Stored, Transfer,
};
use temper_engine_tasks_world::{LIMITS, Reply, World, task};
fn blank(number: u64) -> Amendment {
    Amendment {
        message: number,
        spec: None,
        policy: None,
        dependencies: None,
        tracked: None,
        authorities: Box::new([]),
        balances: Box::new([]),
        reason: Box::new([7]),
    }
}
fn person() -> Authorization {
    Authorization::Person { person: 9, project: 1 }
}
fn amend(w: &mut World, target: u64, authorization: Authorization, amendment: Amendment) -> Reply {
    let token = w.to().into_token();
    w.send(Event::Amend { reply_to: ReplyTo::new(token), task: target, authorization, amendment });
    w.replies[&token.raw()].clone()
}
fn movement(w: &mut World, movement: Movement) -> Reply {
    let token = w.to().into_token();
    w.send(Event::Move { reply_to: ReplyTo::new(token), task: 2, authorization: person(), movement });
    w.replies[&token.raw()].clone()
}
fn charge(w: &mut World, task: u64, cumulative: u64) {
    let reply_to = w.to();
    let attempt = w.record(task).attempt;
    w.send(Event::Charge { reply_to, task, attempt, cumulative });
}
fn funded(number: u64, budget: u64, funder: Funder) -> tasks::New {
    let mut new = task(number, &[]);
    new.authority.budget.spend = budget;
    new.numbers.budget = budget;
    new.funder = funder;
    new
}
fn funding_world(seed: u64, facts: bool) -> World {
    let mut w = World::new(seed, LIMITS);
    w.consume_facts = facts;
    w.open_period(7, 10_000);
    w.carve_pool(7, 1000);
    w.make(Party::Person(1), vec![funded(1, 1000, Funder::Period { project: 1, period: 0 })]);
    w.make(Party::Task(1), vec![funded(2, 300, Funder::Task(1))]);
    // An accepted proposal's holder funds a delegate directly, independently
    // of the delegate's requester; moving 2 must transfer this separate edge.
    w.make(Party::Task(2), vec![funded(3, 100, Funder::Task(1))]);
    w.make(Party::Task(3), vec![funded(4, 40, Funder::Task(3))]);
    for number in 1..=4 {
        w.claim(number, number);
    }
    charge(&mut w, 2, 30);
    charge(&mut w, 3, 10);
    charge(&mut w, 4, 5);
    w
}
fn funded_move(w: &World) -> Movement {
    let pool = Funder::Pool { project: 1, person: 9, period: 7 };
    Movement {
        to: Party::Person(9),
        transfers: Box::new([
            Transfer { task: 2, before: Funder::Task(1), after: pool },
            Transfer { task: 3, before: Funder::Task(1), after: pool },
        ]),
        balances: Box::new([
            Balance {
                funder: Funder::Task(1),
                before: w.record(1).numbers,
                after: Numbers { budget: 1000, spent: 0, spent_below: 45, reserved: 0 },
            },
            Balance {
                funder: pool,
                before: Numbers { budget: 1000, spent: 0, spent_below: 0, reserved: 0 },
                after: Numbers { budget: 1000, spent: 0, spent_below: 0, reserved: 355 },
            },
        ]),
        reason: Box::new([1]),
    }
}
#[test]
fn amendments_remove_waits_and_reach_live_runs_with_immutable_merged_offers() {
    let mut w = World::new(1, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[]), task(2, &[1])]);
    let mut change = blank(w.number());
    change.dependencies = Some(Box::new([99]));
    let before = w.records.clone();
    assert!(
        matches!(amend(&mut w, 2, person(), change), Reply::Refused(problem) if problem.why == Refusal::Dependencies)
    );
    assert_eq!(before, w.records);
    let initial = w.number();
    let mut change = blank(initial);
    change.dependencies = Some(Box::new([]));
    change.spec = Some(Spec { words: Box::new([8]), parameters: Box::new([]), inputs: Box::new([]) });
    assert_eq!(amend(&mut w, 2, person(), change), Reply::Done);
    assert!(w.activations.contains(&2));
    w.claim(2, 1);
    w.turn(2, 1, Some(initial));
    let first = w.number();
    let mut change = blank(first);
    change.policy = Some(tasks::WakePolicy { words: tasks::Rule::Never, ..tasks::WakePolicy::DEFAULT });
    assert_eq!(amend(&mut w, 2, person(), change), Reply::Done);
    let second = w.number();
    assert_eq!(amend(&mut w, 2, person(), blank(second)), Reply::Done);
    assert_eq!(w.messages(2).len(), 1);
    assert_eq!(w.messages(2)[0].number, second);
    assert!(w.relays.contains_key(&(2, 1, first)));
    w.restart();
    w.turn(2, 2, Some(first));
    assert_eq!(w.messages(2)[0].number, second, "reading immutable prior offer keeps replacement");
    w.cancel(1, b"end");
    w.cancel(2, b"end");
    w.complete_cancel();
}
#[test]
fn narrowing_stops_affected_descendants_and_checked_controls_validate_standing() {
    let mut w = World::new(2, LIMITS);
    let mut parent = task(1, &[]);
    parent.authority.tools = tasks::Tools(3);
    let mut child = task(2, &[]);
    child.authority.tools = tasks::Tools(3);
    w.make(Party::Person(1), vec![parent]);
    w.make(Party::Task(1), vec![child]);
    w.claim(1, 1);
    w.claim(2, 2);
    let mut change = blank(w.number());
    let before = w.records.clone();
    assert!(
        matches!(amend(&mut w, 1, Authorization::Tree(Party::Task(2)), change.clone()), Reply::Refused(problem) if problem.why == Refusal::Standing)
    );
    assert_eq!(before, w.records);
    change.message = w.number();
    let mut authorities = Vec::new();
    for task in [1, 2] {
        let before = w.record(task).authority.clone();
        let mut after = before.clone();
        after.tools = tasks::Tools(1);
        authorities.push(AuthorityChange { task, before, after, budget: 100, stop_run: true, message: w.number() });
    }
    change.authorities = authorities.into_boxed_slice();
    assert_eq!(amend(&mut w, 1, person(), change), Reply::Done);
    assert_eq!(w.stops.len(), 2);
    w.restart();
    for task in [1, 2] {
        w.terminal(task, End::Parked);
        assert_eq!(w.record(task).authority.tools, tasks::Tools(1));
    }
    w.send(Event::Hold { task: 2, why: tasks::Hold::Stopped });
    let reply_to = w.to();
    w.send(Event::Control {
        reply_to,
        task: 2,
        authorization: Authorization::Tree(Party::Task(1)),
        action: Control::Release { reason: Box::new([1]) },
    });
    assert!(matches!(w.record(2).phase, tasks::Phase::Active(_)));
    let reply_to = w.to();
    w.send(Event::Control {
        reply_to,
        task: 1,
        authorization: person(),
        action: Control::Cancel { reason: Box::new([1]) },
    });
    w.complete_cancel();
    assert!(matches!(w.records.get(&Key::History { task: 2, revision: 3 }), Some(Stored::History(_))));
}
#[test]
fn moving_normalizes_actual_components_preserves_promises_and_charges_a_live_answer_once() {
    fn story(facts: bool) -> World {
        let mut w = funding_world(3, facts);
        let plan = funded_move(&w);
        assert_eq!(movement(&mut w, plan), Reply::Done);
        assert_eq!(w.record(1).numbers.reserved, 0);
        assert_eq!(w.record(2).numbers, Numbers { budget: 270, spent: 0, spent_below: 0, reserved: 0 });
        assert_eq!(w.record(3).numbers, Numbers { budget: 85, spent: 0, spent_below: 0, reserved: 35 });
        assert_eq!(w.record(4).numbers.budget, 35);
        assert_eq!(w.record(2).run_spent, 30);
        w.restart();
        charge(&mut w, 2, 30);
        assert_eq!(w.record(2).numbers.spent, 0);
        charge(&mut w, 2, 50);
        assert_eq!(w.record(2).numbers.spent, 20);
        w.cancel(1, b"old chat ends");
        w.terminal(1, End::Parked);
        w.settle(1);
        assert!(w.record(2).references.contains(&1));
        w.cancel(2, b"goal ends");
        w.complete_cancel();
        w
    }
    let first = story(true);
    let replay = story(true);
    let no_facts = story(false);
    assert_eq!(first.records, replay.records);
    assert_eq!(first.trace.lines(), replay.trace.lines());
    assert_eq!(first.records, no_facts.records);
    assert_eq!(first.trace.lines(), no_facts.trace.lines());
}
#[test]
fn incomplete_funding_underfunding_and_cycles_refuse_before_mutation() {
    let mut w = funding_world(4, true);
    for fault in 0..5 {
        let mut plan = funded_move(&w);
        match fault {
            0 => plan.transfers = Box::new([plan.transfers[0]]),
            1 => plan.balances[1].after.reserved = 354,
            2 => {
                plan.balances[1].before.budget = 354;
                plan.balances[1].after.budget = 354;
            }
            3 => plan.transfers[0].after = Funder::Task(4),
            4 => {
                plan.to = Party::Task(4);
                plan.transfers[0].after = Funder::Task(4);
            }
            _ => unreachable!(),
        }
        let before = w.records.clone();
        assert!(matches!(movement(&mut w, plan), Reply::Refused(_)));
        assert_eq!(w.records, before);
    }
    w.cancel(1, b"end");
    w.complete_cancel();
}
#[test]
fn same_actual_funder_keeps_generation_and_tracked_tasks_refuse_task_reparenting() {
    let pool = Funder::Pool { project: 1, person: 9, period: 7 };
    let mut w = World::new(8, LIMITS);
    w.open_period(7, 10_000);
    w.carve_pool(7, 500);
    w.make(Party::Person(1), vec![task(1, &[]), funded(5, 500, Funder::Period { project: 1, period: 0 })]);
    w.make(Party::Task(1), vec![funded(2, 100, pool)]);
    w.claim(2, 2);
    charge(&mut w, 2, 20);
    let before = w.record(2).numbers;
    assert_eq!(
        movement(
            &mut w,
            Movement { to: Party::Person(9), transfers: Box::new([]), balances: Box::new([]), reason: Box::new([]) }
        ),
        Reply::Done
    );
    assert_eq!(w.record(2).allotment, 1);
    assert_eq!(w.record(2).numbers, before);
    let mut mark = blank(w.number());
    mark.tracked = Some(Some(7));
    assert_eq!(amend(&mut w, 2, person(), mark), Reply::Done);
    let to_task = Movement {
        to: Party::Task(5),
        transfers: Box::new([Transfer { task: 2, before: pool, after: Funder::Task(5) }]),
        balances: Box::new([
            Balance {
                funder: pool,
                before: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 100 },
                after: Numbers { budget: 500, spent: 0, spent_below: 20, reserved: 0 },
            },
            Balance {
                funder: Funder::Task(5),
                before: w.record(5).numbers,
                after: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 80 },
            },
        ]),
        reason: Box::new([]),
    };
    let before_rows = w.records.clone();
    assert!(matches!(movement(&mut w, to_task.clone()), Reply::Refused(problem) if problem.why == Refusal::Tracked));
    assert_eq!(w.records, before_rows);
    let mut unmark = blank(w.number());
    unmark.tracked = Some(None);
    assert_eq!(amend(&mut w, 2, person(), unmark), Reply::Done);
    assert_eq!(movement(&mut w, to_task), Reply::Done);
    assert_eq!((w.record(2).root, w.record(2).depth), (5, 1));
    w.restart();
    w.cancel(1, b"end");
    w.settle(1);
    w.cancel(5, b"end");
    w.complete_cancel();
}
#[test]
fn budget_amendment_reserves_its_single_actual_source_and_rejects_missing_evidence() {
    let mut w = World::new(9, LIMITS);
    w.make(Party::Person(1), vec![funded(1, 500, Funder::Period { project: 1, period: 0 })]);
    w.make(Party::Task(1), vec![funded(2, 100, Funder::Task(1))]);
    let mut change = blank(w.number());
    let before = w.record(2).authority.clone();
    let mut after = before.clone();
    after.budget.spend = 150;
    change.authorities =
        Box::new([AuthorityChange { task: 2, before, after, budget: 150, stop_run: false, message: change.message }]);
    let before_rows = w.records.clone();
    assert!(
        matches!(amend(&mut w, 2, person(), change.clone()), Reply::Refused(problem) if problem.why == Refusal::Funding)
    );
    assert_eq!(w.records, before_rows);
    change.balances = Box::new([Balance {
        funder: Funder::Task(1),
        before: w.record(1).numbers,
        after: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 150 },
    }]);
    assert_eq!(amend(&mut w, 2, person(), change), Reply::Done);
    assert_eq!(w.record(1).numbers.reserved, 150);
    assert_eq!(w.record(2).numbers.budget, 150);
    w.cancel(1, b"end");
    w.complete_cancel();
}
#[test]
fn move_keeps_old_requester_visibility_or_refuses_reference_pressure() {
    let mut w = World::new(10, tasks::Limits { references: 1, ..LIMITS });
    w.open_period(7, 10_000);
    w.carve_pool(7, 500);
    w.make(Party::Person(1), vec![task(1, &[]), task(5, &[])]);
    w.make(Party::Task(1), vec![funded(2, 100, Funder::Pool { project: 1, person: 9, period: 7 })]);
    let reply_to = w.to();
    w.send(Event::Introduce { reply_to, by: Party::Person(9), left: 2, right: 5 });
    let before = w.records.clone();
    assert!(
        matches!(movement(&mut w, Movement { to: Party::Person(9), transfers: Box::new([]), balances: Box::new([]), reason: Box::new([]) }), Reply::Refused(problem) if problem.why == Refusal::Reference)
    );
    assert_eq!(w.records, before);
    w.cancel(1, b"end");
    w.cancel(5, b"end");
    w.complete_cancel();
}
#[test]
fn move_and_amendment_cuts_restore_only_committed_generations_and_control_messages() {
    let mut w = funding_world(11, true);
    let before = w.records.clone();
    let movement = funded_move(&w);
    let reply_to = w.to();
    w.stage(Event::Move { reply_to, task: 2, authorization: person(), movement });
    w.restart();
    assert_eq!(w.records, before, "uncommitted replacements and closures vanish");
    let movement = funded_move(&w);
    let reply_to = w.to();
    w.stage(Event::Move { reply_to, task: 2, authorization: person(), movement });
    w.durable();
    w.restart();
    assert_eq!(w.record(2).allotment, 2);
    assert_eq!(w.record(2).run_spent, 30);
    let mut amendment = blank(w.number());
    amendment.spec = Some(Spec { words: Box::new([8]), parameters: Box::new([]), inputs: Box::new([]) });
    let before = w.records.clone();
    let reply_to = w.to();
    w.stage(Event::Amend { reply_to, task: 2, authorization: person(), amendment });
    w.restart();
    assert_eq!(w.records, before, "uncommitted revised spec and amendment offer vanish");
    w.cancel(1, b"end");
    w.terminal(1, End::Parked);
    w.settle(1);
    w.cancel(2, b"end");
    w.complete_cancel();
}
#[test]
fn oversized_checked_cancel_and_amendment_during_closing_leave_history_unchanged() {
    let mut w = World::new(12, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    let before = w.records.clone();
    let token = w.to().into_token();
    w.send(Event::Control {
        reply_to: ReplyTo::new(token),
        task: 1,
        authorization: person(),
        action: Control::Cancel { reason: vec![1; LIMITS.result_bytes as usize + 1].into_boxed_slice() },
    });
    assert!(matches!(w.replies[&token.raw()], Reply::Refused(problem) if problem.why == Refusal::Reason));
    assert_eq!(w.records, before);
    w.cancel(1, b"end");
    let before = w.records.clone();
    let amendment = blank(w.number());
    assert!(matches!(amend(&mut w, 1, person(), amendment), Reply::Refused(problem) if problem.why == Refusal::State));
    assert_eq!(w.records, before);
    w.complete_cancel();
}
#[test]
fn zero_allotment_replacement_has_its_own_once_only_closure_generation() {
    let mut w = World::new(13, LIMITS);
    w.open_period(7, 1000);
    w.carve_pool(7, 0);
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![funded(2, 0, Funder::Task(1))]);
    let destination = Funder::Pool { project: 1, person: 9, period: 7 };
    let plan = Movement {
        to: Party::Person(9),
        transfers: Box::new([Transfer { task: 2, before: Funder::Task(1), after: destination }]),
        balances: Box::new([
            Balance { funder: Funder::Task(1), before: w.record(1).numbers, after: w.record(1).numbers },
            Balance {
                funder: destination,
                before: Numbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
                after: Numbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
            },
        ]),
        reason: Box::new([]),
    };
    assert_eq!(movement(&mut w, plan.clone()), Reply::Done);
    assert_eq!(w.record(2).allotment, 2);
    let before = w.records.clone();
    assert!(matches!(movement(&mut w, plan), Reply::Refused(_)));
    assert_eq!(w.records, before);
    w.restart();
    w.cancel(1, b"end");
    w.settle(1);
    w.cancel(2, b"end");
    w.complete_cancel();
    assert!(w.records.contains_key(&Key::Closure { task: 2, generation: 1 }));
    assert!(w.records.contains_key(&Key::Closure { task: 2, generation: 2 }));
}
#[test]
fn durable_narrowing_reissues_stops_after_the_undelivered_stop_is_lost() {
    for started in [false, true] {
        let mut w = World::new(15, LIMITS);
        let mut new = task(1, &[]);
        new.authority.tools = tasks::Tools(3);
        w.make(Party::Person(1), vec![new]);
        if started {
            w.claim(1, 1);
        } else {
            assert!(w.activations.remove(&1));
            let reply_to = w.to();
            w.send(Event::Prepare { reply_to, task: 1 });
            let reply_to = w.to();
            w.send(Event::Claim { reply_to, task: 1, attempt: 1, readable: Box::new([]) });
            w.runs.insert(1, 1);
            w.observe(temper_engine_tasks_world::referee::Seen::Assigned {
                task: 1,
                attempt: 1,
                after: 0,
                adopted: false,
            });
        }
        let before = w.record(1).authority.clone();
        let mut after = before.clone();
        after.tools = tasks::Tools(0);
        let mut amendment = blank(w.number());
        amendment.authorities = Box::new([AuthorityChange {
            task: 1,
            before,
            after,
            budget: 100,
            stop_run: true,
            message: amendment.message,
        }]);
        let reply_to = w.to();
        w.stage(Event::Amend { reply_to, task: 1, authorization: person(), amendment });
        w.durable();
        assert!(
            w.records.contains_key(&Key::Offer(tasks::MessageKey { task: 1, number: 1 })),
            "control has a durable offer even before Started"
        );
        assert!(w.stops.is_empty(), "Stop has not escaped the durable decision yet");
        // restart clears the pending outward Stop and all volatile prior Stop
        // observations; only TaskRecord::narrowing can recover cancellation.
        w.restart();
        assert_eq!(w.stops, [(1, 1)].into_iter().collect());
        w.terminal(1, End::Parked);
        assert!(!w.record(1).narrowing);
        w.cancel(1, b"end");
        w.complete_cancel();
    }
}
#[test]
fn amendment_offers_have_separate_capacity_and_repeated_unread_controls_refuse_atomically() {
    let mut w = World::new(16, tasks::Limits { offers: 1, ..LIMITS });
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.claim(1, 1);
    let ordinary = w.number();
    w.mail_number(ordinary, 1, Party::Person(9), tasks::UserMessage::Words { words: Box::new([1]) });
    assert!(w.relays.contains_key(&(1, 1, ordinary)));
    let first = w.number();
    assert_eq!(amend(&mut w, 1, person(), blank(first)), Reply::Done);
    let second = w.number();
    assert_eq!(amend(&mut w, 1, person(), blank(second)), Reply::Done);
    assert!(
        w.relays.contains_key(&(1, 1, first)) && w.relays.contains_key(&(1, 1, second)),
        "ordinary pressure cannot defer either admitted control"
    );
    let before = w.records.clone();
    let third = w.number();
    assert!(
        matches!(amend(&mut w, 1, person(), blank(third)), Reply::Refused(problem) if problem.why == Refusal::Busy)
    );
    assert_eq!(w.records, before);
    w.restart();
    assert_eq!(w.records.keys().filter(|key| matches!(key, Key::Offer(_))).count(), 3);
    w.turn(1, 1, Some(first));
    assert_eq!(w.messages(1)[0].number, second, "read of old immutable control keeps its replacement");
    assert_eq!(amend(&mut w, 1, person(), blank(third)), Reply::Done);
    assert!(w.relays.contains_key(&(1, 1, third)));
    w.cancel(1, b"end");
    w.complete_cancel();
}

#[test]
fn independent_accounting_referee_rejects_missing_actual_task_and_external_postings() {
    use temper_engine_tasks_world::accounting_referee::Accounting;
    let mut w = funding_world(17, false);
    let before = w.records.clone();
    let plan = funded_move(&w);
    assert_eq!(movement(&mut w, plan.clone()), Reply::Done);
    let mut bad = w.records.clone();
    if let Some(Stored::Live(task)) = bad.get_mut(&Key::Live(1)) {
        task.numbers.spent_below = 0;
    }
    let mut judge = Accounting::default();
    judge.reset(&before);
    judge.begin(&plan.balances);
    judge.emitted(plan.balances[1].funder);
    assert_eq!(judge.committed(&bad), Err("closed spend not posted to actual task funder"));
    // A missing child posting inside a closing generation also changes the
    // closure total, independently of the unchanged actual funder's ledger.
    let mut bad = w.records.clone();
    if let Some(Stored::Closure(closure)) = bad.get_mut(&Key::Closure { task: 3, generation: 1 }) {
        closure.spent = 10;
    }
    let mut judge = Accounting::default();
    judge.reset(&before);
    judge.begin(&plan.balances);
    judge.emitted(plan.balances[1].funder);
    assert_eq!(judge.committed(&bad), Err("closure lost actual descendant spend"));
    let source = Funder::Pool { project: 1, person: 9, period: 7 };
    let destination = Funder::Pool { project: 1, person: 9, period: 8 };
    let mut w = World::new(18, LIMITS);
    for period in [7, 8] {
        w.open_period(period, 10_000);
        w.carve_pool(period, 500);
    }
    w.make(Party::Person(1), vec![task(1, &[])]);
    w.make(Party::Task(1), vec![funded(2, 100, source)]);
    w.claim(2, 1);
    charge(&mut w, 2, 20);
    let before = w.records.clone();
    let plan = Movement {
        to: Party::Person(9),
        transfers: Box::new([Transfer { task: 2, before: source, after: destination }]),
        balances: Box::new([
            Balance {
                funder: source,
                before: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 100 },
                after: Numbers { budget: 500, spent: 0, spent_below: 20, reserved: 0 },
            },
            Balance {
                funder: destination,
                before: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 0 },
                after: Numbers { budget: 500, spent: 0, spent_below: 0, reserved: 80 },
            },
        ]),
        reason: Box::new([]),
    };
    assert_eq!(movement(&mut w, plan.clone()), Reply::Done);
    for fault in 0..3 {
        let mut bad = w.records.clone();
        let funder = if fault == 2 { destination } else { source };
        if let Some(Stored::Funding { numbers, .. }) = bad.get_mut(&Key::Funding(funder)) {
            match fault {
                0 => numbers.spent_below = 0,
                1 => numbers.reserved = 1,
                2 => numbers.reserved = 79,
                _ => unreachable!(),
            }
        }
        let mut judge = Accounting::default();
        judge.reset(&before);
        judge.begin(&plan.balances);
        judge.emitted(source);
        judge.emitted(destination);
        assert!(judge.committed(&bad).is_err(), "actual external funder fault {fault}");
    }
    let mut judge = Accounting::default();
    judge.reset(&before);
    judge.begin(&plan.balances);
    judge.emitted(source);
    judge.emitted(destination);
    assert!(judge.committed(&w.records).is_ok(), "independent external arithmetic accepts intact rows");
}
