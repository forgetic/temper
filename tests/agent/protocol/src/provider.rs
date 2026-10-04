//! Actual provider HTTP/SSE machines joined by byte ledgers. The fake side
//! receives its neutral documents independently of the client's tool encoder.
use crate::{fixture, http};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, Intake, Queue, ReplyTo, Time, Token, Wall};
use temper_channel::wire::Provider;
use temper_fake_llm_domain::{self as domain, api};
use temper_fake_llm_protocol::{documents, oauth, provider};

pub const ACCESS: &[u8] =
    b"e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC03In0sImV4cCI6MTIwfQ.AA";
pub const ACCOUNT: &[u8] = b"acct-7";

#[must_use]
pub fn limits() -> provider::Limits {
    let agent = fixture::limits();
    provider::Limits {
        calls: 2,
        http: skein_http::server::Limits {
            head: agent.head_bytes,
            headers: agent.headers,
            body: u64::from(agent.request_bytes),
            response: agent.head_bytes,
            read: agent.chunk,
            send: agent.chunk,
        },
        sse: skein_http::sse::writer::Limits { event: agent.event_bytes, chunk: agent.chunk },
        documents: documents::Limits { anthropic: agent.anthropic(), openai: agent.openai(), model_ceiling: 100 },
    }
}
#[must_use]
pub fn issuer_limits() -> oauth::Limits {
    oauth::Limits {
        http: skein_http::server::Limits { head: 1024, headers: 8, body: 2048, response: 1024, read: 128, send: 128 },
        documents: temper_oauth::Limits {
            document_bytes: 2048,
            string_bytes: 1024,
            token_bytes: 256,
            client_bytes: 64,
            detail_bytes: 128,
            record_bytes: 2048,
            depth: 8,
            tokens: 64,
        },
        plans: 2,
        rotations: 4,
    }
}

/// A real refresh request populates the shared issuer's access-token state.
/// This fixture is synthetic and contains no real credential value.
#[must_use]
pub fn issued() -> oauth::Issuer {
    let limits = issuer_limits();
    let mut issuer = oauth::Issuer::new(
        oauth::Config {
            path: b"/oauth/token".as_slice().into(),
            client_id: b"client".as_slice().into(),
            refresh_token: b"refresh".as_slice().into(),
        },
        &limits,
    )
    .expect("issuer bounds");
    assert!(
        issuer
            .queue(
                oauth::Plan {
                    status: 200,
                    body: oauth::Body::Token(temper_oauth::TokenResponse {
                        access_token: ACCESS.into(),
                        refresh_token: Some(b"rotated".as_slice().into()),
                        expires_in: 60,
                    }),
                    retry_after: None,
                    head_delay: Duration::ZERO,
                    body_delay: Duration::ZERO,
                },
                &limits
            )
            .is_ok()
    );
    let body = br#"{"client_id":"client","grant_type":"refresh_token","refresh_token":"refresh"}"#;
    let request = format!(
        "POST /oauth/token HTTP/1.1\r\nHost: issuer\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut ledger = Ledger::new(1024, 4096);
    ledger.append(request.as_bytes());
    ledger.append(body);
    let mut server = oauth::Server::new(&limits).expect("server bounds");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut upper = Queue::with_capacity(oauth::MAX_UP);
    let mut lower = Queue::with_capacity(oauth::MAX_DOWN);
    oauth::start(&mut server, &mut issuer, &env, &mut upper, &mut lower);
    for _step in 0..10_000 {
        ledger.collect(&mut lower);
        while upper.pop().is_some() {}
        if server.has_work(&issuer, env.now) {
            oauth::resume(&mut server, &mut issuer, &env, &mut upper, &mut lower);
        } else if let Some(event) = ledger.delivery() {
            oauth::up(&mut server, &mut issuer, &env, event, &mut upper, &mut lower);
        } else {
            break;
        }
    }
    assert!(issuer.authorize(ACCESS, Time::ZERO));
    assert_eq!(issuer.posts(), 1);
    oauth::close(&mut server, &env, &mut upper, &mut lower);
    issuer
}

pub struct Ledger {
    pub output: Vec<u8>,
    pub room_enabled: bool,
    intake: Intake,
    source: Vec<u8>,
    offset: usize,
    read: Read,
    wanted: u32,
    credit: u32,
    slice: usize,
}
impl Ledger {
    #[must_use]
    pub fn new(cap: u32, slice: usize) -> Ledger {
        Ledger {
            output: Vec::new(),
            room_enabled: true,
            intake: Intake::with_capacity(cap),
            source: Vec::new(),
            offset: 0,
            read: Read::Nothing,
            wanted: 0,
            credit: 0,
            slice,
        }
    }
    pub fn append(&mut self, bytes: &[u8]) {
        self.source.extend_from_slice(bytes);
    }
    pub fn collect(&mut self, lower: &mut Queue<Down>) {
        while let Some(down) = lower.pop() {
            match down {
                Down::Demand { read, room } => {
                    self.read = read;
                    self.wanted = room;
                    if read == Read::Nothing && room == 0 {
                        self.credit = 0;
                    }
                }
                Down::Send(bytes) => {
                    let n = u32::try_from(bytes.len()).expect("bounded output");
                    assert!(n <= self.credit, "output must fit exact granted room");
                    self.credit -= n;
                    self.output.extend_from_slice(&bytes);
                }
                Down::Finish => {}
            }
        }
    }
    pub fn delivery(&mut self) -> Option<Up> {
        if self.wanted > 0 && self.room_enabled {
            self.credit = self.wanted;
            self.wanted = 0;
            return Some(Up::Room);
        }
        if self.read == Read::Nothing {
            return None;
        }
        while self.offset < self.source.len() {
            let n = self.slice.max(1).min(self.intake.room() as usize).min(self.source.len() - self.offset);
            if n == 0 {
                break;
            }
            self.intake.append(&self.source[self.offset..self.offset + n]).expect("intake room");
            self.offset += n;
            if let Some(bytes) = self.intake.meet(self.read) {
                self.read = Read::Nothing;
                return Some(Up::Bytes(bytes));
            }
        }
        if let Some(bytes) = self.intake.meet(self.read) {
            self.read = Read::Nothing;
            Some(Up::Bytes(bytes))
        } else {
            None
        }
    }
}

pub struct World {
    pub client: http::World,
    pub server: provider::Server,
    pub service: provider::Service,
    pub issuer: oauth::Issuer,
    pub queries: Vec<api::Query>,
    pub replies: Vec<ReplyTo>,
    pub ledger: Ledger,
    pub env: Env<provider::Limits>,
    pub upper: Queue<provider::Event>,
    pub lower: Queue<Down>,
    sent: usize,
    received: usize,
}
impl World {
    #[must_use]
    pub fn new(provider: Provider, slice: usize) -> World {
        let limits = limits();
        let kind = match provider {
            Provider::Anthropic => documents::Provider::Anthropic,
            Provider::OpenAi => documents::Provider::OpenAi,
        };
        let service = provider::Service::new(
            provider::Config { provider: kind, path: b"/responses".as_slice().into(), headers: Box::new([]) },
            &limits,
        )
        .expect("service bounds");
        let mut client = http::World::with(http::prepared_value(provider, 1, ACCESS, ACCOUNT), Vec::new(), slice);
        client.live = true;
        let mut world = World {
            client,
            server: provider::Server::new(Token::new(10), &limits).expect("server bounds"),
            service,
            issuer: issued(),
            queries: Vec::new(),
            replies: Vec::new(),
            ledger: Ledger::new(limits.http.head, slice),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            upper: Queue::with_capacity(provider::MAX_UP),
            lower: Queue::with_capacity(provider::MAX_DOWN),
            sent: 0,
            received: 0,
        };
        provider::start(
            &mut world.server,
            &mut world.service,
            &world.issuer,
            &world.env,
            &mut world.upper,
            &mut world.lower,
        );
        world.collect();
        world
    }
    pub fn collect(&mut self) {
        self.ledger.collect(&mut self.lower);
        while let Some(event) = self.upper.pop() {
            match event {
                provider::Event::Domain(domain::Event::Call { reply_to, query }) => {
                    self.replies.push(reply_to);
                    self.queries.push(query);
                }
                provider::Event::Close | provider::Event::Closed => {}
            }
        }
        if self.sent < self.client.sent.len() {
            self.ledger.append(&self.client.sent[self.sent..]);
            self.sent = self.client.sent.len();
        }
        if self.received < self.ledger.output.len() {
            self.client.append(&self.ledger.output[self.received..]);
            self.received = self.ledger.output.len();
        }
    }
    pub fn tick(&mut self) -> bool {
        let mut progress = self.client.pump();
        self.collect();
        if self.server.has_work() {
            provider::resume(
                &mut self.server,
                &mut self.service,
                &self.issuer,
                &self.env,
                &mut self.upper,
                &mut self.lower,
            );
            progress = true;
        } else if let Some(event) = self.ledger.delivery() {
            provider::up(
                &mut self.server,
                &mut self.service,
                &self.issuer,
                &self.env,
                event,
                &mut self.upper,
                &mut self.lower,
            );
            progress = true;
        }
        self.collect();
        progress
    }
    pub fn drive(&mut self) {
        for _step in 0..100_000 {
            if !self.tick() {
                return;
            }
        }
        panic!("provider pair did not settle");
    }
    pub fn answer(&mut self, result: Result<api::Answer, api::Error>) {
        let to = self.replies.pop().expect("neutral domain owns a call");
        provider::down(
            &mut self.server,
            &mut self.service,
            &self.issuer,
            &self.env,
            domain::Request::Reply { to, result },
            &mut self.upper,
            &mut self.lower,
        );
        self.collect();
    }
    pub fn close(&mut self) {
        provider::close(&mut self.server, &mut self.service, &self.env, &mut self.upper, &mut self.lower);
        self.client.cancel();
    }
}
