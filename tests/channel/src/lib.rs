//! A frame machine driven between a fragmented byte peer and a slow user.
use skein_lib::{Intake, Queue, Rng, stream};
use temper_channel::{
    Limits, Sizes, codec,
    machine::{self, Endpoint, Event, Fault, Machine, Phase, Request},
    sizes,
    wire::{Channel, Message, Open},
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Seen {
    Frame(Message),
    Ready(u16),
    Sent,
    Unsent,
    ReadEnded,
    Closed(Fault),
    Output(Vec<u8>),
    Finish,
}

pub struct World {
    pub machine: Machine,
    pub limits: Limits,
    pub sizes: Sizes,
    pub seen: Vec<Seen>,
    pub demand: Option<(stream::Read, u32)>,
    peer: Vec<u8>,
    at: usize,
    intake: Intake,
    queued: u32,
    credit: u32,
    room_pending: bool,
    rng: Rng,
    upper: Queue<Event>,
    lower: Queue<stream::Down>,
    ended: bool,
}
impl World {
    #[must_use]
    pub fn new(endpoint: Endpoint, seed: u64) -> World {
        let limits = Limits { chunk: 7, ..Limits::STARTING };
        let sizes = Sizes {
            charter: 512,
            snapshot: 128,
            inbound: 64,
            call: 64,
            answer: 128,
            outcome: 128,
            fact: 32,
            detail: 32,
            token_bytes: 32,
            repositories: 2,
            slots: 2,
            workstreams: 2,
            grants: 2,
            endpoints: 2,
            entries: 8,
            inbox: 2,
            run_calls: 2,
            stalled: 2,
            bounces: 2,
            accounts: 2,
            facts: 2,
            agent_outbox: 4,
            ..Sizes::STARTING
        };
        World {
            machine: Machine::new(endpoint, &limits, &sizes).expect("small limits fit"),
            limits,
            sizes,
            seen: Vec::new(),
            demand: None,
            peer: Vec::new(),
            at: 0,
            intake: Intake::with_capacity(15),
            queued: 0,
            credit: 0,
            room_pending: false,
            rng: Rng::new(seed),
            upper: Queue::with_capacity(machine::MAX_UP),
            lower: Queue::with_capacity(machine::MAX_DOWN),
            ended: false,
        }
    }
    pub fn append(&mut self, message: &Message) {
        let bytes = codec::encode(message, &self.sizes).expect("scripted frame fits");
        self.peer.extend_from_slice(&bytes);
    }
    pub fn raw(&mut self, bytes: &[u8]) {
        self.peer.extend_from_slice(bytes);
    }
    pub fn request(&mut self, request: Request) {
        machine::down(&mut self.machine, &self.limits, &self.sizes, request, &mut self.upper, &mut self.lower);
        self.drain();
    }
    pub fn input(&mut self, event: stream::Up) {
        match event {
            stream::Up::Room => {
                if !self.room_pending
                    && let Some((_, room)) = self.demand
                {
                    self.credit = room;
                }
                self.room_pending = false;
                self.demand = None;
            }
            stream::Up::Bytes(_) => {
                if self.demand.is_some_and(|(read, _)| read != stream::Read::Nothing) {
                    self.demand = None;
                }
            }
            stream::Up::Failed(_) => self.demand = None,
            stream::Up::End => {}
        }
        machine::up(&mut self.machine, &self.limits, &self.sizes, event, &mut self.upper, &mut self.lower);
        self.drain();
    }
    fn drain(&mut self) {
        while let Some(request) = self.lower.pop() {
            match request {
                stream::Down::Demand { read: stream::Read::Nothing, room: 0 } => self.demand = None,
                stream::Down::Demand { read, room } => {
                    assert!(self.demand.is_none(), "a demand is stated once after its previous answer");
                    self.demand = Some((read, room));
                }
                stream::Down::Send(bytes) => {
                    let length = u32::try_from(bytes.len()).expect("small frame");
                    self.credit = self.credit.checked_sub(length).expect("Send consumes credit actually granted below");
                    self.queued =
                        self.queued.checked_add(u32::try_from(bytes.len()).expect("small frame")).expect("small count");
                    assert!(self.queued <= sizes::output_cap(self.machine.endpoint(), &self.sizes).expect("cap fits"));
                    self.seen.push(Seen::Output(bytes.into_vec()));
                }
                stream::Down::Finish => self.seen.push(Seen::Finish),
            }
        }
        while let Some(event) = self.upper.pop() {
            match event {
                Event::Message(message) => self.seen.push(Seen::Frame(message)),
                Event::Ready { version } => self.seen.push(Seen::Ready(version)),
                Event::Sent => self.seen.push(Seen::Sent),
                Event::Unsent => self.seen.push(Seen::Unsent),
                Event::ReadEnded => self.seen.push(Seen::ReadEnded),
                Event::Closed { fault } => self.seen.push(Seen::Closed(fault)),
            }
        }
    }
    /// Lower output drains and answers Room, but its answer remains queued above.
    pub fn hold_room(&mut self) {
        let (_, room) = self.demand.take().expect("a room demand exists");
        assert!(room > 0);
        assert!(!self.room_pending);
        self.queued = 0;
        self.credit = room;
        self.room_pending = true;
    }
    pub fn deliver_room(&mut self) {
        assert!(self.room_pending);
        self.input(stream::Up::Room);
    }
    #[must_use]
    pub const fn credit(&self) -> u32 {
        self.credit
    }

    /// One iteration: fragmented input, delayed output credit, then one answer.
    pub fn tick(&mut self) {
        machine::poll(&mut self.machine, &self.limits, &self.sizes, &mut self.upper, &mut self.lower);
        self.drain();
        if self.rng.chance(400) {
            self.queued = 0;
        }
        let available =
            self.peer.len().saturating_sub(self.at).min(usize::try_from(self.intake.room()).expect("u32 fits"));
        if available > 0 {
            let count = usize::try_from(self.rng.between(1, u64::try_from(available).expect("small tape")))
                .expect("small count");
            self.intake.append(&self.peer[self.at..self.at + count]).expect("fragment fits intake");
            self.at += count;
        }
        if let Some((read, room)) = self.demand {
            if room > 0 && self.queued == 0 && self.rng.chance(250) {
                self.input(stream::Up::Room);
            } else if let Some(bytes) = self.intake.meet(read) {
                self.input(stream::Up::Bytes(bytes));
            }
        }
    }
    /// Drive until the predicate observed by the test holds, within a fixed bound.
    pub fn settle(&mut self, steps: u32) {
        for _ in 0..steps {
            self.tick();
        }
    }
    pub fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            self.input(stream::Up::End);
        }
    }
    pub fn open_agent(&mut self) {
        self.append(&Message::Open {
            open: Open { channel: Channel::Agent, lowest: 1, highest: 1, name: Box::from([]), secret: Box::from([]) },
        });
        let terms = sizes::terms(Endpoint::WorkerAgent, &self.sizes).expect("terms fit");
        self.append(&Message::Terms { terms });
        self.append(&Message::AgentStart {
            charter: Box::from(*b"charter"),
            snapshot: None,
            repositories: Box::from([]),
            endpoints: Box::from([]),
            grants: Box::from([]),
        });
        self.settle(1000);
        assert_eq!(self.machine.phase(), Phase::Open);
    }
}
/// A link run with a slow user; all choices and bytes replay from the seed.
#[must_use]
pub fn replay(seed: u64) -> Vec<Seen> {
    let mut world = World::new(Endpoint::Engine, seed);
    world.append(&Message::Open {
        open: Open {
            channel: Channel::Link,
            lowest: 1,
            highest: 1,
            name: Box::from(*b"worker"),
            secret: Box::from(*b"secret"),
        },
    });
    let terms = sizes::terms(Endpoint::WorkerLink, &world.sizes).expect("terms fit");
    world.append(&Message::Terms { terms });
    world.append(&Message::Hello { slots: 2, workstreams: Box::from([]), hosting: Box::from([]) });
    for call in 0..8 {
        world.append(&Message::Relay { run: 1, attempt: 1, call, body: Box::from(*b"relay") });
    }
    let mut authorized = false;
    for _ in 0..4000 {
        world.tick();
        if world.machine.phase() == Phase::Authorizing && !authorized {
            authorized = true;
            world.request(Request::Accept { version: 1 });
        }
        if world.machine.phase() == Phase::Open && world.rng.chance(50) {
            world.request(Request::Read);
        }
    }
    assert_eq!(world.machine.received_frames(), 11);
    world.request(Request::Close);
    world.seen
}
