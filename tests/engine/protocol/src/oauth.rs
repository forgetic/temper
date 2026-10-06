//! Byte peers around the production OAuth owner. Actual socket settlement is
//! deliberately controlled independently of protocol and keeper terminals.
use skein_io as io;
use skein_lib::{
    Duration, Env, Queue, Time, Token, Wall,
    stream::{self, Read},
};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_protocol::{
    connection::Transport,
    credentials::{self, Config, Endpoint, Initial},
    oauth::{self, Effect, Event, Owner},
};
use temper_legacy_engine_domain::{self as engine, accounts};
pub const DOCUMENTS: temper_oauth::Limits = temper_oauth::Limits {
    document_bytes: 2048,
    string_bytes: 1024,
    token_bytes: 512,
    client_bytes: 64,
    detail_bytes: 64,
    record_bytes: 2048,
    depth: 8,
    tokens: 64,
};
pub const LIMITS: oauth::Limits = oauth::Limits {
    credentials: credentials::Limits { accounts: 2, identities: 2, endpoint_bytes: 64, documents: DOCUMENTS },
    operations: 4,
    http: skein_http::client::Limits { request: 1024, head: 1024, headers: 8, read: 7, send: 7 },
    timeout: Duration::from_secs(5),
};
#[must_use]
pub fn io_limits() -> io::Limits {
    io::Limits {
        sockets: 8,
        refusals: 2,
        intake: 1024,
        receive: 7,
        output: 1024,
        sends: 1024,
        accepts: 1,
        backlog: 4,
        close_timeout: Duration::from_secs(1),
        retry: Duration::from_millis(1),
    }
}
#[must_use]
pub fn config(initial: Initial) -> Config {
    Config {
        account: 1,
        kind: temper_oauth::AccountKind::Bearer,
        endpoint: Endpoint {
            address: crate::link::ADDR,
            host: b"issuer".as_slice().into(),
            path: b"/oauth/token".as_slice().into(),
            transport: Transport::Loopback,
        },
        client: b"client".as_slice().into(),
        initial,
    }
}
#[must_use]
pub fn refresh() -> Initial {
    Initial::Refresh { generation: 4, token: b"refresh-4".as_slice().into() }
}
#[expect(clippy::struct_excessive_bools, reason = "independent byte-peer delivery controls")]
pub struct Peer {
    pub socket: Option<Token>,
    pub sent: Vec<u8>,
    pub aborted: bool,
    pub finished: bool,
    pub blocked_body: bool,
    pub blocked_head: bool,
    rooms: u32,
    tape: Vec<u8>,
    offset: usize,
    demand: Read,
    credit: u32,
    eof: bool,
}
pub struct World {
    pub owner: Owner,
    pub now: Time,
    pub wall: Wall,
    pub limits: oauth::Limits,
    pub notices: Vec<engine::Event>,
    pub records: Vec<(Token, u32, u64, Box<[u8]>)>,
    pub cancelled_keeps: Vec<Token>,
    pub peers: BTreeMap<Token, Peer>,
    upper: Queue<engine::Event>,
    lower: Queue<Effect>,
    pending: VecDeque<Event>,
    sequence: u64,
}
impl World {
    #[must_use]
    #[expect(clippy::new_without_default, reason = "world construction makes its explicit fixture configuration")]
    pub fn new() -> World {
        Self::with(config(refresh()), LIMITS)
    }
    #[must_use]
    pub fn with(config: Config, limits: oauth::Limits) -> World {
        World {
            owner: Owner::new(Box::new([config]), Box::new([]), Time::ZERO, Wall::EPOCH, &limits, &io_limits())
                .expect("bounded configuration"),
            now: Time::ZERO,
            wall: Wall::EPOCH,
            limits,
            notices: Vec::new(),
            records: Vec::new(),
            cancelled_keeps: Vec::new(),
            peers: BTreeMap::new(),
            upper: Queue::with_capacity(oauth::MAX_UP),
            lower: Queue::with_capacity(oauth::MAX_OUT),
            pending: VecDeque::new(),
            sequence: 100,
        }
    }
    fn env(&self) -> Env<oauth::Limits> {
        Env { now: self.now, wall: self.wall, limits: self.limits }
    }
    pub fn request(&mut self, request: accounts::Request) {
        let env = self.env();
        oauth::down(&mut self.owner, &env, request, &mut self.upper, &mut self.lower).expect("valid domain request");
        self.collect();
        self.drive();
    }
    pub fn up(&mut self, event: Event) {
        let env = self.env();
        oauth::up(&mut self.owner, &env, event, &mut self.upper, &mut self.lower);
        self.collect();
        self.drive();
    }
    pub fn connected(&mut self, operation: Token) {
        self.sequence += 1;
        let socket = Token::new(self.sequence);
        self.peers.get_mut(&operation).expect("connecting peer").socket = Some(socket);
        self.up(Event::Io(io::Event::Connecting { owner: operation, socket }));
        self.up(Event::Io(io::Event::Connected { owner: operation }));
    }
    pub fn closed(&mut self, operation: Token) {
        self.up(Event::Io(io::Event::Closed { owner: operation }));
        self.owner.reclaim();
    }
    pub fn fire(&mut self, seconds: u64) {
        self.now = Time::from_nanos(seconds * 1_000_000_000);
        let env = self.env();
        oauth::fire(&mut self.owner, &env, &mut self.upper, &mut self.lower);
        self.collect();
        self.drive();
    }
    pub fn response(&mut self, operation: Token, status: u16, body: &[u8], retry: Option<&[u8]>, unknown: bool) {
        let peer = self.peers.get_mut(&operation).expect("connected peer");
        peer.tape
            .extend_from_slice(format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\n").as_bytes());
        if !unknown {
            peer.tape.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
        }
        if let Some(retry) = retry {
            peer.tape.extend_from_slice(b"Retry-After: ");
            peer.tape.extend_from_slice(retry);
            peer.tape.extend_from_slice(b"\r\n");
        }
        peer.tape.extend_from_slice(b"Connection: close\r\n\r\n");
        peer.tape.extend_from_slice(body);
        peer.eof = unknown;
        self.drive();
    }
    pub fn success(&mut self, operation: Token, valid: u64) {
        let body = temper_oauth::encode_response(
            &temper_oauth::TokenResponse {
                access_token: b"access-5".as_slice().into(),
                refresh_token: Some(b"refresh-5".as_slice().into()),
                expires_in: valid,
            },
            &DOCUMENTS,
        )
        .expect("response");
        self.response(operation, 200, &body, None, false);
    }
    pub fn drive(&mut self) {
        for _step in 0..10000 {
            if let Some(event) = self.pending.pop_front() {
                let env = self.env();
                oauth::up(&mut self.owner, &env, event, &mut self.upper, &mut self.lower);
                self.collect();
                continue;
            }
            if self.owner.is_ready() {
                let env = self.env();
                oauth::resume(&mut self.owner, &env, &mut self.upper, &mut self.lower);
                self.collect();
                continue;
            }
            let mut delivered = None;
            for (&operation, peer) in &mut self.peers {
                if let Some(event) = delivery(peer) {
                    delivered = Some(Event::Io(io::Event::Stream { owner: operation, up: event }));
                    break;
                }
            }
            if let Some(event) = delivered {
                self.pending.push_back(event);
                continue;
            }
            return;
        }
        panic!("byte world progress is bounded");
    }
    fn collect(&mut self) {
        while let Some(notice) = self.upper.pop() {
            self.notices.push(notice);
        }
        while let Some(effect) = self.lower.pop() {
            match effect {
                Effect::Io(io::Request::Connect { owner, .. }) => {
                    assert!(
                        self.peers
                            .insert(
                                owner,
                                Peer {
                                    socket: None,
                                    sent: Vec::new(),
                                    aborted: false,
                                    finished: false,
                                    blocked_body: false,
                                    blocked_head: false,
                                    rooms: 0,
                                    tape: Vec::new(),
                                    offset: 0,
                                    demand: Read::Nothing,
                                    credit: 0,
                                    eof: false
                                }
                            )
                            .is_none()
                    );
                }
                Effect::Io(io::Request::Stream { stream, down }) => {
                    let (&operation, peer) =
                        self.peers.iter_mut().find(|(_, peer)| peer.socket == Some(stream)).expect("socket binding");
                    match down {
                        stream::Down::Demand { read, room } => {
                            peer.demand = read;
                            if room > 0 && !peer.blocked_head && (!peer.blocked_body || peer.rooms == 0) {
                                peer.rooms += 1;
                                peer.credit = room;
                                self.pending
                                    .push_back(Event::Io(io::Event::Stream { owner: operation, up: stream::Up::Room }));
                            }
                        }
                        stream::Down::Send(bytes) => {
                            assert!(bytes.len() <= peer.credit as usize);
                            peer.credit -= u32::try_from(bytes.len()).expect("bounded send length");
                            peer.sent.extend_from_slice(&bytes);
                        }
                        stream::Down::Finish => peer.finished = true,
                    }
                }
                Effect::Io(io::Request::Abort { entity }) => {
                    self.peers
                        .values_mut()
                        .find(|peer| peer.socket == Some(entity))
                        .expect("aborted binding")
                        .aborted = true;
                }
                Effect::Keep { owner, account, generation, record } => {
                    self.records.push((owner, account, generation, record));
                }
                Effect::CancelKeep { owner } => self.cancelled_keeps.push(owner),
                Effect::Security(_)
                | Effect::Io(
                    io::Request::Listen { .. }
                    | io::Request::Bind { .. }
                    | io::Request::Reject { .. }
                    | io::Request::Close { .. }
                    | io::Request::Output { .. }
                    | io::Request::Spawn { .. }
                    | io::Request::Signal { .. },
                ) => panic!("plaintext connect effects only"),
            }
        }
    }
}
fn delivery(peer: &mut Peer) -> Option<stream::Up> {
    let rest = peer.tape.get(peer.offset..).expect("tape offset");
    let count = match peer.demand {
        Read::Nothing => return None,
        Read::Fill(count) => {
            if rest.len() >= count as usize {
                Some(count as usize)
            } else {
                None
            }
        }
        Read::Line { max } => {
            let end = rest.iter().position(|&byte| byte == b'\n').map(|index| index + 1);
            match end {
                Some(end) if end <= max as usize => Some(end),
                Some(_) | None if rest.len() >= max as usize => Some(max as usize),
                Some(_) | None => None,
            }
        }
        Read::Scan { until: delimiter, max } => {
            let delimiter = delimiter.as_bytes();
            let end = rest
                .windows(delimiter.len())
                .position(|window| window == delimiter)
                .map(|index| index + delimiter.len());
            match end {
                Some(end) if end <= max as usize => Some(end),
                Some(_) | None if rest.len() >= max as usize => Some(max as usize),
                Some(_) | None => None,
            }
        }
    };
    if let Some(count) = count {
        let bytes = rest[..count].into();
        peer.offset += count;
        peer.demand = Read::Nothing;
        return Some(stream::Up::Bytes(bytes));
    }
    if peer.eof {
        peer.demand = Read::Nothing;
        return Some(stream::Up::End);
    }
    None
}
