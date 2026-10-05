//! A scripted parent/provider world for concrete version-two sessions. The
//! referee observes only requests: provider bytes, turns and spend, never
//! session state. The harness separately checks boundary and settle contracts.

use skein_lib::{Env, Queue, Time, Token, Wall};
use temper_agent_domain_session::{self as session, llm, record};
use temper_agent_domain_tools::{Authority, Effect, Grants};

pub const PRICES: record::Prices = record::Prices { input: 7, cached: 3, output: 11, unit: 10 };
pub const USAGE: llm::Usage =
    llm::Usage { input_tokens: 11, output_tokens: 4, cache_read_tokens: 5, cache_write_tokens: 3 };
pub const OPAQUE: &[u8] = b"\0provider reasoning\xffsignature";

#[must_use]
pub fn opening(transcript: Option<record::Transcript>, budget: u64) -> record::Opening {
    record::Opening {
        spec: session::Spec {
            endpoint: llm::Endpoint(7),
            model: b"model".as_slice().into(),
            system: b"instructions".as_slice().into(),
            authority: Authority {
                cwd: Box::default(),
                repos: Box::default(),
                grants: Grants { inspect: false, modify: false, shell: false },
                env: Box::default(),
            },
            delegated: Box::new([llm::Descriptor { ticket: Token::new(19), effect: Effect::Write }]),
            prompt: b"wake".as_slice().into(),
            max_tokens: 128,
            budget: crate::BUDGET,
        },
        dialect: 23,
        prices: PRICES,
        budget,
        transcript,
    }
}

pub struct World {
    pub domain: session::Domain,
    pub env: Env<session::Limits>,
    pub out: Queue<session::Request>,
    pub session: Option<Token>,
    pub completing: Option<Token>,
    pub delegated: Vec<Token>,
    pub prompts: Vec<llm::Prompt>,
    pub turns: Vec<record::Turn>,
    pub spend: Vec<(u64, bool)>,
    pub end: Option<session::End>,
    pub trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new(seed: u64, facts: u32) -> World {
        let mut limits = crate::Settings::calm(seed).agent;
        limits.sessions = 1;
        limits.spend = u64::MAX;
        limits.messages = 32;
        limits.facts = facts;
        World {
            domain: session::Domain::new(&limits, seed),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(session::max_out(&limits)),
            session: None,
            completing: None,
            delegated: vec![],
            prompts: vec![],
            turns: vec![],
            spend: vec![],
            end: None,
            trace: vec![],
        }
    }

    pub fn step(&mut self, event: session::Event) {
        match &event {
            session::Event::Completed { owner, .. }
            | session::Event::Cancelled { owner }
            | session::Event::Failed { owner, .. } => {
                assert_eq!(self.completing.take(), Some(*owner), "one terminal per completion");
            }
            session::Event::AnsweredV2 { owner, .. }
            | session::Event::AnswerCancelledV2 { owner, .. }
            | session::Event::AnswerCancelled { owner } => {
                let position =
                    self.delegated.iter().position(|pending| pending == owner).expect("one terminal per delegate");
                self.delegated.remove(position);
            }
            session::Event::Open { .. }
            | session::Event::OpenV2 { .. }
            | session::Event::Continue { .. }
            | session::Event::Close { .. }
            | session::Event::Done { .. }
            | session::Event::Answered { .. } => {}
        }
        self.domain.reclaim();
        session::step(&mut self.domain, &self.env, event, &mut self.out);
        assert!(self.out.len() <= session::max_out(&self.env.limits));
        while let Some(request) = self.out.pop() {
            self.trace.push(format!("{request:?}"));
            match request {
                session::Request::Opened { opener, session } => {
                    assert_eq!(opener, Token::new(31));
                    assert!(self.session.replace(session).is_none());
                }
                session::Request::Complete { owner, prompt, .. } => {
                    assert!(self.completing.replace(owner).is_none());
                    self.prompts.push(prompt);
                }
                session::Request::Delegate { owner, .. } => {
                    assert!(!self.delegated.contains(&owner));
                    self.delegated.push(owner);
                }
                session::Request::Turn { opener, turn } => {
                    assert_eq!(opener, Token::new(31));
                    self.turns.push(turn);
                }
                session::Request::Priced { opener, spent, overflow } => {
                    assert_eq!(opener, Token::new(31));
                    self.spend.push((spent, overflow));
                }
                session::Request::Ended { opener, end, .. } => {
                    assert_eq!(opener, Token::new(31));
                    assert!(self.end.replace(end).is_none());
                }
                session::Request::Yielded { .. }
                | session::Request::Used { .. }
                | session::Request::Cancel { .. }
                | session::Request::Withdraw { .. } => {}
                session::Request::Io { .. } | session::Request::CancelIo { .. } => {
                    panic!("the scenario has no owned operations")
                }
            }
        }
        while self.domain.pop_fact().is_some() {}
    }

    pub fn open(&mut self, opening: record::Opening) {
        self.step(session::Event::OpenV2 { opener: Token::new(31), spec: opening });
    }
    pub fn complete(&mut self, content: Box<[llm::Block]>, stop: llm::Stop, usage: llm::Usage) {
        self.step(session::Event::Completed {
            owner: self.completing.expect("a completion is in flight"),
            completion: llm::Completion { content, stop, usage },
        });
    }
    pub fn close(&mut self) {
        self.step(session::Event::Close { session: self.session.expect("admitted") });
        if let Some(owner) = self.completing {
            self.step(session::Event::Cancelled { owner });
        }
        self.domain.reclaim();
        assert!(self.end.is_some());
        assert_eq!(self.domain.sessions(), 0);
        assert_eq!(self.domain.runs(), 0);
        assert_eq!(self.domain.kits(), 0);
        assert!(self.completing.is_none() && self.delegated.is_empty());
    }
}

#[must_use]
pub fn called() -> Box<[llm::Block]> {
    Box::new([
        llm::Block::Opaque { bytes: OPAQUE.into() },
        llm::Block::ToolCall {
            id: b"provider-call".as_slice().into(),
            name: b"subagent".as_slice().into(),
            input: br#"{"task":"review"}"#.as_slice().into(),
            call: llm::Decoded::Delegated { ticket: Token::new(991), effect: Effect::Write },
        },
    ])
}

#[must_use]
pub fn scenario(seed: u64, facts: u32) -> World {
    let mut world = World::new(seed, facts);
    world.open(opening(None, 100));
    world.complete(called(), llm::Stop::ToolUse, USAGE);
    assert!(world.turns.is_empty(), "the turn waits for its tool results");
    let owner = world.delegated[0];
    world.step(session::Event::AnsweredV2 { owner, text: b"child finished".as_slice().into(), error: false, spent: 9 });
    // Independent referee arithmetic: ceil((14*7 + 5*3 + 4*11)/10)=16,
    // then the child contributes 9. One terminal response charges it once.
    assert_eq!(world.spend, [(16, false), (25, false)]);
    assert_eq!(world.turns.len(), 1);
    assert_eq!(world.turns[0].spent, 25);
    assert_eq!(world.turns[0].messages.len(), 3);
    let blocks = &world.turns[0].messages[1].content;
    assert_eq!(blocks[0], llm::Block::Opaque { bytes: OPAQUE.into() });
    assert!(matches!(blocks[1], llm::Block::ToolCall { call: llm::Decoded::Historical, .. }));
    assert_eq!(
        world.turns[0].messages[2].content[0],
        llm::Block::ToolResult {
            id: b"provider-call".as_slice().into(),
            result: llm::Returned::Text { text: b"child finished".as_slice().into(), error: false }
        }
    );
    world.complete(
        Box::new([llm::Block::Text { text: b"done".as_slice().into() }]),
        llm::Stop::EndTurn,
        llm::Usage { output_tokens: 1, ..llm::Usage::ZERO },
    );
    assert_eq!(world.turns[1].spent, 27); // ceil(11/10)=2, per completion.
    world.close();
    world
}

#[must_use]
pub fn transcript(world: &World) -> record::Transcript {
    record::Transcript {
        version: record::VERSION,
        endpoint: llm::Endpoint(7),
        dialect: 23,
        turns: world.turns.clone().into(),
        after: Box::default(),
    }
}
