//! Keyed setup and the deployment document through the real core (domain/root.md, 5.1).
#![expect(clippy::disallowed_methods, reason = "ordinary test assertions inspect captured requests")]
#![expect(clippy::match_like_matches_macro, reason = "explicit test patterns; matches is banned by the subset")]
use super::resume_fixture::{config, root_with_config};
use crate::{Delivery, Domain, Event, Limits, PartyInput, PartyRequest, Request, StoreInput, StoreRequest};
use alloc::boxed::Box;
use jig_domain_people as people;
use skein_lib::{Env, List, Queue, ReplyTo, Token, Wall};
use temper_engine_domain_forge as forge;

const SIGN_IN: u64 = 10;
const WATCHER: Token = Token::new(30);

fn signed_in(role: Option<people::Role>) -> (Domain, Env<Limits>) {
    signed_in_on(role, forge::Kind::Forgejo)
}
fn signed_in_on(role: Option<people::Role>, kind: forge::Kind) -> (Domain, Env<Limits>) {
    let mut config = config(1);
    config.forges = Box::new([forge::Configured {
        admin_connect: false,
        bot: Box::new([]),
        owned: None,
        web: None,
        read_only: false,
        number: 1,
        kind,
        mode: forge::Mode::Owned,
        asked: true,
    }]);
    config.under_box = true;
    signed_in_root(root_with_config(config), role)
}
fn signed_in_root((mut domain, env): (Domain, Env<Limits>), role: Option<people::Role>) -> (Domain, Env<Limits>) {
    domain.core.people = people::Domain::new(&env.limits.people, Box::new([]), 1, Box::new([]));
    let mut out = Queue::with_capacity(people::max_out(&env.limits.people));
    for record in [
        people::Stored::Person {
            number: 1,
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: b"owner".as_slice().into() },
                login: Box::new([]),
                name: Box::new([]),
            },
        },
        people::Stored::SignIn { number: SIGN_IN, person: 1, expires: Wall::from_nanos(100) },
        people::Stored::Roles {
            project: 0,
            holdings: match role {
                Some(role) => Box::new([people::Holding { person: 1, role }]),
                None => Box::new([]),
            },
        },
    ] {
        people::step(
            &mut domain.core.people,
            &Env { now: env.now, wall: env.wall, limits: env.limits.people },
            people::Event::Restore { record },
            &mut out,
        );
    }
    people::step(
        &mut domain.core.people,
        &Env { now: env.now, wall: env.wall, limits: env.limits.people },
        people::Event::Restored,
        &mut out,
    );
    (domain, env)
}

// Drive the root's existing journal seam; only its store acknowledgements are scripted.
fn settle(domain: &mut Domain, env: &Env<Limits>) -> Box<[Request]> {
    let mut out = Queue::with_capacity(crate::max_out(&env.limits).checked_mul(2).unwrap());
    let mut requests = List::with_capacity(crate::max_out(&env.limits));
    for _ in 0_u32..256 {
        crate::release(domain, env, &mut out);
        crate::release(domain, env, &mut out);
        while let Some(request) = out.pop() {
            match request {
                Request::Store(StoreRequest::Commit { number, .. }) => {
                    crate::try_step(domain, env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
                }
                Request::ToChild(released) => crate::try_step(domain, env, Event::Released(released)).unwrap(),
                request @ (Request::StopGroup { .. }
                | Request::SetUpGroup { .. }
                | Request::SetUp { .. }
                | Request::Bound { .. }
                | Request::Return { .. }
                | Request::Connect { .. }
                | Request::Manifest { .. }
                | Request::Reachable { .. }
                | Request::Install { .. }
                | Request::EraseCarried { .. }
                | Request::Setup { .. }
                | Request::Store(_)
                | Request::Worker(_)
                | Request::Party(_)
                | Request::Forge { .. }
                | Request::Account(_)
                | Request::Busy(_)
                | Request::Drained
                | Request::Stop) => requests.push(request).unwrap(),
            }
        }
        crate::try_step(domain, env, Event::Resume).unwrap();
    }
    requests.into_boxed()
}
fn setup(key: u8) -> Event {
    Event::Party(PartyInput::Application {
        scope: people::ApplicationScope::Deployment,
        reply_to: ReplyTo::new(Token::new(90)),
        sign_in: SIGN_IN,
        key: [key; 16],
        digest: [key; 32],
        carried: None,
        request: crate::Application::Forge(forge::Ask::SetUp {
            forge: 1,
            org: b"work".as_slice().into(),
            repository: b"hello".as_slice().into(),
            project: forge::SetUpProject::Existing(1),
        }),
    })
}
fn connection(domain: &mut Domain, env: &Env<Limits>, state: forge::Connection) {
    crate::try_step(domain, env, Event::Forge(Box::new(forge::Event::Connection { forge: 1, state }))).unwrap();
}
#[test]
fn a_keyed_setup_answers_the_client_and_the_same_key_replays_without_asking_the_box() {
    let (mut domain, env) = signed_in(Some(people::Role::Owner));
    connection(&mut domain, &env, forge::Connection::NotSetUp);
    crate::try_step(&mut domain, &env, setup(1)).unwrap();
    let requests = settle(&mut domain, &env);
    let [Request::SetUp { forge: 1, request, org, repository }] = requests.as_ref() else {
        panic!("one setup ask: {requests:?}")
    };
    assert_eq!((org.as_ref(), repository.as_ref()), (b"work".as_slice(), b"hello".as_slice()));
    crate::try_step(
        &mut domain,
        &env,
        Event::Forge(Box::new(forge::Event::SetUpAnswered {
            forge: 1,
            request: *request,
            answer: forge::SetUpAnswer::SettingUp,
        })),
    )
    .unwrap();
    let expected = Request::Party(PartyRequest::Deliver(Delivery::Application {
        to: ReplyTo::new(Token::new(90)),
        outcome: Some(crate::ApplicationAnswer::SetUp(forge::SetUpAnswer::SettingUp)),
    }));
    assert_eq!(settle(&mut domain, &env).as_ref(), [expected]);
    crate::try_step(&mut domain, &env, setup(1)).unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Application {
            to: ReplyTo::new(Token::new(90)),
            outcome: Some(crate::ApplicationAnswer::SetUp(forge::SetUpAnswer::SettingUp)),
        }))]
    );
}
fn watch() -> Event {
    Event::Party(PartyInput::Watch {
        watcher: WATCHER,
        sign_in: SIGN_IN,
        subject: jig_domain_core::documents::Watched::Application {
            scope: people::ApplicationScope::Deployment,
            connector: 0,
            subject: Token::new(0),
        },
        window: jig_domain_core::documents::Window { rows: 1, after: None },
    })
}
#[test]
fn the_deployment_owner_watches_forges_again_after_a_connection_changes() {
    let (mut domain, env) = signed_in(Some(people::Role::Owner));
    connection(&mut domain, &env, forge::Connection::NotSetUp);
    crate::try_step(&mut domain, &env, watch()).unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Forges {
            watcher: WATCHER,
            forges: forge::Forges { rows: Box::new([row(forge::Connection::NotSetUp)]) },
        }))]
    );
    crate::try_step(&mut domain, &env, Event::Party(PartyInput::Delivered { watcher: WATCHER })).unwrap();
    connection(&mut domain, &env, forge::Connection::SettingUp { step: Some(forge::SetUpStep::Prepare) });
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Forges {
            watcher: WATCHER,
            forges: forge::Forges {
                rows: Box::new([row(forge::Connection::SettingUp { step: Some(forge::SetUpStep::Prepare) })])
            },
        }))]
    );
    crate::try_step(&mut domain, &env, Event::Party(PartyInput::Delivered { watcher: WATCHER })).unwrap();
    crate::try_step(&mut domain, &env, Event::Forge(Box::new(forge::Event::BoxClosed))).unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Forges {
            watcher: WATCHER,
            forges: forge::Forges {
                rows: Box::new([forge::Row {
                    number: 1,
                    kind: forge::Kind::Forgejo,
                    mode: forge::Mode::Owned,
                    connection: forge::Connection::SetUpFailed { why: b"the box closed the pipe".as_slice().into() },
                    failed: Some(forge::FailedStep::Prepare),
                    offer: forge::Offer::NotUnderTheBox,
                    first: None,
                    installations: Box::new([]),
                    ended: None,
                    app: None,
                    web: None,
                    repositories: Box::new([]),
                    owned: None,
                    admin: false,
                    ended_stop: None,
                }])
            },
        }))]
    );
}
#[test]
fn a_party_without_a_deployment_role_cannot_watch_forges_or_ask_the_box() {
    let (mut domain, env) = signed_in(None);
    crate::try_step(&mut domain, &env, watch()).unwrap();
    let requests = settle(&mut domain, &env);
    let [Request::Party(PartyRequest::WatchRefused { watcher: WATCHER, .. })] = requests.as_ref() else {
        panic!("deployment watch refused: {requests:?}")
    };
    crate::try_step(&mut domain, &env, setup(1)).unwrap();
    for request in settle(&mut domain, &env) {
        if let Request::SetUpGroup { .. } | Request::SetUp { .. } | Request::Bound { .. } | Request::Return { .. } =
            request
        {
            panic!("a party without a deployment role never reaches the box")
        }
    }
}

fn row(connection: forge::Connection) -> forge::Row {
    forge::Row {
        number: 1,
        kind: forge::Kind::Forgejo,
        mode: forge::Mode::Owned,
        connection,
        failed: None,
        offer: forge::Offer::Offered,
        first: None,
        installations: Box::new([]),
        ended: None,
        app: None,
        web: None,
        repositories: Box::new([]),
        owned: None,
        admin: false,
        ended_stop: None,
    }
}

fn public_request(ask: forge::Ask, carried: Option<jig_domain_core::Carried>) -> Event {
    Event::Party(PartyInput::Application {
        // Public forge operations must be admitted on deployment even when a caller names a project.
        scope: people::ApplicationScope::Project(1),
        reply_to: ReplyTo::new(Token::new(90)),
        sign_in: SIGN_IN,
        key: [44; 16],
        digest: [44; 32],
        carried,
        request: crate::Application::Forge(ask),
    })
}
#[test]
fn public_connect_preserves_the_admitted_owner_and_carry_then_root_orders_its_erase() {
    let (mut domain, env) = signed_in_on(Some(people::Role::Owner), forge::Kind::GitHub);
    let carried = jig_domain_core::Carried { number: 1_u64 << 63_u32, digest: [73; 32] };
    crate::try_step(
        &mut domain,
        &env,
        public_request(
            forge::Ask::Connect {
                forge: 1,
                how: forge::How::GitHubApp { app: core::num::NonZeroU64::new(7).unwrap() },
            },
            Some(carried),
        ),
    )
    .unwrap();
    let requests = settle(&mut domain, &env);
    let [Request::Connect { forge: 1, request, how: forge::How::GitHubApp { app }, carried: value }] =
        requests.as_ref()
    else {
        panic!("connect: {requests:?}")
    };
    assert_eq!((app.get(), *value), (7, carried));
    let request = *request;
    crate::try_step(
        &mut domain,
        &env,
        Event::Forge(Box::new(forge::Event::ConnectAnswered {
            forge: 1,
            request,
            answer: forge::ConnectAnswer::Connecting,
        })),
    )
    .unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Application {
            to: ReplyTo::new(Token::new(90)),
            outcome: Some(crate::ApplicationAnswer::Connect(forge::ConnectAnswer::Connecting))
        }))]
    );
    crate::try_step(
        &mut domain,
        &env,
        Event::Forge(Box::new(forge::Event::ConnectEnded {
            stop: None,
            why: None,
            forge: 1,
            request,
            connected: false,
        })),
    )
    .unwrap();
    assert_eq!(settle(&mut domain, &env).as_ref(), [Request::EraseCarried { number: carried.number }]);
}
#[test]
fn public_install_defers_the_keyed_answer_until_its_run_answers() {
    let (mut domain, env) = signed_in_on(Some(people::Role::Owner), forge::Kind::GitHub);
    connection(&mut domain, &env, forge::Connection::Connected { id: 1, login: Box::from(b"app[bot]".as_slice()) });
    crate::try_step(&mut domain, &env, public_request(forge::Ask::Install { forge: 1, back: None }, None)).unwrap();
    let requests = settle(&mut domain, &env);
    let [Request::Install { forge: 1, request, back: None }] = requests.as_ref() else {
        panic!("install: {requests:?}")
    };
    let answer = forge::InstallAnswer::Link(Box::from(b"http://fake/install".as_slice()));
    crate::try_step(
        &mut domain,
        &env,
        Event::Forge(Box::new(forge::Event::InstallAnswered {
            stopped: None,
            forge: 1,
            request: *request,
            answer: answer.clone(),
        })),
    )
    .unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::Application {
            to: ReplyTo::new(Token::new(90)),
            outcome: Some(crate::ApplicationAnswer::Install(answer))
        }))]
    );
}

#[test]
fn public_connect_refuses_other_deployment_roles_and_names_their_carry_unused_once() {
    for role in [people::Role::Member, people::Role::Maintainer, people::Role::Observer] {
        let (mut domain, env) = signed_in_on(Some(role), forge::Kind::GitHub);
        let carried = jig_domain_core::Carried { number: 1_u64 << 63_u32, digest: [73; 32] };
        crate::try_step(
            &mut domain,
            &env,
            public_request(
                forge::Ask::Connect {
                    forge: 1,
                    how: forge::How::GitHubApp { app: core::num::NonZeroU64::new(7).unwrap() },
                },
                Some(carried),
            ),
        )
        .unwrap();
        let requests = settle(&mut domain, &env);
        assert!(!requests.iter().any(|r| if let Request::Connect { .. } = r { true } else { false }));
        assert_eq!(
            requests
                .iter()
                .filter(|r| if let Request::Party(PartyRequest::Deliver(Delivery::CarriedUnused { number })) = r {
                    *number == carried.number
                } else {
                    false
                })
                .count(),
            1
        );
        assert!(requests.iter().any(|r| {
            if let Request::Party(PartyRequest::Deliver(Delivery::Application {
                outcome:
                    Some(crate::ApplicationAnswer::Connect(forge::ConnectAnswer::Refused(forge::ForgeRefusal::Role))),
                ..
            })) = r
            {
                true
            } else {
                false
            }
        }));
    }
}

#[test]
fn a_new_binding_answers_only_after_its_commit_is_durable() {
    let (mut domain, env) = super::resume_fixture::root();
    let token = Token::new(90);
    crate::try_step(
        &mut domain,
        &env,
        Event::Forge(Box::new(forge::Event::Bind {
            request: token,
            address: b"Sam@example.com".as_slice().into(),
            subject: b"google-subject".as_slice().into(),
        })),
    )
    .unwrap();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    let mut committed = None;
    // A release pass may issue a commit; none may deliver its dependent answer.
    let binding = forge::Stored::Binding {
        address: b"sam@example.com".as_slice().into(),
        subject: b"google-subject".as_slice().into(),
    };
    for _ in 0_u32..8_u32 {
        crate::release(&mut domain, &env, &mut out);
        while let Some(request) = out.pop() {
            let Request::Store(StoreRequest::Commit { number, writes }) = request else {
                panic!("binding before durable commit: {request:?}");
            };
            assert!(writes.iter().any(|write| match write {
                crate::Write::Save(crate::Record::Forge { row }) => row.as_ref() == &binding,
                crate::Write::CopySettingsRevision(_) | crate::Write::Save(_) | crate::Write::Erase(_) => false,
            }));
            committed = Some(number);
        }
    }
    let number = committed.expect("binding commit issued");
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Bound { request: token, answer: forge::BindAnswer::Bound }]
    );
}

fn runtime_project_root() -> (Domain, Env<Limits>) {
    let mut configured = super::resume_fixture::config_with_project_room(1, 2);
    let seed = &configured.core.seed_projects.first().unwrap().policy;
    let policy = jig_domain_core::ConfiguredPolicy {
        policy: seed.policy.clone(),
        permissions: seed.permissions.clone(),
        templates: seed.templates.clone(),
    };
    configured.core.defaults.policies.insert(1, policy).unwrap();
    let mut limits = super::resume_fixture::limits();
    limits.authority.projects = 2;
    limits.people.projects = 3;
    limits.journal.writes = crate::limits::route_bound(&limits).unwrap().checked_add(limits.call_records).unwrap();
    signed_in_root(super::resume_fixture::root_with_config_and_limits(configured, &limits), Some(people::Role::Owner))
}

fn make_project(name: &str) -> Event {
    Event::Party(PartyInput::Ask {
        reply_to: ReplyTo::new(Token::new(90)),
        sign_in: SIGN_IN,
        key: [81; 16],
        ask: people::Ask::MakeProject { name: name.into(), policy: 1 },
        models: Box::new([]),
    })
}

#[test]
fn make_project_copies_policy_one_and_the_root_hears_it_only_after_commit() {
    let (mut domain, env) = runtime_project_root();
    let policy = domain.core.authority.policy(1).unwrap().clone();
    crate::try_step(&mut domain, &env, make_project("Tally")).unwrap();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits).checked_mul(2).unwrap());
    let mut commit = None;
    let mut saved = false;
    for _ in 0_u32..32_u32 {
        crate::release(&mut domain, &env, &mut out);
        while let Some(request) = out.pop() {
            if let Request::Store(StoreRequest::Commit { number, writes }) = request {
                assert!(commit.replace(number).is_none(), "one project decision");
                for write in writes {
                    if let crate::Write::Save(crate::Record::Core(jig_domain_core::Record::People(
                        people::Stored::Policy { project, name, .. },
                    ))) = write
                    {
                        assert_eq!(project, 2);
                        assert_eq!(name.as_ref(), "Tally");
                        saved = true;
                    }
                }
            } else {
                panic!("nothing leaves the uncommitted project decision: {request:?}");
            }
        }
    }
    assert!(saved, "the new project's policy and name are durable");
    assert_eq!(domain.core.authority.policy(2), Some(&policy));
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number: commit.unwrap(), fill: 0 }))
        .unwrap();
    let mut heard = false;
    let mut answered = false;
    for _ in 0_u32..32_u32 {
        crate::release(&mut domain, &env, &mut out);
        while let Some(request) = out.pop() {
            if let Request::ToChild(released) = request {
                assert_eq!(released.0, Delivery::ProjectMade { project: 2 });
                assert!(!heard, "one notification");
                heard = true;
                crate::try_step(&mut domain, &env, Event::Released(released)).unwrap();
            } else if let Request::Party(PartyRequest::Deliver(Delivery::WebReply { reply, .. })) = request {
                assert_eq!(reply, people::Reply::Outcome(people::Outcome::ProjectMade { project: 2 }));
                answered = true;
            } else {
                panic!("only the root and maker hear the committed project: {request:?}");
            }
        }
    }
    assert!(heard && answered);
    assert_eq!(domain.core.projects.as_slice(), &[1, 2]);
    assert_eq!(domain.core.people.role(1, 2), Some(people::Role::Owner));
}

#[test]
fn a_taken_project_name_is_refused_without_making_a_project() {
    let (mut domain, env) = runtime_project_root();
    crate::try_step(&mut domain, &env, make_project("project 1")).unwrap();
    let requests = settle(&mut domain, &env);
    assert_eq!(
        requests.as_ref(),
        &[Request::Party(PartyRequest::Deliver(Delivery::WebReply {
            to: ReplyTo::new(Token::new(90)),
            reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::NameTaken)),
        }))]
    );
    assert_eq!(domain.core.projects.as_slice(), &[1]);
    assert!(domain.core.authority.policy(2).is_none());
}

#[test]
fn whose_returns_the_live_person_and_none_after_sign_out_or_for_an_unknown_sign_in() {
    const DESTINATION: Token = Token::new(91);
    let (mut domain, env) = signed_in(Some(people::Role::Owner));
    crate::try_step(
        &mut domain,
        &env,
        Event::Party(PartyInput::Whose { reply_to: ReplyTo::new(DESTINATION), sign_in: SIGN_IN }),
    )
    .unwrap();
    let requests = settle(&mut domain, &env);
    let [
        Request::Party(PartyRequest::Deliver(Delivery::WebReply {
            to: destination,
            reply: people::Reply::Signer(Some(signer)),
        })),
    ] = requests.as_ref()
    else {
        panic!("one live signer: {requests:?}")
    };
    assert_eq!(*destination, ReplyTo::new(DESTINATION));
    assert_eq!(signer.kind, people::Kind::Person);
    assert_eq!(signer.identity.key, people::IdentityKey { provider: 0, subject: b"owner".as_slice().into() });
    crate::try_step(
        &mut domain,
        &env,
        Event::Party(PartyInput::SignOut { reply_to: ReplyTo::new(DESTINATION), sign_in: SIGN_IN }),
    )
    .unwrap();
    assert_eq!(
        settle(&mut domain, &env).as_ref(),
        [Request::Party(PartyRequest::Deliver(Delivery::WebReply {
            to: ReplyTo::new(DESTINATION),
            reply: people::Reply::SignedOut
        }))]
    );
    for sign_in in [SIGN_IN, SIGN_IN + 1] {
        crate::try_step(
            &mut domain,
            &env,
            Event::Party(PartyInput::Whose { reply_to: ReplyTo::new(DESTINATION), sign_in }),
        )
        .unwrap();
        assert_eq!(
            settle(&mut domain, &env).as_ref(),
            [Request::Party(PartyRequest::Deliver(Delivery::WebReply {
                to: ReplyTo::new(DESTINATION),
                reply: people::Reply::Signer(None)
            }))]
        );
    }
}

fn admin_root(role: Option<people::Role>) -> (Domain, Env<Limits>) {
    let mut config = config(1);
    config.forges = Box::new([forge::Configured {
        admin_connect: true,
        bot: Box::new([]),
        owned: None,
        web: None,
        read_only: false,
        number: 1,
        kind: forge::Kind::Forgejo,
        mode: forge::Mode::Managed,
        asked: false,
    }]);
    signed_in_root(root_with_config(config), role)
}
fn admin_first() -> forge::FirstAsk {
    forge::FirstAsk {
        org: b"work".as_slice().into(),
        repository: b"hello".as_slice().into(),
        project: forge::SetUpProject::Existing(1),
        label: b"docker".as_slice().into(),
    }
}
#[test]
fn admin_connect_routes_the_whole_how_and_erases_at_both_ends() {
    for first in [None, Some(admin_first())] {
        for connected in [false, true] {
            let (mut domain, env) = admin_root(Some(people::Role::Owner));
            let previous = forge::Connection::Stopped { reason: b"previous end".as_slice().into() };
            connection(&mut domain, &env, previous.clone());
            let carried = jig_domain_core::Carried { number: 1_u64 << 63_u32, digest: [73; 32] };
            let how = forge::How::ForgejoAdmin { first: first.clone() };
            crate::try_step(
                &mut domain,
                &env,
                public_request(forge::Ask::Connect { forge: 1, how: how.clone() }, Some(carried)),
            )
            .unwrap();
            let requests = settle(&mut domain, &env);
            let [Request::Connect { forge: 1, request, how: actual, carried: value }] = requests.as_ref() else {
                panic!("admin connect: {requests:?}")
            };
            assert_eq!((actual, *value), (&how, carried));
            assert_eq!(domain.forge_connection(1), forge::Connection::Connecting);
            assert_eq!(
                domain.forges().rows[0].first,
                first.clone().map(|first| forge::First {
                    org: first.org,
                    repository: first.repository,
                    project: first.project,
                    state: forge::FirstState::Making,
                    label: first.label,
                })
            );
            let request = *request;
            crate::try_step(
                &mut domain,
                &env,
                Event::Forge(Box::new(forge::Event::ConnectAnswered {
                    forge: 1,
                    request,
                    answer: forge::ConnectAnswer::Connecting,
                })),
            )
            .unwrap();
            settle(&mut domain, &env);
            let state = forge::Connection::Connected { id: 1, login: b"bot".as_slice().into() };
            let why = if connected {
                connection(&mut domain, &env, state.clone());
                crate::try_step(
                    &mut domain,
                    &env,
                    Event::Forge(Box::new(forge::Event::FirstMade { forge: 1, id: 42 })),
                )
                .unwrap();
                None
            } else {
                crate::try_step(
                    &mut domain,
                    &env,
                    Event::Forge(Box::new(forge::Event::FirstFailed { forge: 1, step: forge::CreateStep::File })),
                )
                .unwrap();
                Some(Box::from(b"making work/hello: File (Refused)".as_slice()))
            };
            crate::try_step(
                &mut domain,
                &env,
                Event::Forge(Box::new(forge::Event::ConnectEnded {
                    stop: None,
                    forge: 1,
                    request,
                    connected,
                    why: why.clone(),
                })),
            )
            .unwrap();
            assert_eq!(settle(&mut domain, &env).as_ref(), [Request::EraseCarried { number: carried.number }]);
            assert_eq!(domain.forge_connection(1), if connected { state } else { previous });
            assert_eq!(domain.forges().rows[0].ended, why);
            assert_eq!(
                domain.forges().rows[0].first.as_ref().map(|first| first.state),
                first.as_ref().map(|_| if connected {
                    forge::FirstState::Made { id: 42 }
                } else {
                    forge::FirstState::Failed { step: forge::CreateStep::File }
                })
            );
        }
    }
}
#[test]
fn admin_connect_role_refusals_deliver_the_carried_value_unused() {
    for role in [Some(people::Role::Member), Some(people::Role::Maintainer), Some(people::Role::Observer)] {
        let (mut domain, env) = admin_root(role);
        let carried = jig_domain_core::Carried { number: 1_u64 << 63_u32, digest: [73; 32] };
        crate::try_step(
            &mut domain,
            &env,
            public_request(
                forge::Ask::Connect { forge: 1, how: forge::How::ForgejoAdmin { first: None } },
                Some(carried),
            ),
        )
        .unwrap();
        let requests = settle(&mut domain, &env);
        assert!(
            requests
                .contains(&Request::Party(PartyRequest::Deliver(Delivery::CarriedUnused { number: carried.number })))
        );
        assert!(requests.contains(&Request::Party(PartyRequest::Deliver(Delivery::Application {
            to: ReplyTo::new(Token::new(90)),
            outcome: Some(crate::ApplicationAnswer::Connect(forge::ConnectAnswer::Refused(forge::ForgeRefusal::Role))),
        }))));
        assert!(!requests.iter().any(|r| if let Request::Connect { .. } = r { true } else { false }));
    }
}
