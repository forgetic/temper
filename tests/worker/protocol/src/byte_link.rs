//! Strict byte-credit peer for clock/backpressure boundary cases. Sent records
//! stay charged below until full-cap Room, even if their bytes reach the peer.
use crate::agent::Pipe;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_channel::{
    Sizes, codec,
    machine::{self, Endpoint, Machine},
    wire,
};
use temper_worker_domain as worker;
use temper_worker_protocol::{
    Limits,
    link::{self, Link},
};

pub const LIMITS: Limits = Limits {
    channel: temper_channel::Limits {
        chunk: 7,
        ping: Duration::from_nanos(1),
        silence: Duration::from_nanos(3),
        ..temper_channel::Limits::STARTING
    },
    sizes: Sizes {
        charter: 1,
        snapshot: 1,
        inbound: 1,
        call: 1,
        answer: 1,
        outcome: 1,
        fact: 1,
        detail: 1,
        name_bytes: 1,
        token_bytes: 1,
        repositories: 1,
        slots: 1,
        workstreams: 1,
        grants: 1,
        endpoints: 1,
        entries: 1,
        inbox: 1,
        run_calls: 1,
        stalled: 1,
        bounces: 1,
        accounts: 1,
        facts: 1,
        agent_outbox: 1,
        ..Sizes::STARTING
    },
    accounts: 1,
    agents: 1,
    relays: 1,
    skew: Duration::ZERO,
};
const OWNER: Token = Token::new(1);
const SOCKET: Token = Token::new(2);
pub struct World {
    pub link: Link,
    pub now: Time,
    pub pings: u32,
    pub sent_records: u32,
    peer: Machine,
    worker_pipe: Pipe,
    peer_pipe: Pipe,
    worker_up: Queue<worker::Event>,
    worker_down: Queue<skein_io::Request>,
    peer_up: Queue<machine::Event>,
    peer_down: Queue<skein_lib::stream::Down>,
}
impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
impl World {
    #[must_use]
    pub fn new() -> Self {
        let link = Link::new(
            OWNER,
            link::Config {
                address: temper_engine_protocol_world::link::ADDR,
                name: b"a".as_slice().into(),
                secret: b"s".as_slice().into(),
                accounts: Box::new([1]),
            },
            &LIMITS,
            &crate::sockets::io_limits(),
        )
        .expect("small link");
        let mut world = Self {
            link,
            now: Time::ZERO,
            pings: 0,
            sent_records: 0,
            peer: Machine::new(Endpoint::Engine, &LIMITS.channel, &LIMITS.sizes).expect("peer"),
            worker_pipe: Pipe::default(),
            peer_pipe: Pipe::default(),
            worker_up: Queue::with_capacity(link::MAX_UP),
            worker_down: Queue::with_capacity(link::MAX_DOWN),
            peer_up: Queue::with_capacity(machine::MAX_UP),
            peer_down: Queue::with_capacity(machine::MAX_DOWN),
        };
        let env = world.env();
        link::down(&mut world.link, &env, worker::Request::Dial, &mut world.worker_up, &mut world.worker_down)
            .expect("dial");
        let _ = world.worker_down.pop().expect("Connect");
        link::up(
            &mut world.link,
            &env,
            skein_io::Event::Connecting { owner: OWNER, socket: SOCKET },
            &mut world.worker_up,
            &mut world.worker_down,
        );
        link::up(
            &mut world.link,
            &env,
            skein_io::Event::Connected { owner: OWNER },
            &mut world.worker_up,
            &mut world.worker_down,
        );
        world.drain();
        world.settle();
        assert_eq!(world.link.state(), link::State::Open);
        world
    }
    fn env(&self) -> Env<Limits> {
        Env { now: self.now, wall: Wall::EPOCH, limits: LIMITS }
    }
    fn drain(&mut self) {
        while let Some(request) = self.worker_down.pop() {
            match request {
                skein_io::Request::Stream { down, .. } => {
                    if let skein_lib::stream::Down::Send(bytes) = &down {
                        self.sent_records += 1;
                        if matches!(codec::decode(bytes, &LIMITS.sizes), Some(wire::Message::Ping)) {
                            self.pings += 1;
                        }
                    }
                    self.worker_pipe.lower(down, &mut self.peer_pipe);
                }
                skein_io::Request::Close { .. } | skein_io::Request::Abort { .. } => {
                    panic!("healthy blocked stream must not close")
                }
                skein_io::Request::Connect { .. }
                | skein_io::Request::Listen { .. }
                | skein_io::Request::Bind { .. }
                | skein_io::Request::Reject { .. }
                | skein_io::Request::Output { .. }
                | skein_io::Request::Spawn { .. }
                | skein_io::Request::Signal { .. } => panic!("stream already bound"),
            }
        }
        while let Some(event) = self.worker_up.pop() {
            assert_eq!(event, worker::Event::Connected);
            let env = self.env();
            link::down(
                &mut self.link,
                &env,
                worker::Request::Hello {
                    hello: worker::Hello { slots: 1, workstreams: Box::new([]), hosting: Box::new([]) },
                },
                &mut self.worker_up,
                &mut self.worker_down,
            )
            .expect("hello");
        }
        while let Some(event) = self.peer_up.pop() {
            match event {
                machine::Event::Message(wire::Message::Open { .. }) => machine::down(
                    &mut self.peer,
                    &LIMITS.channel,
                    &LIMITS.sizes,
                    machine::Request::Accept { version: 1 },
                    &mut self.peer_up,
                    &mut self.peer_down,
                ),
                machine::Event::Message(wire::Message::Hello { .. }) => machine::down(
                    &mut self.peer,
                    &LIMITS.channel,
                    &LIMITS.sizes,
                    machine::Request::Read,
                    &mut self.peer_up,
                    &mut self.peer_down,
                ),
                machine::Event::Ready { .. }
                | machine::Event::Sent
                | machine::Event::Unsent
                | machine::Event::ReadEnded => {}
                machine::Event::Message(_) | machine::Event::Closed { .. } => {
                    panic!("scripted worker sends hello/pings only")
                }
            }
        }
        while let Some(down) = self.peer_down.pop() {
            self.peer_pipe.lower(down, &mut self.worker_pipe);
        }
    }
    pub fn settle(&mut self) {
        for _ in 0..5000 {
            let mut moved = false;
            if let Some(up) = self.worker_pipe.upper() {
                let env = self.env();
                link::up(
                    &mut self.link,
                    &env,
                    skein_io::Event::Stream { owner: OWNER, up },
                    &mut self.worker_up,
                    &mut self.worker_down,
                );
                self.drain();
                moved = true;
            }
            if let Some(up) = self.peer_pipe.upper() {
                machine::up(&mut self.peer, &LIMITS.channel, &LIMITS.sizes, up, &mut self.peer_up, &mut self.peer_down);
                self.drain();
                moved = true;
            }
            if self.link.is_ready() {
                let env = self.env();
                link::resume(&mut self.link, &env, &mut self.worker_up, &mut self.worker_down);
                self.drain();
                moved = true;
            }
            if self.peer.is_ready() {
                machine::poll(&mut self.peer, &LIMITS.channel, &LIMITS.sizes, &mut self.peer_up, &mut self.peer_down);
                self.drain();
                moved = true;
            }
            if !self.worker_down.is_empty() {
                self.drain();
                moved = true;
            }
            if !moved {
                return;
            }
        }
        panic!("byte link must settle");
    }
    pub fn block_room(&mut self) {
        self.worker_pipe.blocked = true;
        self.sent_records = 0;
    }
    pub fn incoming_ping(&mut self) {
        machine::down(
            &mut self.peer,
            &LIMITS.channel,
            &LIMITS.sizes,
            machine::Request::Send(wire::Message::Ping),
            &mut self.peer_up,
            &mut self.peer_down,
        );
        self.drain();
        self.settle();
    }
    pub fn fire(&mut self) {
        let env = self.env();
        link::fire(&mut self.link, &env, &mut self.worker_up, &mut self.worker_down);
        self.drain();
        self.settle();
    }
}
