//! temper's Open chat wiring; jig owns the lifecycle's exhaustive proofs.
use super::*;
use jig_client::{contract as c, facts, shapes};
use jig_client_world::joint::wire;
use jig_domain_core::{documents as d, reads};
use temper_engine_domain_world::{fixtures, walking};

const CHANNEL: Token = Token::new(7);
const REPLY: &[u8] = b"Here is my reply";
const DISCARDED_REPORT: &[u8] = b"This finish is not a result";
const READ: Token = Token::new(1301);

fn open_configuration(mut configuration: engine::Config) -> engine::Config {
    configuration.core.defaults.templates = fixtures::templates_ending(tasks::EndedBy::Requester);
    configuration
}

fn open_config(seed: u64) -> engine::Config {
    open_configuration(walking::config(seed))
}

fn open_chat() -> (Driver, engine::Assignment) {
    let mut driver = Driver::configured(Store::new(), [open_config(91), open_config(91)], &limits());
    driver.hello(CHANNEL);
    driver.settle();
    driver.sign_in();
    driver.settle();
    let chat = open_ask(&mut driver, QUESTION);
    ask(&mut driver, 60, chat);
    let run = driver.last_assignment();
    assert_eq!(live(&driver, run.task).flags.ended_by, tasks::EndedBy::Requester);
    (driver, run)
}

fn finish(driver: &mut Driver, run: &engine::Assignment, cumulative: u64, words: &[u8]) {
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        cumulative,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: words.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    driver.pump();
}

fn reply_and_finish(driver: &mut Driver, run: &engine::Assignment, read: Option<u64>, words: &[u8]) {
    driver.send(protocol::turn(CHANNEL, run.task, run.attempt, 1, 1, read, words));
    driver.pump();
    finish(driver, run, 1, DISCARDED_REPORT);
}

fn parked_chat() -> (Driver, engine::Assignment) {
    let (mut driver, run) = open_chat();
    reply_and_finish(&mut driver, &run, None, REPLY);
    (driver, run)
}

fn assignments(driver: &Driver) -> usize {
    driver.delivered.iter().filter(|delivery| matches!(delivery, Delivery::Assigned { .. })).count()
}

fn ended(driver: &Driver, task: u64) -> Option<&tasks::TaskRecord> {
    let key = Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Ended(task)));
    if let Some(Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Ended(row)))) = driver.store.rows.get(&key) {
        Some(row)
    } else {
        None
    }
}

fn committed_replies(driver: &Driver, task: u64, words: &[u8]) -> usize {
    driver
        .store
        .rows
        .values()
        .filter(|row| {
            matches!(row,
                Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Turn(turn)))
                    if turn.task == task && turn.transcript.as_ref() == words
            )
        })
        .count()
}

fn assert_parked(driver: &Driver, task: u64, expected_assignments: usize) {
    let row = live(driver, task);
    assert_eq!(row.phase, tasks::Phase::Active(tasks::Active::Idle));
    assert_eq!(row.tries, tasks::Tries::NONE);
    assert!(ended(driver, task).is_none());
    assert!(!driver.store.rows.values().any(|row| matches!(row,
        Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Result(result))) if result.task == task
    )));
    assert_eq!(assignments(driver), expected_assignments);
}

fn past_idle_and_backoff(driver: &mut Driver) {
    let idle_interval = charter().policy.waiting;
    let retries = driver.env.limits.tasks.retries;
    let retry_backoff =
        [retries.transient, retries.permanent, retries.run, retries.agent, retries.lost, retries.invalid]
            .into_iter()
            .map(|retry| retry.max)
            .max()
            .expect("retry classes");
    let past_both = idle_interval.saturating_add(retry_backoff).saturating_add(Duration::from_nanos(1));
    driver.fire_at(Duration::from_nanos(driver.env.now.as_nanos()).saturating_add(past_both));
    driver.settle();
}

fn ask(driver: &mut Driver, key: u8, ask: people::Ask) {
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(1400 + u64::from(key))),
        sign_in: driver.session(),
        key: [key; 16],
        ask,
    }));
    driver.pump();
}

fn read(driver: &mut Driver, read: reads::Read) -> reads::Answer {
    driver.send(engine::Event::Party(engine::PartyInput::Read {
        reply_to: ReplyTo::new(READ),
        sign_in: driver.session(),
        read,
    }));
    driver.pump();
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let Delivery::Read { reply, answer } = delivery
                && *reply == ReplyTo::new(READ)
            {
                Some(answer.clone())
            } else {
                None
            }
        })
        .expect("committed read answer")
}

fn client_me(driver: &mut Driver) -> c::Me {
    let reads::Answer::Document(document) = read(driver, reads::Read::Frame) else { panic!("party frame") };
    let d::Document::Frame(frame) = *document else { panic!("frame document") };
    let configuration = open_config(91);
    wire::owner::frame(&frame, &client_limits(), &walking::models(), &configuration.core.settings.llm_providers)
        .expect("whole client frame")
}

fn open_ask(driver: &mut Driver, words: &[u8]) -> people::Ask {
    let me = client_me(driver);
    let chat = shapes::chat(&me, 1, c::Text::new(words.into(), u32::MAX).expect("bounded words"))
        .expect("offered Open template");
    wire::asks::ask(&chat, &walking::models()).expect("client chat translates")
}

fn client_limits() -> jig_client::DomainLimits {
    let mut bounds = jig_client_world::test::limits().client.domain;
    bounds.text = 64;
    bounds.turn_text = bounds.text;
    bounds.turn_bytes = bounds.text;
    bounds.rows = limits().documents.rows;
    bounds.projects = bounds.rows;
    bounds.segments = limits().documents.segments;
    bounds
}

#[test]
fn an_open_chats_finish_parks_with_its_reply_and_spends_no_try() {
    let (mut driver, run) = parked_chat();
    assert_eq!(committed_replies(&driver, run.task, REPLY), 1);
    assert_eq!(kept_turns(&driver), 1);
    assert_parked(&driver, run.task, 1);
    past_idle_and_backoff(&mut driver);
    assert_parked(&driver, run.task, 1);
    assert_eq!(committed_replies(&driver, run.task, REPLY), 1);
}

#[test]
fn a_persons_words_wake_a_parked_open_chat_once() {
    const WORD: u8 = 61;
    const NEXT_REPLY: &[u8] = b"And here is my next reply";
    let (mut driver, first) = parked_chat();
    let message = driver.say(first.task, WORD);
    let next = driver.last_assignment();
    assert_eq!(assignments(&driver), 2);
    assert_eq!(next.task, first.task);
    assert!(next.attempt > first.attempt);
    assert_eq!(next.messages.len(), 1);
    assert_eq!((next.messages[0].number, next.messages[0].words.as_ref()), (message, [WORD].as_slice()));
    reply_and_finish(&mut driver, &next, Some(message), NEXT_REPLY);
    let row = live(&driver, first.task);
    assert!(row.inbox.is_empty());
    assert_eq!(row.last_message, message);
    assert_eq!(row.last_reply, 2);
    assert!(driver.store.rows.values().any(|row| matches!(row,
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Turn(turn)))
            if turn.task == first.task && turn.attempt == next.attempt && turn.read == Some(message)
    )));
    assert_eq!(committed_replies(&driver, first.task, REPLY), 1);
    assert_eq!(committed_replies(&driver, first.task, NEXT_REPLY), 1);
    assert_eq!(kept_turns(&driver), 2);
    assert_parked(&driver, first.task, 2);
    past_idle_and_backoff(&mut driver);
    assert_parked(&driver, first.task, 2);
}

/// The existing batch fixture supplies room for two live delegates and a compound Close.
fn delegation_driver() -> Driver {
    let (configuration, mut bounds) = batch_settings(1, 1, 500, false, false, false);
    bounds.people.operations = 3;
    bounds.journal.writes = 8192;
    // The stored keyed request retains all three Close operations too.
    let mut store_bounds = temper_engine_domain_world::store::widest();
    store_bounds.people.operations = bounds.people.operations;
    let store = Store::with_limits(&store_bounds);
    let mut driver = Driver::configured(store, configuration.map(open_configuration), &bounds);
    driver.hello(CHANNEL);
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver
}

#[test]
fn closing_an_open_chat_keeps_its_turns_and_settles_its_delegates() {
    const REASON: &[u8] = b"Chat closed";
    let mut driver = delegation_driver();
    let chat = open_ask(&mut driver, QUESTION);
    ask(&mut driver, 62, chat);
    let run = driver.last_assignment();
    let children = call_batch(
        &mut driver,
        &run,
        920,
        Box::new([
            report_delegate(b"cancel this report", Box::new([])),
            report_delegate(b"keep this report", Box::new([])),
        ]),
    );
    let [cancel, moved] = *children else { panic!("two delegates") };
    reply_and_finish(&mut driver, &run, None, REPLY);
    for child in [cancel, moved] {
        let delegate = assigned_task(&driver, child);
        park_task(&mut driver, &delegate);
    }
    driver.settle();
    assert_eq!(live(&driver, run.task).delegates.as_ref(), [cancel, moved]);
    let me = client_me(&mut driver);
    let close = shapes::close_chat(
        &me,
        1,
        run.task,
        &[
            shapes::Delegate { task: cancel, fate: shapes::Fate::Cancel },
            shapes::Delegate { task: moved, fate: shapes::Fate::Move },
        ],
        c::Text::new(REASON.into(), u32::MAX).expect("bounded reason"),
    )
    .expect("offered Close");
    let request = wire::asks::request(&close, &walking::models()).expect("client Close translates");
    let before_assignments = assignments(&driver);
    let at = driver.env.wall;
    ask(&mut driver, 63, request.ask);
    let row = ended(&driver, run.task).expect("chat ended without advancing time");
    assert!(
        matches!(&row.phase, tasks::Phase::Ended(tasks::Ending::Cancelled { reason, result: None, .. }) if reason.as_ref() == REASON)
    );
    assert_eq!(row.ended_at, Some(at));
    assert_eq!(row.tries, tasks::Tries::NONE);
    assert_eq!(row.last_reply, 1);
    assert!(matches!(
        &ended(&driver, cancel).expect("cancelled delegate").phase,
        tasks::Phase::Ended(tasks::Ending::Cancelled { .. })
    ));
    assert_eq!(live(&driver, moved).requester, tasks::Party::Person(me.party.number));
    assert!(ended(&driver, moved).is_none());
    assert_eq!(assignments(&driver), before_assignments);
    assert_eq!(driver.env.wall, at);
    assert_eq!(committed_replies(&driver, run.task, REPLY), 1);
    assert_ended_conversation(&mut driver, run.task);
    let reads::Answer::Historical(past) =
        read(&mut driver, reads::Read::Turn { task: run.task, attempt: run.attempt, turn: 1 })
    else {
        panic!("ended conversation's retained turn")
    };
    let reads::Past::Turn(turn) = past.value else { panic!("historical turn") };
    assert_eq!(turn.transcript.as_ref(), REPLY);
}

fn assert_ended_conversation(driver: &mut Driver, task: u64) {
    let mut from = None;
    loop {
        let reads::Answer::Tasks(page) =
            read(driver, reads::Read::Tasks { query: jig_core_world::fixtures::OWN_TASKS, live: None, ended: from })
        else {
            panic!("conversation page")
        };
        let reads::EndedGroup::Rows { rows, next, .. } = &page.ended else { panic!("ended conversations") };
        if let Some(line) = rows.iter().find(|line| line.chip.number == task) {
            assert_eq!(line.conversation.as_ref().expect("ended conversation's replies").unread, d::Count::Known(1));
            return;
        }
        from = Some(next.clone().expect("ended conversation continuation"));
    }
}

#[test]
fn a_goals_delegate_still_ends_by_its_result_beside_open_chats() {
    const RESULT: &[u8] = b"goal delegate done";
    let mut driver = delegation_driver();
    let open = open_ask(&mut driver, b"make a goal");
    let goal = walking::goal_from(
        open,
        people::TaskExecutor::Agent { charter: 1 },
        jig_core_world::fixtures::Goal { budget: 150, priority: 1, deadline: None },
    );
    ask(&mut driver, 64, goal);
    let run = driver.last_assignment();
    assert_eq!(live(&driver, run.task).label, shapes::GOAL);
    assert_eq!(live(&driver, run.task).flags.ended_by, tasks::EndedBy::Result);
    let child = call_batch(&mut driver, &run, 921, Box::new([report_delegate(b"goal report", Box::new([]))]))[0];
    park_task(&mut driver, &run);
    let delegate = assigned_task(&driver, child);
    assert_eq!(live(&driver, child).flags.ended_by, tasks::EndedBy::Result);
    finish(&mut driver, &delegate, 0, RESULT);
    assert_eq!(
        ended(&driver, child).expect("delegate ended").phase,
        tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report { words: RESULT.into() }))
    );
    assert_eq!(ended(&driver, child).expect("delegate ended").tries, tasks::Tries::NONE);
    assert!(live(&driver, run.task).inbox.iter().any(|message| message.from == tasks::Party::Task(child)
        && message.kind == tasks::MessageKind::Result(tasks::ResultKind::Report)
        && message.words.as_ref() == RESULT));
    assert!(driver.transactions.iter().any(|writes| {
        writes.iter().any(|write| matches!(write, Write::Save(Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Ended(row)))) if row.number == child))
            && writes.iter().any(|write| matches!(write, Write::Save(Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Live(row))))
                if row.number == run.task && row.inbox.iter().any(|message| message.from == tasks::Party::Task(child) && message.words.as_ref() == RESULT)))
    }));
    assert_eq!(assignments(&driver), 3, "goal, delegate, goal woken by result");
    let resumed = driver.last_assignment();
    finish(&mut driver, &resumed, 0, RESULT);
    assert_eq!(
        ended(&driver, run.task).expect("goal ends by result").phase,
        tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report { words: RESULT.into() }))
    );
    assert_eq!(assignments(&driver), 3);
}

#[test]
fn an_open_chat_on_a_program_endpoint_is_refused_and_starts_nothing() {
    let mut driver = Driver::configured(Store::new(), [open_config(91), open_config(91)], &limits());
    let mut program_bounds = protocol::bounds();
    program_bounds.program_endpoints = Box::new([charter().policy.model.endpoint]);
    let program_hello = host::Hello {
        slots: 1,
        stop_bound: Duration::from_millis(100),
        workstreams: Box::new([]),
        hosting: Box::new([]),
        bounds: program_bounds,
    };
    driver.send(engine::Event::Worker(engine::WorkerInput::Link {
        channel: CHANNEL,
        link: engine::Link::Hello { worker: 0, hello: program_hello },
    }));
    driver.settle();
    driver.sign_in();
    driver.settle();
    let chat = open_ask(&mut driver, QUESTION);
    ask(&mut driver, 65, chat);
    let task = driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::Created { first, .. }), ..
            } = delivery
            {
                Some(*first)
            } else {
                None
            }
        })
        .expect("chat created");
    assert_eq!(assignments(&driver), 0);
    assert_eq!(live(&driver, task).tries, tasks::Tries::NONE);
    assert_eq!(live(&driver, task).attempt, 0);
    assert!(ended(&driver, task).is_none());
    let reads::Answer::Document(document) = read(&mut driver, reads::Read::Task { task }) else { panic!("task read") };
    let d::Document::Task(document) = *document else { panic!("task document") };
    assert_eq!(document.run_wait, Some(d::TaskRunWait::Refused(d::TaskWaitRefused::Program(d::OpenProgram::Model))));
    let client_task = wire::task::document(&document, &client_limits(), &walking::models()).expect("client task");
    let me = client_me(&mut driver);
    assert_eq!(
        facts::chat_status(
            &client_task.chip,
            &client_task.waits_on,
            client_task.run_wait.as_ref(),
            client_task.project,
            &me
        ),
        facts::ChatStatus::HostRefused(c::TaskWaitRefused::Program(c::OpenProgram::Model))
    );
    let (control, assigned) = open_chat();
    assert_eq!(assignments(&control), 1);
    assert_eq!(live(&control, assigned.task).tries, tasks::Tries::NONE);
    assert!(ended(&control, assigned.task).is_none());
}

#[test]
fn an_open_chat_that_meets_a_bound_is_held_and_stays_open() {
    let (mut driver, run) = open_chat();
    super::exceeded::meets_bound(&mut driver, &run, host::Bound::Reasoning);
    super::exceeded::assert_held(&driver, &run, tasks::Bound::Reasoning);
    past_idle_and_backoff(&mut driver);
    super::exceeded::assert_held(&driver, &run, tasks::Bound::Reasoning);
    assert_eq!(live(&driver, run.task).flags.ended_by, tasks::EndedBy::Requester);
    assert!(ended(&driver, run.task).is_none());
}
