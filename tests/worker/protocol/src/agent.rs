//! Attached worker stream paired with a real agent endpoint frame machine.
use crate::sockets::LIMITS;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall, stream};
use std::collections::VecDeque;
use temper_channel::{
    machine::{self, Endpoint, Machine},
    wire,
};
use temper_worker_domain::{self as worker, agent::channel};
use temper_worker_protocol::{
    agent::{self, Channel},
    credentials::Table,
};

#[derive(Default)]
pub(crate) struct Pipe {
    tape: VecDeque<u8>,
    demand: Option<(stream::Read, u32)>,
    credit: u32,
    end: bool,
    ended: bool,
    pub(crate) blocked: bool,
}
impl Pipe {
    pub(crate) fn lower(&mut self, down: stream::Down, other: &mut Pipe) {
        match down {
            stream::Down::Demand { read, room } => {
                if read == stream::Read::Nothing && room == 0 {
                    self.demand = None;
                } else {
                    assert!(self.demand.is_none());
                    self.demand = Some((read, room));
                }
            }
            stream::Down::Send(bytes) => {
                assert!(!other.end, "no Send follows Finish");
                let count = u32::try_from(bytes.len()).expect("small frame");
                assert!(count <= self.credit);
                self.credit -= count;
                other.tape.extend(bytes.iter().copied());
            }
            stream::Down::Finish => other.end = true,
        }
    }
    pub(crate) fn upper(&mut self) -> Option<stream::Up> {
        let (read, room) = self.demand?;
        if room > 0 && !self.blocked {
            self.demand = None;
            self.credit = room;
            return Some(stream::Up::Room);
        }
        let count = match read {
            stream::Read::Fill(count) => usize::try_from(count).expect("small read"),
            stream::Read::Nothing => return None,
            stream::Read::Scan { .. } | stream::Read::Line { .. } => panic!("channel exact reads"),
        };
        if self.tape.len() >= count {
            self.demand = None;
            return Some(stream::Up::Bytes(self.tape.drain(..count).collect()));
        }
        if self.end && !self.ended {
            self.ended = true;
            return Some(stream::Up::End);
        }
        None
    }
}
pub struct World {
    pub bridge: Channel,
    pub peer: Machine,
    pub messages: Vec<wire::Message>,
    pub events: Vec<worker::Event>,
    pub now: Time,
    pub endpoints: Box<[wire::EndpointDescriptor]>,
    pub credentials: Table,
    worker_pipe: Pipe,
    peer_pipe: Pipe,
    worker_up: Queue<worker::Event>,
    worker_down: Queue<stream::Down>,
    peer_up: Queue<machine::Event>,
    peer_down: Queue<stream::Down>,
}
impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
impl World {
    #[must_use]
    pub fn new() -> Self {
        let mut table = Table::new(&[1, 2], 2, LIMITS.sizes.token_bytes).expect("accounts");
        table.insert(grant(1, 7, 10), Time::ZERO, Duration::ZERO).expect("oauth");
        table.insert(grant(2, 0, 10), Time::ZERO, Duration::ZERO).expect("static git");
        let mut world = Self {
            bridge: Channel::new(Token::new(33), Time::from_nanos(5_000_000_000), &LIMITS).expect("bridge"),
            peer: Machine::new(Endpoint::Agent, &LIMITS.channel, &LIMITS.sizes).expect("peer"),
            now: Time::ZERO,
            endpoints: Box::new([endpoint(0, 1), endpoint(1, 1)]),
            credentials: table,
            messages: Vec::new(),
            events: Vec::new(),
            worker_pipe: Pipe::default(),
            peer_pipe: Pipe::default(),
            worker_up: Queue::with_capacity(agent::MAX_UP),
            worker_down: Queue::with_capacity(agent::MAX_DOWN),
            peer_up: Queue::with_capacity(machine::MAX_UP),
            peer_down: Queue::with_capacity(machine::MAX_DOWN),
        };
        let env = world.env();
        agent::start(&mut world.bridge, &env, &mut world.worker_up, &mut world.worker_down);
        world.drain();
        world
    }
    fn env(&self) -> Env<temper_worker_protocol::Limits> {
        Env { now: self.now, wall: Wall::EPOCH, limits: LIMITS }
    }
    fn drain(&mut self) {
        while let Some(down) = self.worker_down.pop() {
            self.worker_pipe.lower(down, &mut self.peer_pipe);
        }
        while let Some(down) = self.peer_down.pop() {
            self.peer_pipe.lower(down, &mut self.worker_pipe);
        }
        while let Some(event) = self.worker_up.pop() {
            self.events.push(event);
        }
        while let Some(event) = self.peer_up.pop() {
            match event {
                machine::Event::Message(wire::Message::Open { .. }) => {
                    machine::down(
                        &mut self.peer,
                        &LIMITS.channel,
                        &LIMITS.sizes,
                        machine::Request::Accept { version: 1 },
                        &mut self.peer_up,
                        &mut self.peer_down,
                    );
                }
                machine::Event::Message(message) => {
                    self.messages.push(message);
                    machine::down(
                        &mut self.peer,
                        &LIMITS.channel,
                        &LIMITS.sizes,
                        machine::Request::Read,
                        &mut self.peer_up,
                        &mut self.peer_down,
                    );
                }
                machine::Event::Ready { .. } => {
                    machine::down(
                        &mut self.peer,
                        &LIMITS.channel,
                        &LIMITS.sizes,
                        machine::Request::Read,
                        &mut self.peer_up,
                        &mut self.peer_down,
                    );
                }
                machine::Event::Sent | machine::Event::Unsent | machine::Event::ReadEnded => {}
                machine::Event::Closed { fault } => panic!("peer unexpectedly closed: {fault:?}"),
            }
        }
        while let Some(down) = self.peer_down.pop() {
            self.peer_pipe.lower(down, &mut self.worker_pipe);
        }
    }
    pub fn settle(&mut self) {
        for _ in 0..20_000 {
            let mut moved = false;
            if let Some(up) = self.worker_pipe.upper() {
                let env = self.env();
                agent::up(&mut self.bridge, &env, up, &mut self.worker_up, &mut self.worker_down);
                self.drain();
                moved = true;
            }
            if let Some(up) = self.peer_pipe.upper() {
                machine::up(&mut self.peer, &LIMITS.channel, &LIMITS.sizes, up, &mut self.peer_up, &mut self.peer_down);
                self.drain();
                moved = true;
            }
            if self.bridge.is_ready() {
                let env = self.env();
                agent::resume(&mut self.bridge, &env, &mut self.worker_up, &mut self.worker_down);
                self.drain();
                moved = true;
            }
            if self.peer.is_ready() {
                machine::poll(&mut self.peer, &LIMITS.channel, &LIMITS.sizes, &mut self.peer_up, &mut self.peer_down);
                self.drain();
                moved = true;
            }
            if !moved {
                return;
            }
        }
        panic!("agent world must block or settle");
    }
    pub fn block_accept(&mut self, blocked: bool) {
        self.peer_pipe.blocked = blocked;
    }
    pub fn send(&mut self, owner: Token, message: channel::Down) {
        let env = self.env();
        agent::send(
            &mut self.bridge,
            &env,
            agent::Send { owner, message },
            &self.endpoints,
            &self.credentials,
            &mut self.worker_up,
            &mut self.worker_down,
        )
        .expect("typed send");
        self.drain();
    }
    pub fn read(&mut self, owner: Token) {
        let env = self.env();
        agent::read(&mut self.bridge, &env, owner, &mut self.worker_up, &mut self.worker_down);
        self.drain();
    }
    pub fn peer_send(&mut self, message: wire::Message) {
        machine::down(
            &mut self.peer,
            &LIMITS.channel,
            &LIMITS.sizes,
            machine::Request::Send(message),
            &mut self.peer_up,
            &mut self.peer_down,
        );
        self.drain();
    }
    pub fn eof(&mut self) {
        self.worker_pipe.end = true;
        self.settle();
    }
    pub fn closed(&mut self) {
        agent::closed(&mut self.bridge, &mut self.worker_up);
        self.drain();
    }
}
#[must_use]
pub fn grant(account: u32, generation: u64, valid: u64) -> wire::Grant {
    wire::Grant {
        account,
        generation,
        valid: Duration::from_secs(valid),
        token: b"secret".as_slice().into(),
        account_id: b"identity".as_slice().into(),
    }
}
fn endpoint(endpoint: u32, account: u32) -> wire::EndpointDescriptor {
    wire::EndpointDescriptor {
        endpoint,
        provider: wire::Provider::OpenAi,
        host: b"provider".as_slice().into(),
        address: wire::Address::V4 { bytes: [127, 0, 0, 1] },
        port: 4243,
        path: b"/complete".as_slice().into(),
        account,
        effort: Box::new([]),
        thinking: None,
    }
}
#[must_use]
pub fn start() -> channel::Down {
    channel::Down::Start {
        charter: b"\xffopaque charter".as_slice().into(),
        snapshot: Some(b"\xfeopaque snapshot".as_slice().into()),
        repositories: Box::new([channel::Repository { name: b"source".as_slice().into(), writable: true }]),
        grants: Box::new([
            channel::Grant { account: 1, generation: 7, valid: Duration::from_secs(10) },
            channel::Grant { account: 2, generation: 0, valid: Duration::from_secs(10) },
        ]),
    }
}
