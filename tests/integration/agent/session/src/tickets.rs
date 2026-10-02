//! Tickets: the values a session names but cannot hold, kept for it by its
//! opener's side, as the top-level model keeps them for a real session: the
//! tools the opener serves, the calls the LLM made to them, and the opener's
//! answers. A session's tickets are freed when it ends, and resolving one
//! after that is a bug the world catches.

use std::collections::BTreeMap;

use temper_agent_model_tools::Effect;
use temper_lib::Token;

/// The tools the fake opener serves: a finish, which writes, and a lookup,
/// which only reads and is slow.
pub const SERVED: [(&[u8], Effect, &[u8]); 2] =
    [(b"finish", Effect::Write, br#"{"summary":"string"}"#), (b"lookup", Effect::Read, br#"{"path":"string"}"#)];

/// What a ticket names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ticketed {
    /// A tool the opener serves.
    Tool { name: &'static [u8], effect: Effect, schema: &'static [u8] },
    /// A call the LLM made to such a tool, with the arguments it wrote.
    Call { tool: &'static [u8], arguments: Box<[u8]> },
    /// The opener's answer to a call.
    Answer { text: Box<[u8]>, error: bool },
}

/// The tickets of every session, by the opener's name for the session each
/// is for.
#[derive(Default, Debug)]
pub struct Tickets {
    issued: u64,
    held: BTreeMap<Token, (u64, Ticketed)>,
}

impl Tickets {
    /// A new ticket for `value`, held for the session of `opener`.
    pub fn issue(&mut self, opener: u64, value: Ticketed) -> Token {
        self.issued += 1;
        let ticket = Token::new(self.issued);
        self.held.insert(ticket, (opener, value));
        ticket
    }

    /// What `ticket` names; only while its session lives.
    #[must_use]
    pub fn resolve(&self, ticket: Token) -> &Ticketed {
        let (_, value) = self.held.get(&ticket).expect("a ticket is resolved only while its session lives");
        value
    }

    /// Frees the tickets of the session of `opener`, which has ended.
    pub fn free(&mut self, opener: u64) {
        self.held.retain(|_, (held, _)| *held != opener);
    }

    /// Tickets held, for the sessions that live.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}
