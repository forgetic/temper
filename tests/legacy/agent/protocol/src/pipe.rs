//! Byte peer for the production agent channel. The peer uses real codecs;
//! its demand/credit ledger checks the shared stream contract.
use crate::fixture;
use skein_lib::{Duration, Env, Intake, Queue, Time, Token, Wall, stream};
use temper_channel::{codec, machine::Endpoint, payload::v1 as payload, sizes, wire};
use temper_legacy_agent_domain::{self as agent, run::charter::Checkout};
use temper_legacy_agent_protocol::channel::{self, Channel, Up};

#[must_use]
pub fn limits() -> channel::Limits {
    let mut sizes = temper_channel::Sizes::STARTING;
    sizes.endpoints = 4;
    sizes.accounts = 2;
    sizes.token_bytes = 256;
    sizes.name_bytes = 128;
    sizes.facts = 2;
    sizes.fact = 256;
    channel::Limits {
        protocol: fixture::limits(),
        channel: temper_channel::Limits { chunk: 64, ..temper_channel::Limits::STARTING },
        sizes,
    }
}
#[must_use]
pub fn charter() -> payload::Charter {
    payload::Charter {
        why: payload::Why::Work,
        brief: Box::new([]),
        instructions: b"look".as_slice().into(),
        grants: payload::Permissions { modify: false, shell: false, forge: false, subagents: false, note: false },
        finish: payload::FinishSpec::Report { grows: false },
        budget: payload::Budget { tokens: 1000, turns: 2, time: Duration::from_secs(30) },
        models: Box::new([payload::Model { endpoint: 0, model: b"fake".as_slice().into(), max_tokens: 100 }]),
        policy: payload::Policy {
            text: payload::Capture::Content,
            progress: payload::Capture::Content,
            calls: payload::Capture::Content,
            tools: payload::Capture::Content,
            usage: payload::Capture::Content,
        },
    }
}
#[must_use]
pub fn grant(account: u32, generation: u64) -> wire::Grant {
    wire::Grant {
        account,
        generation,
        valid: Duration::from_secs(10),
        token: b"secret".as_slice().into(),
        account_id: b"user".as_slice().into(),
    }
}
#[must_use]
pub fn start() -> wire::Message {
    wire::Message::AgentStart {
        charter: payload::encode_charter(&charter(), &limits().sizes).expect("charter"),
        snapshot: None,
        repositories: Box::new([wire::AgentRepository { name: b"source".as_slice().into(), writable: false }]),
        endpoints: Box::new([fixture::endpoint(wire::Provider::OpenAi)]),
        grants: Box::new([grant(0, 1)]),
    }
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent input EOF, output EOF, one-time delivery and blocked-room flags"
)]
pub struct World {
    pub channel: Channel,
    pub now: Time,
    pub events: Vec<Up>,
    pub messages: Vec<wire::Message>,
    pub blocked: bool,
    pub output_ended: bool,
    pub configured: channel::Limits,
    incoming: Intake,
    outgoing: Vec<u8>,
    demand: Option<(stream::Read, u32)>,
    credit: u32,
    end: bool,
    ended: bool,
    up: Queue<Up>,
    down: Queue<stream::Down>,
}
impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
impl World {
    #[must_use]
    pub fn new() -> World {
        let configured = limits();
        let mut world = World {
            channel: Channel::new(Token::new(5), Time::ZERO, &configured, Duration::ZERO).expect("owner bounds"),
            now: Time::ZERO,
            configured,
            events: Vec::new(),
            messages: Vec::new(),
            blocked: false,
            output_ended: false,
            incoming: Intake::with_capacity(1 << 20),
            outgoing: Vec::new(),
            demand: None,
            credit: 0,
            end: false,
            ended: false,
            up: Queue::with_capacity(channel::MAX_UP),
            down: Queue::with_capacity(channel::MAX_DOWN),
        };
        world.peer(wire::Message::Open {
            open: wire::Open {
                channel: wire::Channel::Agent,
                lowest: 1,
                highest: 1,
                name: Box::new([]),
                secret: Box::new([]),
            },
        });
        world.peer(wire::Message::Terms {
            terms: sizes::terms(Endpoint::WorkerAgent, &configured.sizes).expect("terms"),
        });
        world.settle();
        world
    }
    #[must_use]
    pub fn env(&self) -> Env<channel::Limits> {
        Env { now: self.now, wall: Wall::EPOCH, limits: self.configured }
    }
    fn drain(&mut self) {
        while let Some(event) = self.up.pop() {
            self.events.push(event);
        }
        while let Some(value) = self.down.pop() {
            match value {
                stream::Down::Demand { read, room } => {
                    if read == stream::Read::Nothing && room == 0 {
                        self.demand = None;
                    } else {
                        assert!(self.demand.is_none(), "one stream demand");
                        self.demand = Some((read, room));
                    }
                }
                stream::Down::Send(bytes) => {
                    assert!(!self.output_ended, "no Send after Finish");
                    let n = u32::try_from(bytes.len()).expect("bounded output");
                    assert!(n <= self.credit, "send fits real lower credit");
                    self.credit -= n;
                    self.outgoing.extend(bytes.iter().copied());
                }
                stream::Down::Finish => self.output_ended = true,
            }
        }
        while self.outgoing.len() >= 8 {
            let head = codec::header(&self.outgoing[..8], &self.configured.sizes).expect("frame header");
            let n = head.length as usize + 8;
            if self.outgoing.len() < n {
                break;
            }
            self.messages.push(codec::decode(&self.outgoing[..n], &self.configured.sizes).expect("production frame"));
            self.outgoing.drain(..n);
        }
    }
    pub fn settle(&mut self) {
        for _ in 0..20_000 {
            let mut moved = false;
            if let Some((read, room)) = self.demand {
                let event = if room > 0 && !self.blocked {
                    self.demand = None;
                    self.credit = room;
                    Some(stream::Up::Room)
                } else if let Some(bytes) = self.incoming.meet(read) {
                    self.demand = None;
                    Some(stream::Up::Bytes(bytes))
                } else if self.end && !self.ended {
                    self.ended = true;
                    Some(stream::Up::End)
                } else {
                    None
                };
                if let Some(event) = event {
                    self.event(event);
                    moved = true;
                }
            }
            if self.channel.is_ready() {
                let env = self.env();
                channel::resume(&mut self.channel, &env, &mut self.up, &mut self.down);
                self.drain();
                moved = true;
            }
            if !moved {
                return;
            }
        }
        panic!("channel must block or settle");
    }
    pub fn peer(&mut self, value: wire::Message) {
        let bytes = codec::encode(&value, &self.configured.sizes).expect("peer fixture fits");
        drop(value);
        self.incoming.append(&bytes).expect("bounded tape");
    }
    pub fn event(&mut self, event: stream::Up) {
        let env = self.env();
        channel::up(&mut self.channel, &env, event, &mut self.up, &mut self.down);
        self.drain();
    }
    pub fn roots(&mut self) {
        let repositories = self
            .channel
            .repositories()
            .iter()
            .map(|r| agent::run::charter::Repository {
                name: r.name.clone(),
                root: Token::new(7),
                writable: r.writable,
            })
            .collect();
        let env = self.env();
        channel::roots(&mut self.channel, &env, Checkout { repositories }, &mut self.up).expect("prepared roots");
        self.drain();
    }
    pub fn down(&mut self, request: agent::Request) {
        let env = self.env();
        assert!(self.channel.can_take(&request));
        channel::down(&mut self.channel, &env, request, &mut self.up).expect("domain request");
        self.drain();
    }
    pub fn eof(&mut self) {
        self.end = true;
        self.settle();
    }
    pub fn closed(&mut self) {
        channel::closed(&mut self.channel, &mut self.up);
        self.drain();
    }
    pub fn fire(&mut self) {
        let env = self.env();
        channel::fire(&mut self.channel, &env, &mut self.up, &mut self.down);
        self.drain();
        self.settle();
    }
    pub fn content(&mut self, value: &agent::Content) -> bool {
        let env = self.env();
        let admitted = channel::content(&mut self.channel, &env, value).expect("content projection");
        self.drain();
        admitted
    }
}
