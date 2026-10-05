//! Byte peer for the production exchange. The tape is an independent server
//! response, and deliveries obey Intake's exact read/room contract.
use crate::fixture;
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, Intake, Queue, Time, Token, Wall};
use temper_channel::wire::{Grant, Provider};
use temper_legacy_agent_domain::{GrantName, llm, tools};
use temper_legacy_agent_protocol::{exchange, grants, translate};

pub struct World {
    pub exchange: exchange::Exchange,
    pub env: Env<temper_legacy_agent_protocol::Limits>,
    pub events: Vec<exchange::Event>,
    pub sent: Vec<u8>,
    pub closing: bool,
    pub slice: usize,
    pub room_enabled: bool,
    pub live: bool,
    source: Vec<u8>,
    offset: usize,
    intake: Intake,
    read: Read,
    wanted: u32,
    credit: u32,
    above: Queue<exchange::Event>,
    below: Queue<Down>,
}
#[must_use]
pub fn admission(owner: u64) -> exchange::Admission {
    exchange::Admission {
        owner: Token::new(owner),
        grant: GrantName { account: 0, generation: 1 },
        prompt: llm::Prompt {
            endpoint: llm::Endpoint(0),
            model: b"model".as_slice().into(),
            system: b"system".as_slice().into(),
            tools: tools::Grants { inspect: true, modify: false, shell: false },
            served: Box::new([]),
            messages: Box::new([llm::Message {
                role: llm::Role::User,
                content: Box::new([llm::Block::Text { text: b"hello".as_slice().into() }]),
            }]),
            max_tokens: 100,
        },
        timeout: Duration::from_secs(20),
        session: b"00000000-0000-4000-8000-000000000001".as_slice().into(),
        request: format!("00000000-0000-4000-8000-{owner:012x}").into_bytes().into(),
    }
}
#[must_use]
pub fn prepared(provider: Provider, owner: u64) -> exchange::Exchange {
    prepared_value(provider, owner, b"access", b"account")
}
#[must_use]
pub fn prepared_value(provider: Provider, owner: u64, token: &[u8], account: &[u8]) -> exchange::Exchange {
    prepared_at(provider, owner, token, account, Time::ZERO)
}
#[must_use]
pub fn prepared_at(provider: Provider, owner: u64, token: &[u8], account: &[u8], now: Time) -> exchange::Exchange {
    let limits = fixture::limits();
    let mut table = grants::Table::new(&[0], &limits).expect("scope");
    table
        .insert(
            Grant {
                account: 0,
                generation: 1,
                token: token.into(),
                account_id: account.into(),
                valid: Duration::from_secs(60),
            },
            now,
            &limits,
        )
        .expect("grant");
    let identity = exchange::Identity {
        headers: Box::new([]),
        anthropic: translate::AnthropicIdentity { system: Box::new([]), metadata: None, context_management: None },
    };
    exchange::Exchange::prepare(admission(owner), &fixture::endpoint(provider), &table, &identity, now, &limits)
        .expect("measured request")
}
impl World {
    #[must_use]
    pub fn new(provider: Provider, tape: Vec<u8>, slice: usize) -> World {
        World::with(prepared(provider, 1), tape, slice)
    }
    #[must_use]
    pub fn with(exchange: exchange::Exchange, tape: Vec<u8>, slice: usize) -> World {
        let limits = fixture::limits();
        let mut world = World {
            exchange,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            events: Vec::new(),
            sent: Vec::new(),
            closing: false,
            slice,
            room_enabled: true,
            live: false,
            source: tape,
            offset: 0,
            intake: Intake::with_capacity(limits.head_bytes.max(limits.chunk)),
            read: Read::Nothing,
            wanted: 0,
            credit: 0,
            above: Queue::with_capacity(exchange::MAX_UP),
            below: Queue::with_capacity(exchange::MAX_DOWN),
        };
        exchange::start(&mut world.exchange, &world.env, &mut world.above, &mut world.below);
        world.collect();
        world
    }
    pub fn pump(&mut self) -> bool {
        if self.exchange.has_work() {
            exchange::resume(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
            self.collect();
            return true;
        }
        if self.closing || self.exchange.is_closed() {
            return false;
        }
        if self.wanted > 0 && self.room_enabled {
            self.credit = self.wanted;
            self.wanted = 0;
            self.deliver(Up::Room);
            return true;
        }
        if self.read != Read::Nothing {
            if self.offset < self.source.len() && self.intake.room() > 0 {
                let count = self
                    .slice
                    .max(1)
                    .min(usize::try_from(self.intake.room()).expect("fits"))
                    .min(self.source.len().saturating_sub(self.offset));
                let end = self.offset.checked_add(count).expect("tape bound");
                self.intake.append(self.source.get(self.offset..end).expect("slice")).expect("intake cap");
                self.offset = end;
            }
            if let Some(bytes) = self.intake.meet(self.read) {
                self.read = Read::Nothing;
                self.deliver(Up::Bytes(bytes));
                return true;
            }
            if self.offset == self.source.len() && !self.live {
                self.read = Read::Nothing;
                self.deliver(Up::End);
                return true;
            }
            return self.offset < self.source.len();
        }
        false
    }
    pub fn append(&mut self, bytes: &[u8]) {
        self.source.extend_from_slice(bytes);
    }
    pub fn drive(&mut self) {
        for _tick in 0..100_000 {
            if !self.pump() {
                return;
            }
        }
        panic!("bounded tape did not settle");
    }
    pub fn deliver(&mut self, event: Up) {
        exchange::up(&mut self.exchange, &self.env, event, &mut self.above, &mut self.below);
        self.collect();
    }
    pub fn cancel(&mut self) {
        exchange::cancel(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
        self.collect();
    }
    pub fn close(&mut self) {
        exchange::closed(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
        self.collect();
    }
    pub fn fire(&mut self, now: Time) {
        self.env.now = now;
        exchange::fire(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
        self.collect();
    }
    pub fn begin(&mut self, prepared: exchange::Exchange) {
        assert!(self.exchange.next(prepared).is_ok(), "kept HTTP state");
        exchange::start(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
        self.collect();
    }
    pub fn replace(&mut self, provider: Provider, owner: u64, tape: Vec<u8>) {
        let result = self.exchange.next(prepared(provider, owner));
        assert!(result.is_ok(), "kept HTTP state");
        self.source = tape;
        self.offset = 0;
        exchange::start(&mut self.exchange, &self.env, &mut self.above, &mut self.below);
        self.collect();
    }
    fn collect(&mut self) {
        while let Some(down) = self.below.pop() {
            match down {
                Down::Demand { read, room } => {
                    self.read = read;
                    self.wanted = room;
                    if read == Read::Nothing && room == 0 {
                        self.credit = 0;
                    }
                }
                Down::Send(bytes) => {
                    let n = u32::try_from(bytes.len()).expect("send fits");
                    assert!(n <= self.credit, "send exceeds granted room");
                    self.credit = self.credit.checked_sub(n).expect("credit");
                    self.sent.extend_from_slice(&bytes);
                }
                Down::Finish => {}
            }
        }
        while let Some(event) = self.above.pop() {
            if event == exchange::Event::Close {
                self.closing = true;
            }
            self.events.push(event);
        }
    }
}
