//! Plaintext byte peers and explicit TLS-owner completions around the real listener.
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use skein_io::{self as io, kernel::Addr};
use skein_lib::{Env, Queue, Time, Token, Wall, stream};
use std::collections::BTreeMap;
use temper_channel::{Sizes, codec, machine::Endpoint, sizes, wire};
use temper_engine_protocol::{
    Limits,
    connection::{Phase, Transport},
    listener::{self, Effect, Event, Listener, Notice, Security, Worker},
    translate::{Repository, Value},
};
use temper_legacy_engine_domain as engine;

pub const LIMITS: Limits = Limits {
    connections: 3,
    workers: 2,
    repositories: 2,
    channel: temper_channel::Limits {
        chunk: 7,
        handshake: skein_lib::Duration::from_secs(5),
        hello: skein_lib::Duration::from_secs(5),
        ping: skein_lib::Duration::from_secs(3),
        silence: skein_lib::Duration::from_secs(9),
        stall: skein_lib::Duration::from_secs(6),
        ..temper_channel::Limits::STARTING
    },
};
pub const SIZES: Sizes =
    Sizes { inbox: 2, run_calls: 2, stalled: 2, bounces: 2, facts: 2, agent_outbox: 4, ..crate::SIZES };
pub const ADDR: Addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4242));

#[must_use]
pub fn io_limits(limits: &Limits, sizes: &Sizes) -> io::Limits {
    let cap = temper_channel::sizes::output_cap(Endpoint::Engine, sizes).expect("configured channel cap");
    io::Limits {
        sockets: limits.connections.checked_add(1).expect("small fixture connection count"),
        refusals: 1,
        intake: limits.channel.chunk.max(8),
        receive: limits.channel.chunk,
        output: cap,
        sends: temper_channel::sizes::stream_slots(Endpoint::Engine, sizes).expect("configured frame slots"),
        accepts: 1,
        backlog: 4,
        close_timeout: skein_lib::Duration::from_secs(1),
        retry: skein_lib::Duration::from_millis(1),
    }
}

#[must_use]
pub fn workers() -> Box<[Worker]> {
    Box::new([
        Worker { name: b"alpha".as_slice().into(), secret: b"one".as_slice().into() },
        Worker { name: b"beta".as_slice().into(), secret: b"two".as_slice().into() },
    ])
}
#[must_use]
pub fn repositories() -> Box<[Repository]> {
    Box::new([
        Repository { name: b"zero".as_slice().into(), remote: b"acme/zero".as_slice().into(), identity: 8 },
        Repository { name: b"one".as_slice().into(), remote: b"acme/one".as_slice().into(), identity: 9 },
    ])
}

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent diagnostic observations and injected output blocking")]
pub struct Peer {
    pub owner: Option<Token>,
    pub frames: Vec<wire::Message>,
    pub closing: bool,
    pub aborted: bool,
    pub rejected: bool,
    pub blocked: bool,
    demand: Option<(stream::Read, u32)>,
    credit: u32,
    tape: Vec<u8>,
    from: usize,
}

pub struct World {
    pub listener: Listener,
    pub limits: Limits,
    pub now: Time,
    pub notices: Vec<Notice>,
    pub peers: BTreeMap<Token, Peer>,
    pub security: Vec<Security>,
    transport: Transport,
    sequence: u64,
    upper: Queue<Notice>,
    lower: Queue<Effect>,
}

impl World {
    #[must_use]
    pub fn new(transport: Transport) -> World {
        Self::limited(transport, LIMITS)
    }
    #[must_use]
    pub fn limited(transport: Transport, limits: Limits) -> World {
        let listener =
            Listener::new(workers(), repositories(), transport, &limits, &SIZES, &io_limits(&limits, &SIZES))
                .expect("small configured listener fits");
        let mut world = World {
            listener,
            limits,
            now: Time::ZERO,
            notices: Vec::new(),
            peers: BTreeMap::new(),
            security: Vec::new(),
            transport,
            sequence: 100,
            upper: Queue::with_capacity(listener::MAX_UP),
            lower: Queue::with_capacity(listener::MAX_OUT),
        };
        assert!(listener::start(&mut world.listener, ADDR, &mut world.lower));
        world.drain();
        world.input(Event::Io(io::Event::Listening { owner: listener::OWNER, listener: Token::new(7), addr: ADDR }));
        world
    }
    fn env(&self) -> Env<Limits> {
        Env { now: self.now, wall: Wall::EPOCH, limits: self.limits }
    }
    pub fn input(&mut self, event: Event) {
        let env = self.env();
        listener::up(&mut self.listener, &env, &SIZES, event, &mut self.upper, &mut self.lower);
        self.drain();
        self.listener.reclaim();
    }
    pub fn seconds(&mut self, seconds: u64) {
        self.now = Time::from_nanos(seconds.saturating_mul(1_000_000_000));
    }
    pub fn fire(&mut self) {
        for _ in 0..16 {
            if !self.listener.is_due(self.now) {
                return;
            }
            let env = self.env();
            listener::fire(&mut self.listener, &env, &SIZES, &mut self.lower);
            self.drain();
            self.settle();
        }
        panic!("due timers settle without blocked-ping loops");
    }
    pub fn accept(&mut self) -> (Token, Option<Token>) {
        self.sequence += 1;
        let socket = Token::new(self.sequence);
        self.peers.insert(
            socket,
            Peer {
                owner: None,
                frames: Vec::new(),
                closing: false,
                aborted: false,
                rejected: false,
                blocked: false,
                demand: None,
                credit: 0,
                tape: Vec::new(),
                from: 0,
            },
        );
        self.input(Event::Io(io::Event::Accepted { owner: listener::OWNER, socket, peer: ADDR }));
        (socket, self.peers[&socket].owner)
    }
    pub fn append(&mut self, socket: Token, message: wire::Message) {
        let bytes = codec::encode(&message, &SIZES).expect("scripted peer frame fits");
        drop(message);
        self.peers.get_mut(&socket).expect("accepted peer").tape.extend_from_slice(&bytes);
    }
    pub fn opening(&mut self, socket: Token, name: &[u8], secret: &[u8], terms: bool, hello: bool) {
        self.append(
            socket,
            wire::Message::Open {
                open: wire::Open {
                    channel: wire::Channel::Link,
                    lowest: 1,
                    highest: 1,
                    name: name.into(),
                    secret: secret.into(),
                },
            },
        );
        if terms {
            self.append(
                socket,
                wire::Message::Terms { terms: sizes::terms(Endpoint::WorkerLink, &SIZES).expect("terms fit") },
            );
        }
        if hello {
            self.append(socket, wire::Message::Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) });
        }
    }
    pub fn open(&mut self, name: &[u8], secret: &[u8]) -> (Token, Token) {
        let (socket, owner) = self.accept();
        let owner = owner.expect("connection admitted");
        if self.transport == Transport::Secured {
            self.input(Event::Secured { owner });
        }
        self.opening(socket, name, secret, true, true);
        self.settle();
        (socket, owner)
    }
    pub fn closed(&mut self, socket: Token) {
        let owner = self.peers[&socket].owner.expect("a bound socket");
        self.input(Event::Io(io::Event::Closed { owner }));
    }
    pub fn down(&mut self, request: engine::Request, values: &[Value]) {
        let env = self.env();
        listener::down(&mut self.listener, &env, &SIZES, request, values, &mut self.lower);
        self.drain();
        self.settle();
    }
    pub fn shutdown(&mut self) {
        listener::close(&mut self.listener, &mut self.lower);
        self.drain();
        self.settle();
    }
    pub fn settle(&mut self) {
        for _ in 0..4096 {
            if self.listener.is_ready() {
                let env = self.env();
                listener::resume(&mut self.listener, &env, &SIZES, &mut self.upper, &mut self.lower);
                self.drain();
                self.listener.reclaim();
                continue;
            }
            let mut delivery = None;
            for peer in self.peers.values_mut() {
                if peer.closing {
                    continue;
                }
                let Some((read, room)) = peer.demand else {
                    continue;
                };
                if room > 0 && !peer.blocked {
                    peer.demand = None;
                    peer.credit = room;
                    delivery = Some((peer.owner.expect("bound peer"), stream::Up::Room));
                    break;
                }
                if let stream::Read::Fill(count) = read {
                    let end = peer.from + usize::try_from(count).expect("small chunk");
                    if end <= peer.tape.len() {
                        let bytes = peer.tape[peer.from..end].into();
                        peer.from = end;
                        peer.demand = None;
                        delivery = Some((peer.owner.expect("bound peer"), stream::Up::Bytes(bytes)));
                        break;
                    }
                }
            }
            let Some((owner, up)) = delivery else {
                return;
            };
            let event = match self.transport {
                Transport::Loopback => Event::Io(io::Event::Stream { owner, up }),
                Transport::Secured => Event::Plain { owner, up },
            };
            self.input(event);
        }
        panic!("finite scripted bytes and blocked outputs settle");
    }
    fn stream(&mut self, socket: Token, down: stream::Down) {
        let peer = self.peers.get_mut(&socket).expect("stream has a bound peer");
        match down {
            stream::Down::Demand { read: stream::Read::Nothing, room: 0 } => peer.demand = None,
            stream::Down::Demand { read, room } => {
                assert!(peer.demand.is_none(), "one outstanding demand");
                peer.demand = Some((read, room));
            }
            stream::Down::Send(bytes) => {
                let length = u32::try_from(bytes.len()).expect("small frame");
                assert!(length <= peer.credit, "stream output has credit");
                peer.credit -= length;
                peer.frames.push(codec::decode(&bytes, &SIZES).expect("production output decodes"));
            }
            stream::Down::Finish => peer.closing = true,
        }
    }
    fn socket(&self, owner: Token) -> Token {
        self.peers
            .iter()
            .find_map(|(&socket, peer)| (peer.owner == Some(owner)).then_some(socket))
            .expect("owner has a socket")
    }
    fn drain(&mut self) {
        while let Some(effect) = self.lower.pop() {
            match effect {
                Effect::Io(io::Request::Bind { socket, owner }) => {
                    self.peers.get_mut(&socket).expect("accepted socket").owner = Some(owner);
                }
                Effect::Io(io::Request::Reject { socket }) => {
                    self.peers.get_mut(&socket).expect("accepted socket").rejected = true;
                }
                Effect::Io(io::Request::Stream { stream, down }) => self.stream(stream, down),
                Effect::Io(io::Request::Close { entity }) => {
                    if let Some(peer) = self.peers.get_mut(&entity) {
                        peer.closing = true;
                    }
                }
                Effect::Io(io::Request::Abort { entity }) => {
                    if let Some(peer) = self.peers.get_mut(&entity) {
                        peer.aborted = true;
                        peer.closing = true;
                    }
                }
                Effect::Io(io::Request::Listen { .. } | io::Request::Connect { .. }) => {}
                Effect::Security(Security::Output { owner, down }) => {
                    let socket = self.socket(owner);
                    self.stream(socket, down);
                }
                Effect::Security(
                    security @ (Security::Start { .. }
                    | Security::Input { .. }
                    | Security::Stop { .. }
                    | Security::Closed { .. }),
                ) => self.security.push(security),
            }
        }
        while let Some(notice) = self.upper.pop() {
            self.notices.push(notice);
        }
    }
    #[must_use]
    pub fn phase(&self, owner: Token) -> Option<Phase> {
        self.listener.phase(owner)
    }
}

/// Byte-level races with bounded admissions and delayed actual closure. The
/// referee checks each announced channel and terminal against the live io binding.
#[must_use]
pub fn random(seed: u64, steps: u32) -> (Vec<String>, [u32; 4]) {
    use skein_lib::Rng;
    use std::collections::BTreeSet;
    let mut rng = Rng::new(seed);
    let mut world = World::new(Transport::Loopback);
    let mut trace = temper_world::Trace::default();
    let mut hello = BTreeSet::new();
    let mut lost = BTreeSet::new();
    let mut observed = 0;
    let mut stats = [0_u32; 4];
    for step in 0..steps {
        let action = rng.between(0, 6);
        let live: Vec<_> = world
            .peers
            .iter()
            .filter_map(|(&socket, peer)| {
                let owner = peer.owner?;
                world.phase(owner).map(|phase| (socket, owner, phase))
            })
            .collect();
        let chosen = if live.is_empty() {
            None
        } else {
            Some(
                live[usize::try_from(rng.between(0, u64::try_from(live.len()).expect("bounded peers") - 1))
                    .expect("bounded index")],
            )
        };
        match (action, chosen) {
            (0, _) => {
                let which = rng.between(0, 2);
                let (name, secret) = match which {
                    0 => (b"alpha".as_slice(), b"one".as_slice()),
                    1 => (b"beta".as_slice(), b"two".as_slice()),
                    2 => (b"alpha".as_slice(), b"bad".as_slice()),
                    _ => unreachable!("draw bounded by two"),
                };
                let (socket, owner) = world.accept();
                if owner.is_some() {
                    world.opening(socket, name, secret, true, true);
                }
                stats[0] += u32::from(owner.is_some());
            }
            (1, _) => {
                let (socket, owner) = world.accept();
                if owner.is_some() {
                    world.opening(socket, b"alpha", b"one", rng.between(0, 1) == 1, false);
                }
                stats[0] += u32::from(owner.is_some());
            }
            (2, Some((socket, _, Phase::Open))) => world.append(socket, wire::Message::Ping),
            (3, Some((socket, _, _))) => {
                let peer = world.peers.get_mut(&socket).expect("peer");
                peer.blocked = !peer.blocked;
            }
            (4, Some((_, owner, Phase::Open))) => world.down(
                engine::Request::Cancel { channel: owner, item: engine::Item { repository: 0, number: 1 }, attempt: 1 },
                &[],
            ),
            (5, Some((socket, _, Phase::Closing))) => {
                world.closed(socket);
                stats[1] += 1;
            }
            (6, _) => {
                let seconds = world.now.as_nanos() / 1_000_000_000 + rng.between(1, 3);
                world.seconds(seconds);
                world.fire();
            }
            (1..=5, _) => {}
            _ => unreachable!("draw bounded by six"),
        }
        world.settle();
        assert!(world.listener.connections() <= LIMITS.connections, "seed {seed}, step {step}: admitted bound");
        for notice in &world.notices[observed..] {
            match notice {
                Notice::Domain(engine::Event::Hello { channel, .. }) => {
                    assert!(hello.insert(*channel), "one Hello per channel");
                    assert_eq!(world.phase(*channel), Some(Phase::Open));
                    stats[2] += 1;
                }
                Notice::Domain(engine::Event::Lost { channel }) => {
                    assert!(hello.contains(channel), "only announced channels are lost");
                    assert!(lost.insert(*channel), "Lost occurs once");
                    assert_eq!(world.phase(*channel), None, "Lost follows actual io Closed");
                    stats[3] += 1;
                }
                Notice::Domain(_) | Notice::Listening { .. } | Notice::Failed { .. } | Notice::Closed => {}
            }
            trace.log(world.now, format_args!("{notice:?}"));
        }
        observed = world.notices.len();
        trace.log(world.now, format_args!("step {step}, action {action}, held {}", world.listener.connections()));
    }
    (trace.lines().to_vec(), stats)
}
