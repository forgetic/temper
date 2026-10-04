//! Both adapters run over actual skein io and simulated kernel socket records.
use skein_io::{
    self as io, Io,
    kernel::{Complete, Submit},
};
use skein_lib::{Duration, Env, Queue, Token, Wall};
use skein_sim::{Config, Pid, Sim};
use std::collections::VecDeque;
use temper_engine_protocol::{
    connection::Transport,
    listener::{self, Effect, Listener, Notice},
    translate::Value,
};
use temper_engine_protocol_world::link::{ADDR, LIMITS as ENGINE, SIZES, repositories, workers};
use temper_legacy_engine_domain as engine;
use temper_worker_domain as worker;
use temper_worker_protocol::{
    Limits,
    link::{self, Link},
};

pub const LIMITS: Limits =
    Limits { channel: ENGINE.channel, sizes: SIZES, accounts: 2, agents: 2, relays: 4, skew: Duration::from_millis(1) };
pub const OWNER: Token = Token::new(77);

#[must_use]
pub fn io_limits() -> io::Limits {
    let bytes = temper_channel::sizes::output_cap(temper_channel::machine::Endpoint::Engine, &SIZES)
        .expect("small engine cap")
        .max(
            temper_channel::sizes::output_cap(temper_channel::machine::Endpoint::WorkerLink, &SIZES)
                .expect("worker cap"),
        );
    let sends = temper_channel::sizes::stream_slots(temper_channel::machine::Endpoint::Engine, &SIZES)
        .expect("engine slots")
        .max(
            temper_channel::sizes::stream_slots(temper_channel::machine::Endpoint::WorkerLink, &SIZES)
                .expect("worker slots"),
        );
    io::Limits {
        sockets: 6,
        refusals: 4,
        intake: 15,
        receive: 7,
        output: bytes,
        sends,
        accepts: 1,
        backlog: 4,
        close_timeout: Duration::from_secs(1),
        retry: Duration::from_millis(1),
    }
}

pub struct Node {
    pub io: Io,
    pub pid: Pid,
    requests: VecDeque<io::Request>,
    completions: Queue<Complete>,
    events: Queue<io::Event>,
    submits: Queue<Submit>,
}
impl Node {
    fn new(sim: &mut Sim) -> Self {
        Self {
            io: Io::new(&io_limits()),
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
            io::down(&mut self.io, &env, self.requests.pop_front().expect("pending"), &mut self.submits);
        } else {
            return false;
        }
        sim.submit(self.pid, &mut self.submits);
        self.io.reclaim();
        true
    }
}

/// A second real worker process, used to authenticate a replacement while
/// the listener still owns the old connection's terminal binding.
pub struct Extra {
    pub link: Link,
    pub node: Node,
    pub events: Vec<worker::Event>,
    upper: Queue<worker::Event>,
    lower: Queue<io::Request>,
}
impl Extra {
    fn new(sim: &mut Sim) -> Self {
        let link = Link::new(
            OWNER,
            link::Config {
                address: ADDR,
                name: b"alpha".as_slice().into(),
                secret: b"one".as_slice().into(),
                accounts: Box::new([1, 2]),
            },
            &LIMITS,
            &io_limits(),
        )
        .expect("replacement link");
        let mut extra = Self {
            link,
            node: Node::new(sim),
            events: Vec::new(),
            upper: Queue::with_capacity(link::MAX_UP),
            lower: Queue::with_capacity(link::MAX_DOWN),
        };
        let env = Env { now: sim.now(), wall: Wall::EPOCH, limits: LIMITS };
        link::down(&mut extra.link, &env, worker::Request::Dial, &mut extra.upper, &mut extra.lower)
            .expect("replacement dial");
        extra.drain(&env);
        extra
    }
    fn drain(&mut self, env: &Env<Limits>) {
        while let Some(request) = self.lower.pop() {
            self.node.requests.push_back(request);
        }
        while let Some(event) = self.upper.pop() {
            if matches!(event, worker::Event::Connected) {
                link::down(
                    &mut self.link,
                    env,
                    worker::Request::Hello {
                        hello: worker::Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) },
                    },
                    &mut self.upper,
                    &mut self.lower,
                )
                .expect("replacement hello");
            }
            self.events.push(event);
        }
        while let Some(request) = self.lower.pop() {
            self.node.requests.push_back(request);
        }
    }
    fn tick(&mut self, sim: &mut Sim) -> bool {
        let mut moved = self.node.tick(sim);
        let env = Env { now: sim.now(), wall: Wall::EPOCH, limits: LIMITS };
        while let Some(event) = self.node.events.pop() {
            link::up(&mut self.link, &env, event, &mut self.upper, &mut self.lower);
            self.drain(&env);
            moved = true;
        }
        if self.link.is_ready() {
            link::resume(&mut self.link, &env, &mut self.upper, &mut self.lower);
            self.drain(&env);
            moved = true;
        }
        moved
    }
}

pub struct World {
    pub sim: Sim,
    pub listener: Listener,
    pub link: Link,
    pub notices: Vec<Notice>,
    pub events: Vec<worker::Event>,
    pub engine_node: Node,
    pub worker_node: Node,
    pub hold_closed: bool,
    pub hold_worker_ready: bool,
    pub held_closed: Vec<io::Event>,
    pub replacement: Option<Extra>,
    pub hold_engine_closed: bool,
    pub held_engine_closed: Vec<io::Event>,
    upper_engine: Queue<Notice>,
    lower_engine: Queue<Effect>,
    upper_worker: Queue<worker::Event>,
    lower_worker: Queue<io::Request>,
}
impl World {
    #[must_use]
    pub fn new(seed: u64, faults: bool) -> Self {
        let mut config = Config::calm();
        config.buffer = 17;
        if faults {
            config.faults.short_recv = 700;
            config.faults.short_send = 700;
            config.faults.latency = 300;
            config.faults.latency_max = Duration::from_millis(2);
            config.faults.no_buffer = 5;
            config.faults.cancel_race = 500;
        }
        let mut sim = Sim::new(seed, config);
        let engine_node = Node::new(&mut sim);
        let worker_node = Node::new(&mut sim);
        let listener = Listener::new(workers(), repositories(), Transport::Loopback, &ENGINE, &SIZES, &io_limits())
            .expect("listener");
        let link = Link::new(
            OWNER,
            link::Config {
                address: ADDR,
                name: b"alpha".as_slice().into(),
                secret: b"one".as_slice().into(),
                accounts: Box::new([1, 2]),
            },
            &LIMITS,
            &io_limits(),
        )
        .expect("link");
        let mut world = Self {
            sim,
            listener,
            link,
            engine_node,
            worker_node,
            notices: Vec::new(),
            events: Vec::new(),
            hold_closed: false,
            hold_worker_ready: false,
            held_closed: Vec::new(),
            replacement: None,
            hold_engine_closed: false,
            held_engine_closed: Vec::new(),
            upper_engine: Queue::with_capacity(listener::MAX_UP),
            lower_engine: Queue::with_capacity(listener::MAX_OUT),
            upper_worker: Queue::with_capacity(link::MAX_UP),
            lower_worker: Queue::with_capacity(link::MAX_DOWN),
        };
        assert!(listener::start(&mut world.listener, ADDR, &mut world.lower_engine));
        world.drain();
        world.settle();
        assert!(world.notices.iter().any(|notice| matches!(notice, Notice::Listening { .. })));
        world.down(worker::Request::Dial);
        world.settle();
        assert_eq!(world.link.state(), link::State::Open);
        world
    }
    fn worker_env(&self) -> Env<Limits> {
        Env { now: self.sim.now(), wall: Wall::EPOCH, limits: LIMITS }
    }
    fn engine_env(&self) -> Env<temper_engine_protocol::Limits> {
        Env { now: self.sim.now(), wall: Wall::EPOCH, limits: ENGINE }
    }
    fn drain(&mut self) {
        while let Some(effect) = self.lower_engine.pop() {
            match effect {
                Effect::Io(request) => self.engine_node.requests.push_back(request),
                Effect::Security(_) => panic!("loopback"),
            }
        }
        while let Some(notice) = self.upper_engine.pop() {
            self.notices.push(notice);
        }
        while let Some(request) = self.lower_worker.pop() {
            self.worker_node.requests.push_back(request);
        }
        while let Some(event) = self.upper_worker.pop() {
            let connected = matches!(event, worker::Event::Connected);
            self.events.push(event);
            if connected {
                let env = self.worker_env();
                link::down(
                    &mut self.link,
                    &env,
                    worker::Request::Hello {
                        hello: worker::Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) },
                    },
                    &mut self.upper_worker,
                    &mut self.lower_worker,
                )
                .expect("hello");
            }
        }
        while let Some(request) = self.lower_worker.pop() {
            self.worker_node.requests.push_back(request);
        }
    }
    pub fn down(&mut self, request: worker::Request) {
        let env = self.worker_env();
        link::down(&mut self.link, &env, request, &mut self.upper_worker, &mut self.lower_worker).expect("worker down");
        self.drain();
    }
    pub fn engine(&mut self, request: engine::Request, values: &[Value]) {
        let env = self.engine_env();
        listener::down(&mut self.listener, &env, &SIZES, request, values, &mut self.lower_engine);
        self.drain();
    }
    #[must_use]
    pub fn channel(&self) -> Token {
        self.listener.channel(0).expect("current worker")
    }
    pub fn settle(&mut self) {
        for _ in 0..200_000 {
            let mut moved = self.engine_node.tick(&mut self.sim);
            moved |= self.worker_node.tick(&mut self.sim);
            if let Some(extra) = &mut self.replacement {
                moved |= extra.tick(&mut self.sim);
            }
            while let Some(event) = self.engine_node.events.pop() {
                if self.hold_engine_closed && matches!(event, io::Event::Closed { .. }) {
                    self.held_engine_closed.push(event);
                    continue;
                }
                let env = self.engine_env();
                listener::up(
                    &mut self.listener,
                    &env,
                    &SIZES,
                    listener::Event::Io(event),
                    &mut self.upper_engine,
                    &mut self.lower_engine,
                );
                self.drain();
                self.listener.reclaim();
                moved = true;
            }
            while let Some(event) = self.worker_node.events.pop() {
                if self.hold_closed && matches!(event, io::Event::Closed { .. }) {
                    self.held_closed.push(event);
                    continue;
                }
                let env = self.worker_env();
                link::up(&mut self.link, &env, event, &mut self.upper_worker, &mut self.lower_worker);
                self.drain();
                moved = true;
            }
            if self.listener.is_ready() {
                let env = self.engine_env();
                listener::resume(&mut self.listener, &env, &SIZES, &mut self.upper_engine, &mut self.lower_engine);
                self.drain();
                self.listener.reclaim();
                moved = true;
            }
            if self.link.is_ready() && !self.hold_worker_ready {
                let env = self.worker_env();
                link::resume(&mut self.link, &env, &mut self.upper_worker, &mut self.lower_worker);
                self.drain();
                moved = true;
            }
            if !moved {
                let due = [
                    self.sim.next_due(),
                    self.engine_node.io.next_deadline(),
                    self.worker_node.io.next_deadline(),
                    self.replacement.as_ref().and_then(|extra| extra.node.io.next_deadline()),
                ]
                .into_iter()
                .flatten()
                .min();
                let Some(at) = due else {
                    return;
                };
                self.sim.advance_to(at);
            }
        }
        panic!("paired socket world did not settle: {}", self.sim.render_trace());
    }
    pub fn replace(&mut self) {
        assert!(self.replacement.is_none());
        self.replacement = Some(Extra::new(&mut self.sim));
    }
    pub fn release_engine_closed(&mut self) {
        for event in std::mem::take(&mut self.held_engine_closed) {
            let env = self.engine_env();
            listener::up(
                &mut self.listener,
                &env,
                &SIZES,
                listener::Event::Io(event),
                &mut self.upper_engine,
                &mut self.lower_engine,
            );
            self.drain();
            self.listener.reclaim();
        }
    }
    pub fn release_closed(&mut self) {
        for event in std::mem::take(&mut self.held_closed) {
            let env = self.worker_env();
            link::up(&mut self.link, &env, event, &mut self.upper_worker, &mut self.lower_worker);
            self.drain();
        }
    }
    pub fn shutdown(&mut self) {
        self.hold_engine_closed = false;
        self.release_engine_closed();
        self.hold_closed = false;
        self.release_closed();
        listener::close(&mut self.listener, &mut self.lower_engine);
        self.drain();
        self.settle();
        self.sim.assert_quiescent(self.engine_node.pid);
        self.sim.assert_quiescent(self.worker_node.pid);
        self.sim.assert_no_open_fds(self.engine_node.pid);
        self.sim.assert_no_open_fds(self.worker_node.pid);
        if let Some(extra) = &self.replacement {
            self.sim.assert_quiescent(extra.node.pid);
            self.sim.assert_no_open_fds(extra.node.pid);
        }
    }
}
