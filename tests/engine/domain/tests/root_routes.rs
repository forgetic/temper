use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::VecDeque;
use temper_engine_domain::{Delivery, Record, Write, engine};
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
    fail_archive_once: bool,
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
            fail_archive_once: false,
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
                    if self.fail_archive_once && matches!(range, temper_engine_domain::Range::EscalationDecision { .. })
                    {
                        self.fail_archive_once = false;
                        self.events.push_back(engine::Event::Unloaded { owner });
                    } else {
                        self.events.push_back(engine::Event::Loaded { owner, rows, next });
                    }
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
                | Delivery::EscalationReply { .. }
                | Delivery::ReadEscalationDecision { .. }
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
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
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
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. }
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
            | Record::EscalationDecision(_)
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
            | Record::EscalationDecision(_)
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
        result: tasks::TaskResult::Verdict { code: 99, words: b"invalid contract".as_slice().into() },
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
    assert!(changed.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Live(task)) if task.escalation == tasks::Escalation::Waiting { revision: 2, holder: tasks::EscalationHolder::Role { project: 1, role: 0 } })));
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
            task.escalation =
                tasks::Escalation::Waiting { revision: u64::MAX, holder: tasks::EscalationHolder::Person(1) };
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
            key: people::IdentityKey { forge: 1, user: 9 },
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
        + configured.tasks.tasks
        + 4;
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
        engine::resume(&mut driver.root, &driver.env, &mut driver.out);
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
            | Delivery::Fleet(_)
            | Delivery::Assigned { .. }
            | Delivery::Refuse { .. }
            | Delivery::ReadResult { .. }
            | Delivery::TurnBusy { .. }
            | Delivery::Load { .. }
            | Delivery::Result { .. }
            | Delivery::ResultReply { .. }
            | Delivery::EscalationReply { .. }
            | Delivery::ReadEscalationDecision { .. } => None,
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
fn restored_loss_spends_a_try_only_after_a_real_durable_turn() {
    for kept_turn in [false, true] {
        let mut original = Driver::new(Store::new());
        original.send(engine::Event::Hello {
            channel: Token::new(7),
            hello: fleet::Hello {
                graces: Some(Duration::from_secs(1)),
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
        engine::fire(&mut restored.root, &restored.env, &mut restored.out);
        restored.collect();
        let end = if kept_turn { tasks::End::Failed(tasks::Class::Lost) } else { tasks::End::Refused };
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
        assert!(writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(task))) if task.number == assignment.task && task.numbers.spent == cumulative && task.run_spent == cumulative && task.last_answer == Some(assignment.attempt) && task.tries.lost == u32::from(kept_turn))), "canonical terminal and correct failure tries share one commit");
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
    use temper_engine_domain_authority as authority;
    let mut config = config(seed);
    let mut policy = config.authority.policy(1).expect("actual project").clone();
    policy.roles[0].requests = authority::Requests(1 | 4 | 256);
    let mut findings = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut config.authority, authority::Event::Policy { project: 1, policy }, &mut findings);
    assert_eq!(findings.pop(), Some(authority::PolicyFact::Changed { project: 1 }));
    config
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
        + configured.tasks.tasks
        + 4;
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
            | Record::Tasks(tasks::Stored::Ended(_) | tasks::Stored::Ledger(_) | tasks::Stored::Closure(_))
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::EscalationDecision(_) => None,
        })
        .collect();
    assert_eq!(original.len(), 2, "two genuine task admissions and priced failures");
    for record in &original {
        assert_eq!(
            record.escalation,
            tasks::Escalation::Waiting { revision: 1, holder: tasks::EscalationHolder::Person(people[0]) }
        );
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
        expected.escalation =
            tasks::Escalation::Waiting { revision: 2, holder: tasks::EscalationHolder::Role { project: 1, role: 0 } };
        assert!(
            writes.iter().any(
                |write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(actual))) if *actual == expected)
            ),
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
    last.escalation =
        tasks::Escalation::Waiting { revision: u64::MAX, holder: tasks::EscalationHolder::Person(people[0]) };
    let mut expected_header = overflow_store.header();
    expected_header.commits = expected_header.commits.checked_add(1).expect("one saved refusal commit");
    let unchanged = overflow_store.rows.clone();
    let mut overflow = Driver::configured(overflow_store, administration_config(9310), &configured);
    overflow.settle();
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
            ask: people::Ask::SetRoles {
                project: 1,
                holdings: Box::new([people::Holding { person: people[1], role: people::Role::Owner }]),
            },
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
        | tasks::Request::RestoreRefused { .. } => panic!("one named snapshot"),
    }
}
