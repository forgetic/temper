//! Exhaustive root routing to its protocol components.
use crate::{engine::Engine, stop};
use alloc::boxed::Box;
use temper_engine_domain as root;

pub(crate) mod box_link;

#[expect(clippy::too_many_lines, reason = "one exhaustive dispatcher routes root outputs to their components")]
pub(crate) fn request(engine: &mut Engine, request: root::Request) {
    if let Some(demo) = &mut engine.demo {
        demo.state = engine.domain.demo_state();
    }
    match request {
        root::Request::Bound { request, answer } => engine.clients.bound(request, answer),
        root::Request::StopGroup { forge } => {
            let mut out = skein_lib::Queue::with_capacity(1);
            engine.box_link.stop(forge, &mut out);
            box_link::drain(engine, &mut out);
        }
        root::Request::SetUpGroup { forge, request } => {
            let mut out = skein_lib::Queue::with_capacity(3);
            engine.box_link.ask(forge, request, &mut out);
            box_link::drain(engine, &mut out);
        }
        root::Request::Return { source, path } => {
            if let Some(request) = account_return(engine, source, &path)
                && let Some(accounts) = &mut engine.accounts
            {
                accounts.down(request);
            }
        }
        root::Request::SetUp { forge, request, org, repository } => {
            crate::setup::lifecycle::first(engine, forge, org, repository);
            let mut out = skein_lib::Queue::with_capacity(3);
            engine.box_link.ask(forge, request, &mut out);
            box_link::drain(engine, &mut out);
        }
        root::Request::Connect { forge, request, how, carried } => match how {
            temper_engine_domain_forge::How::GitHubApp { app } => {
                crate::setup::public::connect(engine, forge, request, app, carried);
            }
            temper_engine_domain_forge::How::ForgejoAdmin { first } => {
                crate::setup::public::admin_connect(engine, forge, request, first, carried);
            }
            temper_engine_domain_forge::How::GitHubManifest { .. } => unreachable!("manifest has its own door"),
        },
        root::Request::Manifest { forge, request, back } => {
            crate::setup::public::manifest(engine, forge, request, back);
        }
        root::Request::Reachable { forge, request, installation, page } => {
            crate::setup::public::reachable(engine, forge, request, installation, page);
        }
        root::Request::Install { forge, request, back } => crate::setup::public::install(engine, forge, request, back),
        root::Request::EraseCarried { number } => crate::setup::public::erase(engine, number),
        root::Request::Setup { run } => crate::setup::request(engine, run),
        root::Request::Store(request) => store(engine, request),
        root::Request::Account(request) => {
            if let Some(accounts) = &mut engine.accounts {
                accounts.request(request);
            }
        }
        root::Request::Forge { call, repository, op } => {
            if crate::forge::ended(engine, repository.forge) {
                engine.events.push(root::Event::Forge(Box::new(temper_engine_domain_forge::Event::Client(
                    skein_forge_client::Event::Answered {
                        call,
                        cost: 0,
                        result: Err(skein_forge::api::Error::NotConnected),
                        free: 0,
                        rate: None,
                    },
                ))));
            } else {
                engine.forge.down(skein_forge_connection::Request::Call { call, repository, op });
            }
        }
        root::Request::ToChild(released) => engine.events.push(root::Event::Released(released)),
        root::Request::Busy(event) => {
            use temper_engine_domain_forge::Event;
            #[expect(clippy::wildcard_enum_match_arm, reason = "only the routing producers below cross this boundary")]
            match *event {
                Event::Client(skein_forge_client::Event::Answered { call, cost, result, free, rate }) => {
                    engine.forge.down(skein_forge_connection::Request::Returned(skein_forge_connection::Answered {
                        call,
                        cost,
                        result,
                        free,
                        rate,
                    }));
                }
                // skein's forge-connection.md, section 2: no returned-hint request; never offer it again.
                // protocol/forge.md, 2: these views are not kept; restart publishes connections and skein's forge-connection.md, 4.4's baseline.
                Event::AppRecorded { .. }
                | Event::Hint { .. }
                | Event::Connection { .. }
                | Event::Installation { .. }
                | Event::BoxClosed
                | Event::ConnectAnswered { .. }
                | Event::InstallAnswered { .. }
                | Event::ConnectEnded { .. }
                | Event::SetUpAnswered { .. }
                | Event::FirstMade { .. }
                | Event::FirstFailed { .. } => {}
                other => unreachable!("only forge, setup and box link routing produce root forge inputs: {other:?}"),
            }
        }
        root::Request::Drained => stop::drained(engine),
        root::Request::Stop => stop::root(engine),
        root::Request::Worker(request) => worker(engine, request),
        root::Request::Party(request) => {
            if let root::PartyRequest::WatchRefused { watcher, .. } = &request
                && engine.admission == Some(*watcher)
            {
                engine.admission = None;
            }
            let event = match request {
                root::PartyRequest::Deliver(root::Delivery::DemoNote { line }) => {
                    engine.notes.push(crate::Note::Demo { line });
                    None
                }
                root::PartyRequest::Deliver(root::Delivery::Forges { watcher, forges }) => {
                    match crate::family::forges(forges, &temper_client_protocol_wire::v1::CEILINGS) {
                        Some(record) => Some(jig_protocol_engine_clients::Event::Application(
                            jig_protocol_engine_clients::FromApplication::Delivery {
                                watcher,
                                family: temper_client_protocol_wire::FORGE,
                                record,
                            },
                        )),
                        None => Some(jig_protocol_engine_clients::Event::Abort),
                    }
                }
                root::PartyRequest::Deliver(root::Delivery::Panel { watcher, panel }) => {
                    match crate::family::panel(
                        *panel,
                        &engine.workers.forges,
                        &temper_client_protocol_wire::v1::CEILINGS,
                    ) {
                        Some(record) => Some(jig_protocol_engine_clients::Event::Application(
                            jig_protocol_engine_clients::FromApplication::Delivery {
                                watcher,
                                family: temper_client_protocol_wire::FORGE,
                                record,
                            },
                        )),
                        None => Some(jig_protocol_engine_clients::Event::Abort),
                    }
                }
                root::PartyRequest::Deliver(root::Delivery::Application { to, outcome }) => {
                    match crate::family::answer(
                        outcome,
                        &temper_client_protocol_wire::v1::CEILINGS,
                        &engine.clients.env.limits.wire,
                    ) {
                        Some(record) => Some(jig_protocol_engine_clients::Event::Application(
                            jig_protocol_engine_clients::FromApplication::Answered {
                                to,
                                family: temper_client_protocol_wire::FORGE,
                                record,
                            },
                        )),
                        None => Some(jig_protocol_engine_clients::Event::Abort),
                    }
                }
                request @ (root::PartyRequest::SignInRefused { .. }
                | root::PartyRequest::WatchRefused { .. }
                | root::PartyRequest::RunText { .. }
                | root::PartyRequest::Deliver(_)) => crate::clients::translation::to_component(request),
            };
            if let Some(event) = event {
                engine.clients.down(event);
            }
        }
    }
}

/// Only the listener's bound socket supplies the origin of a checked client path.
fn account_return(engine: &Engine, source: u32, path: &[u8]) -> Option<skein_oauth_accounts::Request> {
    let listening = engine.clients.bound_address()?;
    let host = crate::clients::printed::address_text(listening)?;
    let parts: [&[u8]; 3] = [b"http://", &host, path];
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len())?;
    }
    let mut location = skein_lib::Writer::new(length);
    for part in parts {
        location.put(part).expect("measured return URL");
    }
    Some(skein_oauth_accounts::Request::Return { account: source, location: location.finish() })
}

fn store(engine: &mut Engine, request: root::StoreRequest) {
    if let root::StoreRequest::Commit { number, writes } = &request {
        engine.clients.commit(*number, writes);
    }
    if !engine.store.down(request) {
        stop::unavailable(engine, b"deployment");
    }
}

fn worker(engine: &mut Engine, request: root::WorkerRequest) {
    match request {
        root::WorkerRequest::Host(request) => match *request {
            root::HostRequest::Decode { to, task, attempt, call } => {
                engine.events.push(crate::tools::decode(to, task, attempt, &call));
            }
            root::HostRequest::Dropped { .. } => {}
        },
        root::WorkerRequest::ReadResult { payload, outcome } => {
            #[expect(clippy::manual_map, reason = "the step subset uses explicit matches")]
            let change = match engine.domain.change_resource(payload) {
                Some((connector, kind, resource)) => {
                    Some(temper_engine_smith::ChangeResource { connector, kind, resource })
                }
                None => None,
            };
            engine.events.push(crate::tools::result(payload, &outcome, change));
        }
        root::WorkerRequest::Deliver(delivery) => match delivery {
            root::Delivery::Assigned { channel, assignment } => {
                engine.workers.assignment(engine.env.now, channel, assignment, &engine.values, &mut engine.events);
            }
            root::Delivery::Link { channel, down } => {
                engine.workers.send(engine.env.now, channel, down, &engine.values, &mut engine.events);
            }
            root::Delivery::TurnBusy { channel, task, attempt, turn } => engine.workers.down(
                engine.env.now,
                jig_protocol_engine_workers::Event::TurnBusy {
                    channel,
                    run: skein_lib::Token::new(task),
                    attempt: skein_lib::Token::new(attempt),
                    turn,
                },
                &mut engine.events,
            ),
            root::Delivery::Refuse { channel } => engine.workers.down(
                engine.env.now,
                jig_protocol_engine_workers::Event::NotAdmitted { channel, text: b"not admitted".as_slice().into() },
                &mut engine.events,
            ),
            root::Delivery::Host(delivery) => match *delivery {
                root::HostDelivery::Render { to, answer, .. } => engine.events.push(crate::tools::render(to, &answer)),
                root::HostDelivery::Answer { channel, task, attempt, name, call } => engine.workers.send(
                    engine.env.now,
                    channel,
                    jig_domain_host::LinkDown::Relayed {
                        run: skein_lib::Token::new(task),
                        attempt: skein_lib::Token::new(attempt),
                        call: name,
                        answer: crate::tools::settled(call.answer),
                    },
                    &engine.values,
                    &mut engine.events,
                ),
            },
            root::Delivery::DemoActed { .. }
            | root::Delivery::DemoNote { .. }
            | root::Delivery::Procedure { .. }
            | root::Delivery::Application { .. }
            | root::Delivery::Forges { .. }
            | root::Delivery::Panel { .. }
            | root::Delivery::WebReply { .. }
            | root::Delivery::CarriedUnused { .. }
            | root::Delivery::Read { .. }
            | root::Delivery::Document { .. }
            | root::Delivery::WatchEnded { .. }
            | root::Delivery::SourceSignedOut { .. }
            | root::Delivery::Result { .. }
            | root::Delivery::ProjectMade { .. }
            | root::Delivery::ForgeCommitted { .. }
            | root::Delivery::ForgeCall { .. }
            | root::Delivery::Bound { .. }
            | root::Delivery::Relay { .. }
            | root::Delivery::ReadEscalationDecision { .. }
            | root::Delivery::Fleet(_)
            | root::Delivery::ReadLoad { .. }
            | root::Delivery::Load { .. }
            | root::Delivery::Drained => stop::unavailable(engine, b"workers"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::account_return;
    use crate::tests::fixtures;

    #[test]
    fn a_return_uses_the_clients_bound_origin_and_the_checked_path() {
        for (address, expected) in [
            (
                ([127, 0, 0, 1], 8123).into(),
                b"http://127.0.0.1:8123/accounts/connect?provider=17&way=sign-in&source=9".as_slice(),
            ),
            (
                ([0, 0, 0, 0, 0, 0, 0, 1], 8123).into(),
                b"http://[::1]:8123/accounts/connect?provider=17&way=sign-in&source=9".as_slice(),
            ),
        ] {
            let mut service = fixtures::started();
            let engine = service.engine.as_mut().expect("engine");
            assert!(engine.clients.up(skein_io::Event::Listening {
                owner: skein_lib::Token::new(0),
                listener: skein_lib::Token::new(77),
                addr: skein_io::Addr::Ip(address),
            }));
            assert_eq!(engine.clients.listening.take(), Some(address), "the system consumes the note");
            let path = b"/accounts/connect?provider=17&way=sign-in&source=9";
            let Some(skein_oauth_accounts::Request::Return { account, location }) = account_return(engine, 9, path)
            else {
                panic!("driver return")
            };
            assert_eq!(account, 9);
            assert_eq!(location.as_ref(), expected);
        }
    }

    #[test]
    fn a_return_without_a_bound_clients_listener_is_dropped() {
        let mut service = fixtures::started();
        let engine = service.engine.as_mut().expect("engine");
        engine.clients.listening = None;
        assert!(account_return(engine, 9, b"/accounts/connect").is_none());
    }
}
