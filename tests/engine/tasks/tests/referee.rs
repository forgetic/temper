use skein_lib::Time;
use temper_engine_domain_tasks::{Party, Status};
use temper_engine_tasks_world::referee::{Seen, Tasks};
use temper_world::{Referee, Verdict};
fn rejects(seen: Vec<Seen>) {
    let mut referee = Referee::new(Tasks::default());
    let mut out = Vec::new();
    for observation in seen {
        referee.observe(Time::ZERO, observation, &mut out);
    }
    assert!(matches!(referee.verdict(), Verdict::Failed(_)), "referee catches violated invariant");
}
fn made(task: u64) -> Seen {
    Seen::Made { task, parent: Party::Person(1), dependencies: vec![], depth: 0 }
}
fn assigned(task: u64, attempt: u64) -> Seen {
    Seen::Assigned { task, attempt, after: 0, adopted: false }
}
fn partial_batch() {
    rejects(vec![Seen::Batch { members: vec![1, 2], accepted: true, made: vec![1] }]);
}
fn refused_batch_created_task() {
    rejects(vec![Seen::Batch { members: vec![1], accepted: false, made: vec![1] }]);
}
fn reused_number() {
    rejects(vec![made(1), made(1)]);
}
fn dependency_on_self() {
    rejects(vec![Seen::Made { task: 1, parent: Party::Person(1), dependencies: vec![1], depth: 0 }]);
}
fn delegate_without_parent() {
    rejects(vec![Seen::Made { task: 2, parent: Party::Task(1), dependencies: vec![], depth: 1 }]);
}
fn assignment_before_dependency_done() {
    rejects(vec![
        made(1),
        Seen::Made { task: 2, parent: Party::Person(1), dependencies: vec![1], depth: 0 },
        assigned(2, 1),
    ]);
}
fn overlapping_runs() {
    rejects(vec![made(1), assigned(1, 1), assigned(1, 2)]);
}
fn nonmonotonic_attempt() {
    rejects(vec![made(1), assigned(1, 2), Seen::Terminal { task: 1, attempt: 2 }, assigned(1, 1)]);
}
fn terminal_of_wrong_attempt() {
    rejects(vec![made(1), assigned(1, 1), Seen::Terminal { task: 1, attempt: 2 }]);
}
fn closing_before_run_ended() {
    rejects(vec![made(1), assigned(1, 1), Seen::Closing { task: 1 }]);
}
fn closing_before_delegate_ended() {
    rejects(vec![
        made(1),
        Seen::Made { task: 2, parent: Party::Task(1), dependencies: vec![], depth: 1 },
        Seen::Closing { task: 1 },
    ]);
}
fn settlement_before_close() {
    rejects(vec![Seen::Settled { task: 1 }]);
}
fn result_before_settlement() {
    rejects(vec![made(1), Seen::Ended { task: 1, status: Status::Done, after: 0 }]);
}
fn duplicate_result() {
    rejects(vec![
        made(1),
        Seen::Closing { task: 1 },
        Seen::Settled { task: 1 },
        Seen::Ended { task: 1, status: Status::Done, after: 0 },
        Seen::Ended { task: 1, status: Status::Done, after: 0 },
    ]);
}
fn parent_result_before_child() {
    rejects(vec![
        made(1),
        Seen::Closing { task: 1 },
        Seen::Settled { task: 1 },
        Seen::Made { task: 2, parent: Party::Task(1), dependencies: vec![], depth: 1 },
        Seen::Ended { task: 1, status: Status::Done, after: 0 },
    ]);
}
fn wrong_cancelled_ending() {
    rejects(vec![
        made(1),
        Seen::Cancelled { task: 1 },
        Seen::Closing { task: 1 },
        Seen::Settled { task: 1 },
        Seen::Ended { task: 1, status: Status::Done, after: 0 },
    ]);
}
fn cancelled_descendant_missing() {
    rejects(vec![made(1), Seen::Cancelled { task: 1 }, Seen::Finished]);
}
fn live_limit() {
    rejects(vec![Seen::Limit { live: 2, cap: 1 }]);
}
fn reply_before_durable() {
    rejects(vec![Seen::Replied { call: 1, after: 1 }]);
}
fn duplicate_reply() {
    rejects(vec![Seen::Replied { call: 1, after: 0 }, Seen::Replied { call: 1, after: 0 }]);
}
fn assignment_before_durable() {
    rejects(vec![made(1), Seen::Assigned { task: 1, attempt: 1, after: 1, adopted: false }]);
}
fn result_before_durable() {
    rejects(vec![
        made(1),
        Seen::Closing { task: 1 },
        Seen::Settled { task: 1 },
        Seen::Ended { task: 1, status: Status::Done, after: 1 },
    ]);
}

#[test]
fn each_initial_invariant_catches_an_independent_violation() {
    dependency_cycle();
    cycle_through_delegate();
    missing_end_deadline();
    stored_limits();
    partial_batch();
    refused_batch_created_task();
    reused_number();
    dependency_on_self();
    delegate_without_parent();
    assignment_before_dependency_done();
    overlapping_runs();
    nonmonotonic_attempt();
    terminal_of_wrong_attempt();
    closing_before_run_ended();
    closing_before_delegate_ended();
    settlement_before_close();
    result_before_settlement();
    duplicate_result();
    parent_result_before_child();
    wrong_cancelled_ending();
    cancelled_descendant_missing();
    live_limit();
    reply_before_durable();
    duplicate_reply();
    assignment_before_durable();
    result_before_durable();
}

fn dependency_cycle() {
    rejects(vec![
        Seen::Made { task: 1, parent: Party::Person(1), dependencies: vec![2], depth: 0 },
        Seen::Made { task: 2, parent: Party::Person(1), dependencies: vec![1], depth: 0 },
    ]);
}
fn cycle_through_delegate() {
    rejects(vec![made(1), Seen::Made { task: 2, parent: Party::Task(1), dependencies: vec![1], depth: 1 }]);
}

fn missing_end_deadline() {
    let mut referee = Referee::new(Tasks::default());
    let mut out = Vec::new();
    referee.observe(Time::ZERO, made(1), &mut out);
    referee.observe(Time::ZERO, Seen::Cancelled { task: 1 }, &mut out);
    referee.fire(Time::ZERO.saturating_add(skein_lib::Duration::from_secs(10)), &mut out);
    assert!(matches!(referee.verdict(), Verdict::Failed(_)), "missing cancelled result expires");
}

fn stored_limits() {
    use temper_engine_tasks_world::{LIMITS, World, task};
    let mut w = World::new(0, LIMITS);
    w.make(Party::Person(1), vec![task(1, &[])]);
    for field in 0..10 {
        let mut record = w.record(1).clone();
        match field {
            0 => record.depth = LIMITS.depth + 1,
            1 => record.made = LIMITS.tree_tasks + 1,
            2 => record.delegates = vec![2; 9].into_boxed_slice(),
            3 => record.dependencies = vec![2; 9].into_boxed_slice(),
            4 => record.spec.words = vec![1; 65].into_boxed_slice(),
            5 => record.spec.inputs = vec![2; 5].into_boxed_slice(),
            6 => {
                record.spec.parameters =
                    vec![temper_engine_domain_tasks::Parameter::Number { name: 1, value: 1 }; 5].into_boxed_slice();
            }
            7 => record.contract = temper_engine_domain_tasks::Contract::Report { words: 33 },
            8 => {
                record.authority.delegation.kinds =
                    vec![temper_engine_domain_tasks::AuthorityExecutor::Role(1); 4].into_boxed_slice();
            }
            9 => {
                record.authority.grants = vec![
                    temper_engine_domain_tasks::Grant {
                        connector: 0,
                        kind: 0,
                        pattern: temper_engine_domain_tasks::Pattern {
                            segments: Box::new([]),
                            last: temper_engine_domain_tasks::Last::None
                        }
                    };
                    5
                ]
                .into_boxed_slice();
            }
            _ => unreachable!(),
        }
        rejects(vec![Seen::Stored { live: vec![record], stubs: 0, limits: Box::new(LIMITS) }]);
    }
    rejects(vec![Seen::Stored { live: vec![], stubs: 33, limits: Box::new(LIMITS) }]);
    let record = w.record(1).clone();
    rejects(vec![Seen::Stored {
        live: vec![record.clone(), record],
        stubs: 0,
        limits: Box::new(temper_engine_domain_tasks::Limits { project_tasks: 1, ..LIMITS }),
    }]);
}
#[test]
fn inbox_referee_catches_each_new_initial_invariant_without_child_state() {
    use temper_engine_domain_tasks::{self as tasks, Key, Message, Party, Stored, UserMessage};
    use temper_engine_tasks_world::{LIMITS, World, inbox_referee::Inbox, task};
    let mut w = World::new(81, LIMITS);
    let mut root = task(1, &[]);
    root.policy.words = tasks::Rule::Never;
    w.make(Party::Person(1), vec![root]);
    w.make(Party::Task(1), vec![task(2, &[])]);
    w.claim(1, 1);
    w.mail(1, Party::Task(2), UserMessage::Words { words: Box::new([1]) });
    w.mail(1, Party::Person(1), UserMessage::Words { words: Box::new([2]) });
    let rows = w.records.clone();
    for broken in 0..11 {
        let mut bad = rows.clone();
        let mut limits = LIMITS;
        match broken {
            0 => {
                let row = bad.remove(&Key::Receipt(1)).expect("receipt");
                bad.insert(Key::Receipt(999), row);
            }
            1 => {
                if let Some(Stored::Message(envelope)) =
                    bad.get_mut(&Key::Message(tasks::MessageKey { task: 1, number: 1 }))
                {
                    envelope.hits = 0;
                }
            }
            2 => {
                if let Some(Stored::Message(envelope)) =
                    bad.get_mut(&Key::Message(tasks::MessageKey { task: 1, number: 1 }))
                {
                    envelope.message =
                        Message::News { subscription: 1, class: tasks::NewsClass::Dropped, words: Box::new([]) };
                }
            }
            3 => {
                if let Some(Stored::Offer(offer)) = bad.get_mut(&Key::Offer(tasks::MessageKey { task: 1, number: 2 })) {
                    offer.attempt = 99;
                }
            }
            4 => {
                let q = tasks::Question { number: 3, asker: 1, answerer: 999 };
                bad.insert(Key::Question(3), Stored::Question(q));
            }
            5 => {
                let sub = tasks::Subscription {
                    number: 3,
                    task: 1,
                    kind: tasks::SubscriptionKind::Topic { connector: 1, topic: 1 },
                    pending: true,
                };
                bad.insert(Key::Subscription(3), Stored::Subscription(sub));
            }
            6 => {
                if let Some(Stored::Live(record)) = bad.get_mut(&Key::Live(1)) {
                    record.references = Box::new([2, 2]);
                }
            }
            7 => {
                if let Some(Stored::Live(record)) = bad.get_mut(&Key::Live(1)) {
                    record.results_due = Box::new([999]);
                }
            }
            8 => limits.inbox_messages = 1,
            9 => limits.inbox_bytes = 1,
            10 => limits.receipts = 0,
            _ => unreachable!(),
        }
        assert!(Inbox::default().committed(&bad, &limits, &[]).is_err(), "independent invariant {broken}");
    }
    rejected_reads(&rows);
}
fn rejected_reads(
    rows: &std::collections::BTreeMap<temper_engine_domain_tasks::Key, temper_engine_domain_tasks::Stored>,
) {
    use temper_engine_domain_tasks::{self as tasks, Key, Stored};
    use temper_engine_tasks_world::{
        LIMITS,
        inbox_referee::{Inbox, Read},
    };
    let read = Read { task: 1, attempt: 1, turn: 1, through: Some(2) };
    let mut taken = rows.clone();
    if let Some(Stored::Live(record)) = taken.get_mut(&Key::Live(1)) {
        record.turn = 1;
        record.last_read = Some(2);
    }
    taken.remove(&Key::Message(tasks::MessageKey { task: 1, number: 2 }));
    taken.remove(&Key::Offer(tasks::MessageKey { task: 1, number: 2 }));
    let mut judge = Inbox::default();
    judge.reset(rows);
    assert!(judge.committed(&taken, &LIMITS, &[read]).is_ok());
    for broken in 0..4 {
        let mut bad = taken.clone();
        let mut read = read;
        match broken {
            0 => read.through = Some(999),
            1 => read.turn = 3,
            2 => {
                bad.remove(&Key::Message(tasks::MessageKey { task: 1, number: 1 }));
            }
            3 => {
                let key = Key::Message(tasks::MessageKey { task: 1, number: 2 });
                bad.insert(key, rows[&key].clone());
            }
            _ => unreachable!(),
        }
        let mut judge = Inbox::default();
        judge.reset(rows);
        assert!(judge.committed(&bad, &LIMITS, &[read]).is_err(), "read invariant {broken}");
    }
}
