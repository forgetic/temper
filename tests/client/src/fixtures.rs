//! Named story values and one client profile, shared by every story.
pub mod b2;
use crate::{app::Temper, front::Joint};
use jig_client_world::harness::FrontFault;
use jig_client_world::harness::{Settings, World};
use jig_domain_tasks as tasks;
use skein_lib::{Duration, RequestLimits, Time, Token};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};
use temper_client_domain::{Limits, forge};
use temper_client_view as view;
use temper_engine_domain as engine;
use temper_engine_domain_world::direct::Driver;
pub const NOW: skein_lib::Wall = skein_lib::Wall::EPOCH;
pub const FORGE: u16 = temper_client_protocol_wire::number(temper_client_protocol_wire::Owner::Forge);
pub const FORGE_HOST: &[u8] = b"forge.example";
/// The stand speaks Forgejo, with walking's forge number, writer and git host.
#[must_use]
pub fn forge_settings() -> [temper_service::settings::Forge; 1] {
    use temper_service::settings::{Forge, Kind, Transport};
    let mut base = b"http://".to_vec();
    base.extend_from_slice(FORGE_HOST);
    [Forge {
        number: 1,
        kind: Kind::Forgejo,
        api: jig_protocol_config::Address::Host { name: FORGE_HOST.into(), port: 80 },
        transport: Transport::Plaintext,
        git: base.clone().into(),
        git_program: None,
        web: Some(base.into()),
        app_read_only: false,
        api_token: None,
        webhook_secret: None,
        user: 1,
        identity: 1,
    }]
}
pub const REPOSITORIES: &[(&[u8], &[u8], u32)] = &[(b"acme", b"shop", 7), (b"acme", b"second", 8)];
pub const PARTY: u64 = 1;
pub const SEED: u64 = 1101;
pub const WITHIN: Duration = Duration::from_secs(1);
#[must_use]
pub fn domain_limits() -> Limits {
    {
        let requests = RequestLimits {
            requests: 4,
            bytes: 262_144,
            out: 16,
            first: Duration::from_millis(10),
            most: Duration::from_millis(80),
            margin: Duration::from_secs(1),
        };
        let client = jig_client::Limits {
            domain: jig_client::DomainLimits {
                accounts: jig_client::AccountCounts { accounts: 16, sources: 8, sign_ins: 4 },
                owners: &[jig_client::Spoken {
                    family: temper_client_protocol_wire::FORGE,
                    version: temper_client_protocol_wire::CLIENT_VERSION,
                }],
                requests,
                ask_bytes: requests.bytes / u64::from(requests.requests),
                handles: 9,
                watches: 9,
                turn_bytes: 65536,
                backoff: jig_client::Backoff { first: Duration::from_millis(10), most: Duration::from_millis(80) },
                authority: 12,
                heartbeat: Duration::from_millis(100),
                reads: 8,
                parts: 8,
                delegates: 2,
                document_bytes: 65536,
                records: 2,
                kept: 4,
                projects: 4,
                text: 64,
                turn_text: 64,
                rows: 4,
                fields: 4,
                segments: 9,
            },
            exchanges: 8,
        };

        Limits {
            forge: forge::Limits { field_bytes: 256, checks: 8 },
            client,
            accounts: temper_client_domain::limits::AccountLimits { accounts: 16, sources: 8, uses: 8, sign_ins: 4 },
        }
    }
}

type Books = view::words::Words;
static BOOKS: OnceLock<Mutex<BTreeMap<u32, Books>>> = OnceLock::new();
pub fn phrasebooks(limits: &Limits) -> Books {
    let mut books = BOOKS.get_or_init(|| Mutex::new(BTreeMap::new())).lock().expect("fixture books lock");
    books
        .entry(limits.client.domain.text)
        .or_insert_with(|| {
            view::words::Words::english(FORGE, limits.client.domain.text, view::words::Shell::Terminal)
                .expect("temper words fit")
        })
        .clone()
}
#[must_use]
pub fn view_bounds(limits: &Limits, words: &view::words::Words) -> view::Limits {
    view::Limits::of(limits, words).expect("world view bound fits")
}
#[must_use]
pub fn inbox() -> temper_client_domain::Address {
    temper_client_domain::Address::Inbox { filter: temper_client_domain::address::Filter::ALL }
}
#[must_use]
pub fn world(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::new(seed, domain_limits()))
}
#[must_use]
pub fn open_world(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::open_chats(seed, domain_limits()))
}
#[must_use]
pub fn refusing_world(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::open_chats_on_a_program(seed, domain_limits()))
}
#[must_use]
pub fn world_with_large_turns(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::with_large_turns(seed, &domain_limits()))
}
#[must_use]
pub fn world_with_long_turns(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::with_long_turns(seed, &domain_limits()))
}

fn world_with(seed: u64, front: Joint) -> World<Temper, Joint> {
    let mut world = World::new(
        Settings {
            seed,
            words: view::words::jig_language(),
            heap_bytes_per_client: 64 * 1024 * 1024,
            iterations: 4096,
            ..Settings::default()
        },
        front,
    );
    world.randomness(2, 0, 0);
    world
}
#[must_use]
pub fn codec() -> jig_protocol_transcript::Limits {
    jig_protocol_transcript::Limits::new(
        32768,
        jig_client::contract::TurnLimits { messages: 32, blocks: 32, text: 8192 },
    )
}

#[must_use]
pub fn chats_page() -> temper_client_domain::Address {
    temper_client_domain::Address::Chats { filter: temper_client_domain::ChatsFilter::All, start: false }
}
#[must_use]
pub fn chats(driver: &Driver) -> u32 {
    u32::try_from(
        driver
            .store
            .rows
            .values()
            .filter(|record| {
                matches!(
                    record,
                    engine::Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Live(row) | tasks::Stored::Ended(row)))
                    if matches!(row.requester, tasks::Party::Person(_)) && row.tracked.is_none()
                )
            })
            .count(),
    )
    .expect("bounded chats")
}
#[must_use]
pub fn faults(seed: u64) -> Vec<(Time, FrontFault)> {
    let choices = [
        FrontFault::Restart { offline: Duration::from_millis(30) },
        FrontFault::NoAnswer,
        FrontFault::DropWatch,
        FrontFault::Busy,
        FrontFault::SignedOut,
    ];
    (0..4)
        .map(|index| {
            (
                Time::from_nanos(Duration::from_millis(20 * (index + 1)).as_nanos()),
                choices[usize::try_from((seed + index) % 5).expect("five faults")].clone(),
            )
        })
        .collect()
}
/// A chat made and held by two failed real worker attempts.
pub fn held_chat(front: &mut Joint) -> u64 {
    let driver = &mut front.engine;
    driver.hello(Token::new(7));
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(Token::new(1001)),
        sign_in: driver.session(),
        key: [31; 16],
        ask: temper_engine_domain_world::walking::chat(1, b"Held chat"),
    }));
    driver.settle();
    let first = driver.last_assignment();
    driver.answer(
        Token::new(7),
        first.task,
        first.attempt,
        0,
        tasks::End::Failed(tasks::Class::Run),
        engine::Parts::none(),
    );
    driver.settle();
    driver.env.now = Time::from_nanos(Duration::from_secs(2).as_nanos());
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let second = driver.last_assignment();
    driver.answer(
        Token::new(7),
        second.task,
        second.attempt,
        0,
        tasks::End::Failed(tasks::Class::Run),
        engine::Parts::none(),
    );
    driver.settle();
    assert!(matches!(driver.task(first.task).expect("held chat").phase, tasks::Phase::Held { .. }));
    first.task
}

/// The stored ended task a client's one-shot task/result read asks for.
#[must_use]
pub fn read(
    store: &temper_engine_domain_world::store::Store,
    range: &jig_domain_core::reads::Range,
) -> jig_domain_core::reads::Loaded {
    use jig_domain_core::{Key, Record, reads};
    let (reads::Scope::Task { task } | reads::Scope::Result { task } | reads::Scope::PersonTask { task }) = range.scope
    else {
        return reads::Loaded::Gone;
    };
    let Some(engine::Record::Core(Record::Tasks(tasks::Stored::Ended(row)))) =
        store.rows.get(&engine::Key::Core(Key::Tasks(tasks::Key::Ended(task))))
    else {
        return reads::Loaded::Gone;
    };
    let value = reads::Row {
        value: Record::Tasks(tasks::Stored::Ended(row.clone())),
        ended_read: None,
        owner: Some(row.clone()),
        ancestors: Box::new([]),
        related: None,
        unjoined: None,
        linked: Box::new([]),
        inbox: None,
    };
    if range.after.is_some() {
        return reads::Loaded::Page { rows: Box::new([]), next: None };
    }
    if range.rows == 0 || value.owned_bytes().expect("finite ended row") > range.bytes {
        return reads::Loaded::Busy;
    }
    reads::Loaded::Page { rows: Box::new([value]), next: None }
}
#[must_use]
pub fn project() -> temper_client_domain::Address {
    temper_client_domain::Address::Inbox { filter: temper_client_domain::address::Filter::ALL }
}
#[must_use]
pub fn adopt(owner: &[u8], name: &[u8]) -> Vec<skein_fake_person::Step<temper_client_domain::Address>> {
    use skein_fake_person::{Find, Step};
    use skein_ui::Role;
    vec![
        Step::See { find: Find::named(Role::Link, b"Adopt a repository"), within: WITHIN },
        Step::Press { find: Find::named(Role::Link, b"Adopt a repository") },
        Step::See { find: Find::named(Role::TextField, b"Owner"), within: WITHIN },
        Step::Type { find: Find::named(Role::TextField, b"Owner"), words: owner.into() },
        Step::Type { find: Find::named(Role::TextField, b"Name"), words: name.into() },
        Step::Type { find: Find::named(Role::TextField, b"Landing branch"), words: b"main".as_slice().into() },
        Step::Press { find: Find::named(Role::Button, b"Adopt") },
    ]
}
#[must_use]
pub fn adopt_and_see_outcome(owner: &[u8], name: &[u8]) -> Vec<skein_fake_person::Step<temper_client_domain::Address>> {
    use skein_fake_person::{Find, Step};
    let mut steps = adopt(owner, name);
    steps.extend([Step::See { find: Find::text(b"Adopted acme/shop on forge 1 as owned."), within: WITHIN }]);
    steps
}

const QUESTION_CHAT_REPLY: Token = Token::new(1002);
const QUESTION_CHAT_KEY: [u8; 16] = [32; 16];
pub(crate) const QUESTION_WORKER: Token = Token::new(7);
pub(crate) const QUESTION_PROJECT: u32 = 1;
const QUESTION_CALL: u64 = 1003;
const QUESTION_DELEGATE: u64 = 1004;
const QUESTION_COMPLETION: u32 = 1;
const QUESTION_POSITION: u32 = 1;
const QUESTION_ASSIGN_AT: Time = Time::from_nanos(2_000_000_000);
/// A question committed by the running chat for its person.
pub fn question(front: &mut Joint, words: &[u8]) -> u64 {
    *front = Joint::with_questions(SEED, &domain_limits());
    let driver = &mut front.engine;
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(QUESTION_CHAT_REPLY),
        sign_in: driver.session(),
        key: QUESTION_CHAT_KEY,
        ask: driver.chat_ask(QUESTION_PROJECT, b"Question chat"),
    }));
    driver.settle();
    let parent = driver.last_assignment();
    driver.call(
        QUESTION_WORKER,
        parent.task,
        parent.attempt,
        QUESTION_DELEGATE,
        engine::Call {
            completion: QUESTION_COMPLETION,
            position: QUESTION_POSITION,
            tool: engine::Tool::Delegate {
                batch: Box::new([temper_engine_domain_world::fixtures::report_delegate(
                    b"Ask the person",
                    Box::new([]),
                )]),
            },
        },
    );
    driver.settle();
    driver.answer(QUESTION_WORKER, parent.task, parent.attempt, 0, tasks::End::Parked, engine::Parts::none());
    driver.settle();
    driver.env.now = QUESTION_ASSIGN_AT;
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    let run = driver.last_assignment();
    assert_ne!(run.task, parent.task, "delegate assigned: {:?}", driver.answered(QUESTION_DELEGATE));
    driver.call(
        QUESTION_WORKER,
        run.task,
        run.attempt,
        QUESTION_CALL,
        engine::Call {
            completion: QUESTION_COMPLETION,
            position: QUESTION_POSITION,
            tool: engine::Tool::Message {
                target: parent.task,
                form: engine::MessageForm::Question,
                words: words.into(),
            },
        },
    );
    driver.settle();
    assert!(
        matches!(driver.answered(QUESTION_CALL), Some(engine::CallAnswer::Sent { .. })),
        "question committed: {:?}",
        driver.answered(QUESTION_CALL)
    );
    parent.task
}
#[test]
fn the_question_fixture_commits_its_numbered_question_before_the_client_starts() {
    let mut front = Joint::new(SEED, domain_limits());
    let task = question(&mut front, b"Which provider first?");
    assert!(
        front
            .engine
            .task(task)
            .expect("chat")
            .inbox
            .iter()
            .any(|message| message.words.as_ref() == b"Which provider first?")
    );
}

const CHAT_REPLY: Token = Token::new(1005);
const CHAT_KEY: [u8; 16] = [35; 16];
const CHAT_PROJECT: u32 = 1;
/// A real running chat, ready for the person's next words.
pub fn chat(front: &mut Joint, words: &[u8]) -> u64 {
    let driver = &mut front.engine;
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(CHAT_REPLY),
        sign_in: driver.session(),
        key: CHAT_KEY,
        ask: driver.chat_ask(CHAT_PROJECT, words),
    }));
    driver.settle();
    driver.last_assignment().task
}
const FIRST_REPLY_TURN: u32 = 1;
const REPLY_SPEND: u64 = 0;
/// A real committed worker turn puts this chat in the inbox's replies group.
pub fn replied_chat(front: &mut Joint, words: &[u8]) -> u64 {
    let task = chat(front, words);
    let assignment = front.engine.last_assignment();
    let event = temper_engine_domain_world::protocol::turn(
        QUESTION_WORKER,
        task,
        assignment.attempt,
        FIRST_REPLY_TURN,
        REPLY_SPEND,
        None,
        b"reply",
    );
    front.engine.send(event);
    front.engine.settle();
    task
}
/// Finish a replied chat normally, retaining its unread turn beside its Result.
pub fn finish_replied_chat(front: &mut Joint, task: u64) {
    let run = front.engine.task(task).expect("running replied chat");
    front.engine.answer(
        QUESTION_WORKER,
        task,
        run.attempt,
        run.run_spent,
        tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"The chat is complete".as_slice().into() },
            cancel_delegates: false,
        },
        engine::Parts::none(),
    );
    front.engine.settle();
}
#[must_use]
pub fn chat_page(task: u64) -> temper_client_domain::Address {
    temper_client_domain::Address::Chat { task }
}

const GOAL_REPLY: Token = Token::new(1201);
const GOAL_KEY: [u8; 16] = [41; 16];
/// A goal's brief, its page's heading.
pub const GOAL_BRIEF: &[u8] = b"Fix the adopted repository";
const GOAL_PROJECT: u32 = 1;
const GOAL_PRIORITY: u32 = 1;
const GOAL_LABEL: u32 = 2;
const DELEGATE_CALL: u64 = 1202;
const DELEGATE_COMPLETION: u32 = 1;
const DELEGATE_POSITION: u32 = 1;
const RETRY_AT: Time = Time::from_nanos(2_000_000_000);
const NO_SPEND: u64 = 0;
const DELEGATE_SPEND: u64 = 3;
const DELEGATE_PROGRESS: &[u8] = b"Findings in progress";
const DEPENDENT_DELEGATES: u32 = 2;
pub struct Dependents {
    pub goal: u64,
    pub first: u64,
    pub second: u64,
}
/// A tracked goal with two report delegates; the first has spent, the second waits for it.
pub fn goal_with_dependent_delegates(front: &mut Joint) -> Dependents {
    use temper_engine_domain_world::fixtures::report_delegate;
    *front = Joint::with_delegates(SEED, &domain_limits(), DEPENDENT_DELEGATES);
    let goal = make_goal_with_delegation(front, false, Some(DEPENDENT_DELEGATES));
    let driver = &mut front.engine;
    let parent = driver.last_assignment();
    assert_eq!(parent.task, goal);
    driver.call(
        QUESTION_WORKER,
        goal,
        parent.attempt,
        DELEGATE_CALL,
        engine::Call {
            completion: DELEGATE_COMPLETION,
            position: DELEGATE_POSITION,
            tool: engine::Tool::Delegate {
                batch: Box::new([
                    report_delegate(b"Investigate the change", Box::new([])),
                    report_delegate(b"Use the findings", Box::new([engine::Dependency::Batch(0)])),
                ]),
            },
        },
    );
    driver.settle();
    let Some(engine::CallAnswer::Delegated(children)) = driver.answered(DELEGATE_CALL) else {
        panic!("dependent delegates admitted: {:?}", driver.answered(DELEGATE_CALL));
    };
    let dependents = Dependents { goal, first: children[0].task, second: children[1].task };
    driver.answer(QUESTION_WORKER, goal, parent.attempt, NO_SPEND, tasks::End::Parked, engine::Parts::none());
    driver.settle();
    let first = driver.last_assignment();
    assert_eq!(first.task, dependents.first, "first delegate runs before its dependant");
    delegate_turn(front, first.task, None, DELEGATE_PROGRESS);
    dependents
}
/// Commit the running delegate's next turn taking all the person's pending words.
/// Its numbered input fence removes those words from the engine's unread inbox.
pub fn read_messages(front: &mut Joint, task: u64) -> u64 {
    let number = front
        .engine
        .task(task)
        .expect("running delegate")
        .inbox
        .iter()
        .rev()
        .find(|message| message.from == tasks::Party::Person(PARTY))
        .expect("pending person message")
        .number;
    delegate_turn(front, task, Some(number), b"I will follow that direction");
    number
}
/// Push the initial turn just beyond the client's retained activity window.
pub fn extend_delegate_conversation(front: &mut Joint, task: u64) {
    let window = domain_limits().client.domain.rows;
    for _ in 1..window {
        delegate_turn(front, task, None, b"The investigation continues");
    }
    delegate_turn(front, task, None, b"The latest findings");
}
fn delegate_turn(front: &mut Joint, task: u64, read: Option<u64>, words: &[u8]) {
    use smith_domain_session::llm;
    let driver = &mut front.engine;
    let run = driver.task(task).expect("running delegate");
    let attempt = run.attempt;
    let number = run.turn + 1;
    // The initial turn spends three; later turns keep that cumulative total.
    let spent = run.run_spent.max(DELEGATE_SPEND);
    let mut turn =
        temper_worker_domain_world::protocol::session_turn(number, spent, words.len(), number == FIRST_REPLY_TURN);
    turn.messages[0].content = Box::new([llm::Block::Text { text: words.into(), replay: None }]);
    let body = smith_protocol_channel::encode_turn(
        &turn,
        &smith_transcript::CEILINGS,
        &temper_worker_domain_world::protocol::endpoints(),
    )
    .expect("delegate turn encodes");
    driver.send(temper_engine_domain_world::protocol::turn(QUESTION_WORKER, task, attempt, number, spent, read, &body));
    driver.settle();
}
/// The same goal after its first delegate's report unblocks the second.
pub fn goal_with_a_finished_dependency(front: &mut Joint) -> Dependents {
    let dependents = goal_with_dependent_delegates(front);
    report_first_delegate(front, dependents.first);
    assert!(
        front.engine.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Assigned { assignment, .. } if assignment.task == dependents.second
        )),
        "dependant runs after the report"
    );
    dependents
}
/// Finish the running first delegate with its report and settle its spend.
pub fn report_first_delegate(front: &mut Joint, task: u64) {
    report_task(front, task, b"The change is feasible");
}
fn report_task(front: &mut Joint, task: u64, words: &[u8]) {
    let driver = &mut front.engine;
    let run = driver.task(task).expect("reporting run");
    let attempt = run.attempt;
    let spent = run.run_spent;
    driver.answer(
        QUESTION_WORKER,
        task,
        attempt,
        spent,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: words.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    driver.settle();
}
const BUDGET_READ: Token = Token::new(1204);
/// A tracked goal whose worker finished with a report retained by the engine.
pub fn finished_goal(front: &mut Joint) -> u64 {
    *front = Joint::with_questions(SEED, &domain_limits());
    let goal = make_goal(front, false);
    let driver = &mut front.engine;
    let run = driver.last_assignment();
    assert_eq!(run.task, goal);
    driver.answer(
        QUESTION_WORKER,
        goal,
        run.attempt,
        NO_SPEND,
        tasks::End::Finished {
            result: tasks::TaskResult::Report { words: b"The repository is ready".as_slice().into() },
            cancel_delegates: false,
        },
        engine::Parts::none(),
    );
    driver.settle();
    goal
}
/// Read the engine's supplied budget, including availability, without computing it in the story.
/// A point read also serves ended tasks, whose watches have ended (client/README.md, 5.8).
pub fn task_budget(front: &mut Joint, task: u64) -> jig_domain_core::documents::BudgetLine {
    use jig_domain_core::{documents::Document, reads};
    let driver = &mut front.engine;
    // The joint world drains deliveries between phases; the sign-in's durable
    // identity remains available after its initial reply has been consumed.
    let sign_in = driver
        .store
        .rows
        .values()
        .find_map(|record| {
            if let engine::Record::Core(jig_domain_core::Record::People(jig_domain_people::Stored::SignIn {
                number,
                person: PARTY,
                ..
            })) = record
            {
                Some(*number)
            } else {
                None
            }
        })
        .expect("fixture person's durable sign-in");
    driver.send(engine::Event::Party(engine::PartyInput::Read {
        reply_to: skein_lib::ReplyTo::new(BUDGET_READ),
        sign_in,
        read: reads::Read::Task { task },
    }));
    driver.settle();
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            let engine::Delivery::Read { reply, answer } = delivery else { return None };
            if *reply != skein_lib::ReplyTo::new(BUDGET_READ) {
                return None;
            }
            match answer {
                reads::Answer::Document(document) => {
                    if let Document::Task(document) = document.as_ref() {
                        Some(document.budget)
                    } else {
                        None
                    }
                }
                reads::Answer::Historical(historical) => {
                    if let reads::Past::Task { document, .. } = &historical.value {
                        Some(document.budget)
                    } else {
                        None
                    }
                }
                reads::Answer::Setting(_)
                | reads::Answer::Revision(_)
                | reads::Answer::Revisions(_)
                | reads::Answer::Tasks(_)
                | reads::Answer::Page(_)
                | reads::Answer::None(_) => None,
            }
        })
        .expect("goal budget document")
}
/// A tracked goal with a failed delegate, held by the real engine.
pub fn goal_with_held_delegate(front: &mut Joint) -> (u64, u64) {
    *front = Joint::with_questions(SEED, &domain_limits());
    let goal = make_goal(front, false);
    let driver = &mut front.engine;
    let parent = driver.last_assignment();
    assert_eq!(parent.task, goal);
    driver.call(
        QUESTION_WORKER,
        parent.task,
        parent.attempt,
        DELEGATE_CALL,
        engine::Call {
            completion: DELEGATE_COMPLETION,
            position: DELEGATE_POSITION,
            tool: engine::Tool::Delegate {
                batch: Box::new([temper_engine_domain_world::fixtures::report_delegate(
                    b"Review the change",
                    Box::new([]),
                )]),
            },
        },
    );
    driver.settle();
    let Some(engine::CallAnswer::Delegated(children)) = driver.answered(DELEGATE_CALL) else {
        panic!("delegate admitted: {:?}", driver.answered(DELEGATE_CALL));
    };
    let held = children[0].task;
    driver.answer(QUESTION_WORKER, parent.task, parent.attempt, NO_SPEND, tasks::End::Parked, engine::Parts::none());
    driver.settle();
    fail_delegate(driver, held);
    driver.env.now = RETRY_AT;
    engine::fire(&mut driver.root, &driver.env);
    engine::release(&mut driver.root, &driver.env, &mut driver.out);
    driver.collect();
    driver.settle();
    fail_delegate(driver, held);
    assert!(matches!(driver.task(held).expect("delegate").phase, tasks::Phase::Held { .. }));
    (goal, held)
}
fn fail_delegate(driver: &mut Driver, held: u64) {
    let run = driver.last_assignment();
    assert_eq!(run.task, held, "delegate has the next worker attempt");
    driver.answer(
        QUESTION_WORKER,
        held,
        run.attempt,
        NO_SPEND,
        tasks::End::Failed(tasks::Class::Run),
        engine::Parts::none(),
    );
    driver.settle();
}
/// Two tracked roots reserve only themselves, leaving room for each other.
pub fn amend_tasks(front: &mut Joint) -> (u64, u64) {
    const DEPENDENCY_KEY: [u8; 16] = [42; 16];
    const ROOT_BUDGET: u64 = 1; // walking's minimum run spend, leaving funding for both roots.
    let goal = make_goal_named(front, false, Some(0), GOAL_KEY, Some(ROOT_BUDGET));
    let dependency = make_goal_named(front, false, Some(0), DEPENDENCY_KEY, Some(ROOT_BUDGET));
    (goal, dependency)
}
fn make_goal(front: &mut Joint, procedure: bool) -> u64 {
    make_goal_with_delegation(front, procedure, None)
}
fn make_goal_with_delegation(front: &mut Joint, procedure: bool, delegates: Option<u32>) -> u64 {
    make_goal_named(front, procedure, delegates, GOAL_KEY, None)
}
fn make_goal_named(
    front: &mut Joint,
    procedure: bool,
    delegates: Option<u32>,
    key: [u8; 16],
    budget: Option<u64>,
) -> u64 {
    let driver = &mut front.engine;
    driver.sign_in();
    driver.settle();
    if procedure {
        adopt_goal_repository(driver);
    }
    let jig_domain_people::Ask::Create { mut batch, .. } = driver.chat_ask(GOAL_PROJECT, GOAL_BRIEF) else {
        panic!("create fixture");
    };
    if let Some(budget) = budget {
        batch[0].budget = jig_domain_people::Budget::Named;
        batch[0].authority.spend = budget;
    }
    batch[0].tracked = Some(GOAL_PRIORITY);
    batch[0].label = GOAL_LABEL;
    batch[0].flags.conversation = false;
    batch[0].flags.listed = true;
    if let Some(delegates) = delegates {
        batch[0].authority.delegation.tasks = delegates;
    }
    if procedure {
        batch[0].authority.tools = NO_TOOLS;
        batch[0].parameters = Box::new([jig_domain_people::Parameter::Resource {
            name: REPOSITORY_PARAMETER,
            connector: FORGE_CONNECTOR,
            resource: GOAL_REPOSITORY,
        }]);
        batch[0].executor =
            jig_domain_people::TaskExecutor::Procedure { connector: FORGE_CONNECTOR, code: FORGE_PROCEDURE };
        batch[0].contract =
            jig_domain_people::Contract::Change { connector: FORGE_CONNECTOR, kind: FORGE_CHANGE, words: REPORT_BYTES };
    }
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(GOAL_REPLY),
        sign_in: driver.session(),
        key,
        ask: jig_domain_people::Ask::Create { project: GOAL_PROJECT, batch },
    }));
    driver.settle();
    let mut reply = None;
    for delivery in driver.delivered.iter().rev() {
        if let engine::Delivery::WebReply { reply: value, .. } = delivery {
            reply = Some(value);
            break;
        }
    }
    let Some(jig_domain_people::Reply::Outcome(jig_domain_people::Outcome::Created { first, .. })) = reply else {
        panic!("goal creation expected: {reply:?}");
    };
    *first
}
#[must_use]
pub fn task_page(task: u64) -> temper_client_domain::Address {
    temper_client_domain::Address::Task { task, filter: temper_client_domain::PlanFilter::All }
}
#[must_use]
pub fn plan_item(task: u64, status: &[u8]) -> skein_fake_person::Find {
    use skein_fake_person::{Find, Target};
    Find::named(skein_ui::Role::Status, status).within(Box::new([Target::ExactRole {
        role: skein_ui::Role::TreeItem,
        name: format!("Task {task}").into_bytes().into_boxed_slice(),
    }]))
}
#[must_use]
pub fn dependency_item(task: u64, find: skein_fake_person::Find) -> skein_fake_person::Find {
    use skein_fake_person::Target;
    find.within(Box::new([
        Target::ExactRole { role: skein_ui::Role::TreeItem, name: format!("Task {task}").into_bytes().into() },
        Target::ExactRole {
            role: skein_ui::Role::List,
            name: format!("Dependencies of task {task}").into_bytes().into(),
        },
    ]))
}
const FORGE_CONNECTOR: u16 = 1;
const NO_TOOLS: u64 = 0;
const FORGE_PROCEDURE: u32 = 2;
const FORGE_CHANGE: u16 = 1;
const REPORT_BYTES: u32 = 32;
/// A tracked procedure root whose chip names its forge-owned panel.
pub fn goal_with_forge_panel(front: &mut Joint) -> u64 {
    *front = Joint::with_procedures(SEED, &domain_limits());
    make_goal(front, true)
}

/// Adopt before pushing under temper's prefix: adoption refuses an unowned branch there.
pub fn adopt_change_repository(front: &mut Joint) {
    front.engine.sign_in();
    front.engine.settle();
    adopt_goal_repository(&mut front.engine);
    // forge_routes.rs's worker also receives the repository's application credential.
    front.engine.send(engine::Event::Credential { number: 1, generation: 1, valid: Duration::from_secs(3600) });
    front.engine.settle();
    assert!(front.engine_adopted(ADOPT_FORGE, GOAL_REPOSITORY_ID));
}

/// A tracked change on the landing joint, handed a branch already pushed on its stand.
/// The repository and sign-in are prepared by `adopt_change_repository`.
/// Leaves the joint's immediate work for the story to drive one step at a time.
pub fn goal_with_pushed_change(front: &mut Joint, branch: &[u8]) -> u64 {
    let driver = &mut front.engine;
    let jig_domain_people::Ask::Create { mut batch, .. } = driver.chat_ask(GOAL_PROJECT, GOAL_BRIEF) else {
        panic!("create fixture");
    };
    // These grants come from this joint's configured forge connector.
    let connector = batch[0].authority.grants[0].connector;
    batch[0].tracked = Some(GOAL_PRIORITY);
    batch[0].label = GOAL_LABEL;
    batch[0].flags.conversation = false;
    batch[0].flags.listed = true;
    batch[0].authority.tools = NO_TOOLS;
    batch[0].parameters = Box::new([
        jig_domain_people::Parameter::Resource { name: REPOSITORY_PARAMETER, connector, resource: GOAL_REPOSITORY },
        jig_domain_people::Parameter::Bytes { name: 3, value: branch.into() },
    ]);
    batch[0].executor = jig_domain_people::TaskExecutor::Procedure { connector, code: FORGE_PROCEDURE };
    batch[0].contract = jig_domain_people::Contract::Change { connector, kind: FORGE_CHANGE, words: REPORT_BYTES };
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(GOAL_REPLY),
        sign_in: driver.session(),
        key: GOAL_KEY,
        ask: jig_domain_people::Ask::Create { project: GOAL_PROJECT, batch },
    }));
    // Commit creation, leaving subsequent procedure work visible to the caller.
    for _ in 0..200 {
        driver.advance(true);
        if let Some(task) = driver.delivered.iter().find_map(|delivery| {
            if let engine::Delivery::WebReply {
                to,
                reply: jig_domain_people::Reply::Outcome(jig_domain_people::Outcome::Created { first, .. }),
            } = delivery
                && *to == skein_lib::ReplyTo::new(GOAL_REPLY)
            {
                Some(*first)
            } else {
                None
            }
        }) {
            return task;
        }
    }
    panic!("pushed change creation: {:?}", driver.delivered);
}

const ADOPT_FORGE: u16 = 1;
const ADOPT_REPLY: Token = Token::new(1203);
const ADOPT_KEY: [u8; 16] = [43; 16];
const ADOPT_DIGEST: [u8; 32] = [43; 32];
const REPOSITORY_PARAMETER: u32 = 1;
const GOAL_REPOSITORY_ID: u32 = 7;
// A task names its repository by the forge and id, packed (domain/forge.md, 4).
const GOAL_REPOSITORY: u64 = temper_engine_domain_forge::tag(ADOPT_FORGE, GOAL_REPOSITORY_ID);
fn adopt_goal_repository(driver: &mut Driver) {
    let mut request = temper_engine_domain_world::fixtures::adoption(ADOPT_FORGE);
    request.owner = b"acme".as_slice().into();
    request.name = b"shop".as_slice().into();
    driver.send(engine::Event::Party(engine::PartyInput::Application {
        scope: jig_domain_people::ApplicationScope::Project(GOAL_PROJECT),
        reply_to: skein_lib::ReplyTo::new(ADOPT_REPLY),
        sign_in: driver.session(),
        key: ADOPT_KEY,
        digest: ADOPT_DIGEST,
        carried: None,
        request: temper_engine_domain::Application::Forge(temper_engine_domain_forge::Ask::Adopt(request)),
    }));
    driver.settle();
}

/// Guard on one-step answer waits, never a client limit.
const ANSWER_STEPS: u32 = 4096;
#[must_use]
pub fn agent(world: &mut World<Temper, Joint>) -> jig_client_world::harness::ClientId {
    let limits = temper_client_tools::limits::client();
    world.front().next_client_limits(limits);
    world.client_with(PARTY, inbox(), limits, crate::agent::new)
}
#[must_use]
pub fn call(words: &[&[u8]]) -> crate::app::Event {
    use std::os::unix::ffi::OsStrExt;
    match temper_client_tools::table::parse(words).unwrap_or_else(|error| panic!("agent call usage: {error:?}")) {
        temper_client_tools::jig::call::Parsed::Call { mut call, json } => {
            // The world supplies the host's already-loaded amendment text.
            if temper_client_tools::table::TOOLS[usize::from(call.tool)].name == b"amend" {
                for slot in &mut call.values {
                    if let Some(temper_client_tools::jig::call::Values::One(
                        temper_client_tools::jig::call::Value::File(path),
                    )) = slot
                    {
                        let bytes = std::fs::read(std::path::Path::new(std::ffi::OsStr::from_bytes(path)))
                            .expect("world's text file");
                        let stripped = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                        *slot = Some(temper_client_tools::jig::call::Values::One(
                            temper_client_tools::jig::call::Value::Text(stripped.into()),
                        ));
                    }
                }
            }
            crate::app::Event::Call { number: 0, call, json, value: None }
        }
        temper_client_tools::jig::call::Parsed::File { file, json } => {
            let temper_client_tools::jig::call::Source::Path(path) = &file else { panic!("world call needs a file") };
            let bytes =
                std::fs::read(std::path::Path::new(std::ffi::OsStr::from_bytes(path))).expect("world's call file");
            let call = temper_client_tools::jig::json::call(
                temper_client_tools::table::TOOLS,
                &bytes,
                file,
                &temper_client_tools::limits::tools(),
            )
            .expect("world's JSON call");
            crate::app::Event::Call { number: 0, call, json, value: None }
        }
    }
}
pub fn give(world: &mut World<Temper, Joint>, agent: jig_client_world::harness::ClientId, words: &[&[u8]]) -> u32 {
    use jig_client_world::harness::Until;
    let start = world.now();
    for _ in 0..ANSWER_STEPS {
        if world.root(agent).agent().ready() && world.root(agent).agent().kept() == 0 {
            break;
        }
        world.run(Until::Steps(2));
    }
    assert!(world.now() <= start.saturating_add(WITHIN), "agent ready within world time");
    assert!(world.root(agent).agent().ready() && world.root(agent).agent().kept() == 0, "agent ready before call");
    let number = world.root(agent).agent().calls() + 1;
    let crate::app::Event::Call { call, json, .. } = call(words) else { unreachable!() };
    world.event(agent, crate::app::Event::Call { number, call, json, value: None });
    number
}
pub fn answered(
    world: &mut World<Temper, Joint>,
    agent: jig_client_world::harness::ClientId,
    number: u32,
) -> temper_client_tools::jig::Printed {
    use jig_client_world::harness::Until;
    let start = world.now();
    for _ in 0..ANSWER_STEPS {
        let outcome = world.run(Until::Steps(2));
        if let Some(printed) = world.root(agent).agent().printed(number) {
            assert_eq!(outcome.verdict, skein_world::domain::Verdict::Passed, "{outcome:?}");
            assert!(world.now() <= start.saturating_add(WITHIN), "agent answered within world time");
            return temper_client_tools::jig::Printed {
                file: temper_client_tools::jig::FileEnd::Keep,
                status: printed.status,
                stdout: printed.stdout.clone(),
                stderr: printed.stderr.clone(),
            };
        }
    }
    panic!("agent call {number} was not answered");
}
#[must_use]
pub fn done_task(printed: &temper_client_tools::jig::Printed) -> u64 {
    let stdout = std::str::from_utf8(&printed.stdout).expect("printed UTF-8");
    stdout
        .strip_prefix("done: task ")
        .and_then(|value| value.split_once(", "))
        .expect("done task outcome")
        .0
        .parse()
        .expect("task number")
}

/// tools.md, 12: two 16 KiB parts within the tools' 32 KiB text bound.
pub const LARGE_TURN_TEXT: usize = 24_576;
#[must_use]
pub fn codec_for(limits: &jig_client::DomainLimits) -> jig_protocol_transcript::Limits {
    let bytes = limits.document_bytes.min(u64::from(jig_client_protocol_wire::CEILINGS.turn_carried_bytes));
    jig_protocol_transcript::Limits::new(
        u32::try_from(bytes).expect("wire bound fits"),
        jig_client::contract::TurnLimits { messages: limits.rows, blocks: limits.rows, text: limits.turn_text },
    )
}
/// Commit smith's session turn through the worker's real turn boundary.
pub fn large_turn(front: &mut Joint, task: u64) -> (u64, u32) {
    use jig_domain_core::{CoreRecord, Record, TurnRecord};
    let empty = large_turn_body(0);
    let per_byte = large_turn_body(1).len().checked_sub(empty.len()).expect("opaque padding grows the encoding");
    let mut row = turn_read_row(
        front,
        task,
        Record::Core(CoreRecord::Turn(TurnRecord {
            opens_window: true,
            task,
            attempt: front.engine.last_assignment().attempt,
            turn: FIRST_REPLY_TURN,
            spent: REPLY_SPEND,
            read: None,
            at: NOW,
            transcript: empty,
        })),
    );
    // The first committed turn also adds its retained reply hint to the owner.
    let owner = row.owner.as_mut().expect("turn owner");
    assert!(owner.replies.is_empty(), "the fixture commits its first reply");
    owner.replies = Box::new([tasks::ReplyHint {
        number: owner.last_reply.checked_add(1).expect("reply ordinal"),
        activity: owner.activity.checked_add(1).expect("reply activity"),
        attempt: front.engine.last_assignment().attempt,
        turn: FIRST_REPLY_TURN,
        at: NOW,
    }]);
    // Keep all visible text; only opaque padding fills the remaining whole-row budget.
    let room = front
        .engine
        .env
        .limits
        .loads
        .read_bytes
        .checked_sub(row.owned_bytes().expect("zero-padding row ownership"))
        .expect("visible turn and its owner fit the read");
    let padding = usize::try_from(room).expect("fixture read fits usize") / per_byte;
    large_turn_with_opaque(front, task, padding)
}
/// Commit a turn with a separately sized opaque reasoning item.
pub fn large_turn_with_opaque(front: &mut Joint, task: u64, opaque: usize) -> (u64, u32) {
    let assignment = front.engine.last_assignment();
    let body = large_turn_body(opaque);
    front.engine.send(temper_engine_domain_world::protocol::turn(
        QUESTION_WORKER,
        task,
        assignment.attempt,
        FIRST_REPLY_TURN,
        REPLY_SPEND,
        None,
        &body,
    ));
    front.engine.settle();
    (assignment.attempt, FIRST_REPLY_TURN)
}
fn large_turn_body(opaque: usize) -> Box<[u8]> {
    use smith_domain_session::llm;
    let mut turn =
        temper_worker_domain_world::protocol::session_turn(FIRST_REPLY_TURN, REPLY_SPEND, LARGE_TURN_TEXT, true);
    for message in &mut turn.messages {
        let mut blocks = message.content.to_vec();
        blocks.push(llm::Block::Opaque { bytes: vec![0; opaque].into_boxed_slice() });
        message.content = blocks.into_boxed_slice();
    }
    smith_protocol_channel::encode_turn(
        &turn,
        &smith_transcript::CEILINGS,
        &temper_worker_domain_world::protocol::endpoints(),
    )
    .expect("large turn encodes")
}
/// Complete ownership of a turn alone and joined to its activity row.
#[must_use]
pub fn turn_row_costs(front: &Joint, task: u64) -> (u64, u64) {
    use jig_domain_core as core;
    let turn = front
        .engine
        .store
        .rows
        .values()
        .find_map(|record| {
            if let engine::Record::Core(value @ core::Record::Core(core::CoreRecord::Turn(turn))) = record
                && turn.task == task
            {
                Some(value.clone())
            } else {
                None
            }
        })
        .expect("durable turn");
    let mut row = turn_read_row(front, task, turn);
    let single = row.owned_bytes().expect("single row cost");
    let line = front
        .engine
        .store
        .rows
        .values()
        .find_map(|record| {
            if let engine::Record::Core(core::Record::Tasks(tasks::Stored::Activity(line))) = record
                && line.task == task
                && matches!(line.line, tasks::Line::Turn { .. })
            {
                Some(line.clone())
            } else {
                None
            }
        })
        .expect("durable turn activity");
    row.related = Some(row.value);
    row.value = core::Record::Tasks(tasks::Stored::Activity(line));
    (single, row.owned_bytes().expect("activity row cost"))
}
fn turn_read_row(front: &Joint, task: u64, value: jig_domain_core::Record) -> jig_domain_core::reads::Row {
    jig_domain_core::reads::Row {
        value,
        owner: Some(Box::new(front.engine.task(task).expect("live turn owner").clone())),
        ancestors: Box::new([]),
        related: None,
        unjoined: None,
        ended_read: None,
        linked: Box::new([]),
        inbox: None,
    }
}
/// Follow the printed continuation calls, retaining the turn's text alone.
pub fn read_whole(
    world: &mut World<Temper, Joint>,
    agent: jig_client_world::harness::ClientId,
    words: &[&[u8]],
) -> (Vec<u8>, u32) {
    let mut words: Vec<Box<[u8]>> = words.iter().map(|word| Box::from(*word)).collect();
    let mut text = Vec::new();
    let mut reads = 0;
    loop {
        let borrowed: Vec<&[u8]> = words.iter().map(AsRef::as_ref).collect();
        let number = give(world, agent, &borrowed);
        let printed = answered(world, agent, number);
        assert_eq!(printed.status, 0, "{printed:?}");
        reads += 1;
        let stdout = std::str::from_utf8(&printed.stdout).expect("turn text UTF-8");
        let mut next = None;
        for line in stdout.lines() {
            if let Some((_, call)) = line.split_once("read on with: ").or_else(|| line.split_once("read with: ")) {
                next = Some(call.split_ascii_whitespace().map(|word| Box::from(word.as_bytes())).collect());
            } else if !line.starts_with("turn ") && !line.starts_with("opaque ") {
                text.extend_from_slice(line.as_bytes());
            }
        }
        let Some(next) = next else { return (text, reads) };
        words = next;
    }
}

/// Copy an executable act from the first acts section of a printed document.
#[must_use]
pub fn printed_act(stdout: &[u8], act: &[u8]) -> Vec<Box<[u8]>> {
    let stdout = std::str::from_utf8(stdout).expect("printed document UTF-8");
    let act = std::str::from_utf8(act).expect("act UTF-8");
    let line = stdout
        .lines()
        .skip_while(|line| *line != "acts:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .find(|line| line.split_ascii_whitespace().next() == Some(act))
        .expect("printed act");
    line.split_ascii_whitespace().map(|word| Box::from(word.as_bytes())).collect()
}

/// Cut the answer after application, before the client's first retry.
pub fn cut(world: &mut World<Temper, Joint>, agent: jig_client_world::harness::ClientId, words: &[&[u8]]) {
    use jig_client_world::harness::Until;
    world.fault(FrontFault::NoAnswer);
    give(world, agent, words);
    let start = world.now();
    let first_retry = start.saturating_add(temper_client_tools::limits::client().client.domain.requests.first);
    for _ in 0..ANSWER_STEPS {
        world.run(Until::Steps(2));
        if world.front().answers_to_lose() == 0 {
            assert!(start <= world.now() && world.now() < first_retry, "answer cut before first retry");
            return;
        }
    }
    panic!("answer was not cut");
}

#[must_use]
pub fn model_world(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::with_model_connections(seed, domain_limits()))
}

#[must_use]
pub fn accounts_world(seed: u64) -> World<Temper, Joint> {
    world_with(seed, Joint::with_accounts(seed, domain_limits()))
}
