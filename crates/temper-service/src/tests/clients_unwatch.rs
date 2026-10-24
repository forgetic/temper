use super::fixtures;
use jig_client_protocol_wire::v19 as wire;
use jig_protocol_engine_clients as kit;
use skein_lib::{Env, List, Queue, Reader, Secret, Time, Token, Wall, Writer};

const STREAM: Token = Token::new(101);
const OPEN: Token = Token::new(102);
const CLOSE: Token = Token::new(103);
const WATCH: u64 = 17;
const ITERATIONS: u8 = 64;
const SIGN_IN: u64 = 41;
const EXPIRY: Wall = Wall::from_nanos(86_400_000_000_000);

struct Stand {
    component: kit::Component,
    env: Env<kit::Limits>,
    out: Queue<kit::Request>,
    credential: Secret,
}
impl Stand {
    fn new() -> Self {
        let config = fixtures::engine().jig;
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config.limits.clients };
        let mut served = List::with_capacity(1);
        served
            .push(kit::Served {
                family: None,
                first: jig_client_protocol_wire::VERSION,
                last: jig_client_protocol_wire::VERSION,
            })
            .unwrap();
        let settings = kit::Settings {
            provider: None,
            addresses: Box::new([]),
            key_bytes: config
                .limits
                .driver
                .client
                .document
                .token_bytes
                .min(config.limits.clients.wire.keyed_value_some),
            moved_bytes: jig_protocol_accounts::moved::bound(&config.limits.driver.client.document),
            redirect_bytes: config.limits.driver.client.uri_bytes,
            deployment_provider: 2,
            link: None,
            heartbeat: config.clients.heartbeat,
            rows: 32,
            served,
            providers: List::with_capacity(0),
            seed: Secret::new(Box::new([7; 32])),
            stream: 31,
            models: jig_protocol_config::clients::models(&config.llm),
        };
        let mut out = Queue::with_capacity(64);
        let component = kit::Component::new(settings, &env, &mut out).unwrap();
        while out.pop().is_some() {}
        let mut credential = Writer::new(40);
        credential.put(&SIGN_IN.to_be_bytes()).unwrap();
        credential.put(&[7; 32]).unwrap();
        Self { component, env, out, credential: Secret::new(credential.finish()) }
    }
    fn down(&mut self, event: kit::Event) {
        self.component.down(&self.env, event, &mut self.out);
    }
    fn authenticate(&mut self) {
        let Some(kit::Request::Secrets(kit::SecretsRequest::Get { owner })) = self.out.pop() else {
            panic!("credential digest requested")
        };
        assert_eq!(owner, kit::SecretOwner::SignIn(SIGN_IN));
        self.down(kit::Event::Secrets(kit::SecretsAnswer::Got {
            owner,
            value: Some(kit::Kept {
                digest: Secret::new(Box::new(skein_lib::sha256::digest(self.credential.expose()))),
                expires: EXPIRY,
            }),
        }));
        assert!(self.component.up(&self.env, &mut self.out));
    }
    fn exchange(&mut self, exchange: Token, message: kit::Message) {
        self.down(kit::Event::Transport(kit::FromTransport::Exchange {
            exchange,
            credential: Some(self.credential.clone()),
            message,
        }));
        self.authenticate();
    }
    fn stream(&mut self) -> [u8; 16] {
        self.down(kit::Event::Transport(kit::FromTransport::StreamOpen {
            stream: STREAM,
            credential: Some(self.credential.clone()),
        }));
        self.authenticate();
        assert!(self.component.up(&self.env, &mut self.out));
        let Some(kit::Request::Transport(kit::ToTransport::Streamed {
            stream: STREAM,
            message: kit::Streamed::Record(record),
        })) = self.out.pop()
        else {
            panic!("stream hello")
        };
        let wire::StreamEvent::Hello(hello) =
            wire::StreamEvent::decode(&self.env.limits.wire, &mut Reader::new(&record)).unwrap()
        else {
            panic!("hello record")
        };
        *hello.stream()
    }
    fn watch(&mut self, stream: [u8; 16]) -> Token {
        self.watch_subject(stream, wire::Subject::Me)
    }
    fn watch_subject(&mut self, stream: [u8; 16], watched: wire::Subject) -> Token {
        let mut subject = Writer::new(1);
        watched.encode(&mut subject).unwrap();
        let subject = wire::Owned::new(
            &self.env.limits.wire,
            wire::OwnedParts {
                owner: wire::Owner::Jig,
                version: jig_client_protocol_wire::VERSION,
                record: subject.finish(),
            },
        )
        .unwrap();
        let value = wire::Open::new(&self.env.limits.wire, wire::OpenParts { stream, watch: WATCH, subject }).unwrap();
        let mut bytes = Writer::new(usize::try_from(value.measure()).unwrap());
        value.encode(&mut bytes).unwrap();
        self.exchange(OPEN, kit::Message::Watch(bytes.finish()));
        let Some(kit::Request::Core(event)) = self.out.pop() else { panic!("core watch") };
        let Ok(temper_engine_domain::PartyInput::Watch { watcher, subject, .. }) =
            crate::clients::translation::to_root(*event)
        else {
            panic!("root watch")
        };
        if let wire::Subject::Accounts = watched {
            assert_eq!(subject, jig_domain_core::documents::Watched::Accounts);
        }
        self.down(kit::Event::Admitted { watcher });
        while self.component.up(&self.env, &mut self.out) {}
        while self.out.pop().is_some() {}
        watcher
    }
    fn unwatch(&mut self, stream: [u8; 16]) {
        let value =
            wire::WatchClose::new(&self.env.limits.wire, wire::WatchCloseParts { stream, watch: WATCH }).unwrap();
        let mut bytes = Writer::new(usize::try_from(value.measure()).unwrap());
        value.encode(&mut bytes).unwrap();
        self.exchange(CLOSE, kit::Message::Unwatch(bytes.finish()));
    }
}

#[test]
fn an_unwatch_after_its_watch_ends_is_answered_when_the_stream_goes() {
    let mut stand = Stand::new();
    let stream = stand.stream();
    let watcher = stand.watch(stream);
    stand.down(kit::Event::WatchEnded { watcher, why: jig_domain_views::End::Gone });
    stand.unwatch(stream);
    stand.down(kit::Event::Transport(kit::FromTransport::StreamGone { stream: STREAM }));
    let mut answer = None;
    for _ in 0..ITERATIONS {
        stand.component.up(&stand.env, &mut stand.out);
        while let Some(request) = stand.out.pop() {
            if let kit::Request::Transport(kit::ToTransport::Reply { exchange: CLOSE, reply }) = request {
                answer = Some(reply);
            }
        }
    }
    let Some(kit::Reply::Record(record)) = answer else { panic!("unwatch answer after the stream closes") };
    assert_eq!(
        wire::CloseAnswer::decode(&stand.env.limits.wire, &mut Reader::new(&record)),
        Ok(wire::CloseAnswer::Closed)
    );
}

#[test]
fn the_accounts_watch_reaches_the_root_and_returns_whole_cut_facts() {
    use jig_domain_accounts as a;
    use jig_domain_core::documents as d;
    use temper_engine_domain::{Delivery, PartyRequest};
    let mut stand = Stand::new();
    let stream = stand.stream();
    let watcher = stand.watch_subject(stream, wire::Subject::Accounts);
    stand.down(kit::Event::Transport(kit::FromTransport::StreamTaken { stream: STREAM }));
    let mut sources = List::with_capacity(1);
    sources
        .push(d::SourceLine {
            name: None,
            number: 9001,
            place: 2,
            kind: a::Kind::Key,
            state: d::AccountState::Later { until: None },
            usable: false,
            expiring: true,
            attention: true,
            spent_until: None,
        })
        .unwrap();
    let mut accounts = List::with_capacity(1);
    accounts
        .push(d::AccountLine {
            provider_name: Some(d::Text { bytes: b"Scripted".as_slice().into(), cut: false }),
            manage_usage: None,
            number: 7,
            place: 3,
            uses: None,
            plan: true,
            provider: Some(3),
            display: d::Text { bytes: b"Personal".as_slice().into(), cut: true },
            holder: a::Holder::Party(23),
            sources,
            cut: true,
        })
        .unwrap();
    let document = d::Document::Accounts(Box::new(d::Accounts {
        program_hosts: List::with_capacity(0),
        readiness: List::with_capacity(0),
        readiness_cut: false,
        accounts,
        signing: List::with_capacity(0),
        last_end: Some(d::LastEnd { source: 9001, end: a::End::Failed { failure: a::Failure::SignInAgain } }),
        usable: false,
        settled: true,
        cut: true,
    }));
    let request = PartyRequest::Deliver(Delivery::Document { watcher, document: Box::new(document) });
    stand.down(crate::clients::translation::to_component(request).expect("whole accounts delivery"));
    let mut delivered = false;
    for _ in 0..ITERATIONS {
        stand.component.up(&stand.env, &mut stand.out);
        while let Some(request) = stand.out.pop() {
            let kit::Request::Transport(kit::ToTransport::Streamed { message: kit::Streamed::Record(record), .. }) =
                request
            else {
                continue;
            };
            let wire::StreamEvent::Delivery(value) =
                wire::StreamEvent::decode(&stand.env.limits.wire, &mut Reader::new(&record)).unwrap()
            else {
                continue;
            };
            let wire::DeliveryValue::OwnerAccounts(value) =
                wire::DeliveryValue::decode(&stand.env.limits.wire, &mut Reader::new(value.document().record()))
                    .unwrap()
            else {
                panic!("accounts wire")
            };
            assert!(!value.usable());
            assert!(value.settled());
            assert!(value.cut());
            let mut pieces = Reader::new(value.accounts());
            let bytes = skein_codec::piece::split(&mut pieces).unwrap().expect("account piece");
            let line = wire::OwnerAccountLine::decode(&stand.env.limits.wire, &mut Reader::new(&bytes)).unwrap();
            assert_eq!(line.number(), 7);
            assert!(line.cut());
            assert_eq!(line.provider(), &Some(3));
            assert!(!line.sources().is_empty());
            delivered = true;
        }
    }
    assert!(delivered);
}
