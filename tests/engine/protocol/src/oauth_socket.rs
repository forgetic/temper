//! The production OAuth owner and the rotating fake issuer on two real skein
//! Io instances. Simulator faults exercise records and actual Closed fences.
use super::oauth::{DOCUMENTS, LIMITS, config, io_limits, refresh};
use skein_io::{
    self as io,
    kernel::{Complete, Submit},
};
use skein_lib::{Env, Queue, Token, Wall};
use skein_sim::{Pid, Sim};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_protocol::{
    credentials,
    oauth::{self, Effect, Event, Owner},
};
use temper_fake_llm_protocol::oauth as issuer;
use temper_legacy_engine_domain::{self as engine, accounts};
pub const SERVER: Token = Token::new(u64::MAX);
#[must_use]
pub fn issuer_limits() -> issuer::Limits {
    issuer::Limits {
        http: skein_http::server::Limits { head: 1024, headers: 8, body: 2048, response: 1024, read: 7, send: 7 },
        documents: DOCUMENTS,
        plans: 4,
        rotations: 8,
    }
}
pub struct Node {
    pub io: io::Io,
    pub pid: Pid,
    requests: VecDeque<io::Request>,
    completions: Queue<Complete>,
    events: Queue<io::Event>,
    submits: Queue<Submit>,
}
impl Node {
    fn new(sim: &mut Sim) -> Node {
        Node {
            io: io::Io::new(&io_limits()),
            pid: sim.spawn_process(),
            requests: VecDeque::new(),
            completions: Queue::with_capacity(1),
            events: Queue::with_capacity(2),
            submits: Queue::with_capacity(2),
        }
    }
    fn tick(&mut self, sim: &mut Sim) -> bool {
        let env = Env { now: sim.now(), wall: Wall::EPOCH, limits: io_limits() };
        sim.reap(self.pid, &mut self.completions);
        if self.io.is_ready() {
            io::resume(&mut self.io, &env, &mut self.events, &mut self.submits);
        } else if let Some(complete) = self.completions.pop() {
            io::up(&mut self.io, &env, complete, &mut self.events, &mut self.submits);
        } else if self.io.is_due(env.now) {
            io::fire(&mut self.io, &env, &mut self.events, &mut self.submits);
        } else if self.io.takes() && !self.requests.is_empty() {
            io::down(&mut self.io, &env, self.requests.pop_front().expect("pending request"), &mut self.submits);
        } else {
            return false;
        }
        sim.submit(self.pid, &mut self.submits);
        self.io.reclaim();
        true
    }
}
struct Connection {
    server: issuer::Server,
    socket: Token,
    upper: Queue<issuer::Event>,
    lower: Queue<skein_lib::stream::Down>,
}
pub struct World {
    pub sim: Sim,
    pub owner: Owner,
    pub issuer: issuer::Issuer,
    pub client: Node,
    pub server: Node,
    pub notices: Vec<engine::Event>,
    pub records: Vec<(Token, Box<[u8]>)>,
    pub cancel_keeps: Vec<Token>,
    pub hold_closed: bool,
    pub held_closed: Vec<io::Event>,
    upper: Queue<engine::Event>,
    lower: Queue<Effect>,
    connections: BTreeMap<Token, Connection>,
    listener: Option<Token>,
    sequence: u64,
}
impl World {
    #[must_use]
    pub fn new(seed: u64, faults: bool) -> World {
        let mut setup = skein_sim::Config::calm();
        setup.buffer = 17;
        if faults {
            setup.faults.short_send = 700;
            setup.faults.short_recv = 700;
            setup.faults.no_buffer = 5;
            setup.faults.cancel_race = 500;
            setup.faults.latency = 300;
            setup.faults.latency_max = skein_lib::Duration::from_millis(2);
        }
        let mut sim = Sim::new(seed, setup);
        let client = Node::new(&mut sim);
        let server = Node::new(&mut sim);
        let issuer = issuer::Issuer::new(
            issuer::Config {
                path: b"/oauth/token".as_slice().into(),
                client_id: b"client".as_slice().into(),
                refresh_token: b"refresh-4".as_slice().into(),
            },
            &issuer_limits(),
        )
        .expect("issuer limits");
        let owner =
            Owner::new(Box::new([config(refresh())]), Box::new([]), sim.now(), Wall::EPOCH, &LIMITS, &io_limits())
                .expect("owner limits");
        let mut world = World {
            sim,
            owner,
            issuer,
            client,
            server,
            notices: Vec::new(),
            records: Vec::new(),
            cancel_keeps: Vec::new(),
            hold_closed: false,
            held_closed: Vec::new(),
            upper: Queue::with_capacity(oauth::MAX_UP),
            lower: Queue::with_capacity(oauth::MAX_OUT),
            connections: BTreeMap::new(),
            listener: None,
            sequence: 1,
        };
        world.server.requests.push_back(io::Request::Listen { owner: SERVER, addr: crate::link::ADDR });
        world.drive_until(0, 0, true);
        assert!(world.listener.is_some());
        world
    }
    fn env(&self) -> Env<oauth::Limits> {
        Env { now: self.sim.now(), wall: Wall::from_nanos(self.sim.now().as_nanos()), limits: LIMITS }
    }
    pub fn queue(&mut self, plan: issuer::Plan) {
        assert!(self.issuer.queue(plan, &issuer_limits()).is_ok());
    }
    pub fn refresh(&mut self, generation: u64) {
        let env = self.env();
        oauth::down(
            &mut self.owner,
            &env,
            accounts::Request::Refresh { account: 1, generation },
            &mut self.upper,
            &mut self.lower,
        )
        .expect("refresh");
        self.collect();
    }
    pub fn event(&mut self, event: Event) {
        let env = self.env();
        oauth::up(&mut self.owner, &env, event, &mut self.upper, &mut self.lower);
        self.collect();
    }
    pub fn drive(&mut self) {
        let records = self.records.len();
        let notices = self.notices.len();
        self.drive_until(records, notices, false);
    }
    fn drive_until(&mut self, records: usize, notices: usize, listening: bool) {
        for _step in 0..200_000 {
            if (listening && self.listener.is_some())
                || (!listening && (self.records.len() > records || self.notices.len() > notices))
            {
                return;
            }
            if self.tick() {
                continue;
            }
            let mut next = self.sim.next_due();
            for at in [self.client.io.next_deadline(), self.server.io.next_deadline()].into_iter().flatten() {
                next = Some(next.map_or(at, |prior| prior.min(at)));
            }
            for connection in self.connections.values() {
                if let Some(at) = connection.server.next_deadline() {
                    next = Some(next.map_or(at, |prior| prior.min(at)));
                }
            }
            if let Some(at) = self.owner.next_deadline() {
                next = Some(next.map_or(at, |prior| prior.min(at)));
            }
            if let Some(at) = next {
                self.sim.advance_to(at);
            } else {
                return;
            }
        }
        panic!("socket world progress is bounded: {}", self.sim.render_trace());
    }
    pub fn settle(&mut self) {
        for _step in 0..100_000 {
            if self.tick() {
                continue;
            }
            let mut next = self.sim.next_due();
            for at in [self.client.io.next_deadline(), self.server.io.next_deadline()].into_iter().flatten() {
                next = Some(next.map_or(at, |prior| prior.min(at)));
            }
            if let Some(at) = next {
                self.sim.advance_to(at);
            } else {
                return;
            }
        }
        panic!("socket settlement is bounded");
    }
    fn tick(&mut self) -> bool {
        let mut moved = self.client.tick(&mut self.sim) | self.server.tick(&mut self.sim);
        while let Some(event) = self.client.events.pop() {
            if self.hold_closed && matches!(event, io::Event::Closed { .. }) {
                self.held_closed.push(event);
            } else {
                self.event(Event::Io(event));
            }
            moved = true;
        }
        while let Some(event) = self.server.events.pop() {
            self.server_event(event);
            moved = true;
        }
        let env = self.env();
        if self.owner.is_ready() {
            oauth::resume(&mut self.owner, &env, &mut self.upper, &mut self.lower);
            self.collect();
            moved = true;
        } else if self.owner.is_due(env.now) {
            oauth::fire(&mut self.owner, &env, &mut self.upper, &mut self.lower);
            self.collect();
            moved = true;
        }
        let env =
            Env { now: self.sim.now(), wall: Wall::from_nanos(self.sim.now().as_nanos()), limits: issuer_limits() };
        for (&token, connection) in &mut self.connections {
            if connection.server.has_work(&self.issuer, env.now) {
                issuer::resume(
                    &mut connection.server,
                    &mut self.issuer,
                    &env,
                    &mut connection.upper,
                    &mut connection.lower,
                );
                moved = true;
            } else if connection.server.next_deadline().is_some_and(|at| at <= env.now) {
                issuer::fire(
                    &mut connection.server,
                    &mut self.issuer,
                    &env,
                    &mut connection.upper,
                    &mut connection.lower,
                );
                moved = true;
            }
            drain_server(connection, token, &mut self.server.requests);
        }
        self.owner.reclaim();
        moved
    }
    fn server_event(&mut self, event: io::Event) {
        match event {
            io::Event::Listening { owner: SERVER, listener, .. } => self.listener = Some(listener),
            io::Event::Accepted { owner: SERVER, socket, .. } => {
                self.sequence += 1;
                let token = Token::new(self.sequence);
                self.server.requests.push_back(io::Request::Bind { socket, owner: token });
                let mut connection = Connection {
                    server: issuer::Server::new(&issuer_limits()).expect("server limits"),
                    socket,
                    upper: Queue::with_capacity(issuer::MAX_UP),
                    lower: Queue::with_capacity(issuer::MAX_DOWN),
                };
                issuer::start(
                    &mut connection.server,
                    &mut self.issuer,
                    &Env { now: self.sim.now(), wall: Wall::EPOCH, limits: issuer_limits() },
                    &mut connection.upper,
                    &mut connection.lower,
                );
                drain_server(&mut connection, token, &mut self.server.requests);
                assert!(self.connections.insert(token, connection).is_none());
            }
            io::Event::Stream { owner, up } => {
                let connection = self.connections.get_mut(&owner).expect("bound issuer connection");
                issuer::up(
                    &mut connection.server,
                    &mut self.issuer,
                    &Env { now: self.sim.now(), wall: Wall::EPOCH, limits: issuer_limits() },
                    up,
                    &mut connection.upper,
                    &mut connection.lower,
                );
                drain_server(connection, owner, &mut self.server.requests);
            }
            io::Event::Closed { owner: SERVER } => self.listener = None,
            io::Event::Closed { owner } => {
                let mut connection = self.connections.remove(&owner).expect("actual Closed retains connection");
                issuer::closed(
                    &mut connection.server,
                    &Env { now: self.sim.now(), wall: Wall::EPOCH, limits: issuer_limits() },
                    &mut connection.upper,
                    &mut connection.lower,
                );
            }
            io::Event::Failed { owner, .. } => {
                if let Some(connection) = self.connections.get(&owner) {
                    self.server.requests.push_back(io::Request::Abort { entity: connection.socket });
                }
            }
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Connecting { .. }
            | io::Event::Connected { .. } => panic!("server-side socket events only"),
        }
    }
    fn collect(&mut self) {
        while let Some(event) = self.upper.pop() {
            self.notices.push(event);
        }
        while let Some(effect) = self.lower.pop() {
            match effect {
                Effect::Io(request) => self.client.requests.push_back(request),
                Effect::Keep { owner, record, .. } => self.records.push((owner, record)),
                Effect::CancelKeep { owner } => self.cancel_keeps.push(owner),
                Effect::Security(_) => panic!("explicit loopback plaintext fixture"),
            }
        }
    }
    pub fn release_closed(&mut self) {
        self.hold_closed = false;
        for event in std::mem::take(&mut self.held_closed) {
            self.event(Event::Io(event));
        }
        self.owner.reclaim();
    }
    pub fn close(&mut self) {
        if let Some(listener) = self.listener {
            self.server.requests.push_back(io::Request::Abort { entity: listener });
        }
        for connection in self.connections.values() {
            self.server.requests.push_back(io::Request::Abort { entity: connection.socket });
        }
        self.settle();
        self.release_closed();
        self.settle();
        self.sim.assert_quiescent(self.client.pid);
        self.sim.assert_quiescent(self.server.pid);
        self.sim.assert_no_open_fds(self.client.pid);
        self.sim.assert_no_open_fds(self.server.pid);
    }
    #[must_use]
    pub fn table(&self) -> &credentials::Table {
        self.owner.table()
    }
}
fn drain_server(connection: &mut Connection, _token: Token, requests: &mut VecDeque<io::Request>) {
    while let Some(down) = connection.lower.pop() {
        requests.push_back(io::Request::Stream { stream: connection.socket, down });
    }
    while let Some(event) = connection.upper.pop() {
        match event {
            issuer::Event::Close => requests.push_back(io::Request::Close { entity: connection.socket }),
            issuer::Event::Requested { .. } | issuer::Event::Closed => {}
        }
    }
}
