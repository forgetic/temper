//! Accounts joins use the root-only pump of front/tests.rs; the page renderer belongs to part c.
use super::{Joint, engine_limits};
use crate::{
    app::{Event, Root, Temper},
    fixtures,
};
use jig_client_world::harness::{App, ClientId, Duty, Front, FrontFault};
use jig_domain_accounts as a;
use skein_lib::{Duration, Env, Queue, Wall};
use temper_engine_domain as engine;
use temper_engine_domain_world::{store::Store, walking};
const CLIENT: ClientId = ClientId(0);
pub const PROVIDER: u32 = 17;
pub const SMALL_ROWS: u32 = 2;
#[must_use]
pub fn fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, false, 2, None, false, WelcomeForge::None, false)
}
#[must_use]
pub fn plan_fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, false, 2, None, true, WelcomeForge::None, false)
}
#[must_use]
pub fn tools_fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, true, 2, None, false, WelcomeForge::None, false)
}
#[must_use]
pub fn small_rows_fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, false, 3, Some(SMALL_ROWS), false, WelcomeForge::None, false)
}
#[must_use]
pub fn welcome_fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, false, 2, None, false, WelcomeForge::Connected, false)
}
/// The first-run forge is owned by the box; the test scripts its connection rows.
#[must_use]
pub fn forge_fixture(seed: u64) -> (Joint, Root) {
    let (mut front, root) = fixture_root(seed, false, 2, None, false, WelcomeForge::Owned, false);
    // The scripted box's Create makes the first repository available to Adopt.
    front.engine.forge = Some(temper_engine_domain_world::forge_stand::ForgeStand::with(&[
        (b"acme", b"shop", 7),
        (b"acme", b"second", 8),
        (b"work", b"hello", 42),
    ]));
    (front, root)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WelcomeForge {
    None,
    Connected,
    Owned,
    Demo { running: bool },
}
#[must_use]
pub fn program_fixture(seed: u64) -> (Joint, Root) {
    fixture_root(seed, false, 2, None, false, WelcomeForge::None, true)
}
#[must_use]
pub fn empty_fixture(seed: u64) -> (Joint, Root) {
    without_deployment_account(fixture(seed))
}
/// A `ChatGPT` sign-in with no configured model already usable.
#[must_use]
pub fn empty_plan_fixture(seed: u64) -> (Joint, Root) {
    without_deployment_account(plan_fixture(seed))
}
fn without_deployment_account((mut front, mut root): (Joint, Root)) -> (Joint, Root) {
    use temper_client_domain::pages::{
        Act as PageAct,
        accounts::{Act, Operation, Row},
    };
    let domain = root.person();
    let account = domain
        .pages()
        .accounts()
        .expect("account fixture")
        .deployment(domain.client())
        .expect("owner deployment")
        .accounts
        .get(0)
        .expect("seeded deployment account")
        .number;
    for act in [
        Act::Submit {
            name: [67; 16],
            row: Row { holder: jig_client::contract::AccountHolder::Deployment, account },
            operation: Operation::RemoveAccount,
        },
        Act::Confirm,
    ] {
        event(
            &mut front,
            &mut root,
            temper_client_domain::Event::Act { action: temper_client_domain::Action::Pages(PageAct::Accounts(act)) }
                .into(),
        );
    }
    (front, root)
}
fn demo_providers(config: &mut engine::Config) {
    // Both providers use this world's restored fake key sources, so Remove
    // goes through the real account policy. Only their usability is scripted.
    let demo = config.core.settings.llm_providers.providers[0].clone();
    let mut other = demo.clone();
    other.number = PROVIDER + 1;
    other.name = Box::from(b"Other fake".as_slice());
    other.models = Box::new([Box::from(b"other-scripted".as_slice())]);
    other.offers = a::Offers { sign_in: true, key: true, programs: Box::new([]) };
    config.core.settings.llm_providers.providers = Box::new([demo, other]);
}
#[expect(clippy::too_many_lines, reason = "one complete client fixture with its configured forge fields")]
fn fixture_root(
    seed: u64,
    tools: bool,
    own_accounts: u32,
    rows: Option<u32>,
    plan: bool,
    forge: WelcomeForge,
    program: bool,
) -> (Joint, Root) {
    let welcome = forge != WelcomeForge::None;
    let forge_setup = matches!(forge, WelcomeForge::Owned | WelcomeForge::Demo { .. });
    let mut limits = if tools { temper_client_tools::limits::client() } else { fixtures::domain_limits() };
    if let Some(rows) = rows {
        limits.client.domain.rows = rows;
    }
    let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|index| {
        let mut config = walking::config(seed);
        config.core.settings.llm_providers = a::Providers {
            providers: Box::new([a::Provider {
                number: PROVIDER,
                name: Box::from(if plan { b"openai".as_slice() } else { b"Scripted".as_slice() }),
                signed_in_plan: plan,
                manage_usage: None,
                models: Box::new([Box::from(b"scripted".as_slice())]),
                offers: a::Offers { sign_in: true, key: true, programs: Box::new([]) },
            }]),
        };
        // Bootstrap restores the demo account alone; the runtime offers both.
        let runtime = index == 1;
        if let WelcomeForge::Demo { .. } = forge
            && runtime
        {
            demo_providers(&mut config);
        }
        if tools || program {
            config.core.settings.llm_providers.providers.get_mut(0).expect("fixture provider").offers.programs =
                Box::new([Box::from(b"claude_code".as_slice())]);
        }
        if tools {
            let mut model = walking::charter().policy.model;
            model.name = Box::from(b"effort-model".as_slice());
            model.efforts = Box::new([Box::from(b"high".as_slice())]);
            model.effort = None;
            config.core.settings.models = Box::new([walking::charter().policy.model, model.clone()]);
            config.core.defaults.charters.get_mut(0).expect("fixture charter").policy.alternatives = Box::new([model]);
            config.core.settings.llm_providers.providers.get_mut(0).expect("fixture provider").models =
                Box::new([Box::from(b"scripted".as_slice()), Box::from(b"effort-model".as_slice())]);
        }
        config.under_box |= forge_setup;
        if welcome {
            config.core.connectors = Box::new([fixtures::FORGE]);
            config.forges = Box::new([temper_engine_domain_forge::Configured {
                admin_connect: false,
                bot: Box::new([]),
                owned: None,
                web: None,
                read_only: false,
                number: 1,
                kind: temper_engine_domain_forge::Kind::Forgejo,
                mode: if forge_setup {
                    temper_engine_domain_forge::Mode::Owned
                } else {
                    temper_engine_domain_forge::Mode::Connected
                },
                asked: forge_setup,
            }]);
        }
        let mut engine_limits = engine_limits();
        // Restore one deployment account and leave the requested room for party accounts.
        engine_limits.documents.lines = engine_limits.documents.lines.max(own_accounts);
        engine_limits.accounts.accounts = own_accounts + 1;
        engine_limits.accounts.holder_accounts = own_accounts;
        engine_limits.accounts.credentials = own_accounts + 1;
        engine_limits.authority.accounts = 4;
        engine_limits.fleet.grants = (own_accounts + 1) * 2;
        // Each extra grant adds one entry to each of the four fleet routing cohorts.
        engine_limits.journal.writes =
            engine_limits.journal.writes.checked_add((engine_limits.fleet.grants - 2) * 4).expect("grant cohorts fit");
        if forge == WelcomeForge::Owned {
            super::first_run_projects(&mut config, &mut engine_limits);
        }
        (config, engine_limits)
    });
    let mut front = Joint::with_engine(seed, &limits, Store::new(), [bootstrap, config], &engine_limits, 1);
    if forge_setup {
        front.engine.send(engine::Event::Forge(Box::new(temper_engine_domain_forge::Event::Connection {
            forge: 1,
            state: temper_engine_domain_forge::Connection::NotSetUp,
        })));
        front.engine.settle();
    }
    if let WelcomeForge::Demo { running } = forge {
        front.demo = Some(demo_answer(running));
    }
    front.bind(CLIENT, fixtures::PARTY);
    let mut root = if tools {
        crate::agent::new(seed).0
    } else {
        Root::Person(Box::new(temper_client_domain::unlinked(&limits, seed)))
    };
    event(
        &mut front,
        &mut root,
        Temper::start(
            if welcome {
                temper_client_domain::Address::Welcome { step: temper_client_domain::Step::Welcome }
            } else {
                temper_client_domain::Address::Accounts
            },
            Vec::new(),
            0,
        ),
    );
    (front, root)
}
pub fn event(front: &mut Joint, root: &mut Root, event: Event) {
    let env = Env { now: front.now, wall: Wall::EPOCH, limits: front.limits };
    let mut out = Queue::with_capacity(Temper::max_out(&env.limits));
    Temper::step(root, &env, event, &mut out);
    flush(front, root, &env, &mut out);
    settle(front, root);
}
fn flush(
    front: &mut Joint,
    root: &mut Root,
    env: &Env<temper_client_domain::Limits>,
    out: &mut Queue<temper_client_domain::Request>,
) {
    Temper::take(root, env, out);
    while let Some(request) = out.pop() {
        if let temper_client_domain::Request::Leave { to, .. } = request {
            front.leaves.push(to);
            continue;
        }
        match Temper::duty(request) {
            Duty::Wire(request) => front.wire(CLIENT, env.now, env.wall, request),
            Duty::Show { address, .. } => Temper::step(root, env, Temper::went(address), out),
            Duty::Store(_) | Duty::SignIn { .. } | Duty::SignOut | Duty::Fact => {}
        }
    }
}
pub fn settle(front: &mut Joint, root: &mut Root) {
    let mut env = Env { now: front.now, wall: Wall::EPOCH, limits: front.limits };
    let mut out = Queue::with_capacity(Temper::max_out(&env.limits));
    for _ in 0..32 {
        let callbacks = front.poll(env.now, env.wall);
        if callbacks.is_empty() {
            return;
        }
        for (who, event) in callbacks {
            assert_eq!(who, CLIENT);
            env.now = env.now.saturating_add(Duration::from_millis(3));
            front.received(CLIENT, &event);
            Temper::step(root, &env, event, &mut out);
            flush(front, root, &env, &mut out);
        }
    }
    panic!("account root/front did not settle");
}
pub fn lose_next(front: &mut Joint) {
    front.fault(front.now, FrontFault::NoAnswer);
}
/// The root-only fixture plays the link's whole-answer loss under its actual attempt.
pub fn lost(front: &mut Joint, root: &mut Root) {
    let (who, attempt) = front.lost_answer.take().expect("a whole answer was lost");
    assert_eq!(who, CLIENT);
    event(
        front,
        root,
        temper_client_domain::Event::Client {
            event: jig_client::Event::Up {
                up: jig_client::Up::Answer {
                    attempt,
                    envelope: skein_lib::RequestEnvelope::Lost,
                    answer: None,
                    why: None,
                },
            },
        }
        .into(),
    );
}
/// The fake secrets store acknowledges the key under its source after moving the carrier.
pub fn kept(front: &mut Joint) -> u32 {
    let (source, carried) = front
        .engine
        .protocol
        .accounts
        .iter()
        .rev()
        .find_map(|request| {
            if let a::Request::Signing(a::Signing::Keep { source, carried, .. }) = request {
                Some((*source, *carried))
            } else {
                None
            }
        })
        .expect("key reached its component");
    assert_eq!(front.transport.carried()[&carried].expose(), b"test-private-key");
    front.transport.unused(carried);
    front.engine.send(engine::Event::Account(a::Event::Component(a::FromComponent::Granted {
        account: source,
        generation: 1,
        valid: Duration::from_secs(3600),
        form: a::Form::Credential(a::Scheme::Bearer),
    })));
    front.engine.settle();
    source
}
#[must_use]
pub fn signing(front: &Joint) -> u32 {
    front
        .engine
        .protocol
        .accounts
        .iter()
        .rev()
        .find_map(|request| {
            if let a::Request::Signing(a::Signing::SignIn { source, .. }) = request { Some(*source) } else { None }
        })
        .expect("sign-in reached its component")
}
pub fn assert_no_carried(front: &Joint) {
    assert!(front.transport.carried().is_empty(), "adopted and unused carriers were released");
}

/// A chat explicitly funded by the account whose row the story exercises.
pub fn account_chat(front: &mut Joint, account: u32) -> u64 {
    use jig_domain_people as people;
    let mut ask = front.engine.chat_ask(1, b"Wait for my plan");
    let people::Ask::Create { batch, .. } = &mut ask else { panic!("chat template") };
    batch[0].llm = Some(people::LlmChoice {
        account: Some(account),
        main: people::ModelEffort { model: 0, effort: None },
        subagents: Box::new([]),
    });
    front.engine.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(skein_lib::Token::new(1001)),
        sign_in: front.transport.sign_in(CLIENT).expect("authenticated fixture client"),
        key: [101; 16],
        ask,
    }));
    // Waiting for this limited account deliberately keeps the fleet's deadline armed.
    front.settles_with_a_deadline_armed = true;
    front.settle();
    front
        .engine
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let engine::Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::Created { first, .. }),
                ..
            } = delivery
            {
                Some(*first)
            } else {
                None
            }
        })
        .expect("created account-funded task")
}

/// A scripted demo read and start, beside the real root's account and forge facts.
#[must_use]
pub fn demo_fixture(seed: u64, state: &temper_client_protocol_wire::v1::DemoState) -> (Joint, Root) {
    fixture_root(
        seed,
        false,
        2,
        None,
        false,
        WelcomeForge::Demo { running: *state == temper_client_protocol_wire::v1::DemoState::Running },
        false,
    )
}

fn demo_answer(running: bool) -> temper_client_protocol_wire::v1::DemoAnswer {
    use temper_client_protocol_wire::v1 as w;
    let mut cues = skein_lib::List::with_capacity(1);
    cues.push(b"Change the title".as_slice().into()).expect("one cue");
    w::DemoAnswer::new(
        &w::CEILINGS,
        w::DemoAnswerParts {
            project: 1,
            forge: 2,
            provider: PROVIDER,
            owner: b"acme".as_slice().into(),
            name: b"shop".as_slice().into(),
            state: if running { w::DemoState::Running } else { w::DemoState::NotStarted },
            cues,
        },
    )
    .expect("demo fixture")
}

/// Fresh page on the same engine and person, with an explicit departure policy.
pub fn reopen(front: &mut Joint, root: &mut Root, address: temper_client_domain::Address, departing: bool) {
    let seed = fixtures::SEED.wrapping_add(1);
    // Close the previous client's watches before replacing its root.
    front.detach(CLIENT);
    front.hello(CLIENT);
    *root = Root::Person(Box::new(if departing {
        temper_client_domain::unlinked_departing(&front.limits, seed)
    } else {
        temper_client_domain::unlinked(&front.limits, seed)
    }));
    event(front, root, Temper::start(address, Vec::new(), 0));
}
