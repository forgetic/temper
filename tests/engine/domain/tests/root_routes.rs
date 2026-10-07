use jig_core_accounts as accounts;
use jig_core_authority as authority;
use jig_core_brief as brief;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::VecDeque;
use temper_engine_domain::{Delivery, Key, Record, Write, engine};
use temper_engine_domain_world::commits::Store;
use temper_engine_domain_world::walking::{Settings, World, config, limits};
use temper_engine_domain_world::walking_referee::{FINAL_SPEND, QUESTION, REPORT};

use temper_engine_domain_world::direct::Driver;

fn turn(driver: &mut Driver, assignment: &engine::Assignment, number: u32, cumulative: u64) {
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        turn: engine::Turn { number, cumulative, read: None, transcript: b"step".as_slice().into() },
    });
}

#[test]
fn a_watched_run_shows_each_turn_as_it_commits() {
    let (mut driver, assignment) = batch_fixture();
    driver.send(engine::Event::Watch {
        watcher: Token::new(770),
        sign_in: driver.session(),
        key: [1; 16],
        subject: views::Subject::Run { task: Token::new(assignment.task), attempt: Token::new(assignment.attempt) },
    });
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Watching { watcher } if *watcher == Token::new(770)
    )));
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { watcher, chunks, .. }
            if *watcher == Token::new(770) && matches!(&**chunks, [views::Chunk::Snapshot { .. }])
    )));
    driver.send(engine::Event::ViewDelivered { watcher: Token::new(770), done: true });
    driver.viewed.clear();
    turn(&mut driver, &assignment, 1, 2);
    assert!(!driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { chunks, .. } if chunks.iter().any(|chunk| matches!(chunk,
            views::Chunk::Report { kind: views::Kind::Progress, .. }
        ))
    )));
    driver.settle();
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { watcher, chunks, missed: 0 }
            if *watcher == Token::new(770) && matches!(&**chunks,
                [views::Chunk::Report { task, attempt, kind: views::Kind::Progress, content, .. }]
                    if *task == Token::new(assignment.task)
                        && *attempt == Token::new(assignment.attempt)
                        && content.as_ref() == 1_u32.to_be_bytes())
    )));
    driver.send(engine::Event::ViewDelivered { watcher: Token::new(770), done: true });
    driver.viewed.clear();
    turn(&mut driver, &assignment, 2, 4);
    driver.settle();
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { watcher, chunks, missed: 0 }
            if *watcher == Token::new(770) && matches!(&**chunks,
                [views::Chunk::Report { kind: views::Kind::Progress, content, .. }]
                    if content.as_ref() == 2_u32.to_be_bytes())
    )));
}

#[test]
fn a_watch_key_replays_while_open_and_can_open_again_after_unwatch() {
    let (mut driver, assignment) = batch_fixture();
    let subject = views::Subject::Tree { task: Token::new(assignment.task) };
    let writes = driver.transactions.len();
    driver.send(engine::Event::Watch { watcher: Token::new(780), sign_in: driver.session(), key: [28; 16], subject });
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Watching { watcher } if *watcher == Token::new(780)
    )));
    assert_eq!(driver.transactions.len(), writes, "opening the view writes no record");

    driver.viewed.clear();
    driver.send(engine::Event::Watch { watcher: Token::new(781), sign_in: driver.session(), key: [28; 16], subject });
    assert_eq!(driver.viewed, [views::Request::Watching { watcher: Token::new(780) }]);
    assert_eq!(driver.transactions.len(), writes, "replay writes no record");

    driver.send(engine::Event::Unwatch { watcher: Token::new(780) });
    driver.send(engine::Event::ViewDelivered { watcher: Token::new(780), done: true });
    driver.viewed.clear();
    driver.send(engine::Event::Watch { watcher: Token::new(781), sign_in: driver.session(), key: [28; 16], subject });
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Watching { watcher } if *watcher == Token::new(781)
    )));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select the exact named tree snapshot")]
fn a_task_tree_watch_carries_a_child_phase_after_its_commit() {
    let (mut driver, assignment) = batch_fixture_with(2, 2);
    driver.send(engine::Event::Watch {
        watcher: Token::new(771),
        sign_in: driver.session(),
        key: [2; 16],
        subject: views::Subject::Tree { task: Token::new(assignment.task) },
    });
    let snapshot = driver
        .viewed
        .iter()
        .find_map(|request| match request {
            views::Request::Deliver { watcher, chunks, .. } if *watcher == Token::new(771) => match &**chunks {
                [views::Chunk::Snapshot { content, .. }] => Some(content.as_ref()),
                _ => None,
            },
            _ => None,
        })
        .expect("task tree snapshot");
    assert_eq!(&snapshot[..8], &assignment.task.to_be_bytes());
    driver.send(engine::Event::ViewDelivered { watcher: Token::new(771), done: true });
    driver.viewed.clear();
    let child = call_batch(&mut driver, &assignment, 772, Box::new([report_delegate(b"child", Box::new([]))]))[0];
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { watcher, chunks, .. } if *watcher == Token::new(771)
            && chunks.iter().any(|chunk| matches!(chunk,
                views::Chunk::Phase { task, .. } if *task == Token::new(child)
            ))
    )));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select the exact named goal snapshot and outcome")]
fn a_project_goals_watch_shows_a_later_priority_change() {
    let (mut driver, _, _, maintainer_session, _, member_session) = people_roles_driver();
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 2,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(773)),
        sign_in: member_session,
        key: [239; 16],
        ask: people::Ask::SetGoal { project: 1, spec: b"goal".as_slice().into(), charter: 1, budget: 20, priority: 1 },
    });
    driver.settle();
    let task = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::GoalStarted { task }), .. } => {
                Some(*task)
            }
            _ => None,
        })
        .expect("goal started");
    driver.send(engine::Event::Watch {
        watcher: Token::new(774),
        sign_in: maintainer_session,
        key: [3; 16],
        subject: views::Subject::Goals { project: 1 },
    });
    let snapshot = driver
        .viewed
        .iter()
        .find_map(|request| match request {
            views::Request::Deliver { watcher, chunks, .. } if *watcher == Token::new(774) => match &**chunks {
                [views::Chunk::Snapshot { content, .. }] => Some(content.as_ref()),
                _ => None,
            },
            _ => None,
        })
        .expect("project goals snapshot");
    assert_eq!(&snapshot[..8], &task.to_be_bytes());
    driver.send(engine::Event::ViewDelivered { watcher: Token::new(774), done: true });
    driver.viewed.clear();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(775)),
        sign_in: maintainer_session,
        key: [240; 16],
        ask: people::Ask::Prioritise { project: 1, goals: Box::new([(task, 7)]) },
    });
    driver.settle();
    assert!(driver.viewed.iter().any(|request| matches!(request,
        views::Request::Deliver { watcher, chunks, .. } if *watcher == Token::new(774)
            && chunks.iter().any(|chunk| matches!(chunk,
                views::Chunk::Report { task: report_task, kind: views::Kind::Progress, content, .. }
                    if *report_task == Token::new(task) && content[4..] == 7_u32.to_be_bytes()
            ))
    )));
}

fn people_roles_config(seed: u64) -> (engine::Config, engine::Limits) {
    let mut limits = limits();
    limits.fleet.slots = 2;
    limits.authority.roles = 4;
    limits.people.people = 4;
    limits.people.sign_ins = 4;
    limits.people.holdings = 4;
    limits.people.requests = 8;
    limits.people.pending = 4;
    limits.journal.writes = 1000;
    limits.journal.deliveries = 40;
    limits.journal.held = 120;
    let mut config = config(seed);
    let mut authority =
        authority::Domain::new(config.authority.rules().clone(), limits.authority).expect("larger policy room");
    let mut policy = config.authority.policy(1).expect("configured project").clone();
    let owner = policy.roles[0].clone();
    let mut maintainer = owner.clone();
    maintainer.number = 1;
    let mut member = owner.clone();
    member.number = 2;
    member.authority.budget.spend = 100;
    member.period_spend = 500;
    member.decides = authority::Proposals(0);
    let mut observer = owner.clone();
    observer.number = 3;
    observer.requests = authority::Requests(128);
    observer.decides = authority::Proposals(0);
    policy.roles = Box::new([owner, maintainer, member, observer]);
    let mut out = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut authority, authority::Event::Policy { project: 1, policy }, &mut out);
    assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: 1 }));
    config.authority = authority;
    (config, limits)
}

fn sign_in_person(driver: &mut Driver, user: u64, reply: u64) -> (u64, u64) {
    driver.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(reply)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: (user).to_be_bytes().into() },
            login: b"person".as_slice().into(),
            name: b"Person".as_slice().into(),
        },
    });
    driver.settle();
    (driver.store.header().people, driver.store.header().sign_ins)
}

fn people_roles_driver() -> (Driver, u64, u64, u64, u64, u64) {
    let (config, limits) = people_roles_config(219);
    let mut driver = Driver::configured(Store::new(), config, &limits);
    driver.settle();
    let (owner, owner_session) = sign_in_person(&mut driver, 7, 2001);
    let (maintainer, maintainer_session) = sign_in_person(&mut driver, 8, 2002);
    let (member, member_session) = sign_in_person(&mut driver, 9, 2003);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2004)),
        sign_in: owner_session,
        key: [200; 16],
        ask: people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([
                people::Holding { person: owner, role: people::Role::Owner },
                people::Holding { person: maintainer, role: people::Role::Maintainer },
                people::Holding { person: member, role: people::Role::Member },
            ]),
        },
    });
    driver.settle();
    (driver, owner_session, maintainer, maintainer_session, member, member_session)
}

#[test]
fn a_member_starts_a_chat_and_an_observer_is_refused() {
    let (mut driver, owner_session, maintainer, _, member, member_session) = people_roles_driver();
    let (observer, observer_session) = sign_in_person(&mut driver, 11, 2030);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2031)),
        sign_in: owner_session,
        key: [207; 16],
        ask: people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([
                people::Holding { person: 1, role: people::Role::Owner },
                people::Holding { person: maintainer, role: people::Role::Maintainer },
                people::Holding { person: member, role: people::Role::Member },
                people::Holding { person: observer, role: people::Role::Observer },
            ]),
        },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2033)),
        sign_in: observer_session,
        key: [209; 16],
        ask: people::Ask::StartChat { project: 1, words: b"hello".as_slice().into() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Role)), .. }
    )));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2032)),
        sign_in: member_session,
        key: [208; 16],
        ask: people::Ask::StartChat { project: 1, words: b"hello".as_slice().into() },
    });
    for _ in 0..20 {
        driver.advance(true);
        if driver.delivered.iter().any(|delivery| {
            matches!(
                delivery,
                Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Started { .. }), .. }
            )
        }) {
            break;
        }
    }
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Started { .. }), .. }
    )));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select the named question delivery")]
fn a_person_answers_a_numbered_question_from_their_task() {
    let (mut driver, parent) = batch_fixture_with(2, 1);
    let child = call_batch(&mut driver, &parent, 2034, Box::new([report_delegate(b"ask", Box::new([]))]))[0];
    let asker = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &asker,
        2035,
        engine::Tool::Message {
            target: parent.task,
            form: engine::MessageForm::Question,
            words: b"ship?".as_slice().into(),
        },
    );
    let question = driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Sent { message }, .. }
                if *call == Token::new(2035) =>
            {
                Some(*message)
            }
            _ => None,
        })
        .expect("numbered question committed");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2036)),
        sign_in: driver.session(),
        key: [210; 16],
        ask: people::Ask::AnswerQuestion { project: 1, task: parent.task, question, words: b"yes".as_slice().into() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::QuestionAnswered {
            task, question: answered, ..
        }), .. } if *task == parent.task && *answered == question
    )));
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task)))
    else {
        panic!("parent remains live")
    };
    assert!(
        row.inbox
            .iter()
            .any(|word| word.kind == (tasks::MessageKind::Answer { question }) && word.from == tasks::Party::Person(1))
    );
}

#[test]
fn a_maintainer_prioritises_project_goals_and_a_member_cannot() {
    let (mut driver, _, _, maintainer_session, _, member_session) = people_roles_driver();
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 2,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    for index in 0..2 {
        driver.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(2040 + index)),
            sign_in: member_session,
            key: [u8::try_from(211 + index).expect("small key"); 16],
            ask: people::Ask::SetGoal {
                project: 1,
                spec: b"goal".as_slice().into(),
                charter: 1,
                budget: 20,
                priority: 1,
            },
        });
        driver.settle();
    }
    let goals: Vec<_> = driver
        .store
        .rows
        .values()
        .filter_map(|row| match row {
            Record::Tasks(tasks::Stored::Live(task)) if task.tracked.is_some() => Some(task.number),
            Record::Tasks(
                tasks::Stored::Live(_)
                | tasks::Stored::Ended(_)
                | tasks::Stored::History(_)
                | tasks::Stored::Ledger(_)
                | tasks::Stored::PersonProposal(_),
            )
            | Record::People(_)
            | Record::Call(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_) => None,
        })
        .collect();
    assert_eq!(goals.len(), 2);
    let priorities: Box<[(u64, u32)]> = Box::new([(goals[0], 4), (goals[1], 9)]);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2042)),
        sign_in: member_session,
        key: [213; 16],
        ask: people::Ask::Prioritise { project: 1, goals: priorities.clone() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Role)), .. }
    )));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2043)),
        sign_in: maintainer_session,
        key: [214; 16],
        ask: people::Ask::Prioritise { project: 1, goals: priorities.clone() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Prioritised { project: 1 }), .. }
    )));
    for (number, priority) in priorities {
        let Some(Record::Tasks(tasks::Stored::Live(task))) =
            driver.store.rows.get(&Key::Tasks(tasks::Key::Live(number)))
        else {
            panic!("goal remains live")
        };
        assert_eq!(task.tracked, Some(priority));
    }
}

#[test]
fn a_person_amends_their_live_task_and_the_change_reaches_its_run() {
    let (mut driver, assignment) = batch_fixture();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2044)),
        sign_in: driver.session(),
        key: [215; 16],
        ask: people::Ask::Amend {
            project: 1,
            task: assignment.task,
            amendment: people::Amendment {
                spec: Some(people::Spec {
                    words: b"revised goal".as_slice().into(),
                    parameters: Box::new([]),
                    inputs: Box::new([]),
                }),
                wake: None,
                dependencies: None,
                authority: None,
                reason: b"new context".as_slice().into(),
            },
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Amended { task }), .. }
            if *task == assignment.task
    )));
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("amended task remains live")
    };
    assert_eq!(row.spec.words.as_ref(), b"revised goal");
    assert!(driver.store.rows.values().any(|record| matches!(record,
        Record::Tasks(tasks::Stored::History(history))
            if history.task == assignment.task && history.change == tasks::Change::Amended
                && history.by == tasks::Party::Person(1)
    )));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select the named proposal outcome")]
fn a_members_wider_amendment_waits_for_a_maintainer_to_accept() {
    let (mut driver, _, _, maintainer_session, _, member_session) = people_roles_driver();
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 2,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2045)),
        sign_in: member_session,
        key: [216; 16],
        ask: people::Ask::SetGoal { project: 1, spec: b"goal".as_slice().into(), charter: 1, budget: 50, priority: 1 },
    });
    driver.settle();
    let task = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::GoalStarted { task }), .. } => {
                Some(*task)
            }
            _ => None,
        })
        .expect("goal committed");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2046)),
        sign_in: member_session,
        key: [217; 16],
        ask: people::Ask::Amend {
            project: 1,
            task,
            amendment: people::Amendment {
                spec: None,
                wake: None,
                dependencies: None,
                authority: Some(people::Authority {
                    tools: 0,
                    grants: Box::new([]),
                    delegation: people::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
                    spend: 150,
                    deadline: None,
                    notes: 0,
                    note_resources: Box::new([]),
                }),
                reason: b"more scope".as_slice().into(),
            },
        },
    });
    driver.settle();
    let proposal = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::AmendProposed { task: named, proposal }),
                ..
            } if *named == task => Some(*proposal),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("widening proposal committed: {:?}", driver.delivered.iter().rev().take(4).collect::<Vec<_>>())
        });
    let Some(Record::Tasks(tasks::Stored::Live(row))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(task)))
    else {
        panic!("goal remains live")
    };
    assert_eq!(row.authority.budget.spend, 50);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2047)),
        sign_in: maintainer_session,
        key: [218; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: task,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    assert!(
        driver.delivered.iter().any(|delivery| matches!(delivery,
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ProposalDecided {
                proposer, proposal: decided, choice: people::ProposalChoice::Accepted, ..
            }), .. } if *proposer == task && *decided == proposal
        )),
        "acceptance: {:?}",
        driver.delivered.iter().rev().take(4).collect::<Vec<_>>()
    );
    let Some(Record::Tasks(tasks::Stored::Live(row))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(task)))
    else {
        panic!("goal remains live")
    };
    assert_eq!(row.authority.budget.spend, 150);
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select one web outcome among unrelated deliveries")]
fn a_members_goal_past_their_allotment_becomes_a_proposal_a_maintainer_accepts() {
    let (mut driver, _, maintainer, maintainer_session, member, member_session) = people_roles_driver();
    hello(&mut driver);
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2010)),
        sign_in: member_session,
        key: [201; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: b"ship a goal".as_slice().into(),
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
        .expect("member receives proposal identity");
    assert!(!driver.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(_)))));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2011)),
        sign_in: maintainer_session,
        key: [202; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: member,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ProposalDecided {
            proposer, proposal: decided, choice: people::ProposalChoice::Accepted, ..
        }), .. } if *proposer == member && *decided == proposal
    )));
    assert!(driver.store.rows.values().any(|row| matches!(row,
        Record::Tasks(tasks::Stored::Live(task))
            if task.requester == tasks::Party::Person(member) && task.tracked == Some(7)
    )));
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::PersonProposal(proposal))),
        Some(Record::Tasks(tasks::Stored::PersonProposal(row)))
            if row.state == tasks::PersonProposalState::Accepted { by: tasks::Party::Person(maintainer) }
    ));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "select one web outcome among unrelated deliveries")]
fn two_maintainers_decide_one_proposal_and_the_second_is_told_by_whom_and_how() {
    let (mut driver, owner_session, first, first_session, member, member_session) = people_roles_driver();
    hello(&mut driver);
    driver.settle();
    let (second, second_session) = sign_in_person(&mut driver, 10, 2020);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2021)),
        sign_in: owner_session,
        key: [203; 16],
        ask: people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([
                people::Holding { person: 1, role: people::Role::Owner },
                people::Holding { person: first, role: people::Role::Maintainer },
                people::Holding { person: second, role: people::Role::Maintainer },
                people::Holding { person: member, role: people::Role::Member },
            ]),
        },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2022)),
        sign_in: member_session,
        key: [204; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: b"race for goal".as_slice().into(),
            charter: 1,
            budget: 150,
            priority: 3,
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
        .expect("member proposed goal");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2023)),
        sign_in: first_session,
        key: [205; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: member,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    let before = driver.delivered.len();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2024)),
        sign_in: second_session,
        key: [206; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: member,
            proposal,
            decision: people::ProposalDecision::Reject { reason: b"too late".as_slice().into() },
        },
    });
    driver.settle();
    assert!(driver.delivered[before..].iter().any(|delivery| matches!(delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ProposalDecided {
            proposer, proposal: named, by, choice: people::ProposalChoice::Accepted,
        }), .. } if *proposer == member && *named == proposal && *by == first
    )));
    assert!(driver.store.rows.contains_key(&Key::ProposalDecision(proposal)));
}

#[test]
fn durable_start_turn_and_answer_callbacks_survive_full_journal_pressure() {
    let mut driver = Driver::new(Store::new());
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
        reply_to: ReplyTo::new(Token::new(201)),
        sign_in: driver.session(),
        key: [8; 16],
        ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
    });
    driver.pressure();
    assert!(!driver.delivered.iter().any(|delivery| matches!(delivery, Delivery::Assigned { .. })));
    driver.settle();
    let assignments: Vec<_> = driver
        .delivered
        .iter()
        .filter_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } => Some(assignment.clone()),
            Delivery::Reply { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Cancel { .. }
            | Delivery::Result { .. }
            | Delivery::View(_)
            | Delivery::Fleet(_)
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Relay { .. }
            | Delivery::Inbound { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. }
            | Delivery::InboxPage { .. }
            | Delivery::InboxView { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. } => None,
        })
        .collect();
    assert_eq!(assignments.len(), 1);
    let assignment = &assignments[0];
    turn(&mut driver, assignment, 1, 3);
    driver.pressure();
    assert!(!driver.delivered.iter().any(|delivery| matches!(delivery, Delivery::AcknowledgeTurn { .. })));
    driver.settle();
    turn(&mut driver, assignment, 2, 8);
    driver.settle();
    assert_eq!(
        driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::AcknowledgeTurn { .. })).count(),
        2
    );
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: FINAL_SPEND,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: REPORT.into() },
            cancel_delegates: false,
        },
    });
    driver.pressure();
    assert!(!driver.delivered.iter().any(|delivery| matches!(delivery, Delivery::Acknowledge { .. })));
    driver.settle();
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Acknowledge { .. })).count(), 1);
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Result { .. })).count(), 1);
}

#[test]
fn restart_recovers_named_ended_result_without_replaying_a_raw_notice() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(92) });
    world.run();
    let header = world.store.header();
    let task = world
        .store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Ended(task)) => Some(task.number),
            Record::Tasks(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::People(_)
            | Record::RunProof(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_)
            | Record::Terminal(_)
            | Record::Call(_) => None,
        })
        .expect("ended task");
    let mut driver = Driver::new(world.store);
    assert!(!driver.root.quiescent(), "cold startup has outstanding work");
    driver.settle();
    assert!(driver.delivered.is_empty(), "startup does not repeat any person or worker notice");
    driver.send(engine::Event::ReadResult { reply_to: ReplyTo::new(Token::new(301)), sign_in: header.sign_ins, task });
    assert!(!driver.root.quiescent(), "held result query is an obligation");
    driver.settle();
    assert_eq!(driver.store.header().messages, header.messages, "reading allocates no result position");
    assert_eq!(driver.store.header().tasks, header.tasks, "reading allocates no task");
    assert_eq!(driver.delivered.len(), 1);
    let Delivery::ResultReply { to, task: found, words, .. } = driver.delivered.pop().expect("one reply") else {
        panic!("task-derived result reply");
    };
    assert_eq!(to.into_token(), Token::new(301));
    assert_eq!(found, task);
    assert_eq!(words.as_ref(), REPORT);
    assert!(
        driver.transactions.iter().any(|writes| writes.iter().any(|write| matches!(
            write,
            Write::Save(Record::People(people::Stored::ReadPosition { person: 1, position: 1 }))
        ))),
        "read position commits before reply"
    );
    driver.send(engine::Event::ReadResult { reply_to: ReplyTo::new(Token::new(302)), sign_in: header.sign_ins, task });
    driver.settle();
    assert!(
        matches!(
            driver.delivered.pop(),
            Some(Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::Unknown), .. })
        ),
        "a read result cannot be read twice"
    );
    let rows = driver.store.rows.clone();
    let mut restarted = Driver::new(driver.store);
    restarted.settle();
    restarted.send(engine::Event::ReadInbox {
        reply_to: ReplyTo::new(Token::new(303)),
        sign_in: header.sign_ins,
        most: 2,
    });
    restarted.settle();
    assert!(matches!(restarted.delivered.pop(), Some(Delivery::InboxPage { entries, .. }) if entries.is_empty()));
    assert_eq!(restarted.store.rows, rows, "read position survives restart");
}

#[test]
fn unread_results_page_in_commit_order_across_restarts_with_bounded_loads() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(9300) });
    world.run();
    let mut store = world.store;
    let original =
        store
            .rows
            .values()
            .find_map(|row| {
                if let Record::Tasks(tasks::Stored::Ended(task)) = row { Some(task.as_ref().clone()) } else { None }
            })
            .expect("one actual committed result");
    store.rows.remove(&Key::Tasks(tasks::Key::Ended(original.number)));
    for (task, position) in [(1_u64, 3_u64), (2, 1), (3, 4), (4, 2)] {
        let mut row = original.clone();
        row.number = task;
        row.root = task;
        row.result_position = position;
        store.rows.insert(Key::Tasks(tasks::Key::Ended(task)), Record::Tasks(tasks::Stored::Ended(Box::new(row))));
    }
    let Some(Record::Deployment(header)) = store.rows.get_mut(&Key::Deployment) else { panic!("header") };
    header.tasks = 4;
    header.messages = 4;
    let sign_in = header.sign_ins;
    let mut first = Driver::new(store);
    first.settle();
    first.send(engine::Event::ReadResult { reply_to: ReplyTo::new(Token::new(939)), sign_in, task: 1 });
    first.settle();
    assert!(
        matches!(
            first.delivered.pop(),
            Some(Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::Busy), .. })
        ),
        "a later named result cannot skip older unread results"
    );
    assert!(!first.store.rows.contains_key(&Key::People(people::Key::ReadPosition(1))));
    first.send(engine::Event::ReadInbox { reply_to: ReplyTo::new(Token::new(940)), sign_in, most: 2 });
    first.settle();
    let Some(Delivery::InboxPage { entries, .. }) = first.delivered.pop() else { panic!("first page") };
    assert_eq!(entries.iter().map(|entry| (entry.task, entry.position)).collect::<Vec<_>>(), [(2, 1), (4, 2)]);
    assert!(first.result_loads.len() >= 4, "ended rows are paged through the existing load seam");
    assert!(
        first.result_loads.iter().all(|(most, rows)| *most <= first.env.limits.loads.rows
            && *rows <= usize::try_from(*most).expect("bounded page"))
    );
    let mut second = Driver::new(first.store);
    second.settle();
    second.send(engine::Event::ReadInbox { reply_to: ReplyTo::new(Token::new(941)), sign_in, most: 2 });
    second.settle();
    let Some(Delivery::InboxPage { entries, .. }) = second.delivered.pop() else { panic!("second page") };
    assert_eq!(entries.iter().map(|entry| (entry.task, entry.position)).collect::<Vec<_>>(), [(1, 3), (3, 4)]);
    second.send(engine::Event::ReadInbox { reply_to: ReplyTo::new(Token::new(942)), sign_in, most: 2 });
    second.settle();
    assert!(matches!(second.delivered.pop(), Some(Delivery::InboxPage { entries, .. }) if entries.is_empty()));
}

#[test]
fn a_full_inbox_pages_the_rest_from_the_store() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(9319) });
    world.run();
    let mut store = world.store;
    let original = store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Ended(task)) => Some(task.as_ref().clone()),
            Record::Call(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::Tasks(_)
            | Record::People(_) => None,
        })
        .expect("one committed result for the store fixture");
    store.rows.remove(&Key::Tasks(tasks::Key::Ended(original.number)));
    for number in 1..=5 {
        let mut row = original.clone();
        row.number = number;
        row.root = number;
        row.result_position = number;
        store.rows.insert(Key::Tasks(tasks::Key::Ended(number)), Record::Tasks(tasks::Stored::Ended(Box::new(row))));
    }
    let Some(Record::Deployment(header)) = store.rows.get_mut(&Key::Deployment) else { panic!("header") };
    header.tasks = 5;
    header.messages = 5;
    let sign_in = header.sign_ins;
    let mut driver = Driver::new(store);
    driver.settle();
    for (page, expected) in [[5, 4], [3, 2], [1, 0]].into_iter().enumerate() {
        let before = if page == 0 {
            None
        } else {
            let Some(Delivery::InboxView { next, .. }) = driver.delivered.pop() else { panic!("prior page") };
            next
        };
        driver.send(engine::Event::ViewInbox {
            reply_to: ReplyTo::new(Token::new(950 + page as u64)),
            sign_in,
            most: 2,
            before,
        });
        driver.settle();
        let Some(Delivery::InboxView { entries, next, .. }) = driver.delivered.last() else {
            panic!("whole inbox page")
        };
        let tasks: Vec<_> = entries
            .iter()
            .map(|entry| match entry {
                temper_engine_domain::InboxViewEntry::Result { result, .. } => result.task,
                temper_engine_domain::InboxViewEntry::Waiting(_) => panic!("historical fixture has no waiting entry"),
            })
            .collect();
        let wanted = if expected[1] == 0 { &expected[..1] } else { &expected[..] };
        assert_eq!(tasks, wanted);
        assert_eq!(next.is_some(), page < 2);
    }
    assert!(driver.result_loads.len() >= 3, "each page scans committed ended rows through bounded store loads");
    assert!(matches!(
        driver.store.rows.get(&Key::People(people::Key::ReadPosition(1))),
        Some(Record::People(people::Stored::ReadPosition { position: 5, .. }))
    ));
}

#[test]
fn an_entry_addressed_to_a_role_leaves_every_inbox_once_one_acts() {
    use temper_engine_domain_world::roles;
    let prior = roles::World::new(roles::Settings::calm(9320, roles::Base::FinalRole));
    let sessions = prior.sessions;
    let task = prior.task;
    let mut driver = Driver::configured(prior.store, administration_config(9320), &roles::limits());
    driver.settle();
    driver.delivered.clear();
    for (at, sign_in) in sessions.into_iter().enumerate() {
        driver.send(engine::Event::ViewInbox {
            reply_to: ReplyTo::new(Token::new(960 + at as u64)),
            sign_in,
            most: 4,
            before: None,
        });
        driver.settle();
        let Some(Delivery::InboxView { entries, .. }) = driver.delivered.pop() else { panic!("role inbox") };
        assert!(entries.iter().any(|entry| matches!(entry,
            temper_engine_domain::InboxViewEntry::Waiting(people::Entry { task: found, kind: people::EntryKind::Escalation { .. }, .. }) if *found == task)));
    }
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(962)),
        sign_in: sessions[0],
        key: [96; 16],
        ask: people::Ask::DecideEscalation {
            project: 1,
            task,
            revision: 2,
            decision: people::EscalationDecision::Release,
        },
    });
    for _ in 0..50 {
        driver.advance(true);
    }
    driver.delivered.clear();
    for (at, sign_in) in sessions.into_iter().enumerate() {
        driver.send(engine::Event::ViewInbox {
            reply_to: ReplyTo::new(Token::new(963 + at as u64)),
            sign_in,
            most: 4,
            before: None,
        });
        for _ in 0..50 {
            driver.advance(true);
        }
        let Some(Delivery::InboxView { entries, .. }) = driver.delivered.pop() else { panic!("updated role inbox") };
        assert!(!entries.iter().any(|entry| matches!(entry,
            temper_engine_domain::InboxViewEntry::Waiting(people::Entry { task: found, kind: people::EntryKind::Escalation { .. }, .. }) if *found == task)));
    }
}

#[test]
fn a_person_task_addressed_to_a_role_taken_by_one_handed_back_answered_by_another() {
    let (mut driver, root) = batch_fixture_custom(2, 1, 500, true, false, true);
    let first = driver.session();
    driver.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(970)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: 8_u64.to_be_bytes().into() },
            login: b"second".as_slice().into(),
            name: b"Second".as_slice().into(),
        },
    });
    driver.settle();
    let second = driver.store.header().sign_ins;
    let mut delegate = report_delegate(b"Choose between the reports", Box::new([]));
    delegate.executor = tasks::Executor::Person(tasks::PersonAddress::Role(0));
    delegate.contract = tasks::Contract::Verdict {
        choices: Box::new([tasks::Verdict { code: 1, words: 16 }, tasks::Verdict { code: 2, words: 16 }]),
    };
    let task = call_batch(&mut driver, &root, 971, Box::new([delegate]))[0];
    for (at, session) in [first, second].into_iter().enumerate() {
        driver.send(engine::Event::ViewInbox {
            reply_to: ReplyTo::new(Token::new(972 + at as u64)),
            sign_in: session,
            most: 4,
            before: None,
        });
        driver.settle();
        assert!(driver.delivered.iter().any(|item| matches!(item,
            Delivery::InboxView { entries, .. } if entries.iter().any(|entry| matches!(entry,
                temper_engine_domain::InboxViewEntry::Waiting(people::Entry { task: found, kind: people::EntryKind::PersonTask, .. }) if *found == task)))));
    }
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(974)),
        sign_in: first,
        key: [74; 16],
        ask: people::Ask::TakePerson { project: 1, task },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::PersonTaken { task: found }), .. } if *found == task)));
    let Some(Record::Tasks(tasks::Stored::Live(row))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(task)))
    else {
        panic!("claimed task")
    };
    assert_eq!(row.taken_by, Some(1));
    for (at, session) in [first, second].into_iter().enumerate() {
        driver.send(engine::Event::ViewInbox {
            reply_to: ReplyTo::new(Token::new(978 + at as u64)),
            sign_in: session,
            most: 4,
            before: None,
        });
        driver.settle();
        let Some(Delivery::InboxView { entries, .. }) = driver.delivered.last() else { panic!("claimed inbox") };
        assert_eq!(entries.iter().any(|entry| matches!(entry,
            temper_engine_domain::InboxViewEntry::Waiting(people::Entry { task: found, kind: people::EntryKind::PersonTask, .. }) if *found == task)), at == 0);
    }
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(975)),
        sign_in: first,
        key: [75; 16],
        ask: people::Ask::HandBackPerson { project: 1, task },
    });
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(row))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(task)))
    else {
        panic!("returned task")
    };
    assert_eq!(row.taken_by, None);
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(976)),
        sign_in: second,
        key: [76; 16],
        ask: people::Ask::TakePerson { project: 1, task },
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(977)),
        sign_in: second,
        key: [77; 16],
        ask: people::Ask::AnswerPerson {
            project: 1,
            task,
            result: people::PersonResult::Verdict { code: 2, words: b"second".as_slice().into() },
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::PersonAnswered { task: found }), .. } if *found == task)), "{:?}", driver.delivered);
    assert!(driver.store.rows.contains_key(&Key::Tasks(tasks::Key::Ended(task))));
}

#[test]
fn a_person_stops_a_run_and_releases_it() {
    let (mut driver, assignment) = running_fixture();
    let session = driver.session();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(980)),
        sign_in: session,
        key: [80; 16],
        ask: people::Ask::Stop { project: 1, task: assignment.task },
    });
    for _ in 0..40 {
        driver.advance(true);
    }
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Stopped { task }), .. }
            if *task == assignment.task)));
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::Cancel { task, attempt, .. } if *task == assignment.task && *attempt == assignment.attempt)));
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("stopped task")
    };
    assert!(matches!(row.phase, tasks::Phase::Held { why: tasks::Hold::Stopped, .. }));
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
        saved: None,
    });
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(981)),
        sign_in: session,
        key: [81; 16],
        ask: people::Ask::Release { project: 1, task: assignment.task },
    });
    for _ in 0..40 {
        driver.advance(true);
    }
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Released { task }), .. }
            if *task == assignment.task)));
}

fn hello(driver: &mut Driver) {
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
}

fn chat(driver: &mut Driver, key: u8) {
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(401)),
        sign_in: driver.session(),
        key: [key; 16],
        ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
    });
}

fn assigned(driver: &Driver) -> engine::Assignment {
    driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } => Some(assignment.clone()),
            Delivery::Reply { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Cancel { .. }
            | Delivery::Result { .. }
            | Delivery::View(_)
            | Delivery::Fleet(_)
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Relay { .. }
            | Delivery::Inbound { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. }
            | Delivery::InboxPage { .. }
            | Delivery::InboxView { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. } => None,
        })
        .expect("assigned chat")
}

#[test]
fn unavailable_account_retains_chat_until_its_real_refresh_terminal() {
    let mut configuration = config(93);
    configuration.account_valid = None;
    let mut driver = Driver::configured(Store::new(), configuration, &limits());
    hello(&mut driver);
    for _ in 0..50 {
        driver.advance(true);
    }
    assert!(driver.root.ready());
    assert!(!driver.root.quiescent(), "credential protocol still owes a refresh terminal");
    driver.sign_in();
    for _ in 0..50 {
        driver.advance(true);
    }
    chat(&mut driver, 9);
    for _ in 0..50 {
        driver.advance(true);
    }
    assert!(!driver.delivered.iter().any(|delivery| matches!(delivery, Delivery::Assigned { .. })));
    let (account, generation) = driver
        .accounts
        .iter()
        .find_map(|request| match request {
            accounts::Request::Refresh { account, generation } => Some((*account, *generation)),
            accounts::Request::Keep { .. }
            | accounts::Request::Cancel { .. }
            | accounts::Request::Granted { .. }
            | accounts::Request::Availability { .. }
            | accounts::Request::Refused { .. }
            | accounts::Request::Closed { .. } => None,
        })
        .expect("real account refresh request");
    driver.send(engine::Event::Refreshed { account, generation, valid: Duration::from_secs(60) });
    driver.settle();
    assert_eq!(assigned(&driver).grant.generation, generation);
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Assigned { .. })).count(), 1);
}

#[test]
fn repeated_unknown_losses_under_commit_pressure_consume_no_callback_room() {
    let mut driver = Driver::new(Store::new());
    driver.settle();
    driver.sign_in();
    driver.sign_in();
    driver.sign_in();
    assert_eq!(driver.store.pending.len(), 3);
    for channel in 1000..5000 {
        driver.send(engine::Event::Lost { channel: Token::new(channel) });
    }
    assert_eq!(driver.store.pending.len(), 3);
    driver.settle();
    assert!(driver.root.quiescent());
}

#[test]
fn refused_terminal_clears_fleet_handoff_without_charging_rejected_spend() {
    let mut driver = Driver::new(Store::new());
    hello(&mut driver);
    driver.settle();
    driver.sign_in();
    driver.settle();
    chat(&mut driver, 10);
    driver.settle();
    let assignment = assigned(&driver);
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: FINAL_SPEND,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: vec![b'x'; 129].into_boxed_slice() },
            cancel_delegates: false,
        },
    });
    driver.settle();
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Acknowledge { .. })).count(), 1);
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("invalid terminal leaves retryable live task");
    };
    assert_eq!(task.run_spent, 0);
    assert_eq!(task.numbers.spent, 0);
    assert_eq!(task.tries.invalid, 1);
    assert!(matches!(task.phase, tasks::Phase::Active(tasks::Active::BackingOff { .. })));
}

#[test]
fn a_refused_assignment_spends_no_try() {
    let mut limits = limits();
    limits.fleet.attempts = 1;
    let mut driver = Driver::configured(Store::new(), config(94), &limits);
    hello(&mut driver);
    driver.settle();
    driver.sign_in();
    driver.settle();
    chat(&mut driver, 11);
    driver.settle();
    chat(&mut driver, 12);
    driver.settle();
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Assigned { .. })).count(), 1);
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&temper_engine_domain::Key::Tasks(tasks::Key::Live(2)))
    else {
        panic!("second chat is durable but unplaced");
    };
    assert!(matches!(task.phase, tasks::Phase::Active(tasks::Active::BackingOff { .. })));
    assert_eq!(task.run_spent, 0);
    assert_eq!(task.numbers.spent, 0);
    assert_eq!(task.tries, tasks::Tries::NONE);
}

#[test]
fn static_authority_waits_are_durable_holds_and_do_not_spin_the_ready_pass() {
    for deadline in [false, true] {
        let mut configuration = config(95);
        if deadline {
            configuration.chat_authority.budget.deadline = Some(Wall::EPOCH);
        } else {
            configuration.chat_authority.budget.spend = 1;
        }
        let mut driver = Driver::configured(Store::new(), configuration, &limits());
        driver.env.wall = Wall::from_nanos(1);
        hello(&mut driver);
        driver.settle();
        driver.sign_in();
        driver.settle();
        chat(&mut driver, 13);
        driver.settle();
        let Some(Record::Tasks(tasks::Stored::Live(task))) =
            driver.store.rows.get(&temper_engine_domain::Key::Tasks(tasks::Key::Live(1)))
        else {
            panic!("authorized chat with unavailable run is durable");
        };
        let expected = if deadline { tasks::Hold::Deadline } else { tasks::Hold::Budget };
        assert!(matches!(task.phase, tasks::Phase::Held { why, .. } if why == expected));
        let header = driver.store.header();
        let outputs = driver.delivered.len();
        for _ in 0..20 {
            driver.advance(true);
        }
        assert_eq!(driver.store.header(), header);
        assert_eq!(driver.delivered.len(), outputs);
        assert!(driver.root.quiescent());
    }
}

#[test]
fn invalid_nonfinal_people_restore_page_stops_before_issuing_its_continuation() {
    let mut store = Store::new();
    for number in 1..=3 {
        let record = Record::People(people::Stored::Person {
            number,
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                login: b"same".as_slice().into(),
                name: b"Same".as_slice().into(),
            },
        });
        store.rows.insert(record.key(), record);
    }
    let mut driver = Driver::new(store);
    for _ in 0..30 {
        driver.advance(true);
    }
    assert!(driver.stopped, "duplicate identity in middle page is a terminal startup failure");
    assert!(!driver.root.ready());
    assert!(!driver.root.quiescent());
    assert!(driver.events.is_empty(), "failed page did not issue another continuation");
}

#[test]
fn invalid_nonfinal_task_restore_page_stops_before_issuing_its_continuation() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(96) });
    world.run();
    let mut record = world
        .store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Ended(record)) => Some(record.as_ref().clone()),
            Record::Tasks(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::People(_)
            | Record::RunProof(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_)
            | Record::Terminal(_)
            | Record::Call(_) => None,
        })
        .expect("ended fixture");
    record.spec.words = vec![b'x'; 65].into_boxed_slice();
    record.phase = tasks::Phase::Waiting;
    let row = Record::Tasks(tasks::Stored::Live(Box::new(record)));
    world.store.rows.insert(row.key(), row);
    let mut driver = Driver::new(world.store);
    for _ in 0..60 {
        driver.advance(true);
    }
    assert!(driver.stopped, "oversized child shape ends startup");
    assert!(!driver.root.ready());
    assert!(!driver.root.quiescent());
    assert!(driver.events.is_empty());
}

#[test]
fn root_configuration_refuses_a_decoded_page_larger_than_synchronous_route_room() {
    let mut limits = limits();
    limits.loads.rows = u32::MAX;
    assert!(engine::worst_case(&limits).is_none());
}

#[test]
fn maximum_cold_hello_batch_and_duplicate_losses_fit_one_startup_decision() {
    let mut limits = limits();
    limits.fleet.workers = 4;
    let mut driver = Driver::configured(Store::new(), config(97), &limits);
    for channel in 1..=4 {
        driver.send(engine::Event::Hello {
            channel: Token::new(channel),
            hello: fleet::Hello {
                stop_bound: Duration::from_secs(5),
                slots: 1,
                workstreams: Box::new([]),
                hosting: Box::new([]),
            },
        });
        driver.send(engine::Event::Lost { channel: Token::new(channel) });
        driver.send(engine::Event::Lost { channel: Token::new(channel) });
    }
    driver.send(engine::Event::Hello {
        channel: Token::new(1),
        hello: fleet::Hello {
            stop_bound: limits.fleet.grace,
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Refuse { .. })).count(), 5);
    assert!(!driver.stopped);
    limits.fleet.workers = 16;
    assert!(engine::worst_case(&limits).is_none(), "cold callback outputs plus continuation must fit");
}

#[test]
fn overflowing_child_limits_refuse_root_startup_without_panicking() {
    let mut configured = limits();
    configured.tasks.tasks = u32::MAX;
    assert!(engine::worst_case(&configured).is_none());
    configured = limits();
    configured.fleet.workers = u32::MAX;
    configured.fleet.slots = u32::MAX;
    assert!(engine::worst_case(&configured).is_none());
}

#[test]
fn authenticated_result_query_refuses_monotonic_expiry_after_backward_wall_jump_before_fire() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(101) });
    world.run();
    let header = world.store.header();
    let task = world
        .store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Ended(task)) => Some(task.number),
            Record::Tasks(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::People(_)
            | Record::RunProof(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_)
            | Record::Terminal(_)
            | Record::Call(_) => None,
        })
        .expect("ended chat");
    let mut driver = Driver::new(world.store);
    driver.settle();
    driver.env.now = Time::from_nanos(driver.env.limits.people.sign_in_lifetime.as_nanos() * 2);
    driver.env.wall = Wall::EPOCH;
    driver.send(engine::Event::ReadResult { reply_to: ReplyTo::new(Token::new(901)), sign_in: header.sign_ins, task });
    driver.settle();
    assert!(matches!(
        driver.delivered.as_slice(),
        [Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::SignIn), .. }]
    ));
    assert_eq!(driver.store.header(), header);
}

fn running_fixture() -> (Driver, engine::Assignment) {
    let mut driver = Driver::new(Store::new());
    hello(&mut driver);
    driver.settle();
    driver.sign_in();
    driver.settle();
    chat(&mut driver, 12);
    driver.settle();
    assert!(
        driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { .. })),
        "delegate fixture delivery: {:?}",
        driver.delivered
    );
    let assignment = assigned(&driver);
    turn(&mut driver, &assignment, 1, 3);
    driver.settle();
    (driver, assignment)
}

fn unavailable_call(driver: &mut Driver, assignment: &engine::Assignment, call: u64) {
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(call),
        body: engine::Call { completion: 2, position: 0, tool: engine::Tool::Unavailable },
    });
}

fn delegate_fixture() -> (Driver, engine::Assignment) {
    let mut configuration = config(91);
    let mut rules = configuration.authority.rules().clone();
    rules.maximum_run_spend = 60;
    rules.ceiling.delegation.depth = 2;
    let mut policy = configuration.authority.policy(1).expect("fixture project").clone();
    policy.ceiling.delegation.depth = 2;
    policy.roles[0].authority.delegation.depth = 2;
    let mut authority =
        jig_core_authority::Domain::new(rules, *configuration.authority.limits()).expect("expanded fixture authority");
    let mut policy_out = Queue::with_capacity(jig_core_authority::POLICY_MAX_OUT);
    jig_core_authority::step(&mut authority, jig_core_authority::Event::Policy { project: 1, policy }, &mut policy_out);
    assert_eq!(policy_out.pop(), Some(jig_core_authority::PolicyFact::Added { project: 1 }));
    configuration.authority = authority;
    configuration.chat_authority.delegation.kinds = Box::new([jig_core_authority::Executor::Charter(1)]);
    configuration.chat_authority.delegation.tasks = 1;
    configuration.chat_authority.delegation.depth = 1;
    let mut driver = Driver::configured(Store::new(), configuration, &limits());
    hello(&mut driver);
    driver.settle();
    driver.sign_in();
    driver.settle();
    chat(&mut driver, 12);
    driver.settle();
    assert!(
        driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { .. })),
        "delegate fixture delivery: {:?}",
        driver.delivered
    );
    let assignment = assigned(&driver);
    (driver, assignment)
}

fn batch_fixture() -> (Driver, engine::Assignment) {
    batch_fixture_with(1, 1)
}

fn batch_fixture_with(slots: u32, depth: u32) -> (Driver, engine::Assignment) {
    batch_fixture_with_pool(slots, depth, 500)
}

fn batch_fixture_with_pool(slots: u32, depth: u32, pool_budget: u64) -> (Driver, engine::Assignment) {
    batch_fixture_custom(slots, depth, pool_budget, false, false, false)
}

fn batch_fixture_custom(
    slots: u32,
    depth: u32,
    pool_budget: u64,
    second_owner: bool,
    procedure: bool,
    person: bool,
) -> (Driver, engine::Assignment) {
    let mut bounds = limits();
    bounds.tasks.tasks = 4;
    bounds.tasks.project_tasks = 4;
    bounds.tasks.tree_tasks = 4;
    bounds.tasks.depth = depth;
    bounds.tasks.delegates = 3;
    bounds.tasks.batch = 3;
    bounds.tasks.dependencies = 2;
    bounds.tasks.inbox_messages = 6;
    bounds.tasks.inbox_bytes = 384;
    if person {
        bounds.tasks.contract_choices = 2;
        bounds.people.requests = 8;
    }
    if second_owner {
        bounds.people.initial_owners = 2;
    }
    bounds.authority.batch = 3;
    if procedure || person {
        bounds.authority.executors = 2;
        bounds.tasks.executor_kinds = 2;
    }
    bounds.fleet.attempts = 5;
    bounds.fleet.slots = slots;
    bounds.call_records = 4;
    bounds.brief.sections = 5;
    bounds.journal.writes = 3000;
    bounds.journal.deliveries = 100;
    bounds.journal.held = 300;
    let mut configuration = config(92);
    configuration.person_budget = pool_budget;
    if second_owner {
        configuration.owners = Box::new([
            people::InitialOwner {
                project: 1,
                identity: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
            },
            people::InitialOwner {
                project: 1,
                identity: people::IdentityKey { provider: 0, subject: 8_u64.to_be_bytes().into() },
            },
        ]);
    }
    let mut rules = configuration.authority.rules().clone();
    // Each claimed run leaves room in its task allotment for delegates made during the run.
    rules.maximum_run_spend = 5;
    if procedure || person {
        rules.ceiling.delegation.kinds = Box::new([
            jig_core_authority::Executor::Charter(1),
            if procedure { jig_core_authority::Executor::Procedure(1) } else { jig_core_authority::Executor::Role(0) },
        ]);
    }
    rules.ceiling.delegation.tasks = 4;
    rules.ceiling.delegation.depth = depth + 1;
    let mut policy = configuration.authority.policy(1).expect("fixture project").clone();
    if procedure || person {
        policy.ceiling.delegation.kinds.clone_from(&rules.ceiling.delegation.kinds);
        policy.roles[0].authority.delegation.kinds.clone_from(&rules.ceiling.delegation.kinds);
    }
    policy.ceiling.delegation.tasks = 4;
    policy.ceiling.delegation.depth = depth + 1;
    policy.roles[0].authority.delegation.tasks = 4;
    policy.roles[0].authority.delegation.depth = depth + 1;
    let mut authority = jig_core_authority::Domain::new(rules, bounds.authority).expect("larger fixture authority");
    let mut policy_out = Queue::with_capacity(jig_core_authority::POLICY_MAX_OUT);
    jig_core_authority::step(&mut authority, jig_core_authority::Event::Policy { project: 1, policy }, &mut policy_out);
    assert_eq!(policy_out.pop(), Some(jig_core_authority::PolicyFact::Added { project: 1 }));
    configuration.authority = authority;
    configuration.chat_authority.delegation.kinds = if procedure || person {
        Box::new([
            jig_core_authority::Executor::Charter(1),
            if procedure { jig_core_authority::Executor::Procedure(1) } else { jig_core_authority::Executor::Role(0) },
        ])
    } else {
        Box::new([jig_core_authority::Executor::Charter(1)])
    };
    configuration.chat_authority.delegation.tasks = 3;
    configuration.chat_authority.delegation.depth = depth;
    let mut driver = Driver::configured(Store::new(), configuration, &bounds);
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    driver.sign_in();
    driver.settle();
    chat(&mut driver, 12);
    driver.settle();
    let assignment = assigned(&driver);
    (driver, assignment)
}

fn report_delegate(words: &[u8], dependencies: Box<[engine::Dependency]>) -> engine::Delegate {
    engine::Delegate {
        executor: tasks::Executor::Agent { charter: 1 },
        spec: tasks::Spec { words: words.into(), parameters: Box::new([]), inputs: Box::new([]) },
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
        dependencies,
        wake: tasks::WakePolicy::DEFAULT,
    }
}

#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one delivery from the closed vocabulary")]
fn call_batch(
    driver: &mut Driver,
    parent: &engine::Assignment,
    call: u64,
    batch: Box<[engine::Delegate]>,
) -> Box<[u64]> {
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        call: Token::new(call),
        body: engine::Call {
            completion: 1,
            position: u32::try_from(call).expect("small call"),
            tool: engine::Tool::Delegate { batch },
        },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::CallAnswer {
                call: answered,
                answer: temper_engine_domain::CallAnswer::Delegated(numbers),
                ..
            } if *answered == Token::new(call) => Some(numbers.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("whole batch answered: {:?}", driver.delivered))
}

#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one assignment from the closed vocabulary")]
fn assigned_task(driver: &Driver, task: u64) -> engine::Assignment {
    driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::Assigned { assignment, .. } if assignment.task == task => Some(assignment.clone()),
            _ => None,
        })
        .expect("task assigned")
}

fn tool_call(driver: &mut Driver, source: &engine::Assignment, call: u64, tool: engine::Tool) {
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: source.task,
        attempt: source.attempt,
        call: Token::new(call),
        body: engine::Call { completion: 1, position: u32::try_from(call).expect("small call"), tool },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
}

#[test]
fn a_proposal_reaches_its_covering_task_and_is_accepted() {
    let (mut driver, parent) = batch_fixture_with(3, 2);
    let child = call_batch(&mut driver, &parent, 61, Box::new([report_delegate(b"child", Box::new([]))]))[0];
    let child_run = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &child_run,
        62,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([report_delegate(b"grandchild", Box::new([]))])),
            reason: b"need a helper".as_slice().into(),
            as_holder: false,
        },
    );
    let proposal = driver
        .delivered
        .iter()
        .find_map(|item| {
            if let Delivery::CallAnswer {
                call, answer: temper_engine_domain::CallAnswer::Proposed { proposal }, ..
            } = item
            {
                (*call == Token::new(62)).then_some(*proposal)
            } else {
                None
            }
        })
        .expect("proposal accepted for routing");
    assert!(driver.delivered.iter().any(|item| matches!(
        item,
        Delivery::Inbound { task, word, .. }
            if *task == parent.task
                && word.kind == tasks::MessageKind::Proposal {
                    proposer: child,
                    proposal,
                    kind: tasks::ProposalKind::Batch,
                }
    )));
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        turn: engine::Turn {
            number: 1,
            cumulative: 1,
            read: Some(proposal),
            transcript: b"consider proposal".as_slice().into(),
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(
        item,
        Delivery::AcknowledgeTurn { task, attempt, turn, .. }
            if *task == parent.task && *attempt == parent.attempt && *turn == 1
    )));
    let live = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("live child");
    assert!(
        matches!(live, Record::Tasks(tasks::Stored::Live(row)) if matches!(&row.proposal, Some(p) if p.state == tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Task(parent.task), since: driver.env.wall }))
    );
    tool_call(
        &mut driver,
        &parent,
        63,
        engine::Tool::Decide { proposer: child, proposal, decision: engine::ProposalChoice::Accept },
    );
    assert!(driver.delivered.iter().any(|item| matches!(item, Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::ProposalDecided { proposal: found, outcome: tasks::ProposalOutcome::Accepted }, .. } if *call == Token::new(63) && *found == proposal)));
    let live = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("live child");
    assert!(
        matches!(live, Record::Tasks(tasks::Stored::Live(row)) if row.proposal.is_none() && row.delegates.len() == 1 && row.inbox.iter().any(|word| word.kind == tasks::MessageKind::ProposalDecision { proposal, accepted: true }))
    );
}

#[test]
fn a_chats_goal_accepted_as_its_persons_outlives_the_chat() {
    let (mut driver, parent) = batch_fixture_with(3, 2);
    let child = call_batch(&mut driver, &parent, 64, Box::new([report_delegate(b"chat work", Box::new([]))]))[0];
    let child_run = assigned_task(&driver, child);
    let mut goal = report_delegate(b"long goal", Box::new([]));
    goal.authority.budget.spend = 150;
    tool_call(
        &mut driver,
        &child_run,
        65,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([goal])),
            reason: b"goal outlives chat".as_slice().into(),
            as_holder: true,
        },
    );
    let proposal = driver
        .delivered
        .iter()
        .find_map(|item| {
            if let Delivery::CallAnswer {
                call, answer: temper_engine_domain::CallAnswer::Proposed { proposal }, ..
            } = item
            {
                (*call == Token::new(65)).then_some(*proposal)
            } else {
                None
            }
        })
        .expect("goal proposal routed");
    let person = driver.store.header().people;
    assert!(
        matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))), Some(Record::Tasks(tasks::Stored::Live(row))) if matches!(&row.proposal, Some(p) if matches!(p.state, tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Person(found), .. } if found == person)))
    );
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(406)),
        sign_in: driver.session(),
        key: [66; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: child,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item, Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ProposalDecided { proposer, proposal: found, choice: people::ProposalChoice::Accepted, .. }), .. } if *proposer == child && *found == proposal)));
    let goal_number = driver
        .store
        .rows
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Key::Tasks(tasks::Key::Live(number)), Record::Tasks(tasks::Stored::Live(row)))
                if row.requester == tasks::Party::Person(person) && *number != parent.task =>
            {
                Some(*number)
            }
            _ => None,
        })
        .expect("goal owned by accepting person");
    assert_ne!(goal_number, child);
    tool_call(&mut driver, &parent, 67, engine::Tool::Cancel { target: child, reason: b"chat done".as_slice().into() });
    assert!(
        driver.store.rows.contains_key(&Key::Tasks(tasks::Key::Live(goal_number))),
        "person's goal survives chat subtree cancellation"
    );
}

#[test]
fn a_proposal_routed_past_a_procedure_to_a_person_is_accepted() {
    let (mut driver, root) = batch_fixture_custom(3, 3, 500, false, true, false);
    let mut procedure = report_delegate(b"procedure", Box::new([]));
    procedure.executor = tasks::Executor::Procedure { connector: 1, code: 1 };
    procedure.authority.budget.spend = 50;
    procedure.authority.delegation.kinds = Box::new([tasks::AuthorityExecutor::Charter(1)]);
    procedure.authority.delegation.tasks = 1;
    procedure.authority.delegation.depth = 1;
    let parent = call_batch(&mut driver, &root, 601, Box::new([procedure]))[0];
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::Procedure { task, step: 1, connector: 1, code: 1 } if *task == parent
    )));
    driver.send(engine::Event::ProcedureStep {
        task: parent,
        step: 1,
        connector: 1,
        code: 1,
        action: engine::ProcedureAction::Delegate(Box::new([report_delegate(b"child", Box::new([]))])),
    });
    driver.settle();
    let child = driver
        .store
        .rows
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Key::Tasks(tasks::Key::Live(number)), Record::Tasks(tasks::Stored::Live(row)))
                if row.requester == tasks::Party::Task(parent) =>
            {
                Some(*number)
            }
            _ => None,
        })
        .expect("procedure delegate committed");
    let child_run = assigned_task(&driver, child);
    let mut goal = report_delegate(b"larger goal", Box::new([]));
    goal.authority.budget.spend = 150;
    tool_call(
        &mut driver,
        &child_run,
        602,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([goal])),
            reason: b"needs person funding".as_slice().into(),
            as_holder: false,
        },
    );
    let proposal = driver
        .delivered
        .iter()
        .find_map(|item| {
            if let Delivery::CallAnswer {
                call, answer: temper_engine_domain::CallAnswer::Proposed { proposal }, ..
            } = item
            {
                (*call == Token::new(602)).then_some(*proposal)
            } else {
                None
            }
        })
        .expect("proposal routed");
    let person = driver.store.header().people;
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))),
        Some(Record::Tasks(tasks::Stored::Live(row))) if matches!(&row.proposal,
            Some(p) if matches!(p.state, tasks::ProposalState::Pending {
                holder: tasks::ProposalHolder::Person(found), ..
            } if found == person)
        )
    ));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(603)),
        sign_in: driver.session(),
        key: [67; 16],
        ask: people::Ask::DecideProposal {
            project: 1,
            proposer: child,
            proposal,
            decision: people::ProposalDecision::Accept,
        },
    });
    driver.settle();
    assert!(
        driver.delivered.iter().any(|item| matches!(item,
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ProposalDecided {
                proposer, proposal: found, choice: people::ProposalChoice::Accepted, ..
            }), .. } if *proposer == child && *found == proposal
        )),
        "deliveries: {:?}",
        driver.delivered
    );
}

#[test]
fn a_recurring_procedure_uses_the_root_period_route() {
    let mut driver = Driver::new(Store::new());
    hello(&mut driver);
    driver.settle();
    let mut member = tasks::New {
        number: 1,
        project: 1,
        executor: tasks::Executor::Agent { charter: 1 },
        spec: tasks::Spec { words: b"period work".as_slice().into(), parameters: Box::new([]), inputs: Box::new([]) },
        contract: tasks::Contract::Report { words: 16 },
        authority: report_delegate(b"template", Box::new([])).authority,
        numbers: tasks::Numbers { budget: 10, spent: 0, spent_below: 0, reserved: 0 },
        funder: tasks::Funder::Period { project: 1, period: 0 },
        dependencies: Box::new([]),
        wake: tasks::WakePolicy::DEFAULT,
        recurring: None,
        tracked: None,
    };
    member.authority.budget.spend = 10;
    let mut authority = report_delegate(b"template", Box::new([])).authority;
    authority.delegation.kinds = Box::new([tasks::AuthorityExecutor::Charter(1)]);
    authority.delegation.tasks = 2;
    authority.delegation.depth = 1;
    authority.budget.spend = 30;
    driver.send(engine::Event::StartRecurring {
        project: 1,
        authority,
        template: tasks::RecurringTemplate {
            key: 1,
            batch: Box::new([member]),
            overlap: tasks::RecurringOverlap::Skip,
        },
    });
    driver.settle();
    let master = driver
        .store
        .rows
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Key::Tasks(tasks::Key::Live(number)), Record::Tasks(tasks::Stored::Live(row)))
                if row.recurring.is_some() =>
            {
                Some(*number)
            }
            _ => None,
        })
        .expect("recurring task committed");
    let child = driver
        .store
        .rows
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Key::Tasks(tasks::Key::Live(number)), Record::Tasks(tasks::Stored::Live(row)))
                if row.requester == tasks::Party::Task(master) =>
            {
                Some(*number)
            }
            _ => None,
        })
        .expect("period batch committed");
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))),
        Some(Record::Tasks(tasks::Stored::Live(row))) if row.funder == tasks::Funder::Recurring {
            project: 1, task: master, period: 1,
        }
    ));
    let run = assigned_task(&driver, child);
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: child,
        attempt: run.attempt,
        cumulative: 0,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"done".as_slice().into() },
            cancel_delegates: false,
        },
    });
    driver.settle();
    driver.send(engine::Event::Period { project: 1, period: 2, budget: 1_000 });
    driver.settle();
    let next = driver
        .store
        .rows
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (Key::Tasks(tasks::Key::Live(number)), Record::Tasks(tasks::Stored::Live(row)))
                if row.requester == tasks::Party::Task(master) && *number != child =>
            {
                Some(*number)
            }
            _ => None,
        })
        .expect("new period batch committed");
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(next))),
        Some(Record::Tasks(tasks::Stored::Live(row))) if row.funder == tasks::Funder::Recurring {
            project: 1, task: master, period: 2,
        }
    ));
}

#[test]
fn a_stalled_proposal_passes_up() {
    let (mut driver, root) = batch_fixture_with(3, 4);
    let mut holder = report_delegate(b"holder", Box::new([]));
    holder.authority.budget.spend = 50;
    holder.authority.delegation.kinds = Box::new([tasks::AuthorityExecutor::Charter(1)]);
    holder.authority.delegation.tasks = 2;
    holder.authority.delegation.depth = 3;
    let middle = call_batch(&mut driver, &root, 68, Box::new([holder]))[0];
    let middle_run = assigned_task(&driver, middle);
    let leaf = call_batch(&mut driver, &middle_run, 69, Box::new([report_delegate(b"leaf", Box::new([]))]))[0];
    let leaf_run = assigned_task(&driver, leaf);
    tool_call(
        &mut driver,
        &leaf_run,
        70,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([report_delegate(b"needed", Box::new([]))])),
            reason: b"need helper".as_slice().into(),
            as_holder: false,
        },
    );
    let first = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(leaf))).expect("leaf live");
    assert!(
        matches!(first, Record::Tasks(tasks::Stored::Live(row)) if matches!(&row.proposal, Some(p) if matches!(p.state, tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Task(task), .. } if task == middle))),
        "{first:?} delivered {:?}",
        driver.delivered.last()
    );
    driver.env.wall = Wall::from_nanos(Duration::from_millis(11).as_nanos());
    driver.env.now = Time::from_nanos(Duration::from_millis(11).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(leaf))).expect("leaf live");
    assert!(
        matches!(second, Record::Tasks(tasks::Stored::Live(row)) if matches!(&row.proposal, Some(p) if matches!(p.state, tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Task(task), .. } if task == root.task)))
    );
}

#[test]
fn a_proposal_rejected_tells_the_proposer_why() {
    let (mut driver, parent) = batch_fixture_with(2, 2);
    let child = call_batch(&mut driver, &parent, 71, Box::new([report_delegate(b"child", Box::new([]))]))[0];
    let child_run = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &child_run,
        72,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([report_delegate(b"more", Box::new([]))])),
            reason: b"need more".as_slice().into(),
            as_holder: false,
        },
    );
    let proposal = driver
        .delivered
        .iter()
        .find_map(|item| {
            if let Delivery::CallAnswer {
                call, answer: temper_engine_domain::CallAnswer::Proposed { proposal }, ..
            } = item
            {
                (*call == Token::new(72)).then_some(*proposal)
            } else {
                None
            }
        })
        .expect("proposal pending");
    tool_call(
        &mut driver,
        &parent,
        73,
        engine::Tool::Decide {
            proposer: child,
            proposal,
            decision: engine::ProposalChoice::Reject { reason: b"different plan".as_slice().into() },
        },
    );
    let row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("proposer live");
    assert!(
        matches!(row, Record::Tasks(tasks::Stored::Live(task)) if task.proposal.is_none() && task.inbox.iter().any(|word| word.kind == tasks::MessageKind::ProposalDecision { proposal, accepted: false } && word.words.as_ref() == b"different plan"))
    );
    assert!(driver.delivered.iter().any(|item| matches!(item, Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::ProposalDecided { outcome: tasks::ProposalOutcome::Rejected, .. }, .. } if *call == Token::new(73))));
}

#[test]
fn an_amendment_reaches_a_live_run() {
    let (mut driver, parent) = batch_fixture_with(2, 1);
    let member = report_delegate(b"original", Box::new([]));
    let mut wider = member.authority.clone();
    wider.budget.spend = 20;
    let child = call_batch(&mut driver, &parent, 90, Box::new([member]))[0];
    let running = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &parent,
        91,
        engine::Tool::Amend {
            target: child,
            amendment: tasks::Amendment {
                spec: Some(tasks::Spec {
                    words: b"revised".as_slice().into(),
                    parameters: Box::new([]),
                    inputs: Box::new([]),
                }),
                wake: None,
                dependencies: None,
                authority: Some(wider),
                reason: b"the target changed".as_slice().into(),
            },
        },
    );
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Controlled, .. }
            if *call == Token::new(91))));
    assert!(driver.transactions.iter().any(|writes| {
        writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Call(record))
            if record.answer == temper_engine_domain::CallAnswer::Controlled)
        }) && writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(task)))
            if task.number == child && task.spec.words.as_ref() == b"revised" && task.numbers.budget == 20)
        }) && writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Tasks(tasks::Stored::History(row)))
            if row.task == child && row.change == tasks::Change::Amended)
        })
    }));
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::Inbound { task, attempt, word, .. }
            if *task == child && *attempt == running.attempt
                && matches!(word.kind, tasks::MessageKind::Amendment { .. }))));
    let Some(Record::Tasks(tasks::Stored::Live(parent_row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task)))
    else {
        panic!("parent funder")
    };
    assert_eq!(parent_row.numbers.reserved, 25);
}

#[test]
fn a_narrowing_stops_the_old_run_and_offers_the_amendment_to_the_next() {
    let (mut driver, parent) = batch_fixture_with(2, 1);
    let member = report_delegate(b"child", Box::new([]));
    let mut narrower = member.authority.clone();
    narrower.budget.spend = 5;
    let child = call_batch(&mut driver, &parent, 90, Box::new([member]))[0];
    let first = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &parent,
        91,
        engine::Tool::Amend {
            target: child,
            amendment: tasks::Amendment {
                spec: None,
                wake: None,
                dependencies: None,
                authority: Some(narrower),
                reason: b"smaller scope".as_slice().into(),
            },
        },
    );
    assert!(driver.delivered.iter().any(|item| matches!(item, Delivery::Cancel { task, attempt, .. }
        if *task == child && *attempt == first.attempt)));
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: child,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
    });
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert_eq!(second.task, child);
    assert!(second.attempt > first.attempt);
    assert!(second.inbox.iter().any(
        |word| matches!(word.kind, tasks::MessageKind::Amendment { .. }) && word.words.as_ref() == b"smaller scope"
    ));
    let Some(Record::Tasks(tasks::Stored::Live(task))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child)))
    else {
        panic!("amended child live")
    };
    assert_eq!(task.numbers.budget, 5);
    assert!(!task.narrowing);
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the story selects its two stopped runs")]
fn a_cancel_closes_three_levels_with_runs_live_deepest_first() {
    let (mut driver, parent) = batch_fixture_with(3, 2);
    let mut child_spec = report_delegate(b"child", Box::new([]));
    child_spec.authority.budget.spend = 20;
    child_spec.authority.delegation =
        tasks::Delegation { kinds: Box::new([tasks::AuthorityExecutor::Charter(1)]), tasks: 1, depth: 1 };
    let child = call_batch(&mut driver, &parent, 90, Box::new([child_spec]))[0];
    let child_run = assigned_task(&driver, child);
    let grandchild =
        call_batch(&mut driver, &child_run, 91, Box::new([report_delegate(b"grandchild", Box::new([]))]))[0];
    let grandchild_run = assigned_task(&driver, grandchild);
    tool_call(
        &mut driver,
        &parent,
        92,
        engine::Tool::Cancel { target: child, reason: b"goal withdrawn".as_slice().into() },
    );
    let stopped: Vec<_> = driver
        .delivered
        .iter()
        .filter_map(|item| match item {
            Delivery::Cancel { task, attempt, .. } => Some((*task, *attempt)),
            _ => None,
        })
        .collect();
    assert_eq!(stopped, [(grandchild, grandchild_run.attempt), (child, child_run.attempt)]);
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Controlled, .. }
            if *call == Token::new(92))));
    for run in [&grandchild_run, &child_run] {
        driver.send(engine::Event::Answer {
            saved: None,
            channel: Token::new(7),
            task: run.task,
            attempt: run.attempt,
            cumulative: 0,
            end: tasks::End::Parked,
        });
        driver.settle();
    }
    let endings: Vec<_> = driver
        .transactions
        .iter()
        .flat_map(|writes| writes.iter())
        .filter_map(|write| match write {
            Write::Save(Record::Tasks(tasks::Stored::Ended(task))) => Some((task.number, task.phase.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(endings.len(), 2);
    assert_eq!(endings[0].0, grandchild);
    assert_eq!(endings[1].0, child);
    assert!(endings.iter().all(|(_, phase)| matches!(phase, tasks::Phase::Ended(tasks::Ending::Cancelled { .. }))));
}

#[test]
fn release_tool_resets_a_delegates_exhausted_tries() {
    let (mut driver, parent) = batch_fixture_with(2, 1);
    let child = call_batch(&mut driver, &parent, 90, Box::new([report_delegate(b"child", Box::new([]))]))[0];
    let first = assigned_task(&driver, child);
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: child,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Run),
    });
    driver.settle();
    driver.env.now = Time::from_nanos(Duration::from_secs(2).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(2).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert_eq!(second.task, child);
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: child,
        attempt: second.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Run),
    });
    driver.settle();
    tool_call(&mut driver, &parent, 91, engine::Tool::Release { target: child });
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Controlled, .. }
            if *call == Token::new(91))));
    let Some(Record::Tasks(tasks::Stored::Live(task))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child)))
    else {
        panic!("released child remains live")
    };
    assert_eq!(task.tries, tasks::Tries::NONE);
    assert!(matches!(task.phase, tasks::Phase::Active(_)));
    assert!(driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { assignment, .. }
        if assignment.task == child && assignment.attempt > second.attempt)));
}

#[test]
fn introduced_siblings_can_exchange_named_words_after_the_references_commit() {
    let (mut driver, parent) = batch_fixture();
    let numbers = call_batch(
        &mut driver,
        &parent,
        201,
        Box::new([report_delegate(b"left", Box::new([])), report_delegate(b"right", Box::new([]))]),
    );
    for _ in 0..30 {
        driver.advance(true);
    }
    tool_call(
        &mut driver,
        &parent,
        202,
        engine::Tool::Message {
            target: numbers[1],
            form: engine::MessageForm::Words,
            words: b"before".as_slice().into(),
        },
    );
    assert!(driver.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Sent { .. }, .. }
        if *call == Token::new(202))));
    tool_call(&mut driver, &parent, 203, engine::Tool::Introduce { left: numbers[0], right: numbers[1] });
    assert!(driver.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Introduced, .. }
        if *call == Token::new(203))));
    let Some(Record::Tasks(tasks::Stored::Live(left))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(numbers[0])))
    else {
        panic!("left task live")
    };
    assert_eq!(left.references.as_ref(), [numbers[1]]);
    let Some(Record::Tasks(tasks::Stored::Live(right))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(numbers[1])))
    else {
        panic!("right task live")
    };
    assert_eq!(right.references.as_ref(), [numbers[0]]);
    assert!(
        right.inbox.iter().any(|word| word.from == tasks::Party::Task(parent.task) && word.words.as_ref() == b"before")
    );
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one answer from the closed delivery vocabulary")]
fn task_subscriptions_are_named_by_the_root_and_unsubscribe_removes_them() {
    let (mut driver, parent) = batch_fixture();
    let numbers = call_batch(&mut driver, &parent, 211, Box::new([report_delegate(b"watched", Box::new([]))]));
    tool_call(
        &mut driver,
        &parent,
        212,
        engine::Tool::Subscribe {
            kind: tasks::SubscriptionKind::Task { target: numbers[0], held: true, result: true },
        },
    );
    let subscription = driver
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::CallAnswer {
                call,
                answer: temper_engine_domain::CallAnswer::Subscribed { subscription },
                ..
            } if *call == Token::new(212) => Some(*subscription),
            _ => None,
        })
        .expect("subscription answered after commit");
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task)))
    else {
        panic!("subscriber live")
    };
    assert_eq!(
        row.subscriptions.as_ref(),
        [tasks::Subscription {
            number: subscription,
            kind: tasks::SubscriptionKind::Task { target: numbers[0], held: true, result: true },
        }]
    );
    tool_call(&mut driver, &parent, 213, engine::Tool::Unsubscribe { subscription });
    assert!(driver.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Unsubscribed, .. }
        if *call == Token::new(213))));
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task)))
    else {
        panic!("subscriber live")
    };
    assert!(row.subscriptions.is_empty());
}

fn park_task(driver: &mut Driver, assignment: &engine::Assignment) {
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
    });
    for _ in 0..30 {
        driver.advance(true);
    }
}

fn finish_task(driver: &mut Driver, assignment: &engine::Assignment, result: tasks::TaskResult) {
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 0,
        end: tasks::End::Finished { result, cancel_delegates: false },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one answer from the closed vocabulary")]
fn a_delegate_batch_and_its_named_answer_commit_together() {
    let (mut driver, parent) = delegate_fixture();
    let child_authority = tasks::Authority {
        tools: tasks::Tools(0),
        grants: Box::new([]),
        delegation: tasks::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
        budget: tasks::Budget { spend: 10, deadline: None },
        notes: tasks::Scopes(0),
        note_resources: Box::new([]),
    };
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        call: Token::new(99),
        body: engine::Call {
            completion: 1,
            position: 0,
            tool: engine::Tool::Delegate {
                batch: Box::new([engine::Delegate {
                    executor: tasks::Executor::Agent { charter: 1 },
                    spec: tasks::Spec {
                        words: b"investigate".as_slice().into(),
                        parameters: Box::new([]),
                        inputs: Box::new([]),
                    },
                    contract: tasks::Contract::Report { words: 128 },
                    authority: child_authority,
                    symbolic_grants: Box::new([]),
                    dependencies: Box::new([]),
                    wake: tasks::WakePolicy::DEFAULT,
                }]),
            },
        },
    });
    assert!(
        !driver
            .delivered
            .iter()
            .any(|item| matches!(item, Delivery::CallAnswer { call, .. } if *call == Token::new(99)))
    );
    for _ in 0..20 {
        driver.advance(true);
    }
    let child = driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Delegated(numbers), .. }
                if *call == Token::new(99) =>
            {
                Some(numbers[0])
            }
            _ => None,
        })
        .expect("delegation answer after commit");
    assert!(driver.transactions.iter().any(|writes| {
        writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Call(record))
            if record.answer == temper_engine_domain::CallAnswer::Delegated(Box::new([child])))
        }) && writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(task)))
                if task.number == child && task.requester == tasks::Party::Task(parent.task))
        })
    }));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects one answer and assignment")]
fn a_delegate_result_enters_its_requesters_inbox_with_the_end() {
    let (mut driver, parent) = delegate_fixture();
    let child_authority = tasks::Authority {
        tools: tasks::Tools(0),
        grants: Box::new([]),
        delegation: tasks::Delegation { kinds: Box::new([]), tasks: 0, depth: 0 },
        budget: tasks::Budget { spend: 10, deadline: None },
        notes: tasks::Scopes(0),
        note_resources: Box::new([]),
    };
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        call: Token::new(100),
        body: engine::Call {
            completion: 1,
            position: 0,
            tool: engine::Tool::Delegate {
                batch: Box::new([engine::Delegate {
                    executor: tasks::Executor::Agent { charter: 1 },
                    spec: tasks::Spec {
                        words: b"spike".as_slice().into(),
                        parameters: Box::new([]),
                        inputs: Box::new([]),
                    },
                    contract: tasks::Contract::Report { words: 128 },
                    authority: child_authority,
                    symbolic_grants: Box::new([]),
                    dependencies: Box::new([]),
                    wake: tasks::WakePolicy::DEFAULT,
                }]),
            },
        },
    });
    for _ in 0..20 {
        driver.advance(true);
    }
    let child = driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::Delegated(numbers), .. }
                if *call == Token::new(100) =>
            {
                Some(numbers[0])
            }
            _ => None,
        })
        .expect("one child was made");
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    let child_assignment = driver
        .delivered
        .iter()
        .find_map(|item| match item {
            Delivery::Assigned { assignment, .. } if assignment.task == child => Some(assignment.clone()),
            _ => None,
        })
        .expect("child starts after parent parks");
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: child,
        attempt: child_assignment.attempt,
        cumulative: 0,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"spike complete".as_slice().into() },
            cancel_delegates: false,
        },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    let parent_row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task))).expect("requester stays live");
    let Record::Tasks(tasks::Stored::Live(parent_row)) = parent_row else { panic!("live requester row") };
    assert!(parent_row.inbox.iter().any(|message| message.from == tasks::Party::Task(child)
        && message.kind == tasks::MessageKind::Result(tasks::ResultKind::Report)
        && message.words.as_ref() == b"spike complete"));
    assert!(driver.transactions.iter().any(|writes| {
        writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Ended(task))) if task.number == child))
            && writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(task)))
                if task.number == parent.task && task.inbox.iter().any(|message| message.from == tasks::Party::Task(child))))
    }));
}

#[test]
fn a_plan_of_spikes_a_choice_and_changes_runs_in_dependency_order() {
    let (mut driver, parent) = batch_fixture();
    let spike = report_delegate(b"spike", Box::new([]));
    let mut choice = report_delegate(b"choose", Box::new([engine::Dependency::Batch(0)]));
    choice.contract = tasks::Contract::Verdict { choices: Box::new([tasks::Verdict { code: 7, words: 128 }]) };
    let mut change = report_delegate(b"change", Box::new([engine::Dependency::Batch(1)]));
    change.contract = tasks::Contract::Change { connector: 1, kind: 2, words: 128 };
    let numbers = call_batch(&mut driver, &parent, 110, Box::new([spike, choice, change]));
    assert_eq!(numbers.len(), 3);
    assert!(!driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { assignment, .. }
        if assignment.task == numbers[1] || assignment.task == numbers[2])));
    park_task(&mut driver, &parent);
    let first = assigned_task(&driver, numbers[0]);
    assert!(!driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { assignment, .. }
        if assignment.task == numbers[1] || assignment.task == numbers[2])));
    finish_task(&mut driver, &first, tasks::TaskResult::Report { words: b"spike says yes".as_slice().into() });
    let second = assigned_task(&driver, numbers[1]);
    assert!(second.sections.iter().any(|section| section.kind == engine::BriefKind::Core(brief::Core::Results)
        && matches!(&section.body, engine::BriefBody::Text(text) if text.windows(b"spike says yes".len()).any(|part| part == b"spike says yes"))));
    assert!(!driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { assignment, .. }
        if assignment.task == numbers[2])));
    finish_task(&mut driver, &second, tasks::TaskResult::Verdict { code: 7, words: b"choose path".as_slice().into() });
    let third = assigned_task(&driver, numbers[2]);
    assert!(third.sections.iter().any(|section| section.kind == engine::BriefKind::Core(brief::Core::Results)
        && matches!(&section.body, engine::BriefBody::Text(text) if text.windows(b"verdict 7".len()).any(|part| part == b"verdict 7"))));
    finish_task(
        &mut driver,
        &third,
        tasks::TaskResult::Change { connector: 1, kind: 2, resource: 17, words: b"changed".as_slice().into() },
    );
    let parent_row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(parent.task))).expect("parent remains live");
    let Record::Tasks(tasks::Stored::Live(parent_row)) = parent_row else { panic!("parent row") };
    assert_eq!(parent_row.inbox.len(), 3, "one durable result per delegate");
}

#[test]
fn a_batch_beyond_authority_is_refused_whole() {
    let (mut driver, parent) = batch_fixture();
    let allowed = report_delegate(b"small", Box::new([]));
    let mut excessive = report_delegate(b"large", Box::new([]));
    excessive.authority.budget.spend = 200;
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        call: Token::new(111),
        body: engine::Call {
            completion: 1,
            position: 111,
            tool: engine::Tool::Delegate { batch: Box::new([allowed, excessive]) },
        },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::DelegationDenied { findings, .. }, .. }
            if *call == Token::new(111) && !findings.is_empty())));
    assert_eq!(driver.store.header().tasks, parent.task, "no member received an ID");
    assert!(!driver.store.rows.values().any(|row| matches!(row,
        Record::Tasks(tasks::Stored::Live(task)) if task.requester == tasks::Party::Task(parent.task))));
}

#[test]
fn a_negative_verdict_starts_dependents_and_a_failure_holds_them() {
    let (mut driver, parent) = batch_fixture();
    let mut verdict = report_delegate(b"check", Box::new([]));
    verdict.contract = tasks::Contract::Verdict { choices: Box::new([tasks::Verdict { code: 0, words: 128 }]) };
    let next = report_delegate(b"respond", Box::new([engine::Dependency::Batch(0)]));
    let held = report_delegate(b"finish", Box::new([engine::Dependency::Batch(1)]));
    let numbers = call_batch(&mut driver, &parent, 112, Box::new([verdict, next, held]));
    park_task(&mut driver, &parent);
    let first = assigned_task(&driver, numbers[0]);
    finish_task(&mut driver, &first, tasks::TaskResult::Verdict { code: 0, words: b"no".as_slice().into() });
    let second = assigned_task(&driver, numbers[1]);
    assert!(second.sections.iter().any(|section| section.kind == engine::BriefKind::Core(brief::Core::Results)
        && matches!(&section.body, engine::BriefBody::Text(text) if text.windows(b"verdict 0".len()).any(|part| part == b"verdict 0"))));
    finish_task(&mut driver, &second, tasks::TaskResult::Failure { reason: b"blocked".as_slice().into() });
    let row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(numbers[2]))).expect("dependent held live");
    let Record::Tasks(tasks::Stored::Live(task)) = row else { panic!("dependent row") };
    assert!(matches!(task.phase, tasks::Phase::Held { why: tasks::Hold::Dependency(id), .. } if id == numbers[1]));
    assert!(!driver.delivered.iter().any(|item| matches!(item, Delivery::Assigned { assignment, .. }
        if assignment.task == numbers[2])));
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the script selects the requester's next assignment")]
fn an_ended_delegate_can_be_named_as_a_later_tasks_input() {
    let (mut driver, parent) = batch_fixture();
    let first = call_batch(&mut driver, &parent, 113, Box::new([report_delegate(b"first", Box::new([]))]));
    park_task(&mut driver, &parent);
    let first_assignment = assigned_task(&driver, first[0]);
    finish_task(&mut driver, &first_assignment, tasks::TaskResult::Report { words: b"found it".as_slice().into() });
    let parent_again = driver
        .delivered
        .iter()
        .rev()
        .find_map(|item| match item {
            Delivery::Assigned { assignment, .. }
                if assignment.task == parent.task && assignment.attempt > parent.attempt =>
            {
                Some(assignment.clone())
            }
            _ => None,
        })
        .expect("result wakes requester");
    let mut second = report_delegate(b"use result", Box::new([]));
    second.spec.inputs = Box::new([first[0]]);
    let second = call_batch(&mut driver, &parent_again, 114, Box::new([second]));
    park_task(&mut driver, &parent_again);
    let assigned = assigned_task(&driver, second[0]);
    assert!(assigned.sections.iter().any(|section| section.kind == engine::BriefKind::Core(brief::Core::Results)
        && matches!(&section.body, engine::BriefBody::Text(text) if text.windows(b"found it".len()).any(|part| part == b"found it"))));
}

#[test]
fn a_missing_historical_input_refuses_the_whole_batch() {
    let (mut driver, parent) = batch_fixture();
    let mut member = report_delegate(b"use missing", Box::new([]));
    member.spec.inputs = Box::new([999]);
    driver.send(engine::Event::Call {
        channel: Token::new(7),
        task: parent.task,
        attempt: parent.attempt,
        call: Token::new(115),
        body: engine::Call { completion: 1, position: 115, tool: engine::Tool::Delegate { batch: Box::new([member]) } },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::DelegationRefused(problem), .. }
            if *call == Token::new(115) && problem.why == tasks::Refusal::Inputs)));
    assert_eq!(driver.store.header().tasks, parent.task);
}

#[test]
fn a_call_asked_twice_across_a_restart_is_decided_once() {
    let (mut driver, assignment) = running_fixture();
    unavailable_call(&mut driver, &assignment, 71);
    assert!(
        !driver
            .delivered
            .iter()
            .any(|delivery| matches!(delivery, Delivery::CallAnswer { call, .. } if *call == Token::new(71))),
        "the answer waits for its named decision to commit"
    );
    driver.settle();
    assert!(matches!(
        driver.store.rows.get(&Key::Call(temper_engine_domain::CallKey {
            task: assignment.task,
            attempt: assignment.attempt,
            completion: 2,
            position: 0,
        })),
        Some(Record::Call(record)) if record.answer == temper_engine_domain::CallAnswer::Unavailable
    ));
    assert_eq!(driver.store.header().calls, 1);
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::CallAnswer { channel, task, attempt, call, answer }
            if *channel == Token::new(7)
                && *task == assignment.task
                && *attempt == assignment.attempt
                && *call == Token::new(71)
                && *answer == temper_engine_domain::CallAnswer::Unavailable
    )));

    let mut restarted = Driver::new(driver.store);
    restarted.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([fleet::Hosted {
                run: Token::new(assignment.task),
                attempt: Token::new(assignment.attempt),
                phase: fleet::Phase::Active,
            }]),
        },
    });
    restarted.settle();
    unavailable_call(&mut restarted, &assignment, 72);
    restarted.settle();
    assert_eq!(restarted.store.header().calls, 1, "the repeated name did not make another decision");
    assert!(restarted.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::CallAnswer { call, answer, .. }
            if *call == Token::new(72) && *answer == temper_engine_domain::CallAnswer::Unavailable
    )));
}

#[test]
fn a_lost_attempt_is_told_of_the_calls_committed_after_its_last_turn() {
    let (mut driver, first) = running_fixture();
    unavailable_call(&mut driver, &first, 81);
    driver.settle();
    driver.send(engine::Event::Lost { channel: Token::new(7) });
    driver.env.now = Time::from_nanos(Duration::from_secs(6).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(6).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    driver.env.now = Time::from_nanos(Duration::from_secs(7).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(7).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.send(engine::Event::Hello {
        channel: Token::new(8),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert!(second.attempt > first.attempt);
    assert_eq!(
        second.answered.as_ref(),
        [temper_engine_domain::CallRecord {
            key: temper_engine_domain::CallKey { task: first.task, attempt: first.attempt, completion: 2, position: 0 },
            answer: temper_engine_domain::CallAnswer::Unavailable,
        }]
    );
}

#[test]
fn calls_answer_busy_under_backpressure_and_succeed_on_retry() {
    let (mut driver, assignment) = running_fixture();
    driver.sign_in();
    driver.sign_in();
    driver.sign_in();
    assert_eq!(driver.store.pending.len(), 3, "three issued commits fill the journal");
    unavailable_call(&mut driver, &assignment, 82);
    assert_eq!(driver.call_busy.as_slice(), [Token::new(82)]);
    assert_eq!(driver.store.header().calls, 0);
    assert!(!driver.store.rows.keys().any(|key| matches!(key, Key::Call(_))));
    driver.settle();
    unavailable_call(&mut driver, &assignment, 82);
    assert!(
        !driver
            .delivered
            .iter()
            .any(|delivery| matches!(delivery, Delivery::CallAnswer { call, .. } if *call == Token::new(82))),
        "retry still waits for durability"
    );
    driver.settle();
    assert_eq!(driver.store.header().calls, 1);
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::CallAnswer { call, answer, .. }
            if *call == Token::new(82) && *answer == temper_engine_domain::CallAnswer::Unavailable
    )));
}

#[test]
fn invalid_current_proof_stops_before_any_restored_closing_effect_or_result() {
    let (driver, assignment) = running_fixture();
    for corruption in 0..5 {
        let mut store = Store::new();
        store.rows = driver.store.rows.clone();
        let key = temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task));
        let Some(Record::Tasks(tasks::Stored::Live(task))) = store.rows.get_mut(&key) else {
            panic!("live task");
        };
        task.last_answer = Some(assignment.attempt);
        task.phase = tasks::Phase::Closing(tasks::Closing {
            stage: tasks::Stage::Effects,
            ending: tasks::Ending::Done(tasks::TaskResult::Report { words: REPORT.into() }),
        });
        let proof_key = temper_engine_domain::Key::RunProof { task: assignment.task };
        let Some(Record::RunProof(proof)) = store.rows.get_mut(&proof_key) else {
            panic!("proof");
        };
        proof.terminal = Some(temper_engine_domain::TerminalRecord {
            task: assignment.task,
            attempt: assignment.attempt,
            cumulative: 3,
            end: tasks::End::Finished {
                result: tasks::TaskResult::Report { words: REPORT.into() },
                cancel_delegates: false,
            },
        });
        match corruption {
            0 => {
                store.rows.remove(&proof_key);
            }
            1 => {
                let Some(Record::RunProof(proof)) = store.rows.get_mut(&proof_key) else {
                    panic!("proof");
                };
                proof.turn.as_mut().expect("kept turn").cumulative = 999;
            }
            2 => {
                let Some(Record::RunProof(proof)) = store.rows.get_mut(&proof_key) else {
                    panic!("proof");
                };
                proof.terminal = Some(temper_engine_domain::TerminalRecord {
                    task: assignment.task,
                    attempt: assignment.attempt,
                    cumulative: 999,
                    end: tasks::End::Parked,
                });
            }
            3 => {
                let header = store.header();
                let Some(Record::Tasks(tasks::Stored::Live(task))) = store.rows.get_mut(&key) else {
                    panic!("task");
                };
                task.attempt = header.runs + 1;
                task.last_answer = Some(task.attempt);
                let Some(Record::RunProof(proof)) = store.rows.get_mut(&proof_key) else {
                    panic!("proof");
                };
                proof.attempt = header.runs + 1;
            }
            4 => {
                let Some(Record::RunProof(proof)) = store.rows.get_mut(&proof_key) else {
                    panic!("proof");
                };
                proof.terminal = None;
            }
            _ => unreachable!(),
        }
        let header = store.header();
        let before = store.rows.clone();
        let mut restarted = Driver::new(store);
        for _ in 0..100 {
            restarted.advance(true);
        }
        assert!(restarted.stopped, "corruption {corruption}");
        assert_eq!(restarted.store.rows, before, "no child restoration consequences committed");
        assert_eq!(restarted.store.header(), header);
        assert!(restarted.delivered.is_empty(), "no cancellation/result before root proof validation");
    }
}

#[test]
fn root_restore_refuses_malformed_task_shapes() {
    let (driver, assignment) = running_fixture();
    for malformed in 0..3 {
        let mut store = Store::new();
        store.rows = driver.store.rows.clone();
        let Some(Record::Tasks(tasks::Stored::Live(task))) =
            store.rows.get_mut(&temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task)))
        else {
            panic!("task");
        };
        match malformed {
            0 => task.requester = tasks::Party::Task(assignment.task),
            1 => task.executor = tasks::Executor::Agent { charter: 99 },
            2 => task.contract = tasks::Contract::Verdict { choices: Box::new([]) },
            _ => unreachable!(),
        }
        let before = store.rows.clone();
        let mut restarted = Driver::new(store);
        for _ in 0..100 {
            restarted.advance(true);
        }
        assert!(restarted.stopped);
        assert_eq!(restarted.store.rows, before);
        assert!(restarted.delivered.is_empty());
    }
}

#[test]
fn bounded_invalid_typed_terminal_preserves_original_root_evidence_and_charges_once() {
    let (mut driver, assignment) = running_fixture();
    let end = tasks::End::Finished {
        result: tasks::TaskResult::Verdict { code: 99, words: b"invalid contract".as_slice().into() },
        cancel_delegates: false,
    };
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 8,
        end: end.clone(),
    });
    driver.settle();
    let key = temper_engine_domain::Key::Terminal { task: assignment.task, attempt: assignment.attempt };
    assert_eq!(
        driver.store.rows.get(&key),
        Some(&Record::Terminal(temper_engine_domain::TerminalRecord {
            task: assignment.task,
            attempt: assignment.attempt,
            cumulative: 8,
            end: end.clone()
        }))
    );
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("retryable invalid task");
    };
    assert_eq!(task.run_spent, 8);
    assert_eq!(task.numbers.spent, 8);
    assert_eq!(task.tries.invalid, 1);
    let rows = driver.store.rows.clone();
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 8,
        end,
    });
    driver.settle();
    assert_eq!(driver.store.rows, rows, "fenced duplicate never reaches child priced admission");
}

fn escalation_archive_driver() -> (Driver, temper_engine_domain::EscalationDecisionRecord) {
    use temper_engine_domain_world::{escalation, escalation_referee::Story};
    let mut world = escalation::World::new(escalation::Settings::calm(9200, Story::Release));
    world.run();
    let archive = world
        .store
        .rows
        .values()
        .find_map(|row| if let Record::EscalationDecision(archive) = row { Some(archive.clone()) } else { None })
        .expect("actual completed escalation archive");
    let configured = escalation::limits();
    let mut driver = Driver::configured(world.store, config(9200), &configured);
    driver.settle();
    (driver, archive)
}

#[test]
fn failed_named_escalation_history_read_closes_once_and_same_key_retries() {
    let (mut driver, archive) = escalation_archive_driver();
    let rows = driver.store.rows.clone();
    let session = driver.store.header().sign_ins;
    driver.delivered.clear();
    driver.fail_archive_once = true;
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(900)),
        sign_in: session,
        key: [9; 16],
        ask: people::Ask::DecideEscalation {
            project: archive.project,
            task: archive.task,
            revision: archive.revision,
            decision: people::EscalationDecision::Release,
        },
    });
    driver.settle();
    assert!(!driver.stopped);
    assert!(driver.root.quiescent());
    assert_eq!(driver.store.rows, rows, "failed IO saves no keyed answer or task/financial change");
    assert_eq!(driver.delivered.len(), 1);
    assert!(matches!(
        &driver.delivered[0],
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Busy)), .. }
    ));
    driver.delivered.clear();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(901)),
        sign_in: session,
        key: [9; 16],
        ask: people::Ask::DecideEscalation {
            project: archive.project,
            task: archive.task,
            revision: archive.revision,
            decision: people::EscalationDecision::Release,
        },
    });
    driver.settle();
    assert_eq!(driver.delivered.len(), 1);
    assert!(
        matches!(&driver.delivered[0], Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::EscalationDecided { by, .. }), .. } if *by == archive.by)
    );
    assert!(driver.root.quiescent(), "shared read and people's pending slots retired");
}

fn held_waiting_store() -> Store {
    use temper_engine_domain_world::{escalation, escalation_referee::Story};
    let mut world = escalation::World::new(escalation::Settings::calm(9202, Story::Reject));
    for _ in 0..300 {
        world.iterate();
        if world.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(task)) if matches!(task.escalation, tasks::Escalation::Waiting { .. }))) {
            return Store { applied: world.store.applied, rows: world.store.rows.clone(), pending: VecDeque::new() };
        }
    }
    panic!("actual failure never committed Waiting");
}

fn held_driver(store: Store) -> Driver {
    let configured = temper_engine_domain_world::escalation::limits();
    Driver::configured(store, config(9202), &configured)
}

#[test]
fn release_rejudges_a_still_expired_deadline_and_escalates_the_new_hold() {
    let mut store = held_waiting_store();
    let mut task_number = 0;
    let mut requester = 0;
    for row in store.rows.values_mut() {
        if let Record::Tasks(tasks::Stored::Live(task)) = row {
            task_number = task.number;
            let tasks::Party::Person(person) = task.requester else { unreachable!() };
            requester = person;
            task.phase = tasks::Phase::Held { was: tasks::Was::Active(tasks::Active::Due), why: tasks::Hold::Deadline };
            task.authority.budget.deadline = Some(Wall::EPOCH);
        }
    }
    let sign_in = store
        .rows
        .values()
        .find_map(|row| {
            if let Record::People(people::Stored::SignIn { number, person, .. }) = row
                && *person == requester
            {
                Some(*number)
            } else {
                None
            }
        })
        .expect("requester session");
    let mut driver = held_driver(store);
    driver.settle();
    driver.env.wall = Wall::from_nanos(1);
    driver.delivered.clear();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(990)),
        sign_in,
        key: [90; 16],
        ask: people::Ask::DecideEscalation {
            project: 1,
            task: task_number,
            revision: 1,
            decision: people::EscalationDecision::Release,
        },
    });
    driver.settle();
    assert!(
        driver.delivered.iter().any(|delivery| matches!(
            delivery,
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::EscalationDecided {
                    choice: people::EscalationChoice::Released,
                    ..
                }),
                ..
            }
        )),
        "{:?}",
        driver.delivered
    );
    assert!(driver.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(task))
        if task.number == task_number && matches!(task.phase, tasks::Phase::Held { why: tasks::Hold::Deadline, .. })
            && matches!(task.escalation, tasks::Escalation::Waiting { revision: 2, .. }))));
}

#[test]
fn restored_waiting_rechecks_snapshot_membership_and_same_holder_changes_nothing() {
    let store = held_waiting_store();
    let rows = store.rows.clone();
    let mut same = held_driver(store);
    same.settle();
    assert!(!same.stopped);
    assert!(same.root.quiescent());
    assert_eq!(same.store.rows, rows, "same holder preserves revision without another commit");
    let mut store = held_waiting_store();
    let requester = store
        .rows
        .values()
        .find_map(|row| {
            if let Record::Tasks(tasks::Stored::Live(task)) = row
                && let tasks::Party::Person(person) = task.requester
            {
                return Some(person);
            }
            None
        })
        .expect("person chat");
    for row in store.rows.values_mut() {
        if let Record::People(people::Stored::Roles { holdings, .. }) = row {
            *holdings = holdings
                .iter()
                .copied()
                .filter(|holding| holding.person != requester)
                .collect::<Vec<_>>()
                .into_boxed_slice();
        }
    }
    let mut changed = held_driver(store);
    changed.settle();
    assert!(!changed.stopped);
    assert!(changed.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(task)) if matches!(task.escalation, tasks::Escalation::Waiting { revision: 2, holder: tasks::EscalationHolder::Role { project: 1, role: 0 }, .. }))));
    assert!(
        !changed
            .delivered
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Assigned { .. } | Delivery::Result { .. }))
    );
}

#[test]
fn exhausted_revision_with_changed_restored_holder_stops_without_committing() {
    let mut store = held_waiting_store();
    for row in store.rows.values_mut() {
        if let Record::Tasks(tasks::Stored::Live(task)) = row {
            task.escalation = tasks::Escalation::Waiting {
                revision: u64::MAX,
                holder: tasks::EscalationHolder::Person(1),
                entry: 1,
                since: Wall::EPOCH,
            };
        }
        if let Record::People(people::Stored::Roles { holdings, .. }) = row {
            *holdings =
                holdings.iter().copied().filter(|holding| holding.person != 1).collect::<Vec<_>>().into_boxed_slice();
        }
    }
    let rows = store.rows.clone();
    let mut driver = held_driver(store);
    for _ in 0..150 {
        driver.advance(true);
        if driver.stopped {
            break;
        }
    }
    assert!(driver.stopped);
    assert_eq!(driver.store.rows, rows);
    assert!(driver.store.pending.is_empty());
    assert!(driver.delivered.is_empty(), "invalid recheck releases no child consequence");
}

#[test]
fn rejected_restore_stays_rejected_and_current_read_checks_privacy_and_both_expiry_clocks() {
    use temper_engine_domain_world::{escalation, escalation_referee::Story};
    let mut world = escalation::World::new(escalation::Settings::calm(9203, Story::Reject));
    world.run();
    let task = world
        .store
        .rows
        .values()
        .find_map(|row| if let Record::Tasks(tasks::Stored::Live(task)) = row { Some(task.number) } else { None })
        .expect("rejected held task");
    let before = world.store.rows.clone();
    let mut configured = escalation::limits();
    configured.people.people = 3;
    configured.people.sign_ins = 3;
    let mut driver = Driver::configured(world.store, config(9203), &configured);
    driver.settle();
    assert_eq!(driver.store.rows, before, "rejection never reopens or reroutes at startup");
    let owner_session = 1;
    driver.delivered.clear();
    driver.send(engine::Event::ReadEscalation {
        reply_to: ReplyTo::new(Token::new(910)),
        sign_in: owner_session,
        task,
    });
    driver.settle();
    assert!(
        matches!(&driver.delivered[..], [Delivery::EscalationReply { context, .. }] if matches!(context.escalation, tasks::Escalation::Rejected { .. }))
    );
    driver.delivered.clear();
    driver.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(911)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: 9_u64.to_be_bytes().into() },
            login: b"outsider".as_slice().into(),
            name: b"Outsider".as_slice().into(),
        },
    });
    driver.settle();
    let outsider = driver.session();
    driver.delivered.clear();
    driver.send(engine::Event::ReadEscalation { reply_to: ReplyTo::new(Token::new(912)), sign_in: outsider, task });
    driver.settle();
    assert!(matches!(
        &driver.delivered[..],
        [Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::Standing), .. }]
    ));
    driver.delivered.clear();
    driver.env.now = Time::from_nanos(Duration::from_secs(61).as_nanos());
    driver.env.wall = Wall::EPOCH;
    driver.send(engine::Event::ReadEscalation {
        reply_to: ReplyTo::new(Token::new(913)),
        sign_in: owner_session,
        task,
    });
    assert!(
        matches!(
            &driver.delivered[..],
            [Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::SignIn), .. }]
        ),
        "monotonic expiry refuses even with backward wall before fire"
    );
}

#[test]
#[expect(clippy::too_many_lines, reason = "one history replay story retains its full script")]
fn coalesced_history_waiters_survive_simultaneous_io_completion_under_full_journal() {
    use temper_engine_domain_world::{escalation, escalation_referee::Story};
    let mut world = escalation::World::new(escalation::Settings::calm(9204, Story::Release));
    world.run();
    let archive = world
        .store
        .rows
        .values()
        .find_map(|row| if let Record::EscalationDecision(archive) = row { Some(archive.clone()) } else { None })
        .expect("actual immutable decision");
    let mut configured = escalation::limits();
    configured.people.waiters = 8;
    configured.journal.writes = tasks::max_out(&configured.tasks) * 8
        + people::max_out(&configured.people) * 4
        + configured.people.pending * 2
        + fleet::max_out(&configured.fleet) * 4
        + temper_engine_domain_forge::max_out(&configured.forge)
        + configured.tasks.tasks
        + 6;
    configured.journal.deliveries =
        configured.tasks.tasks * 4 + 8 + configured.people.pending * configured.people.waiters;
    configured.journal.held = configured.journal.deliveries * 3 + 16;
    let mut insufficient = configured;
    insufficient.journal.deliveries -= 1;
    assert_eq!(engine::worst_case(&insufficient), None, "startup prices every pending flight's duplicate waiters");
    let session = world.store.header().sign_ins;
    let mut driver = Driver::configured(world.store, config(9204), &configured);
    driver.settle();
    for key in 9_u8..11 {
        for waiter in 0_u64..8 {
            driver.send(engine::Event::Ask {
                reply_to: ReplyTo::new(Token::new(950 + u64::from(key - 9) * 8 + waiter)),
                sign_in: session,
                key: [key; 16],
                ask: people::Ask::DecideEscalation {
                    project: archive.project,
                    task: archive.task,
                    revision: archive.revision,
                    decision: people::EscalationDecision::Release,
                },
            });
        }
    }
    for _ in 0..20 {
        if driver.events.len() == 2 {
            break;
        }
        engine::release(&mut driver.root, &driver.env, &mut driver.out);
        driver.collect();
        driver.root.reclaim();
    }
    assert_eq!(driver.events.len(), 2, "one real named IO for each coalesced flight");
    driver.sign_in();
    driver.sign_in();
    driver.sign_in();
    assert_eq!(driver.store.pending.len(), 3, "all store commit slots full before IO completes");
    driver.advance(false);
    driver.advance(false);
    assert!(driver.events.is_empty());
    assert!(!driver.root.quiescent());
    assert!(
        !driver.delivered.iter().any(|delivery| matches!(
            delivery,
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::EscalationDecided { .. }), .. }
        )),
        "no waiter reply escapes pressure before its keyed outcome can commit"
    );
    driver.settle();
    let replies: Vec<_> = std::mem::take(&mut driver.delivered)
        .into_iter()
        .filter_map(|delivery| match delivery {
            Delivery::WebReply {
                to,
                reply: people::Reply::Outcome(people::Outcome::EscalationDecided { by, .. }),
                ..
            } => Some((to.into_token().raw(), by)),
            Delivery::WebReply { .. }
            | Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::View(_)
            | Delivery::Fleet(_)
            | Delivery::Assigned { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Relay { .. }
            | Delivery::Inbound { .. }
            | Delivery::Load { .. }
            | Delivery::Result { .. }
            | Delivery::ResultReply { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::InboxPage { .. }
            | Delivery::InboxView { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. } => None,
        })
        .collect();
    assert_eq!(replies.len(), 16);
    let mut rights = std::collections::BTreeSet::new();
    for (to, by) in replies {
        assert!((950..966).contains(&to));
        assert!(rights.insert(to), "each admitted waiter ends once");
        assert_eq!(by, archive.by);
    }
    assert!(driver.root.quiescent(), "all query/pending/store obligations retired");
}

#[test]
fn restored_loss_spends_a_try_with_or_without_a_durable_turn() {
    for kept_turn in [false, true] {
        let mut original = Driver::new(Store::new());
        original.send(engine::Event::Hello {
            channel: Token::new(7),
            hello: fleet::Hello {
                stop_bound: Duration::from_secs(1),
                slots: 1,
                workstreams: Box::new([]),
                hosting: Box::new([]),
            },
        });
        original.settle();
        original.sign_in();
        original.settle();
        original.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(501)),
            sign_in: original.session(),
            key: [18; 16],
            ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
        });
        original.settle();
        let assignment = original
            .delivered
            .iter()
            .find_map(|delivery| {
                if let Delivery::Assigned { assignment, .. } = delivery { Some(assignment.clone()) } else { None }
            })
            .expect("actual worker assignment before loss");
        if kept_turn {
            turn(&mut original, &assignment, 1, 3);
            original.settle();
            assert!(original.delivered.iter().any(|delivery| matches!(delivery, Delivery::AcknowledgeTurn { task, attempt, turn: 1, .. } if *task == assignment.task && *attempt == assignment.attempt)));
        }
        let ledgers: Vec<_> = original
            .store
            .rows
            .values()
            .filter_map(|row| if let Record::Tasks(tasks::Stored::Ledger(ledger)) = row { Some(*ledger) } else { None })
            .collect();
        let mut restored = Driver::new(original.store);
        for _ in 0..200 {
            restored.advance(true);
            if restored.root.ready() && restored.events.is_empty() && restored.store.pending.is_empty() {
                break;
            }
        }
        assert!(restored.root.ready() && restored.events.is_empty() && restored.store.pending.is_empty());
        // A restored claim waits for fleet grace; it cannot be quiescent yet.
        restored.env.now = Time::from_nanos(6_000_000_000);
        restored.env.wall = Wall::from_nanos(6_000_000_000);
        engine::fire(&mut restored.root, &restored.env);
        engine::release(&mut restored.root, &restored.env, &mut restored.out);
        restored.collect();
        let end = tasks::End::Failed(tasks::Class::Lost);
        let cumulative = if kept_turn { 3 } else { 0 };
        assert_eq!(restored.store.pending.len(), 1, "one canonical loss transaction");
        let writes = &restored.store.pending.front().expect("loss transaction").1;
        let canonical = writes
            .iter()
            .find_map(|write| if let Write::Save(Record::Terminal(terminal)) = write { Some(terminal) } else { None })
            .expect("loss commits actual canonical terminal");
        assert_eq!(
            (canonical.task, canonical.attempt, canonical.cumulative),
            (assignment.task, assignment.attempt, cumulative),
        );
        assert_eq!(canonical.end, end);
        assert!(writes.iter().any(|write| matches!(write, Write::Save(Record::RunProof(proof)) if proof.task == assignment.task && proof.attempt == assignment.attempt && proof.terminal.as_ref() == Some(canonical))), "canonical terminal and proof share one commit");
        assert!(writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(task))) if task.number == assignment.task && task.numbers.spent == cumulative && task.run_spent == cumulative && task.last_answer == Some(assignment.attempt) && task.tries.lost == 1)), "canonical terminal and lost try share one commit");
        restored.settle();
        let after: Vec<_> = restored
            .store
            .rows
            .values()
            .filter_map(|row| if let Record::Tasks(tasks::Stored::Ledger(ledger)) = row { Some(*ledger) } else { None })
            .collect();
        assert_eq!(after, ledgers, "topology loss never charges or posts funding again");
        assert!(
            !restored.delivered.iter().any(|delivery| matches!(delivery, Delivery::Acknowledge { .. })),
            "no worker terminal was offered after restart"
        );
    }
}

fn administration_config(seed: u64) -> engine::Config {
    use jig_core_authority as authority;
    let mut config = config(seed);
    let mut policy = config.authority.policy(1).expect("actual project").clone();
    policy.roles[0].requests = authority::Requests(1 | 4 | 256);
    let mut findings = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut config.authority, authority::Event::Policy { project: 1, policy }, &mut findings);
    assert_eq!(findings.pop(), Some(authority::PolicyFact::Changed { project: 1 }));
    config
}

#[test]
fn an_owner_makes_a_service_once_and_the_service_cannot_start_a_chat() {
    let mut driver = Driver::configured(Store::new(), administration_config(9501), &limits());
    driver.settle();
    driver.sign_in();
    driver.settle();
    let owner_session = driver.session();
    let ask = people::Ask::MakeService { project: 1, name: b"builder".as_slice().into(), role: people::Role::Owner };
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9502)),
        sign_in: owner_session,
        key: [95; 16],
        ask: ask.clone(),
    });
    driver.settle();
    let service = driver
        .delivered
        .iter()
        .find_map(|delivery| {
            if let Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::ServiceMade { person }), ..
            } = delivery
            {
                Some(*person)
            } else {
                None
            }
        })
        .expect("service creation answered after commit");
    assert!(driver.store.rows.values().any(|row| matches!(row,
        Record::People(people::Stored::Person { number, identity })
            if *number == service && identity.key.provider == 1 && identity.key.subject.as_ref() == service.to_be_bytes()
    )));
    let created = driver.store.header().people;
    let mut restarted = Driver::configured(driver.store, administration_config(9501), &limits());
    restarted.settle();
    restarted.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9503)),
        sign_in: owner_session,
        key: [95; 16],
        ask,
    });
    restarted.settle();
    assert_eq!(restarted.store.header().people, created);
    assert!(restarted.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::ServiceMade { person }), .. }
            if *person == service
    )));
    restarted.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(9504)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 1, subject: service.to_be_bytes().into() },
            login: Box::new([]),
            name: b"builder".as_slice().into(),
        },
    });
    restarted.settle();
    let service_session = restarted
        .delivered
        .iter()
        .find_map(|delivery| {
            if let Delivery::WebReply { sign_in: Some(session), reply: people::Reply::SignedIn { person, .. }, .. } =
                delivery
                && *person == service
            {
                Some(*session)
            } else {
                None
            }
        })
        .expect("service sign-in uses its existing party");
    restarted.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9505)),
        sign_in: service_session,
        key: [96; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::new([]) },
    });
    restarted.settle();
    assert!(restarted.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Role)), .. }
    )));
}

#[test]
fn a_policy_change_applies_to_later_decisions_only() {
    let configuration = administration_config(9401);
    let mut driver = Driver::configured(Store::new(), configuration, &limits());
    driver.settle();
    driver.sign_in();
    driver.settle();
    let session = driver.session();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9402)),
        sign_in: session,
        key: [41; 16],
        ask: people::Ask::StartChat { project: 1, words: b"first".as_slice().into() },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    let first = driver
        .store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Live(task)) => Some((task.number, task.authority.budget.spend)),
            Record::ProposalDecision(_)
            | Record::Call(_)
            | Record::EscalationDecision(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::Tasks(_)
            | Record::People(_)
            | Record::Forge { .. } => None,
        })
        .expect("first chat is durable");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9403)),
        sign_in: session,
        key: [42; 16],
        ask: people::Ask::ChangePolicy {
            project: 1,
            change: people::PolicyChange::Role(people::PolicyRole {
                number: 0,
                authority: people::Authority {
                    tools: 0,
                    grants: Box::new([]),
                    delegation: people::Delegation {
                        kinds: Box::new([people::Executor::Charter(1)]),
                        tasks: 2,
                        depth: 1,
                    },
                    spend: 500,
                    deadline: None,
                    notes: 0,
                    note_resources: Box::new([]),
                },
                period_spend: 50,
                requests: 1 | 4 | 256,
                decides: authority::Proposals::ALL.0,
            }),
        },
    });
    for _ in 0..30 {
        driver.advance(true);
    }
    assert!(matches!(
        driver.store.rows.get(&Key::People(people::Key::Policy(1))),
        Some(Record::People(people::Stored::Policy { project: 1, value })) if value.roles[0].period_spend == 50
    ));
    assert!(
        driver.store.rows.values().any(|row| matches!(row,
            Record::Tasks(tasks::Stored::Live(task)) if task.number == first.0 && task.authority.budget.spend == first.1
        )),
        "existing task keeps its grant"
    );
    let store = driver.store;
    let mut restarted = Driver::configured(store, administration_config(9401), &limits());
    for _ in 0..30 {
        restarted.advance(true);
    }
    restarted.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9404)),
        sign_in: session,
        key: [43; 16],
        ask: people::Ask::StartChat { project: 1, words: b"later".as_slice().into() },
    });
    for _ in 0..30 {
        restarted.advance(true);
    }
    assert!(
        restarted.delivered.iter().any(|delivery| matches!(
            delivery,
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Authority)),
                ..
            }
        )),
        "restored policy governs later decisions"
    );
}

#[test]
fn a_policy_change_beyond_the_deployments_rules_is_refused() {
    let mut driver = Driver::configured(Store::new(), administration_config(9411), &limits());
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9412)),
        sign_in: driver.session(),
        key: [44; 16],
        ask: people::Ask::ChangePolicy {
            project: 1,
            change: people::PolicyChange::ProjectSpend { period_spend: 1001 },
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Authority)), .. }
    )));
    assert!(!driver.store.rows.contains_key(&Key::People(people::Key::Policy(1))));
}

#[test]
fn a_policy_change_survives_a_restart() {
    let mut driver = Driver::configured(Store::new(), administration_config(9413), &limits());
    driver.settle();
    driver.sign_in();
    driver.settle();
    let session = driver.session();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9414)),
        sign_in: session,
        key: [45; 16],
        ask: people::Ask::ChangePolicy { project: 1, change: people::PolicyChange::ProjectSpend { period_spend: 800 } },
    });
    driver.settle();
    assert!(matches!(driver.store.rows.get(&Key::People(people::Key::Policy(1))),
        Some(Record::People(people::Stored::Policy { value, .. })) if value.period_spend == 800));
    let mut restarted = Driver::configured(driver.store, administration_config(9413), &limits());
    restarted.settle();
    restarted.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9415)),
        sign_in: session,
        key: [46; 16],
        ask: people::Ask::StartChat { project: 1, words: b"later".as_slice().into() },
    });
    restarted.settle();
    assert!(restarted.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Authority)), .. }
    )));
}

#[test]
fn an_owner_changes_a_pool_once_with_a_key() {
    let mut driver = Driver::configured(Store::new(), administration_config(9411), &limits());
    driver.settle();
    driver.sign_in();
    driver.settle();
    let session = driver.session();
    let person = driver.store.header().people;
    let ask = people::Ask::SetPool { project: 1, person, budget: 300 };
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9412)),
        sign_in: session,
        key: [44; 16],
        ask: ask.clone(),
    });
    driver.settle();
    let pool = tasks::Funder::Pool { project: 1, person, period: 1 };
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ledger(pool))),
        Some(Record::Tasks(tasks::Stored::Ledger(row))) if row.numbers.budget == 300));
    let before = driver.transactions.len();
    driver.send(engine::Event::Ask { reply_to: ReplyTo::new(Token::new(9413)), sign_in: session, key: [44; 16], ask });
    driver.settle();
    assert_eq!(driver.transactions.len(), before, "saved answer does not carve again");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(9414)),
        sign_in: session,
        key: [45; 16],
        ask: people::Ask::SetPool { project: 1, person, budget: 250 },
    });
    driver.settle();
    let period = tasks::Funder::Period { project: 1, period: 1 };
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ledger(pool))),
        Some(Record::Tasks(tasks::Stored::Ledger(row))) if row.numbers.budget == 250));
    assert!(matches!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ledger(period))),
        Some(Record::Tasks(tasks::Stored::Ledger(row))) if row.numbers.reserved == 250));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one real-store route proves multi-task atomicity, pressure retry and whole-cohort overflow rollback"
)]
fn multiple_waiting_recipients_preflight_together_and_full_journal_refuses_without_a_saved_key() {
    use temper_engine_domain_world::roles;
    let prior = roles::World::new(roles::Settings::calm(9310, roles::Base::Requester));
    let sessions = prior.sessions;
    let people = prior.people;
    let mut configured = roles::limits();
    configured.people.sign_ins = 8;
    configured.journal.writes = tasks::max_out(&configured.tasks) * 8
        + people::max_out(&configured.people) * 4
        + configured.people.pending * 2
        + fleet::max_out(&configured.fleet) * 4
        + temper_engine_domain_forge::max_out(&configured.forge)
        + configured.tasks.tasks
        + 6;
    let mut driver = Driver::configured(prior.store, administration_config(9310), &configured);
    driver.settle();
    hello(&mut driver);
    driver.settle();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(1200)),
        sign_in: sessions[0],
        key: [120; 16],
        ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
    });
    driver.settle();
    let assignment = assigned(&driver);
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 3,
        end: tasks::End::Failed(tasks::Class::Run),
    });
    driver.settle();
    let original: Vec<_> = driver
        .store
        .rows
        .values()
        .filter_map(|row| match row {
            Record::Tasks(tasks::Stored::Live(record)) => Some(record.clone()),
            Record::Deployment(_)
            | Record::People(_)
            | Record::Tasks(
                tasks::Stored::Ended(_)
                | tasks::Stored::Ledger(_)
                | tasks::Stored::History(_)
                | tasks::Stored::PersonProposal(_),
            )
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::EscalationDecision(_)
            | Record::Forge { .. }
            | Record::ProposalDecision(_)
            | Record::Call(_) => None,
        })
        .collect();
    assert_eq!(original.len(), 2, "two genuine task admissions and priced failures");
    for record in &original {
        assert!(matches!(
            record.escalation,
            tasks::Escalation::Waiting { revision: 1, holder: tasks::EscalationHolder::Person(person), .. }
                if person == people[0]
        ));
    }
    let before = driver.store.rows.clone();
    let ask = people::Ask::SetRoles {
        project: 1,
        holdings: Box::new([people::Holding { person: people[1], role: people::Role::Owner }]),
    };
    driver.delivered.clear();
    driver.sign_in();
    driver.sign_in();
    driver.sign_in();
    assert_eq!(driver.store.pending.len(), 3, "real issued journal pressure");
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(1201)),
        sign_in: sessions[1],
        key: [121; 16],
        ask: ask.clone(),
    });
    assert!(matches!(
        driver.delivered.last(),
        Some(Delivery::WebReply { reply: people::Reply::Refused(people::Refusal::Busy), .. })
    ));
    driver.settle();
    let answer_key = temper_engine_domain::Key::People(people::Key::Answer(people::RequestKey {
        person: people[1],
        key: [121; 16],
    }));
    assert!(!driver.store.rows.contains_key(&answer_key), "pressure remains retryable");
    driver.delivered.clear();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(1202)),
        sign_in: sessions[1],
        key: [121; 16],
        ask,
    });
    assert!(driver.delivered.is_empty(), "no role success before durability");
    let (_, writes) = driver.store.pending.front().expect("one serialized role decision");
    assert!(writes.iter().any(|write| matches!(write, Write::Save(Record::People(people::Stored::Roles { holdings, .. })) if holdings.as_ref() == [people::Holding { person: people[1], role: people::Role::Owner }])));
    assert!(writes.iter().any(|write| matches!(write, Write::Save(Record::People(people::Stored::Answer { key, outcome: people::Outcome::RolesSet { project: 1 }, .. })) if key.person == people[1] && key.key == [121;16])));
    for record in &original {
        let mut expected = record.clone();
        let tasks::Escalation::Waiting { entry: old_entry, .. } = record.escalation else {
            panic!("source is waiting")
        };
        assert!(
            writes.iter().any(|write| match write {
                Write::Save(Record::Tasks(tasks::Stored::Live(actual))) if actual.number == record.number => {
                    match actual.escalation {
                        tasks::Escalation::Waiting {
                            revision: 2,
                            holder: tasks::EscalationHolder::Role { project: 1, role: 0 },
                            entry,
                            ..
                        } if entry > old_entry => {
                            expected.escalation = actual.escalation.clone();
                            **actual == *expected
                        }
                        tasks::Escalation::Unheld { .. }
                        | tasks::Escalation::Routing { .. }
                        | tasks::Escalation::Waiting { .. }
                        | tasks::Escalation::Rejected { .. } => false,
                    }
                }
                Write::Save(_) | Write::Erase(_) => false,
            }),
            "each exact affected task belongs to the same cohort"
        );
    }
    assert!(
        !writes.iter().any(|write| matches!(
            write,
            Write::Save(
                Record::Tasks(tasks::Stored::Ledger(_))
                    | Record::RunProof(_)
                    | Record::Terminal(_)
                    | Record::Turn(_)
                    | Record::EscalationDecision(_)
                    | Record::Forge { .. }
                    | Record::ProposalDecision(_)
            )
        )),
        "role change never rewrites economic or accepted-work evidence"
    );
    driver.settle();

    let mut overflow_store = Store::new();
    overflow_store.rows = before;
    overflow_store.applied = overflow_store.header().commits;
    let Some(Record::Tasks(tasks::Stored::Live(last))) =
        overflow_store.rows.get_mut(&temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("genuine second held task");
    };
    last.escalation = tasks::Escalation::Waiting {
        revision: u64::MAX,
        holder: tasks::EscalationHolder::Person(people[0]),
        entry: 1,
        since: Wall::EPOCH,
    };
    let mut overflow = Driver::configured(overflow_store, administration_config(9310), &configured);
    overflow.settle();
    let mut expected_header = overflow.store.header();
    expected_header.commits = expected_header.commits.checked_add(1).expect("one saved refusal commit");
    let unchanged = overflow.store.rows.clone();
    overflow.delivered.clear();
    overflow.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(1203)),
        sign_in: sessions[1],
        key: [122; 16],
        ask: people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([people::Holding { person: people[1], role: people::Role::Owner }]),
        },
    });
    overflow.settle();
    assert!(matches!(
        overflow.delivered.last(),
        Some(Delivery::WebReply {
            reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Limit)),
            ..
        })
    ));
    assert_eq!(overflow.store.header(), expected_header, "only the saved refusal advances the commit count");
    assert_eq!(overflow.store.rows.len(), unchanged.len() + 1, "one new keyed refusal record");
    let refused_key = people::RequestKey { person: people[1], key: [122; 16] };
    assert_eq!(
        overflow.store.rows.get(&temper_engine_domain::Key::People(people::Key::Answer(refused_key))),
        Some(&Record::People(people::Stored::Answer {
            key: refused_key,
            ask: Box::new(people::Ask::SetRoles {
                project: 1,
                holdings: Box::new([people::Holding { person: people[1], role: people::Role::Owner }]),
            }),
            outcome: people::Outcome::Refused(people::Refusal::Limit),
            at: overflow.env.wall,
        })),
        "the only added row is the exact durable keyed refusal"
    );
    for (key, row) in unchanged.into_iter().filter(|(key, _)| *key != temper_engine_domain::Key::Deployment) {
        assert_eq!(
            overflow.store.rows.get(&key),
            Some(&row),
            "one exhausted candidate rolls back every task and membership row"
        );
    }
}

#[test]
#[expect(clippy::too_many_lines, reason = "one complete role-stage admission story")]
fn read_only_role_stages_have_one_not_ready_terminal_and_empty_projects_need_no_financial_ledger() {
    let mut insufficient = temper_engine_domain_world::roles::limits();
    assert!(engine::worst_case(&insufficient).is_some(), "valid role cascade factory");
    insufficient.journal.writes -= 1;
    assert!(
        engine::worst_case(&insufficient).is_none(),
        "exact cross-route room includes the complete role handoff cohort"
    );
    let mut oversized = temper_engine_domain_world::roles::limits();
    oversized.people.holdings = oversized.journal.transcript_bytes;
    assert!(engine::worst_case(&oversized).is_none(), "one roster-bearing durable row must fit before any clone");
    let configured = limits();
    let mut child = tasks::Domain::new(&configured.tasks, 9312, Box::new([1]));
    let environment = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: configured.tasks };
    let mut out = Queue::with_capacity(tasks::max_out(&configured.tasks));
    for request in [1300, 1301] {
        let event = if request == 1300 {
            tasks::Event::InspectEscalations { reply_to: ReplyTo::new(Token::new(request)), project: 1 }
        } else {
            tasks::Event::RecheckEscalations { reply_to: ReplyTo::new(Token::new(request)), project: 1 }
        };
        tasks::step(&mut child, &environment, event, &mut out);
        assert_eq!(out.len(), 1);
        match out.pop().expect("one startup terminal") {
            tasks::Request::EscalationsInspected { reply_to, result } => {
                assert_eq!(reply_to.into_token(), Token::new(request));
                assert_eq!(result, Err(tasks::Refusal::NotReady));
            }
            tasks::Request::EscalationsRechecked { reply_to, result } => {
                assert_eq!(reply_to.into_token(), Token::new(request));
                assert_eq!(result, Err(tasks::Refusal::NotReady));
            }
            tasks::Request::EscalationNeeded { .. }
            | tasks::Request::EscalationInspected { .. }
            | tasks::Request::EscalationDecided { .. }
            | tasks::Request::ProposalDecided { .. }
            | tasks::Request::PersonProposed { .. }
            | tasks::Request::PersonProposalDecided { .. }
            | tasks::Request::ProposalRerouteNeeded { .. }
            | tasks::Request::ProposalStalled { .. }
            | tasks::Request::EscalationStalled { .. }
            | tasks::Request::Made { .. }
            | tasks::Request::Refused { .. }
            | tasks::Request::Done { .. }
            | tasks::Request::Acknowledged { .. }
            | tasks::Request::TurnAcknowledged { .. }
            | tasks::Request::Activate { .. }
            | tasks::Request::Stop { .. }
            | tasks::Request::Adopt { .. }
            | tasks::Request::Close { .. }
            | tasks::Request::Ended { .. }
            | tasks::Request::Save { .. }
            | tasks::Request::Erase { .. }
            | tasks::Request::Sent { .. }
            | tasks::Request::Relay { .. }
            | tasks::Request::Notify { .. }
            | tasks::Request::Timer { .. }
            | tasks::Request::RecurringDue { .. }
            | tasks::Request::RestoreRefused { .. } => panic!("no mutation or lost startup terminal"),
        }
    }
    tasks::step(&mut child, &environment, tasks::Event::Restored, &mut out);
    assert!(out.is_empty());
    tasks::step(
        &mut child,
        &environment,
        tasks::Event::InspectEscalations { reply_to: ReplyTo::new(Token::new(1302)), project: 1 },
        &mut out,
    );
    match out.pop().expect("one empty snapshot terminal") {
        tasks::Request::EscalationsInspected { reply_to, result } => {
            assert_eq!(reply_to.into_token(), Token::new(1302));
            assert!(result.expect("root validates project").is_empty());
        }
        tasks::Request::EscalationsRechecked { .. }
        | tasks::Request::EscalationNeeded { .. }
        | tasks::Request::EscalationInspected { .. }
        | tasks::Request::EscalationDecided { .. }
        | tasks::Request::ProposalDecided { .. }
        | tasks::Request::PersonProposed { .. }
        | tasks::Request::PersonProposalDecided { .. }
        | tasks::Request::ProposalRerouteNeeded { .. }
        | tasks::Request::ProposalStalled { .. }
        | tasks::Request::EscalationStalled { .. }
        | tasks::Request::Made { .. }
        | tasks::Request::Refused { .. }
        | tasks::Request::Done { .. }
        | tasks::Request::Acknowledged { .. }
        | tasks::Request::TurnAcknowledged { .. }
        | tasks::Request::Activate { .. }
        | tasks::Request::Stop { .. }
        | tasks::Request::Adopt { .. }
        | tasks::Request::Close { .. }
        | tasks::Request::Ended { .. }
        | tasks::Request::Save { .. }
        | tasks::Request::Erase { .. }
        | tasks::Request::Sent { .. }
        | tasks::Request::Relay { .. }
        | tasks::Request::Notify { .. }
        | tasks::Request::Timer { .. }
        | tasks::Request::RecurringDue { .. }
        | tasks::Request::RestoreRefused { .. } => panic!("one named snapshot"),
    }
}

fn chat_driver() -> (Driver, engine::Assignment) {
    let mut driver = Driver::new(Store::new());
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
        reply_to: ReplyTo::new(Token::new(1001)),
        sign_in: driver.session(),
        key: [31; 16],
        ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
    });
    driver.settle();
    let assignment = assigned_from_last(&driver.delivered);
    (driver, assignment)
}

fn assigned_from_last(delivered: &[Delivery]) -> engine::Assignment {
    delivered
        .iter()
        .rev()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } => Some(assignment.clone()),
            Delivery::Relay { .. }
            | Delivery::Inbound { .. }
            | Delivery::InboxPage { .. }
            | Delivery::InboxView { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::View(_)
            | Delivery::Fleet(_)
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. }
            | Delivery::Result { .. } => None,
        })
        .expect("durable assignment")
}

fn say(driver: &mut Driver, task: u64, key: u8) -> u64 {
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(2000 + u64::from(key))),
        sign_in: driver.session(),
        key: [key; 16],
        ask: people::Ask::Say { project: 1, task, words: Box::from([key]) },
    });
    driver.settle();
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| match delivery {
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::Said { task: named, message }),
                ..
            } if *named == task => Some(*message),
            Delivery::Relay { .. }
            | Delivery::Inbound { .. }
            | Delivery::InboxPage { .. }
            | Delivery::InboxView { .. }
            | Delivery::BeginInboxView { .. }
            | Delivery::CallAnswer { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Procedure { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::Reply { .. }
            | Delivery::AcknowledgeTurn { .. }
            | Delivery::Acknowledge { .. }
            | Delivery::Cancel { .. }
            | Delivery::View(_)
            | Delivery::Fleet(_)
            | Delivery::Assigned { .. }
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. }
            | Delivery::Result { .. } => None,
        })
        .expect("committed keyed word")
}

#[test]
fn words_typed_while_a_run_works_reach_it_once_committed() {
    let (mut driver, assignment) = chat_driver();
    let before = driver.delivered.len();
    let number = say(&mut driver, assignment.task, 41);
    assert!(driver.delivered[before..].iter().any(|delivery| matches!(delivery,
        Delivery::Inbound { channel, task, attempt, word }
        if *channel == Token::new(7) && *task == assignment.task && *attempt == assignment.attempt
            && word.number == number && word.words.as_ref() == [41])));
    let row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task))).expect("live chat");
    let Record::Tasks(tasks::Stored::Live(task)) = row else { panic!("task row") };
    assert_eq!(task.inbox.len(), 1);
    assert_eq!(task.inbox[0].number, number);
}

#[test]
fn a_read_fence_takes_only_what_the_run_read() {
    let (mut driver, assignment) = chat_driver();
    let first = say(&mut driver, assignment.task, 42);
    let second = say(&mut driver, assignment.task, 43);
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        turn: engine::Turn { number: 1, cumulative: 1, read: Some(first), transcript: b"read one".as_slice().into() },
    });
    driver.settle();
    let row = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task))).expect("live chat");
    let Record::Tasks(tasks::Stored::Live(task)) = row else { panic!("task row") };
    assert_eq!(task.inbox.len(), 1);
    assert_eq!(task.inbox[0].number, second);
    assert!(driver.store.rows.contains_key(&Key::Turn { task: assignment.task, attempt: assignment.attempt, turn: 1 }));
}

#[test]
fn words_to_a_parked_chat_wake_it() {
    let (mut driver, assignment) = chat_driver();
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
    });
    driver.settle();
    let number = say(&mut driver, assignment.task, 44);
    let next = assigned_from_last(&driver.delivered);
    assert!(next.attempt > assignment.attempt);
    assert_eq!(next.inbox.len(), 1);
    assert_eq!(next.inbox[0].number, number);
    assert_eq!(next.inbox[0].words.as_ref(), [44]);
}

#[test]
fn a_chat_parks_and_resumes_from_its_transcript() {
    let (mut driver, first) = chat_driver();
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        turn: engine::Turn { number: 1, cumulative: 1, read: None, transcript: b"first".as_slice().into() },
    });
    driver.settle();
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 1,
        end: tasks::End::Parked,
    });
    driver.settle();
    say(&mut driver, first.task, 45);
    let second = assigned_from_last(&driver.delivered);
    assert_eq!(second.transcript.len(), 1);
    assert_eq!(second.transcript[0].as_ref(), b"first");
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: second.task,
        attempt: second.attempt,
        turn: engine::Turn { number: 1, cumulative: 1, read: None, transcript: b"second".as_slice().into() },
    });
    driver.settle();
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: second.task,
        attempt: second.attempt,
        cumulative: 1,
        end: tasks::End::Parked,
    });
    driver.settle();
    say(&mut driver, second.task, 46);
    let third = assigned_from_last(&driver.delivered);
    assert_eq!(third.transcript.len(), 2);
    assert_eq!(third.transcript[0].as_ref(), b"first");
    assert_eq!(third.transcript[1].as_ref(), b"second");
    assert!(third.attempt > second.attempt);
}

#[test]
fn a_chat_past_the_resume_limit_starts_fresh_with_the_tail_in_its_brief() {
    let (mut driver, first) = chat_driver();
    let mut body = vec![b'x'; 300];
    body[298..].copy_from_slice(b"yz");
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        turn: engine::Turn { number: 1, cumulative: 1, read: None, transcript: body.into_boxed_slice() },
    });
    driver.settle();
    driver.send(engine::Event::Answer {
        saved: None,
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 1,
        end: tasks::End::Parked,
    });
    driver.settle();
    say(&mut driver, first.task, 47);
    let next = assigned_from_last(&driver.delivered);
    assert!(next.transcript.is_empty());
    let tail = next
        .sections
        .iter()
        .find_map(|section| match &section.body {
            engine::BriefBody::Text(bytes) if section.kind == engine::BriefKind::Core(brief::Core::TranscriptTail) => {
                Some(bytes)
            }
            engine::BriefBody::Text(_) | engine::BriefBody::Missing(_) => None,
        })
        .expect("bounded transcript tail section");
    assert!(tail.ends_with(b"yz"), "the newest conversation bytes survive the cut");
    assert!(tail.len() <= 128, "tail fits its section budget");
}

#[test]
fn a_run_failing_transiently_is_retried_then_held_past_its_tries() {
    let (mut driver, first) = chat_driver();
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("retryable task")
    };
    assert_eq!(task.tries.transient, 1);
    assert!(matches!(task.phase, tasks::Phase::Active(tasks::Active::BackingOff { .. })));
    driver.env.now = Time::from_nanos(Duration::from_secs(2).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(2).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert!(second.attempt > first.attempt);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: second.task,
        attempt: second.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("held task")
    };
    assert_eq!(task.tries.transient, 2);
    assert!(matches!(task.phase, tasks::Phase::Held { why: tasks::Hold::Failures(tasks::Class::Transient), .. }));
}

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "the one end-to-end story checks each routing stage"
)]
fn a_delegate_held_past_its_tries_is_escalated_two_levels_to_a_person_released_and_finished() {
    let (mut driver, root) = batch_fixture_with(3, 4);
    let mut holder = report_delegate(b"middle", Box::new([]));
    holder.authority.budget.spend = 50;
    holder.authority.delegation.kinds = Box::new([tasks::AuthorityExecutor::Charter(1)]);
    holder.authority.delegation.tasks = 2;
    holder.authority.delegation.depth = 3;
    let middle = call_batch(&mut driver, &root, 601, Box::new([holder]))[0];
    let middle_run = assigned_task(&driver, middle);
    let leaf = call_batch(&mut driver, &middle_run, 602, Box::new([report_delegate(b"leaf", Box::new([]))]))[0];
    let first = assigned_task(&driver, leaf);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: leaf,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    driver.env.now = Time::from_nanos(Duration::from_secs(2).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(2).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert_eq!(second.task, leaf);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: leaf,
        attempt: second.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    let held = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(leaf))).expect("held delegate");
    let revision = match held {
        Record::Tasks(tasks::Stored::Live(row)) => match row.escalation {
            tasks::Escalation::Waiting { revision, holder: tasks::EscalationHolder::Task(task), .. }
                if task == middle =>
            {
                revision
            }
            ref other => panic!("first recipient must be middle: {other:?}"),
        },
        other => panic!("held delegate row: {other:?}"),
    };
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::Inbound { task, word, .. } if *task == middle
            && word.kind == tasks::MessageKind::Escalation { task: leaf, revision })));
    tool_call(
        &mut driver,
        &middle_run,
        603,
        engine::Tool::DecideEscalation { task: leaf, revision, decision: engine::EscalationChoice::Pass },
    );
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::CallAnswer { call, answer: temper_engine_domain::CallAnswer::EscalationDecided { outcome: tasks::EscalationOutcome::Passed { .. }, .. }, .. }
            if *call == Token::new(603))));
    let held = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(leaf))).expect("held delegate");
    let second_revision = match held {
        Record::Tasks(tasks::Stored::Live(row)) => match row.escalation {
            tasks::Escalation::Waiting { revision, holder: tasks::EscalationHolder::Task(task), .. }
                if task == root.task =>
            {
                revision
            }
            ref other => panic!("second recipient must be root task: {other:?}"),
        },
        other => panic!("held delegate row: {other:?}"),
    };
    assert!(second_revision > revision);
    tool_call(
        &mut driver,
        &root,
        604,
        engine::Tool::DecideEscalation {
            task: leaf,
            revision: second_revision,
            decision: engine::EscalationChoice::Pass,
        },
    );
    let held = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(leaf))).expect("held delegate");
    let person_revision = match held {
        Record::Tasks(tasks::Stored::Live(row)) => match row.escalation {
            tasks::Escalation::Waiting { revision, holder: tasks::EscalationHolder::Person(_), .. } => revision,
            ref other => panic!("third recipient must be person: {other:?}"),
        },
        other => panic!("held delegate row: {other:?}"),
    };
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(605)),
        sign_in: driver.session(),
        key: [65; 16],
        ask: people::Ask::DecideEscalation {
            project: 1,
            task: leaf,
            revision: person_revision,
            decision: people::EscalationDecision::Release,
        },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::EscalationDecided {
            task, choice: people::EscalationChoice::Released, ..
        }), .. } if *task == leaf)));
    let third = assigned_from_last(&driver.delivered);
    assert_eq!(third.task, leaf);
    assert!(third.attempt > second.attempt);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: leaf,
        attempt: third.attempt,
        cumulative: 0,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"finished".as_slice().into() },
            cancel_delegates: false,
        },
        saved: None,
    });
    driver.settle();
    assert!(matches!(
        driver.store.rows.get(&Key::Tasks(tasks::Key::Ended(leaf))),
        Some(Record::Tasks(tasks::Stored::Ended(_)))
    ));
}

#[test]
fn a_moved_task_is_funded_anew_by_its_new_requester() {
    let (mut driver, root) = batch_fixture_with(3, 2);
    let mut child_spec = report_delegate(b"long goal", Box::new([]));
    child_spec.authority.budget.spend = 50;
    child_spec.authority.delegation.kinds = Box::new([tasks::AuthorityExecutor::Charter(1)]);
    child_spec.authority.delegation.tasks = 1;
    child_spec.authority.delegation.depth = 1;
    let child = call_batch(&mut driver, &root, 620, Box::new([child_spec]))[0];
    let child_run = assigned_task(&driver, child);
    let grandchild = call_batch(&mut driver, &child_run, 621, Box::new([report_delegate(b"subgoal", Box::new([]))]))[0];
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: child,
        attempt: child_run.attempt,
        turn: engine::Turn { number: 1, cumulative: 5, read: None, transcript: b"worked".as_slice().into() },
    });
    driver.settle();
    let person = driver.store.header().people;
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(622)),
        sign_in: driver.session(),
        key: [62; 16],
        ask: people::Ask::Move { project: 1, task: child, reason: b"keep the goal".as_slice().into() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Moved { task }), .. } if *task == child)));
    let Some(Record::Tasks(tasks::Stored::Live(moved))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child)))
    else {
        panic!("moved task remains live")
    };
    assert_eq!(moved.requester, tasks::Party::Person(person));
    assert_eq!(moved.funder, tasks::Funder::Pool { project: 1, person, period: 1 });
    assert_eq!(moved.numbers, tasks::Numbers { budget: 45, spent: 0, spent_below: 0, reserved: 10 });
    assert_eq!(moved.root, child);
    assert_eq!(moved.depth, 0);
    let Some(Record::Tasks(tasks::Stored::Live(beneath))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(grandchild)))
    else {
        panic!("grandchild remains live")
    };
    assert_eq!((beneath.root, beneath.depth), (child, 1));
    let Some(Record::Tasks(tasks::Stored::Live(old))) = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(root.task)))
    else {
        panic!("old requester remains live")
    };
    assert!(!old.delegates.contains(&child));
    assert!(old.references.contains(&child));
    assert_eq!((old.numbers.reserved, old.numbers.spent_below), (5, 5));
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: root.task,
        attempt: root.attempt,
        cumulative: 0,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"chat done".as_slice().into() },
            cancel_delegates: false,
        },
        saved: None,
    });
    driver.settle();
    assert!(driver.store.rows.contains_key(&Key::Tasks(tasks::Key::Live(child))));
    assert!(driver.store.rows.contains_key(&Key::Tasks(tasks::Key::Live(grandchild))));
}

#[test]
fn a_move_the_new_funder_cannot_cover_is_refused_whole() {
    let (mut driver, root) = batch_fixture_with_pool(2, 1, 100);
    let child = call_batch(&mut driver, &root, 630, Box::new([report_delegate(b"child", Box::new([]))]))[0];
    let before_parent = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(root.task))).cloned();
    let before_child = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).cloned();
    let pool = tasks::Funder::Pool { project: 1, person: driver.store.header().people, period: 1 };
    let before_pool = driver.store.rows.get(&Key::Tasks(tasks::Key::Ledger(pool))).cloned();
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(631)),
        sign_in: driver.session(),
        key: [63; 16],
        ask: people::Ask::Move { project: 1, task: child, reason: b"out of room".as_slice().into() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(
        item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Authority)), .. }
    )));
    assert_eq!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(root.task))), before_parent.as_ref());
    assert_eq!(driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))), before_child.as_ref());
    assert_eq!(driver.store.rows.get(&Key::Tasks(tasks::Key::Ledger(pool))), before_pool.as_ref());
}

#[test]
fn a_move_carves_the_new_persons_pool_in_the_same_commit() {
    let (mut driver, root) = batch_fixture_custom(2, 1, 500, true, false, false);
    let child = call_batch(&mut driver, &root, 640, Box::new([report_delegate(b"second owner goal", Box::new([]))]))[0];
    driver.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(641)),
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: 8_u64.to_be_bytes().into() },
            login: b"second".as_slice().into(),
            name: b"Second".as_slice().into(),
        },
    });
    driver.settle();
    let session = driver.store.header().sign_ins;
    let person = driver.store.header().people;
    let pool = tasks::Funder::Pool { project: 1, person, period: 1 };
    assert!(!driver.store.rows.contains_key(&Key::Tasks(tasks::Key::Ledger(pool))));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(642)),
        sign_in: session,
        key: [64; 16],
        ask: people::Ask::Move { project: 1, task: child, reason: b"adopt goal".as_slice().into() },
    });
    driver.settle();
    assert!(driver.delivered.iter().any(|item| matches!(item,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Moved { task }), .. } if *task == child)));
    assert!(driver.transactions.iter().any(|writes| {
        writes.iter().any(|write| matches!(write,
            Write::Save(Record::Tasks(tasks::Stored::Live(row))) if row.number == child && row.requester == tasks::Party::Person(person)))
        && writes.iter().any(|write| matches!(write,
            Write::Save(Record::Tasks(tasks::Stored::Ledger(row))) if row.funder == pool && row.numbers.reserved == 10))
        && writes.iter().any(|write| matches!(write,
            Write::Save(Record::People(people::Stored::Answer { outcome: people::Outcome::Moved { task }, .. })) if *task == child))
    }));
}

#[test]
fn moving_a_held_delegate_rechecks_its_escalation_recipient() {
    let (mut driver, root) = batch_fixture_with(2, 1);
    let child = call_batch(&mut driver, &root, 650, Box::new([report_delegate(b"held goal", Box::new([]))]))[0];
    let first = assigned_task(&driver, child);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: child,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    driver.env.now = Time::from_nanos(Duration::from_secs(2).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(2).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: child,
        attempt: second.attempt,
        cumulative: 0,
        end: tasks::End::Failed(tasks::Class::Transient),
        saved: None,
    });
    driver.settle();
    let before = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("held child");
    assert!(matches!(before, Record::Tasks(tasks::Stored::Live(row)) if matches!(row.escalation,
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Task(parent), .. } if parent == root.task)));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(651)),
        sign_in: driver.session(),
        key: [65; 16],
        ask: people::Ask::Move { project: 1, task: child, reason: b"adopt held work".as_slice().into() },
    });
    driver.settle();
    let person = driver.store.header().people;
    let after = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("moved held child");
    assert!(matches!(after, Record::Tasks(tasks::Stored::Live(row)) if matches!(row.escalation,
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Person(found), .. } if found == person)));
}

#[test]
fn moving_a_proposer_rechecks_its_decision_holder() {
    let (mut driver, root) = batch_fixture_with(2, 2);
    let child = call_batch(&mut driver, &root, 660, Box::new([report_delegate(b"goal", Box::new([]))]))[0];
    let run = assigned_task(&driver, child);
    tool_call(
        &mut driver,
        &run,
        661,
        engine::Tool::Propose {
            action: engine::ProposedAction::Batch(Box::new([report_delegate(b"helper", Box::new([]))])),
            reason: b"need help".as_slice().into(),
            as_holder: false,
        },
    );
    let before = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("proposer live");
    assert!(matches!(before, Record::Tasks(tasks::Stored::Live(row)) if matches!(&row.proposal,
        Some(proposal) if matches!(proposal.state,
            tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Task(holder), .. } if holder == root.task))));
    driver.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(662)),
        sign_in: driver.session(),
        key: [66; 16],
        ask: people::Ask::Move { project: 1, task: child, reason: b"adopt proposal".as_slice().into() },
    });
    driver.settle();
    let person = driver.store.header().people;
    let after = driver.store.rows.get(&Key::Tasks(tasks::Key::Live(child))).expect("moved proposer");
    assert!(matches!(after, Record::Tasks(tasks::Stored::Live(row)) if matches!(&row.proposal,
        Some(proposal) if matches!(proposal.state,
            tasks::ProposalState::Pending { holder: tasks::ProposalHolder::Person(holder), .. } if holder == person))));
}

#[test]
fn saved_work_reaches_the_next_attempt() {
    let (mut driver, first) = chat_driver();
    driver.send(engine::Event::Answer {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
        saved: Some(Box::new([3])),
    });
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("parked task")
    };
    assert_eq!(
        task.saved.as_ref(),
        [tasks::SavedResource { connector: 0, path: Box::new([Box::from(3u32.to_be_bytes())]) }]
    );
    say(&mut driver, first.task, 49);
    let second = assigned_from_last(&driver.delivered);
    assert_eq!(second.saved.as_ref(), [3]);
    assert!(second.attempt > first.attempt);
}

#[test]
fn a_worker_lost_mid_run_resumes_at_the_last_committed_turn() {
    let (mut driver, first) = chat_driver();
    turn(&mut driver, &first, 1, 1);
    driver.settle();
    assert!(driver.store.rows.contains_key(&Key::Turn { task: first.task, attempt: first.attempt, turn: 1 }));
    driver.send(engine::Event::Lost { channel: Token::new(7) });
    driver.env.now = Time::from_nanos(Duration::from_secs(6).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(6).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("lost task waits for retry")
    };
    assert_eq!(task.tries.lost, 1);
    driver.env.now = Time::from_nanos(Duration::from_secs(7).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(7).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.send(engine::Event::Hello {
        channel: Token::new(8),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    driver.settle();
    let second = assigned_from_last(&driver.delivered);
    assert!(second.attempt > first.attempt);
    assert_eq!(second.transcript.len(), 1);
    assert_eq!(second.transcript[0].as_ref(), b"step");
}

#[test]
fn a_worker_frozen_past_its_grace_gets_no_next_attempt_until_its_sum_has_passed() {
    let (mut driver, first) = chat_driver();
    driver.send(engine::Event::Hello {
        channel: Token::new(8),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(5),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    assert!(
        driver.delivered.iter().any(|delivery| matches!(delivery, Delivery::Refuse { channel }
        if *channel == Token::new(8))),
        "a worker whose stop bound reaches engine grace cannot place runs"
    );
    driver.send(engine::Event::Lost { channel: Token::new(7) });
    driver.env.now = Time::from_nanos(Duration::from_secs(4).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(4).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    for _ in 0..10 {
        driver.advance(true);
    }
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("held claim")
    };
    assert_eq!(task.tries.lost, 0);
    assert_eq!(driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Assigned { .. })).count(), 1);
    driver.env.now = Time::from_nanos(Duration::from_secs(6).as_nanos());
    driver.env.wall = Wall::from_nanos(Duration::from_secs(6).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let Some(Record::Tasks(tasks::Stored::Live(task))) =
        driver.store.rows.get(&Key::Tasks(tasks::Key::Live(first.task)))
    else {
        panic!("retryable lost task")
    };
    assert_eq!(task.tries.lost, 1);
}
