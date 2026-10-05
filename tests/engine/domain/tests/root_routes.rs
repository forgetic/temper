use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::VecDeque;
use temper_engine_domain::{Delivery, Record, engine};
use temper_engine_domain_accounts as accounts;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;
use temper_engine_domain_world::commits::Store;
use temper_engine_domain_world::walking::{Settings, World, config, limits};
use temper_engine_domain_world::walking_referee::{FINAL_SPEND, QUESTION, REPORT};

struct Driver {
    root: engine::Domain,
    env: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<engine::Event>,
    store: Store,
    delivered: Vec<Delivery>,
    serial: u64,
    accounts: Vec<accounts::Request>,
    stopped: bool,
}

impl Driver {
    fn new(store: Store) -> Driver {
        Driver::configured(store, config(91), &limits())
    }

    fn configured(store: Store, config: engine::Config, limits: &engine::Limits) -> Driver {
        Driver {
            root: engine::Domain::new(config, limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            out: Queue::with_capacity(engine::max_out(limits)),
            events: VecDeque::from([engine::Event::Start]),
            store,
            delivered: Vec::new(),
            serial: 100,
            accounts: Vec::new(),
            stopped: false,
        }
    }

    fn send(&mut self, event: engine::Event) {
        engine::step(&mut self.root, &self.env, event, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    fn collect(&mut self) {
        for _ in 0..self.out.len() {
            match self.out.pop().expect("output count") {
                engine::Request::Commit { number, writes } => self.store.pending.push_back((number, writes)),
                engine::Request::Load { owner, range, after, most, bytes } => {
                    let (rows, next) = self.store.page(range, after, most);
                    assert!(rows.len() <= usize::try_from(most).expect("small count"));
                    assert!(bytes >= self.env.limits.loads.bytes);
                    self.events.push_back(engine::Event::Loaded { owner, rows, next });
                }
                engine::Request::Deliver(delivery) => self.delivered.push(delivery),
                engine::Request::Account(request) => self.accounts.push(request),
                engine::Request::Stop => self.stopped = true,
                engine::Request::TurnBusy { .. } | engine::Request::AnswerBusy { .. } => {
                    panic!("unexpected route refusal")
                }
            }
        }
    }

    fn advance(&mut self, apply: bool) {
        if let Some(event) = self.events.pop_front() {
            self.send(event);
        }
        if apply && !self.store.pending.is_empty() {
            let number = self.store.apply();
            self.send(engine::Event::Committed { number });
        }
        engine::resume(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    fn settle(&mut self) {
        for _ in 0..200 {
            self.advance(true);
            if self.root.quiescent() && self.events.is_empty() && self.store.pending.is_empty() {
                return;
            }
        }
        panic!("direct root routes did not settle: {:?}", self.root);
    }

    fn sign_in(&mut self) {
        self.serial += 1;
        self.send(engine::Event::SignedIn {
            reply_to: ReplyTo::new(Token::new(self.serial)),
            identity: people::Identity {
                key: people::IdentityKey { forge: 1, user: 7 },
                login: b"person".as_slice().into(),
                name: b"Person".as_slice().into(),
            },
        });
    }

    fn session(&self) -> u64 {
        self.delivered
            .iter()
            .find_map(|delivery| match delivery {
                Delivery::WebReply { sign_in, reply: people::Reply::SignedIn { .. }, .. } => *sign_in,
                Delivery::WebReply { .. }
                | Delivery::Reply { .. }
                | Delivery::Acknowledge { .. }
                | Delivery::AcknowledgeTurn { .. }
                | Delivery::Cancel { .. }
                | Delivery::Result { .. }
                | Delivery::Fleet(_)
                | Delivery::Assigned { .. }
                | Delivery::Refuse { .. }
                | Delivery::ReadResult { .. }
                | Delivery::TurnBusy { .. }
                | Delivery::Load { .. }
                | Delivery::ResultReply { .. } => None,
            })
            .expect("durable sign-in reply")
    }

    // Leave the newly durable callback behind three later unanswered commits.
    fn pressure(&mut self) {
        assert_eq!(self.store.pending.len(), 1);
        self.sign_in();
        self.sign_in();
        assert_eq!(self.store.pending.len(), 3);
        let number = self.store.apply();
        self.send(engine::Event::Committed { number });
        self.sign_in();
        assert_eq!(self.store.pending.len(), 3);
        for _ in 0..20 {
            self.advance(false);
        }
        assert!(!self.root.quiescent(), "owed callback and store terminals prevent completion");
        assert_eq!(self.store.pending.len(), 3, "no callback write entered the full journal");
    }
}

fn turn(driver: &mut Driver, assignment: &engine::Assignment, number: u32, cumulative: u64) {
    driver.send(engine::Event::Turn {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        turn: engine::Turn { number, cumulative, read: None, transcript: b"step".as_slice().into() },
    });
}

#[test]
fn durable_start_turn_and_answer_callbacks_survive_full_journal_pressure() {
    let mut driver = Driver::new(Store::new());
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            graces: Some(Duration::from_secs(1)),
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
            | Delivery::Fleet(_)
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. } => None,
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
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: FINAL_SPEND,
        end: tasks::End::Finished { result: tasks::Result::Report { words: REPORT.into() }, cancel_delegates: false },
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
            | Record::Terminal(_) => None,
        })
        .expect("ended task");
    let mut driver = Driver::new(world.store);
    assert!(!driver.root.quiescent(), "cold startup has outstanding work");
    driver.settle();
    assert!(driver.delivered.is_empty(), "startup does not repeat any person or worker notice");
    driver.send(engine::Event::ReadResult { reply_to: ReplyTo::new(Token::new(301)), sign_in: header.sign_ins, task });
    assert!(!driver.root.quiescent(), "held result query is an obligation");
    driver.settle();
    assert_eq!(driver.store.header(), header, "reading ended history allocates no number or charge");
    assert_eq!(driver.delivered.len(), 1);
    let Delivery::ResultReply { to, task: found, words, .. } = driver.delivered.pop().expect("one reply") else {
        panic!("task-derived result reply");
    };
    assert_eq!(to.into_token(), Token::new(301));
    assert_eq!(found, task);
    assert_eq!(words.as_ref(), REPORT);
    for _ in 0..20 {
        driver.advance(true);
    }
    assert!(driver.delivered.is_empty(), "result reply consumed its one destination");
}

fn hello(driver: &mut Driver) {
    driver.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            graces: Some(Duration::from_secs(1)),
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
            | Delivery::Fleet(_)
            | Delivery::WebReply { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. }
            | Delivery::ResultReply { .. } => None,
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
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: FINAL_SPEND,
        end: tasks::End::Finished {
            result: tasks::Result::Report { words: vec![b'x'; 129].into_boxed_slice() },
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
fn fleet_capacity_refusal_releases_the_unplaced_durable_claim() {
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
                key: people::IdentityKey { forge: 1, user: 7 },
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
            | Record::Terminal(_) => None,
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
                graces: Some(Duration::from_secs(5)),
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
        hello: fleet::Hello { graces: None, slots: 1, workstreams: Box::new([]), hosting: Box::new([]) },
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
            | Record::Terminal(_) => None,
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
    let assignment = assigned(&driver);
    turn(&mut driver, &assignment, 1, 3);
    driver.settle();
    (driver, assignment)
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
            ending: tasks::Ending::Done(tasks::Result::Report { words: REPORT.into() }),
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
                result: tasks::Result::Report { words: REPORT.into() },
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
fn root_restore_refuses_task_shapes_without_an_actual_root_route() {
    let (driver, assignment) = running_fixture();
    for unsupported in 0..3 {
        let mut store = Store::new();
        store.rows = driver.store.rows.clone();
        let Some(Record::Tasks(tasks::Stored::Live(task))) =
            store.rows.get_mut(&temper_engine_domain::Key::Tasks(tasks::Key::Live(assignment.task)))
        else {
            panic!("task");
        };
        match unsupported {
            0 => task.requester = tasks::Party::Deployment { project: 1 },
            1 => task.executor = tasks::Executor::Agent { charter: 99 },
            2 => task.contract = tasks::Contract::Verdict { choices: Box::new([tasks::Verdict { code: 1, words: 8 }]) },
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
        result: tasks::Result::Verdict { code: 99, words: b"invalid contract".as_slice().into() },
        cancel_delegates: false,
    };
    driver.send(engine::Event::Answer {
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
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        cumulative: 8,
        end,
    });
    driver.settle();
    assert_eq!(driver.store.rows, rows, "fenced duplicate never reaches child priced admission");
}
