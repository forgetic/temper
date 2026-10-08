//! A proposed person goal crosses its approval, task, and Smith planning cuts.

use crate::world;
use jig_core_authority as authority;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
use skein_lib::{Duration, Queue, ReplyTo, Token};
use smith_agent_world::Job;
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::{Delivery, Key, Record, engine};
use temper_engine_domain_world::{commits::Store, direct::Driver, walking};

const PLAN: &[u8] = br#"{"batch":[{"executor":{"kind":"agent","charter":1},"spec":{"words":"@report Complete the goal's first task"},"contract":{"kind":"report","words":128},"authority":{"tools":0,"grants":[],"delegation":{"kinds":[],"tasks":0,"depth":0},"budget":{"spend":10},"notes":0}}]}"#;

fn plan_script() -> Script {
    let call = |name: &[u8], arguments: &[u8]| Turn {
        lines: Box::new([Line::Call { name: name.into(), arguments: arguments.into() }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    Script {
        cue: b"@goalplan".as_slice().into(),
        turns: Box::new([
            call(b"delegate", PLAN),
            call(b"wait", b"{}"),
            Turn {
                lines: Box::new([Line::Text { text: b"Waiting for the first task.".as_slice().into() }]),
                finish: Finish::Stop,
                tokens: 20,
            },
            call(b"finish", br#"{"report":"The goal's first task is complete."}"#),
        ]),
    }
}

fn configured() -> Driver {
    let mut limits = walking::limits();
    limits.fleet.slots = 5;
    limits.fleet.attempts = 8;
    limits.fleet.calls = 4;
    limits.fleet.turns = 16;
    limits.authority.roles = 3;
    limits.authority.batch = 3;
    limits.tasks.tasks = 5;
    limits.tasks.project_tasks = 5;
    limits.tasks.tree_tasks = 5;
    limits.tasks.delegates = 3;
    limits.tasks.depth = 2;
    limits.tasks.batch = 3;
    limits.tasks.spec_bytes = 128;
    limits.tasks.inbox_messages = 7;
    limits.tasks.inbox_bytes = 1024;
    limits.people.people = 4;
    limits.people.sign_ins = 4;
    limits.people.holdings = 4;
    limits.people.requests = 8;
    limits.people.pending = 4;
    limits.call_records = 4;
    limits.brief.briefs = 5;
    limits.brief.sections = 8;
    limits.forge.brief_sections = 80;
    limits.brief.brief_bytes = 1024;
    limits.journal.writes = 3000;
    limits.journal.deliveries = 512;
    limits.journal.held = 1536;
    limits.journal.transcript_bytes = 32768;
    limits.journal.result_bytes = 1024;
    let mut config = walking::config(903);
    config.run.model.account = 1;
    config.run.model.endpoint = 0;
    config.run.model.name = b"fake-1".as_slice().into();
    config.run.model.input_price = 0;
    config.run.model.cached_price = 0;
    config.run.model.output_price = 0;
    config.run.model.price_unit = 1;
    config.run.model.max_tokens = 4096;
    config.run.turns = 64;
    config.run.time = Duration::from_secs(3600);
    config.resume_bytes = 8192;
    config.chat_authority.tools = authority::Tools(3);
    config.chat_authority.delegation.kinds = Box::new([authority::Executor::Charter(1)]);
    config.chat_authority.delegation.tasks = 3;
    config.chat_authority.delegation.depth = 1;
    let mut rules = config.authority.rules().clone();
    // Leave the standing goal and chat room to fund their planned delegates.
    rules.maximum_run_spend = 10;
    rules.ceiling.tools = authority::Tools(3);
    rules.ceiling.delegation.depth = 2;
    rules.ceiling.delegation.tasks = 4;
    let mut domain = authority::Domain::new(rules, limits.authority).expect("larger authority");
    let mut policy = config.authority.policy(1).expect("configured project").clone();
    policy.ceiling.tools = authority::Tools(3);
    policy.ceiling.delegation.depth = 2;
    policy.ceiling.delegation.tasks = 4;
    let mut owner = policy.roles[0].clone();
    owner.authority.tools = authority::Tools(3);
    owner.authority.delegation.depth = 2;
    owner.authority.delegation.tasks = 4;
    let mut maintainer = owner.clone();
    maintainer.number = 1;
    let mut member = owner.clone();
    member.number = 2;
    member.authority.budget.spend = 100;
    member.period_spend = 500;
    member.decides = authority::Proposals(0);
    policy.roles = Box::new([owner, maintainer, member]);
    let mut out = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain, authority::Event::Policy { project: 1, policy }, &mut out);
    assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: 1 }));
    config.authority = domain;
    Driver::configured(Store::new(), config, &limits)
}

fn sign_in(driver: &mut Driver, user: u64, reply: u64) -> u64 {
    driver.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(reply)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: (user).to_be_bytes().into() },
            login: b"person".as_slice().into(),
            name: b"Person".as_slice().into(),
        },
    });
    driver.settle();
    driver.store.header().sign_ins
}

#[test]
#[expect(clippy::too_many_lines, reason = "the story follows proposal, approval, planning, and completion")]
#[expect(clippy::wildcard_enum_match_arm, reason = "the story selects its committed deliveries")]
fn a_smith_goal_is_proposed_accepted_planned_and_done() {
    let mut driver = configured();
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 5,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    let owner_session = sign_in(&mut driver, 7, 301);
    let maintainer_session = sign_in(&mut driver, 8, 302);
    let maintainer = driver.store.header().people;
    let member_session = sign_in(&mut driver, 9, 303);
    let member = driver.store.header().people;
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(304)),
        sign_in: owner_session,
        key: [34; 16],
        ask: people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([
                people::Holding { person: 1, role: people::Role::Owner },
                people::Holding { person: maintainer, role: people::Role::Maintainer },
                people::Holding { person: member, role: people::Role::Member },
            ]),
        },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(305)),
        sign_in: member_session,
        key: [35; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: b"@goalplan Ship the goal".as_slice().into(),
            charter: 1,
            budget: 150,
            priority: 7,
        },
    });
    driver.settle();
    let proposal = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::GoalProposed { proposal }), ..
            } => Some(*proposal),
            _ => None,
        })
        .expect("goal proposal committed");
    assert!(!driver.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(_)))));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(306)),
        sign_in: maintainer_session,
        key: [36; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: member,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    assert!(
        matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::PersonProposal(proposal))),
        Some(Record::Tasks(tasks::Stored::PersonProposal(row)))
            if row.state == tasks::PersonProposalState::Accepted { by: tasks::Party::Person(maintainer) }),
        "{:?}",
        driver.delivered.iter().rev().take(3).collect::<Vec<_>>()
    );
    let goal = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } if assignment.task > 0 => Some(assignment.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("accepted goal assigned: delivered={:?} rows={:?}", driver.delivered, driver.store.rows)
        });
    let mut planner = world::scripted_waiting_agent_for(&goal, None, plan_script());
    world::run_assignment(&mut driver, &goal, &mut planner);
    assert!(matches!(planner.answer(), run::Answer::Parked { .. }), "{:?}", planner.answer());
    let child = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } if assignment.task != goal.task => Some(assignment.clone()),
            _ => None,
        })
        .expect("plan assigned its first task");
    let mut worker = world::agent_for(&child, None, Job::Reporting);
    world::run_assignment(&mut driver, &child, &mut worker);
    assert!(matches!(worker.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Report(_), .. }));
    let resumed = driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. }
                if assignment.task == goal.task && assignment.attempt > goal.attempt =>
            {
                Some(assignment.clone())
            }
            _ => None,
        })
        .expect("child result woke goal planner");
    let first = planner.turns().first().expect("concrete Smith transcript");
    let transcript = smith::Transcript {
        version: first.version,
        endpoint: first.endpoint,
        dialect: first.dialect,
        turns: planner.turns().into(),
    };
    assert_eq!(resumed.transcript.len(), transcript.turns.len());
    let mut planner = world::scripted_waiting_agent_for(&resumed, Some(transcript), plan_script());
    world::run_assignment(&mut driver, &resumed, &mut planner);
    assert!(
        matches!(planner.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Report(_), .. }),
        "{:?}",
        planner.answer()
    );
    assert!(
        matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(goal.task))),
        Some(Record::Tasks(tasks::Stored::Ended(row))) if matches!(row.phase, tasks::Phase::Ended(tasks::Ending::Done(_)))),
        "goal live={:?} ended={:?} child={:?}",
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(goal.task))),
        driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(goal.task))),
        driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(child.task)))
    );
}

fn burst_parent_script() -> Script {
    let child = r#"{"executor":{"kind":"agent","charter":1},"spec":{"words":"@burst Send one coordinator update"},"contract":{"kind":"report","words":128},"authority":{"tools":2,"grants":[],"delegation":{"kinds":[],"tasks":0,"depth":0},"budget":{"spend":10},"notes":0}}"#;
    let delegate = format!(r#"{{"batch":[{child},{child},{child}]}}"#).into_bytes();
    Script {
        cue: b"@burstplan".as_slice().into(),
        turns: Box::new([
            Turn {
                lines: Box::new([Line::Call {
                    name: b"delegate".as_slice().into(),
                    arguments: delegate.into_boxed_slice(),
                }]),
                finish: Finish::ToolCalls,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() }]),
                finish: Finish::ToolCalls,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Text { text: b"Waiting for the batch.".as_slice().into() }]),
                finish: Finish::Stop,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() }]),
                finish: Finish::ToolCalls,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Text { text: b"Still waiting for three updates.".as_slice().into() }]),
                finish: Finish::Stop,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() }]),
                finish: Finish::ToolCalls,
                tokens: 20,
            },
            Turn {
                lines: Box::new([Line::Text { text: b"The batch is still being gathered.".as_slice().into() }]),
                finish: Finish::Stop,
                tokens: 20,
            },
        ]),
    }
}

fn burst_child_script(target: u64) -> Script {
    let message = format!(r#"{{"target":{target},"form":"words","words":"one update"}}"#).into_bytes();
    let send = || Turn {
        lines: Box::new([Line::Call {
            name: b"message".as_slice().into(),
            arguments: message.clone().into_boxed_slice(),
        }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    Script {
        cue: b"@burst".as_slice().into(),
        turns: Box::new([
            send(),
            Turn {
                lines: Box::new([Line::Call {
                    name: b"finish".as_slice().into(),
                    arguments: br#"{"report":"Three updates sent."}"#.as_slice().into(),
                }]),
                finish: Finish::ToolCalls,
                tokens: 20,
            },
        ]),
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the world follows each committed Smith message before and after the wake threshold"
)]
#[expect(clippy::wildcard_enum_match_arm, reason = "the story selects assignments and committed answers")]
fn a_smith_coordinator_is_woken_once_by_a_burst() {
    let mut driver = configured();
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 5,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    let owner_session = sign_in(&mut driver, 7, 401);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(402)),
        sign_in: owner_session,
        key: [42; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: b"@burstplan Coordinate updates".as_slice().into(),
            charter: 1,
            budget: 50,
            priority: 1,
        },
    });
    driver.settle();
    let coordinator = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::GoalStarted { task }), .. } => {
                Some(*task)
            }
            _ => None,
        })
        .expect("coordinator goal started");
    let first = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } if assignment.task == coordinator => Some(assignment.clone()),
            _ => None,
        })
        .expect("coordinator assigned");
    let mut planner = world::scripted_waiting_agent_for(&first, None, burst_parent_script());
    world::run_assignment(&mut driver, &first, &mut planner);
    assert!(matches!(planner.answer(), run::Answer::Parked { .. }));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(403)),
        sign_in: owner_session,
        key: [43; 16],
        ask: people::Ask::Amend {
            project: 1,
            task: coordinator,
            amendment: people::Amendment {
                spec: None,
                wake: Some(people::WakePolicy {
                    words: people::WakeRule::Batch { count: 3, age: Duration::from_secs(100) },
                    notices: people::WakeRule::Immediate,
                    news: people::WakeRule::Immediate,
                    results: people::ResultsWake::LastOrFailure,
                    questions: true,
                    answers: true,
                    timers: true,
                }),
                dependencies: None,
                authority: None,
                reason: b"Gather three updates".as_slice().into(),
            },
        },
    });
    driver.settle();
    assert!(
        matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(coordinator))),
        Some(Record::Tasks(tasks::Stored::Live(row)))
            if row.wake.words == tasks::WakeRule::Batch { count: 3, age: Duration::from_secs(100) }),
        "amend replies={:?} task={:?}",
        driver.delivered.iter().rev().take(4).collect::<Vec<_>>(),
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(coordinator)))
    );
    let amended = driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. }
                if assignment.task == coordinator && assignment.attempt > first.attempt =>
            {
                Some(assignment.clone())
            }
            _ => None,
        })
        .expect("amendment woke coordinator");
    let first_turn = planner.turns().first().expect("concrete Smith transcript");
    let transcript = smith::Transcript {
        version: first_turn.version,
        endpoint: first_turn.endpoint,
        dialect: first_turn.dialect,
        turns: planner.turns().into(),
    };
    let mut planner = world::scripted_waiting_agent_for(&amended, Some(transcript), burst_parent_script());
    world::run_assignment(&mut driver, &amended, &mut planner);
    assert!(matches!(planner.answer(), run::Answer::Parked { .. }), "{:?}", planner.answer());
    let children: Vec<_> = driver
        .delivered
        .iter()
        .filter_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } if assignment.task != coordinator => Some(assignment.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        children.len(),
        3,
        "three Smith children assigned: tasks={:?} calls={:?} deliveries={:?}",
        driver.store.rows.iter().filter(|(key, _)| matches!(key, Key::Tasks(tasks::Key::Live(_)))).collect::<Vec<_>>(),
        driver.delivered.iter().filter(|item| matches!(item, Delivery::CallAnswer { .. })).collect::<Vec<_>>(),
        driver.delivered.iter().filter(|item| matches!(item, Delivery::Assigned { .. })).collect::<Vec<_>>()
    );
    let mut messengers = Vec::new();
    for (index, child) in children.into_iter().enumerate() {
        let mut messenger = world::scripted_agent_for(&child, burst_child_script(coordinator));
        messenger.enable_parent_host_calls();
        assert!(!messenger.drive(1000), "Smith yields the next host call");
        let pending = messenger.pending_host_calls();
        assert_eq!(pending.len(), 1);
        let submission = pending[0].clone();
        let body = temper_engine_smith::call(submission.name, &submission.tool, &submission.input)
            .expect("declared message input");
        let before = driver.delivered.len();
        driver.send(engine::Event::Call {
            channel: Token::new(7),
            task: child.task,
            attempt: child.attempt,
            call: submission.relay.owner,
            body,
        });
        driver.settle();
        let answer = driver.delivered[before..]
            .iter()
            .find_map(|delivery| match delivery {
                Delivery::CallAnswer { call, answer, .. } if *call == submission.relay.owner => Some(answer),
                _ => None,
            })
            .expect("committed message outcome");
        assert!(matches!(answer, temper_engine_domain::CallAnswer::Sent { .. }), "message {}: {answer:?}", index + 1);
        messenger
            .return_host_reply(submission.relay, run::HostReply::Answered(temper_engine_smith::answer(answer)))
            .expect("one pending Smith relay");
        let wakes = driver.delivered.iter().filter(|delivery| matches!(delivery,
            Delivery::Assigned { assignment, .. } if assignment.task == coordinator && assignment.attempt > first.attempt)).count();
        assert_eq!(wakes, 1 + usize::from(index == 2), "wake count after message {}", index + 1);
        messengers.push((child, messenger));
    }
    for (child, mut messenger) in messengers {
        world::run_assignment(&mut driver, &child, &mut messenger);
        assert!(matches!(messenger.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Report(_), .. }));
    }
    let wakes = driver
        .delivered
        .iter()
        .filter(|delivery| {
            matches!(delivery,
        Delivery::Assigned { assignment, .. } if assignment.task == coordinator && assignment.attempt > first.attempt)
        })
        .count();
    assert_eq!(wakes, 2);
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(coordinator))),
        Some(Record::Tasks(tasks::Stored::Live(row))) if row.inbox.iter().filter(|entry| entry.kind == tasks::MessageKind::Words).count() == 3));
}
