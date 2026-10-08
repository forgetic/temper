//! Bounded root sweep across durable task endings and the held route.

use jig_core_authority as authority;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use skein_lib::{Duration, Queue, ReplyTo, Rng, Token};
use std::collections::BTreeSet;
use temper_engine_domain::{Delivery, Key, Record, engine};
use temper_engine_domain_world::commits::Store;
use temper_engine_domain_world::direct::Driver;
use temper_engine_domain_world::walking::{config, limits};

#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one delivery from the closed vocabulary")]
fn chat(seed: u64, child: bool) -> (Driver, engine::Assignment) {
    let mut bounds = limits();
    let mut configuration = config(seed);
    if child {
        bounds.tasks.tasks = 4;
        bounds.tasks.project_tasks = 4;
        bounds.tasks.tree_tasks = 4;
        bounds.tasks.depth = 1;
        bounds.tasks.delegates = 2;
        bounds.tasks.batch = 2;
        bounds.authority.batch = 2;
        let mut rules = configuration.authority.rules().clone();
        rules.maximum_run_spend = 60;
        rules.ceiling.delegation.tasks = 4;
        rules.ceiling.delegation.depth = 2;
        let mut policy = configuration.authority.policy(1).expect("project policy").clone();
        policy.ceiling.delegation.tasks = 4;
        policy.ceiling.delegation.depth = 2;
        policy.roles[0].authority.delegation.tasks = 4;
        policy.roles[0].authority.delegation.depth = 2;
        let mut domain = authority::Domain::new(rules, bounds.authority).expect("authority bounds");
        let mut out = Queue::with_capacity(authority::POLICY_MAX_OUT);
        authority::step(&mut domain, authority::Event::Policy { project: 1, policy }, &mut out);
        assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: 1 }));
        configuration.authority = domain;
        configuration.chat_authority.delegation.kinds = Box::new([authority::Executor::Charter(1)]);
        configuration.chat_authority.delegation.tasks = 1;
        configuration.chat_authority.delegation.depth = 1;
    }
    bounds.fleet.attempts = 5;
    bounds.call_records = 4;
    bounds.brief.sections = 5;
    bounds.journal.writes = 3000;
    bounds.journal.deliveries = 100;
    bounds.journal.held = 300;
    let mut driver = Driver::configured(Store::new(), configuration, &bounds);
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(401)),
        sign_in: driver.session(),
        key: [5; 16],
        ask: people::Ask::StartChat { project: 1, words: b"root sweep".as_slice().into() },
    });
    driver.settle();
    let assignment = driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::Assigned { assignment, .. } => Some(assignment.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("committed chat assigned, seed {seed}: {:?}", driver.delivered));
    (driver, assignment)
}

#[test]
#[expect(
    clippy::wildcard_enum_match_arm,
    clippy::too_many_lines,
    reason = "one seeded root story selects its concrete route and durable outcome"
)]
fn drawn_root_stories_reach_every_durable_ending_and_held_route() {
    let mut seen = BTreeSet::new();
    for seed in 0..16_u64 {
        let mut rng = Rng::new(seed + 8330);
        let route = seed % 4;
        let (mut driver, assignment) = chat(seed + 8330, route == 2);
        if rng.below(2) == 0 {
            driver.advance(true);
        }
        match route {
            0 | 1 => {
                let result = if route == 0 {
                    tasks::TaskResult::Report { words: b"done".as_slice().into() }
                } else {
                    tasks::TaskResult::Failure { reason: b"failed".as_slice().into() }
                };
                driver.send(engine::Event::Answer {
                    pushed: Box::new([]),
                    saved: None,
                    channel: Token::new(7),
                    task: assignment.task,
                    attempt: assignment.attempt,
                    cumulative: 0,
                    end: tasks::End::Finished { result, cancel_delegates: false },
                });
                driver.settle();
                let Some(Record::Tasks(tasks::Stored::Ended(row))) =
                    driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(assignment.task)))
                else {
                    panic!("durable chat ending, seed {seed}")
                };
                match &row.phase {
                    tasks::Phase::Ended(tasks::Ending::Done(_)) if route == 0 => {
                        seen.insert("done");
                    }
                    tasks::Phase::Ended(tasks::Ending::Failed { .. }) if route == 1 => {
                        seen.insert("failed");
                    }
                    other => panic!("unexpected chat ending {other:?}, seed {seed}"),
                }
            }
            2 => {
                let child = engine::Delegate {
                    executor: tasks::Executor::Agent { charter: 1 },
                    spec: tasks::Spec {
                        words: b"child".as_slice().into(),
                        parameters: Box::new([]),
                        inputs: Box::new([]),
                    },
                    contract: tasks::Contract::Report { words: 128 },
                    authority: tasks::Authority {
                        tools: tasks::Tools(0),
                        grants: Box::new([]),
                        delegation: tasks::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
                        budget: tasks::Budget { spend: 10, deadline: None },
                        notes: tasks::Scopes(0),
                        note_resources: Box::new([]),
                    },
                    symbolic_grants: Box::new([]),
                    dependencies: Box::new([]),
                    wake: tasks::WakePolicy::DEFAULT,
                };
                driver.send(engine::Event::Call {
                    channel: Token::new(7),
                    task: assignment.task,
                    attempt: assignment.attempt,
                    call: Token::new(90),
                    body: engine::Call {
                        completion: 1,
                        position: 1,
                        tool: engine::Tool::Delegate { batch: Box::new([child]) },
                    },
                });
                for _ in 0..30 {
                    driver.advance(true);
                }
                let child = driver
                    .delivered
                    .iter()
                    .find_map(|item| match item {
                        Delivery::CallAnswer {
                            call,
                            answer: temper_engine_domain::CallAnswer::Delegated(numbers),
                            ..
                        } if *call == Token::new(90) => numbers.first().copied(),
                        _ => None,
                    })
                    .expect("child was made atomically");
                driver.send(engine::Event::Call {
                    channel: Token::new(7),
                    task: assignment.task,
                    attempt: assignment.attempt,
                    call: Token::new(91),
                    body: engine::Call {
                        completion: 2,
                        position: 2,
                        tool: engine::Tool::Cancel { target: child, reason: b"withdrawn".as_slice().into() },
                    },
                });
                driver.settle();
                assert!(
                    matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(child))),
                        Some(Record::Tasks(tasks::Stored::Ended(row)))
                        if matches!(row.phase, tasks::Phase::Ended(tasks::Ending::Cancelled { .. }))
                    ),
                    "cancelled child durable, seed {seed}"
                );
                seen.insert("cancelled");
            }
            3 => {
                driver.send(engine::Event::Ask {
                    reply_to: ReplyTo::new(Token::new(501)),
                    sign_in: driver.session(),
                    key: [6; 16],
                    ask: people::Ask::Stop { project: 1, task: assignment.task },
                });
                driver.settle();
                assert!(
                    matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task))),
                        Some(Record::Tasks(tasks::Stored::Live(row)))
                        if matches!(row.phase, tasks::Phase::Held { why: tasks::Hold::StoppedBy { party: 1 }, .. })
                    ),
                    "stopped chat held durably, seed {seed}"
                );
                seen.insert("held");
            }
            _ => unreachable!(),
        }
        assert!(!driver.stopped, "root stopped, seed {seed}");
    }
    assert_eq!(seen, BTreeSet::from(["done", "failed", "cancelled", "held"]));
}
