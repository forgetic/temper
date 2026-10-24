use jig_domain_accounts as accounts;
use jig_domain_core as core;
use jig_domain_people as people;
use skein_lib::{Duration, ReplyTo, Token};
use temper_engine_domain::{self as engine, Delivery};
use temper_engine_domain_forge as forge;
use temper_engine_domain_world::{direct::Driver, fixtures, forge_stand::ForgeStand, store::Store, walking};

fn config(under_box: bool) -> engine::Config {
    let mut config = walking::config(71);
    config.under_box = under_box;
    config.core.defaults.templates[0].budget = people::TemplateBudget::AsAllowed { cap: Some(200) };
    let mut rules = config.core.defaults.rules.clone();
    rules.ceiling.delegation.depth = 2;
    config.core.seed_projects[0].policy.policy.ceiling.delegation.depth = 2;
    config.core.seed_projects[0].policy.policy.roles[0].authority.delegation.depth = 2;
    config.core.defaults.rules = rules;
    config.core.grants = Box::new([people::ActGrant {
        connector: config.core.connectors[0],
        may: Box::new([
            people::ActKind::Close,
            people::ActKind::AddKey,
            people::ActKind::RemoveAccount,
            people::ActKind::Adopt,
        ]),
    }]);
    config.forges = Box::new([
        forge::Configured {
            admin_connect: false,
            bot: Box::new([]),
            owned: None,
            web: None,
            read_only: false,
            number: 1,
            kind: forge::Kind::Forgejo,
            mode: forge::Mode::Fake,
            asked: true,
        },
        forge::Configured {
            admin_connect: false,
            bot: Box::new([]),
            owned: None,
            web: None,
            read_only: false,
            number: 2,
            kind: forge::Kind::Forgejo,
            mode: forge::Mode::Fake,
            asked: false,
        },
    ]);
    config.demo =
        Some(forge::DemoConfig { provider: 1, adoption: fixtures::adoption(1), deadline: Duration::from_secs(10) });
    config
}
fn driver(connected: bool, under_box: bool) -> (Driver, u64) {
    driver_grant(connected, under_box, true)
}
fn driver_grant(connected: bool, under_box: bool, granted: bool) -> (Driver, u64) {
    let mut limits = fixtures::forge_limits();
    limits.tasks.depth = 2;
    limits.accounts.accounts = 2;
    limits.fleet.grants = limits.accounts.credentials + limits.accounts.accounts;
    limits.people.act_grants = 1;
    // Two starts, one account reply awaiting reclaim, and the adoption's admission.
    limits.people.pending = 4;
    limits.people.requests = 8;
    limits.journal.writes = walking::writes(&limits) + 4;
    limits.journal.deliveries += 4;
    limits.journal.held += 12;
    let configuration = std::array::from_fn(|_| {
        let mut configuration = config(under_box);
        if !granted {
            configuration.core.grants = Box::new([]);
        }
        configuration
    });
    let [bootstrap, configuration] = configuration;
    let store = fixtures::first_start_over(Store::new(), bootstrap, &limits);
    let mut d = Driver::cold(store, configuration, &limits);
    d.forge = Some(ForgeStand::new());
    d.settle();
    d.sign_in();
    d.settle();
    if connected {
        connection(&mut d, forge::Connection::Connected { id: 1, login: b"temper".as_slice().into() });
    }
    let sign_in = d.session();
    d.delivered.clear();
    (d, sign_in)
}
fn connection(d: &mut Driver, state: forge::Connection) {
    d.send(engine::Event::Forge(Box::new(forge::Event::Connection { forge: 1, state })));
    d.settle();
}
fn start(d: &mut Driver, sign_in: u64, number: u8, carry: bool) {
    d.send(engine::Event::Party(engine::PartyInput::Application {
        reply_to: ReplyTo::new(Token::new(u64::from(number))),
        sign_in,
        key: [number; 16],
        digest: [9; 32],
        scope: people::ApplicationScope::Project(999),
        request: engine::Application::Forge(forge::Ask::StartDemo),
        carried: carry.then_some(core::Carried { number: u64::from(number), digest: [7; 32] }),
    }));
    d.settle();
}
fn answers(d: &Driver) -> Vec<Option<engine::ApplicationAnswer>> {
    d.delivered
        .iter()
        .filter_map(
            |delivery| {
                if let Delivery::Application { outcome, .. } = delivery { Some(outcome.clone()) } else { None }
            },
        )
        .collect()
}
fn keeps(d: &Driver) -> usize {
    d.protocol
        .accounts
        .iter()
        .filter(|request| matches!(request, accounts::Request::Signing(accounts::Signing::Keep { .. })))
        .count()
}

#[test]
fn demo_start_adds_the_deployments_account_and_adopts_the_repository_once() {
    let (mut d, sign_in) = driver(true, false);
    start(&mut d, sign_in, 81, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!(keeps(&d), 1);
    assert!(d.store.rows.values().any(|record| matches!(record, engine::Record::Forge { row }
        if matches!(row.as_ref(), forge::Stored::Repository(row)
            if row.project == 1 && row.owner.as_ref() == b"org" && row.name.as_ref() == b"repo"))));
    assert!(d.store.rows.values().any(|record| matches!(
        record,
        engine::Record::Core(core::Record::Accounts(accounts::Stored::Account {
            holder: accounts::Holder::Deployment,
            provider: 1,
            ..
        }))
    )));
    let calls = d.forge.as_ref().expect("fake forge").calls;
    d.delivered.clear();
    start(&mut d, sign_in, 82, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!((keeps(&d), d.forge.as_ref().expect("fake forge").calls), (1, calls));
    assert!(matches!(d.delivered.first(), Some(Delivery::CarriedUnused { number: 82 })));
}

#[test]
fn demo_start_replays_its_retained_answer_without_repeating_acts() {
    let (mut d, sign_in) = driver(true, false);
    start(&mut d, sign_in, 81, true);
    let calls = d.forge.as_ref().expect("fake forge").calls;
    d.delivered.clear();
    start(&mut d, sign_in, 81, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!(keeps(&d), 1);
    assert_eq!(d.forge.as_ref().expect("fake forge").calls, calls);
    assert!(matches!(d.delivered.first(), Some(Delivery::CarriedUnused { number: 81 })));
}

#[test]
fn demo_start_fails_when_its_account_is_refused_or_the_key_is_missing() {
    for carry in [false, true] {
        let (mut d, sign_in) = driver(true, false);
        d.protocol.key_failure = Some(accounts::KeyFailure::NotStored);
        start(&mut d, sign_in, 81, carry);
        assert_eq!(
            answers(&d),
            [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Failed(forge::StartFailure::Account)))]
        );
        assert_eq!(d.forge.as_ref().expect("fake forge").calls, 0);
    }
}

#[test]
fn demo_start_fails_when_its_adoption_is_refused() {
    let (mut d, sign_in) = driver(true, false);
    d.forge = Some(ForgeStand::with(&[(b"org", b"another", 3)]));
    start(&mut d, sign_in, 81, true);
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Failed(forge::StartFailure::Adoption)))]
    );
    assert_eq!(keeps(&d), 1);
}

#[test]
fn demo_start_names_the_groups_failure_and_truncates_its_reason() {
    let (mut d, sign_in) = driver(false, true);
    start(&mut d, sign_in, 81, true);
    assert!(answers(&d).is_empty());
    assert!(d.immediate.iter().any(|request| matches!(request, engine::Request::SetUpGroup { forge: 1, .. })));
    connection(&mut d, forge::Connection::SetUpFailed { why: vec![b'x'; 300].into_boxed_slice() });
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Failed(forge::StartFailure::Group {
            why: vec![b'x'; 256].into_boxed_slice()
        })))]
    );
}

#[test]
fn demo_start_waits_for_connection_until_the_configured_deadline() {
    let (mut d, sign_in) = driver(false, false);
    start(&mut d, sign_in, 81, true);
    assert!(answers(&d).is_empty());
    d.fire_at(Duration::from_secs(10));
    d.settle();
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Failed(forge::StartFailure::NotConnected)))]
    );
}

#[test]
fn demo_restart_asks_the_group_makes_no_act_and_forgets_the_start_answer() {
    let (mut d, sign_in) = driver(true, true);
    start(&mut d, sign_in, 81, true);
    let old_keeps = keeps(&d);
    let old_calls = d.forge.as_ref().expect("fake forge").calls;
    d.root = engine::Domain::new(config(true), &d.env.limits).expect("valid demo config");
    d.immediate.clear();
    d.delivered.clear();
    d.send(engine::Event::Restart);
    d.settle();
    assert!(d.immediate.iter().any(|request| matches!(request, engine::Request::SetUpGroup { forge: 1, .. })));
    assert_eq!((keeps(&d), d.forge.as_ref().expect("fake forge").calls), (old_keeps, old_calls));
    d.sign_in();
    d.settle();
    let sign_in = d.session();
    d.delivered.clear();
    start(&mut d, sign_in, 81, true);
    assert_eq!(answers(&d), [None]);
}

#[test]
fn demo_start_skips_an_existing_account_but_still_adopts() {
    let (mut d, sign_in) = driver(false, false);
    start(&mut d, sign_in, 81, true);
    d.fire_at(Duration::from_secs(10));
    d.settle();
    connection(&mut d, forge::Connection::Connected { id: 1, login: b"temper".as_slice().into() });
    d.delivered.clear();
    start(&mut d, sign_in, 82, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!(keeps(&d), 1);
    assert!(d.delivered.iter().any(|delivery| matches!(delivery, Delivery::CarriedUnused { number: 82 })));
    assert!(d.store.rows.values().any(|record| matches!(record, engine::Record::Forge { row }
        if matches!(row.as_ref(), forge::Stored::Repository(_)))));
}

#[test]
fn demo_start_skips_an_existing_repository_but_still_adds_the_account() {
    let (mut d, sign_in) = driver(true, false);
    d.send(engine::Event::Party(engine::PartyInput::Application {
        reply_to: ReplyTo::new(Token::new(80)),
        sign_in,
        key: [80; 16],
        digest: [8; 32],
        scope: people::ApplicationScope::Project(1),
        request: engine::Application::Forge(forge::Ask::Adopt(fixtures::adoption(1))),
        carried: None,
    }));
    d.settle();
    let calls = d.forge.as_ref().expect("fake forge").calls;
    d.delivered.clear();
    start(&mut d, sign_in, 81, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!(keeps(&d), 1);
    assert_eq!(d.forge.as_ref().expect("fake forge").calls, calls);
}

#[test]
fn simultaneous_demo_starts_share_one_pair_of_acts_and_return_unused_keys() {
    let (mut d, sign_in) = driver(false, false);
    start(&mut d, sign_in, 81, true);
    start(&mut d, sign_in, 82, true);
    assert!(answers(&d).is_empty());
    connection(&mut d, forge::Connection::Connected { id: 1, login: b"temper".as_slice().into() });
    assert_eq!(answers(&d), vec![Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running)); 2]);
    assert_eq!(keeps(&d), 1);
    assert!(d.delivered.iter().any(|delivery| matches!(delivery, Delivery::CarriedUnused { number: 82 })));
}

#[test]
fn a_busy_box_link_fails_the_demo_start_with_the_groups_reason() {
    let (mut d, sign_in) = driver(false, true);
    start(&mut d, sign_in, 81, true);
    let request = d
        .immediate
        .iter()
        .find_map(
            |request| if let engine::Request::SetUpGroup { request, .. } = request { Some(*request) } else { None },
        )
        .expect("demo group ask");
    d.send(engine::Event::Forge(Box::new(forge::Event::SetUpAnswered {
        forge: 1,
        request,
        answer: forge::SetUpAnswer::Refused(forge::SetUpRefusal::Busy),
    })));
    d.settle();
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Failed(forge::StartFailure::Group {
            why: b"busy".as_slice().into()
        })))]
    );
    connection(&mut d, forge::Connection::Connected { id: 1, login: b"temper".as_slice().into() });
    d.delivered.clear();
    start(&mut d, sign_in, 82, true);
    assert_eq!(answers(&d), [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::Running))]);
    assert_eq!(keeps(&d), 1);
    assert!(matches!(d.delivered.first(), Some(Delivery::CarriedUnused { number: 82 })));
}

#[test]
fn another_connected_forge_ends_the_demo_persists_and_stops_once_and_restart_asks_no_group() {
    let (mut d, sign_in) = driver(true, true);
    start(&mut d, sign_in, 81, true);
    let watcher = Token::new(901);
    d.send(engine::Event::Party(engine::PartyInput::Watch {
        watcher,
        sign_in,
        subject: core::documents::Watched::Application {
            scope: people::ApplicationScope::Deployment,
            connector: 0,
            subject: Token::new(0),
        },
        window: core::documents::Window { rows: 2, after: None },
    }));
    d.settle();
    assert!(
        d.delivered
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Forges { forges, .. } if forges.rows.len() == 2))
    );
    d.send(engine::Event::Party(engine::PartyInput::Delivered { watcher }));
    d.settle();
    d.delivered.clear();
    for _ in 0..2 {
        d.send(engine::Event::Forge(Box::new(forge::Event::Connection {
            forge: 2,
            state: forge::Connection::Connected { id: 2, login: b"other".as_slice().into() },
        })));
        d.settle();
    }
    assert!(d.store.rows.values().any(|record| matches!(record, engine::Record::Forge { row }
        if matches!(row.as_ref(), forge::Stored::Demo(forge::Demo { forge: 1, state: forge::DemoState::Ended })))));
    assert_eq!(d.root.forges().rows.iter().map(|row| row.number).collect::<Vec<_>>(), [2]);
    assert!(d.delivered.iter().any(|delivery| matches!(delivery, Delivery::Forges { forges, .. } if forges.rows.len() == 1 && forges.rows[0].number == 2)));
    assert_eq!(
        d.immediate.iter().filter(|request| matches!(request, engine::Request::StopGroup { forge: 1 })).count(),
        1
    );
    start(&mut d, sign_in, 82, true);
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::NotOffered))],
        "deliveries: {:?}; stopped: {}",
        d.delivered,
        d.stopped
    );
    d.root = engine::Domain::new(config(true), &d.env.limits).expect("valid demo config");
    d.immediate.clear();
    d.send(engine::Event::Restart);
    d.settle();
    assert_eq!(d.root.demo_state(), Some(forge::DemoState::Ended));
    assert!(d.root.forge_ended(1));
    assert!(
        !d.immediate
            .iter()
            .any(|request| matches!(request, engine::Request::SetUpGroup { .. } | engine::Request::StopGroup { .. }))
    );
}

#[test]
fn another_forge_connecting_answers_a_held_demo_start_not_offered() {
    let (mut d, sign_in) = driver(false, true);
    start(&mut d, sign_in, 81, true);
    assert!(answers(&d).is_empty());
    d.send(engine::Event::Forge(Box::new(forge::Event::Connection {
        forge: 2,
        state: forge::Connection::Connected { id: 2, login: b"other".as_slice().into() },
    })));
    d.settle();
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::NotOffered))],
        "deliveries: {:?}; stopped: {}",
        d.delivered,
        d.stopped
    );
    connection(&mut d, forge::Connection::Connected { id: 1, login: b"temper".as_slice().into() });
    d.fire_at(Duration::from_secs(10));
    d.settle();
    assert_eq!(
        answers(&d),
        [Some(engine::ApplicationAnswer::StartDemo(forge::StartAnswer::NotOffered))],
        "deliveries: {:?}; stopped: {}",
        d.delivered,
        d.stopped
    );
}

fn end(d: &mut Driver) {
    d.send(engine::Event::Forge(Box::new(forge::Event::Connection {
        forge: 2,
        state: forge::Connection::Connected { id: 2, login: b"other".as_slice().into() },
    })));
}
fn open_chat(d: &mut Driver, sign_in: u64, number: u8) -> u64 {
    let mut ask = d.chat_ask(1, b"hello");
    let people::Ask::Create { batch, .. } = &mut ask else { panic!("chat fixture") };
    batch[0].budget = people::Budget::AsAllowed { cap: Some(200) };
    d.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(u64::from(number))),
        sign_in,
        key: [number; 16],
        ask,
    }));
    d.pump();
    d.delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let Delivery::WebReply { to, reply: people::Reply::Outcome(people::Outcome::Created { first, .. }) } =
                delivery
                && to == &ReplyTo::new(Token::new(u64::from(number)))
            {
                Some(*first)
            } else {
                None
            }
        })
        .expect("chat admitted")
}
fn keep_key(d: &mut Driver, sign_in: u64, holder: people::AccountHolder, number: u8) {
    d.send(engine::Event::Party(engine::PartyInput::Key {
        reply_to: ReplyTo::new(Token::new(u64::from(number))),
        sign_in,
        key: [number; 16],
        ask: Box::new(people::Ask::AccountChange {
            holder,
            change: people::AccountChange::AddKey { provider: 1, join: None, digest: [8; 32] },
        }),
        carried: core::Carried { number: u64::from(number), digest: [8; 32] },
    }));
    d.settle();
    assert!(d.delivered.iter().any(|delivery| matches!(delivery,
        Delivery::WebReply { to, reply: people::Reply::Outcome(people::Outcome::AccountKeyKept { .. }) }
            if to == &ReplyTo::new(Token::new(u64::from(number))))));
}
fn live_demo(account: bool) -> (Driver, u64, u64) {
    demo_with_chat(account, true)
}
fn demo_with_chat(account: bool, granted: bool) -> (Driver, u64, u64) {
    let (mut d, sign_in) = driver_grant(true, false, granted);
    keep_key(&mut d, sign_in, people::AccountHolder::Own, 90);
    // Running is committed before the account/adoption complete. The end also
    // owns chats of a running demo whose start was interrupted.
    start(&mut d, sign_in, 81, false);
    d.hello(Token::new(7));
    d.settle();
    let task = open_chat(&mut d, sign_in, 91);
    if account {
        keep_key(&mut d, sign_in, people::AccountHolder::Deployment, 95);
    }
    assert_eq!(d.root.demo_state(), Some(forge::DemoState::Running));
    (d, task, sign_in)
}
fn saved_acts(d: &Driver) -> Vec<forge::DemoAct> {
    d.transactions
        .iter()
        .flatten()
        .filter_map(|write| {
            if let engine::Write::Save(engine::Record::Forge { row }) = write
                && let forge::Stored::DemoAct(row) = row.as_ref()
            {
                Some(*row)
            } else {
                None
            }
        })
        .collect()
}
fn kept_acts(d: &Driver) -> Vec<forge::DemoAct> {
    d.store
        .rows
        .values()
        .filter_map(|record| {
            if let engine::Record::Forge { row } = record
                && let forge::Stored::DemoAct(row) = row.as_ref()
            {
                Some(*row)
            } else {
                None
            }
        })
        .collect()
}
fn acted(d: &Driver) -> Vec<(people::Act, people::Outcome)> {
    d.store
        .rows
        .values()
        .filter_map(|record| {
            if let engine::Record::Core(core::Record::People(people::Stored::Acted { act, outcome, .. })) = record {
                Some((act.clone(), outcome.clone()))
            } else {
                None
            }
        })
        .collect()
}
fn restart(d: &mut Driver, granted: bool) {
    let mut configuration = config(false);
    if !granted {
        configuration.core.grants = Box::new([]);
    }
    restart_with(d, configuration);
}
fn restart_with(d: &mut Driver, configuration: engine::Config) {
    reopen(d, configuration);
    assert!(d.root.ready() && !d.stopped);
}
fn reopen(d: &mut Driver, configuration: engine::Config) {
    d.root = engine::Domain::new(configuration, &d.env.limits).expect("restart config");
    d.events.clear();
    d.delivered.clear();
    d.send(engine::Event::Restart);
    d.pump();
}
fn assert_erased(d: &Driver, rows: &[forge::DemoAct]) {
    assert!(kept_acts(d).is_empty());
    for row in rows {
        assert!(d.transactions.iter().flatten().any(|write| matches!(write, engine::Write::Erase(engine::Key::Forge(forge::Key::DemoAct(key))) if *key == row.key)));
    }
}

#[test]
fn demo_end_closes_chats_with_delegates_and_removes_its_account_erasing_answered_rows() {
    use jig_domain_tasks as tasks;
    let (mut d, task, _sign_in) = live_demo(true);
    let parent = d.last_assignment();
    assert_eq!(parent.task, task);
    let mut delegate = fixtures::report_delegate(b"work", Box::new([]));
    // A conversation requested by a task is a delegate, not another demo chat.
    delegate.flags.conversation = true;
    fixtures::tool_call(&mut d, &parent, 61, engine::Tool::Delegate { batch: Box::new([delegate]) });
    let Some(engine::CallAnswer::Delegated(children)) = d.answered(61) else { panic!("delegate admitted") };
    let child = children[0].task;
    end(&mut d);
    d.pump();
    let rows = saved_acts(&d);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.act == forge::DemoActKind::Close { project: 1, task }));
    assert_erased(&d, &rows);
    let answers = acted(&d);
    assert!(answers.iter().any(|(act, outcome)| matches!((act, outcome), (people::Act::Close { project: 1, task: closed, grace: Some(grace), reason }, people::Outcome::Compound { operations: 2, .. }) if *closed == task && *grace == Duration::from_secs(5) && reason.as_ref() == b"Ended with the playground")));
    assert!(answers.iter().any(|(act, outcome)| matches!(
        (act, outcome),
        (people::Act::RemoveAccount { .. }, people::Outcome::AccountRemoved { .. })
    )));
    assert!(!d.store.rows.values().any(|record| matches!(
        record,
        engine::Record::Core(core::Record::Accounts(accounts::Stored::Account {
            holder: accounts::Holder::Deployment,
            provider: 1,
            ..
        }))
    )));
    let grace = d.task(task).expect("live chat").grace.as_ref().expect("template grace");
    assert_eq!(grace.reason.as_ref(), b"Ended with the playground");
    assert_eq!(grace.by, tasks::Party::Deployment { project: 1 });
    assert_eq!(grace.until, skein_lib::Wall::from_nanos(Duration::from_secs(5).as_nanos()));
    assert!(d.store.rows.values().any(|record| matches!(record,
        engine::Record::Core(core::Record::Tasks(tasks::Stored::Ended(row))) if row.number == child
            && matches!(&row.phase, tasks::Phase::Ended(tasks::Ending::Cancelled { by: tasks::Party::Deployment { project: 1 }, reason, .. }) if reason.as_ref() == b"Ended with the playground"))));
    assert!(d.transactions.iter().any(|writes| {
        let ended = writes.iter().any(|write| matches!(write, engine::Write::Save(engine::Record::Forge { row }) if matches!(row.as_ref(), forge::Stored::Demo(forge::Demo { state: forge::DemoState::Ended, .. }))));
        let acts = writes.iter().filter(|write| matches!(write, engine::Write::Save(engine::Record::Forge { row }) if matches!(row.as_ref(), forge::Stored::DemoAct(_)))).count();
        ended && acts == 2
    }));
    d.fire_at(Duration::from_secs(5));
    d.settle();
    assert!(
        matches!(&d.task(task).expect("live chat").phase, tasks::Phase::Closing(closing) if matches!(&closing.ending, tasks::Ending::Cancelled { by: tasks::Party::Deployment { project: 1 }, reason, .. } if reason.as_ref() == b"Ended with the playground"))
    );
}

#[test]
fn demo_end_selects_only_persons_live_conversations_and_never_chats_opened_afterward() {
    let (mut d, task, sign_in) = live_demo(false);
    let parent = d.last_assignment();
    // The same project's ordinary task has no conversation flag.
    let mut ask = d.chat_ask(1, b"ordinary task");
    let people::Ask::Create { batch, .. } = &mut ask else { panic!("create fixture") };
    batch[0].flags = people::Flags::PLAIN;
    batch[0].budget = people::Budget::Named;
    batch[0].authority.spend = 10;
    batch[0].label = 2;
    batch[0].tracked = Some(1);
    d.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in,
        key: [92; 16],
        ask,
    }));
    d.pump();
    let reply = d
        .delivered
        .iter()
        .find_map(|delivery| {
            if let Delivery::WebReply { to, reply } = delivery
                && to == &ReplyTo::new(Token::new(92))
            {
                Some(reply)
            } else {
                None
            }
        })
        .expect("ordinary task reply");
    assert!(matches!(reply, people::Reply::Outcome(people::Outcome::Created { .. })), "ordinary task: {reply:?}");
    end(&mut d);
    d.pump();
    assert_eq!(
        saved_acts(&d).iter().map(|row| row.act).collect::<Vec<_>>(),
        [forge::DemoActKind::Close { project: 1, task }]
    );
    let ordinary = d.store.header().tasks;
    assert_ne!(ordinary, task);
    assert!(!d.task(ordinary).expect("live ordinary task").flags.conversation);
    assert!(d.task(ordinary).expect("live ordinary task").grace.is_none());
    // A second connection cannot reselect the ordinary task or a later chat.
    d.fire_at(Duration::from_secs(5));
    d.pump();
    d.answer(Token::new(7), parent.task, parent.attempt, 0, jig_domain_tasks::End::Parked, engine::Parts::none());
    d.pump();
    let later = open_chat(&mut d, sign_in, 93);
    end(&mut d);
    d.pump();
    assert!(d.task(later).expect("live later chat").grace.is_none());
    assert_eq!(saved_acts(&d).len(), 1);
}

#[test]
fn demo_end_without_a_deployment_account_asks_only_for_the_chat() {
    let (mut d, task, _sign_in) = live_demo(false);
    end(&mut d);
    d.pump();
    assert!(d.store.rows.values().any(|record| matches!(
        record,
        engine::Record::Core(core::Record::Accounts(accounts::Stored::Account {
            holder: accounts::Holder::Party(_),
            provider: 1,
            ..
        }))
    )));
    let rows = saved_acts(&d);
    assert_eq!(rows.iter().map(|row| row.act).collect::<Vec<_>>(), [forge::DemoActKind::Close { project: 1, task }]);
    assert_erased(&d, &rows);
}

#[test]
fn demo_end_committed_before_its_answer_is_resent_with_the_same_key_and_closed_once() {
    let (mut d, task, _sign_in) = live_demo(false);
    end(&mut d);
    // The first decision keeps both the row and the core's compound, before its held answer.
    assert!(!d.store.pending.is_empty());
    d.store.apply();
    let rows = kept_acts(&d);
    assert_eq!(rows.len(), 1);
    assert!(d.store.rows.values().any(|record| matches!(record,
        engine::Record::Core(core::Record::People(people::Stored::Acted { key, .. })) if key.key == rows[0].key)));
    let before = d.task(task).expect("live chat").clone();
    let answers = acted(&d);
    assert_eq!(answers.len(), 1);
    restart(&mut d, true);
    assert_erased(&d, &rows);
    assert_eq!(acted(&d), answers);
    assert_eq!(d.task(task).expect("live chat").grace, before.grace);
    assert_eq!(d.task(task).expect("live chat").last_message, before.last_message);
}

#[test]
fn demo_end_unknown_and_ended_answers_erase_the_kept_act() {
    let (mut d, task, _sign_in) = live_demo(false);
    end(&mut d);
    d.pump();
    let row = saved_acts(&d)[0];
    // While a cancellation is closing, a fresh key receives Ended. A missing task/account gets Unknown.
    d.fire_at(Duration::from_secs(5));
    d.settle();
    let rows = [
        forge::DemoAct { key: [201; 16], act: row.act },
        forge::DemoAct { key: [202; 16], act: forge::DemoActKind::Close { project: 1, task: u64::MAX } },
        forge::DemoAct { key: [203; 16], act: forge::DemoActKind::RemoveAccount { account: u32::MAX } },
    ];
    assert_eq!(row.act, forge::DemoActKind::Close { project: 1, task });
    d.store.seed(
        rows.iter()
            .map(|row| engine::Record::Forge { row: Box::new(forge::Stored::DemoAct(*row)) })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        Box::new([]),
    );
    restart(&mut d, true);
    assert_erased(&d, &rows);
    assert!(
        acted(&d).iter().any(|(_, answer)| matches!(answer, people::Outcome::Refused(people::Refusal::Ended { .. })))
    );
    assert_eq!(
        acted(&d)
            .iter()
            .filter(|(_, answer)| matches!(answer, people::Outcome::Refused(people::Refusal::Unknown)))
            .count(),
        2
    );
}

#[test]
fn demo_end_without_the_grant_serves_keeps_names_and_resends_until_a_granted_restart() {
    let (mut d, task, _sign_in) = demo_with_chat(true, false);
    assert!(d.root.ready());
    end(&mut d);
    d.settle();
    let rows = kept_acts(&d);
    assert_eq!(rows.len(), 2);
    assert!(acted(&d).is_empty());
    assert_eq!(d.delivered.iter().filter(|delivery| matches!(delivery, Delivery::DemoNote { line } if line.starts_with(b"demo end:") && line.windows(b"engine.acts".len()).any(|part| part == b"engine.acts"))).count(), 2);
    restart(&mut d, false);
    assert_eq!(kept_acts(&d), rows);
    assert_eq!(d.delivered.iter().filter(|delivery| matches!(delivery, Delivery::DemoNote { .. })).count(), 2);
    assert!(d.task(task).expect("live chat").grace.is_none());
    restart(&mut d, true);
    assert_erased(&d, &rows);
    assert_eq!(acted(&d).len(), 2);
    assert!(d.task(task).expect("live chat").grace.is_some());
}

#[test]
fn demo_end_does_not_act_on_a_chat_already_closing() {
    let (mut d, task, sign_in) = live_demo(false);
    d.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(94)),
        sign_in,
        key: [94; 16],
        ask: people::Ask::Cancel { project: 1, task, reason: b"done".as_slice().into(), grace: None },
    }));
    d.settle();
    assert!(matches!(d.task(task).expect("live chat").phase, jig_domain_tasks::Phase::Closing(_)));
    end(&mut d);
    d.settle();
    assert!(saved_acts(&d).is_empty());
    assert!(acted(&d).is_empty());
}

fn pending_demo_end() -> (Driver, forge::DemoAct) {
    let (mut d, task, _sign_in) = live_demo(false);
    let row = forge::DemoAct { key: [210; 16], act: forge::DemoActKind::Close { project: 1, task } };
    d.store.seed(Box::new([engine::Record::Forge { row: Box::new(forge::Stored::DemoAct(row)) }]), Box::new([]));
    (d, row)
}

#[test]
fn demo_end_without_a_first_owner_keeps_and_names_the_act() {
    let (mut d, row) = pending_demo_end();
    let mut configuration = config(false);
    configuration.core.owners = Box::new([]);
    restart_with(&mut d, configuration);
    assert_eq!(kept_acts(&d), vec![row]);
    assert!(d.delivered.iter().any(|delivery| matches!(delivery, Delivery::DemoNote { line } if line.ends_with(b"no first owner configured") && line.len() <= 256)));
}

#[test]
fn a_bad_release_template_stops_before_a_demo_end_is_replayed() {
    let (mut d, row) = pending_demo_end();
    let mut configuration = config(false);
    configuration.core.defaults.templates[0].grace = Some(Duration::from_secs(6));
    reopen(&mut d, configuration);
    assert!(d.stopped);
    assert_eq!(
        d.root.settings_failure(),
        Some(core::RestartRequest::Settings {
            failure: core::settings::Failure {
                setting: core::settings::Setting::Template(1),
                fault: core::settings::Fault::Grace
            },
            changed: false,
        })
    );
    assert_eq!(kept_acts(&d), vec![row]);
}

#[test]
fn demo_end_reads_the_owners_changed_conversation_grace() {
    let (mut d, task, sign_in) = live_demo(false);
    let mut template = config(false).core.defaults.templates[0].clone();
    template.grace = Some(Duration::from_secs(1));
    d.send(engine::Event::Party(engine::PartyInput::ChangeSettings {
        reply_to: ReplyTo::new(Token::new(211)),
        sign_in,
        key: [211; 16],
        digest: [211; 32],
        change: Box::new(core::settings::Change {
            against: 1,
            items: Box::new([core::settings::Item {
                setting: core::settings::Setting::Template(1),
                edit: core::settings::Edit::Set(core::settings::Value::Template(Box::new(template))),
            }]),
            running: core::settings::Running::Stays,
        }),
    }));
    d.settle();
    end(&mut d);
    d.settle();
    assert_eq!(
        d.task(task).expect("live chat").grace.as_ref().expect("changed grace").until,
        skein_lib::Wall::from_nanos(Duration::from_secs(1).as_nanos())
    );
}
