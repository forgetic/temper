//! What waits for an answer (programming-style.md, 4.2): every request the
//! top level makes of a sub-model or of the protocol layer that is ended
//! later is named by the token of a [`Wait`], which says what the answer is
//! for: a step of an item's job, a brief's read, a notes' wiki operation, a
//! run's call, a person's call, a write made on the side, or an operation
//! of the store. Payloads a forge op names by token are filled in from the
//! wait of the write that carries them, as the call goes out.
//!
//! What the top level carries for the fleet ([`Carried`]) is named by tokens
//! of its own, which the fleet echoes exactly once: a run's answer until the
//! hub has it durably or does not want it, a run's call and its answer, a
//! run's report on its way to the views.

use alloc::boxed::Box;

use temper_engine_model_brief as brief;
use temper_engine_model_notes as notes;
use temper_engine_model_views::Kind;
use temper_lib::{Id, List, ReplyTo, Token};

use crate::boundary::{Answer, Ask, Call, Item, Served};
use crate::items::Entry;

/// What an answer is for.
#[derive(Debug)]
pub(crate) enum Wait {
    /// A step of the item's job, whose state says which.
    Job { entry: Id<Entry> },
    /// The hub's answer to taking the item in.
    Take { entry: Id<Entry> },
    /// A record written on the side, as an item's relations change, or a
    /// write whose outcome changes nothing (the labels of an item handed
    /// in, a pull request opened again on a release).
    Aside { entry: Option<Id<Entry>> },
    /// A brief's read of a section's source.
    Brief { owner: Token, bounds: Bounds, source: brief::Source },
    /// A notes' wiki operation.
    Wiki { owner: Token, op: Wiki },
    /// A run's call, the fleet's `to`.
    Relay { to: ReplyTo },
    /// A person's call: their permission read, the forge's write, or the
    /// hub's answer.
    Person { to: ReplyTo, person: u64, ask: Ask },
    /// The views' operation on the store.
    Views { owner: Token, expire: bool },
    /// Answered: the wait is retired.
    Done,
}

/// What a brief's read may bring, and how a source with more is cut.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Bounds {
    pub(crate) keep: brief::Keep,
    pub(crate) fit: brief::Fit,
    pub(crate) parts: u32,
    pub(crate) bytes: u32,
}

/// A wiki operation of the notes', in the forge's terms.
#[derive(Debug)]
pub(crate) enum Wiki {
    /// Listing the pages of a scope: those found so far, and the wiki's next
    /// page after `after`.
    List {
        scope: notes::Scope,
        found: List<notes::Listed>,
    },
    Fetch,
    /// Writing a page: `revision` is the page's as read first when it is
    /// edited (`None` while it is read, for an edit).
    Create {
        page: Box<notes::Page>,
    },
    Edit {
        repository: u32,
        name: Box<[u8]>,
        page: Box<notes::Page>,
        read: bool,
    },
    Delete,
}

/// What the top level carries for the fleet, by a token the fleet echoes.
#[derive(Debug)]
pub(crate) enum Carried {
    /// A run's answer, until the hub has it durably or does not want it.
    Answer { item: Item, attempt: u64, answer: Box<Answer> },
    /// A run's call.
    Call { body: Box<Call> },
    /// The answer to a run's call.
    Served { served: Box<Served> },
    /// A run's report, for the views.
    Report { kind: Kind, content: Box<[u8]> },
}

impl Wait {
    /// The item whose job the wait is a step of.
    pub(crate) const fn job(&self) -> Option<Id<Entry>> {
        match self {
            Wait::Job { entry } => Some(*entry),
            Wait::Take { .. }
            | Wait::Aside { .. }
            | Wait::Brief { .. }
            | Wait::Wiki { .. }
            | Wait::Relay { .. }
            | Wait::Person { .. }
            | Wait::Views { .. }
            | Wait::Done => None,
        }
    }

    /// The fleet's `to` of the run's call the wait answers.
    pub(crate) fn relay(self) -> Option<ReplyTo> {
        match self {
            Wait::Relay { to } => Some(to),
            Wait::Job { .. }
            | Wait::Take { .. }
            | Wait::Aside { .. }
            | Wait::Brief { .. }
            | Wait::Wiki { .. }
            | Wait::Person { .. }
            | Wait::Views { .. }
            | Wait::Done => None,
        }
    }

    /// The brief's read the wait answers: its owner and bounds.
    pub(crate) const fn brief(&self) -> Option<(Token, Bounds)> {
        match self {
            Wait::Brief { owner, bounds, .. } => Some((*owner, *bounds)),
            Wait::Job { .. }
            | Wait::Take { .. }
            | Wait::Aside { .. }
            | Wait::Wiki { .. }
            | Wait::Relay { .. }
            | Wait::Person { .. }
            | Wait::Views { .. }
            | Wait::Done => None,
        }
    }
}

impl Carried {
    /// The run's answer carried, with its item and attempt.
    pub(crate) fn answer(&self) -> Option<(Item, u64, &Answer)> {
        match self {
            Carried::Answer { item, attempt, answer } => Some((*item, *attempt, answer)),
            Carried::Call { .. } | Carried::Served { .. } | Carried::Report { .. } => None,
        }
    }

    pub(crate) fn answer_mut(&mut self) -> Option<&mut Answer> {
        match self {
            Carried::Answer { answer, .. } => Some(answer),
            Carried::Call { .. } | Carried::Served { .. } | Carried::Report { .. } => None,
        }
    }

    pub(crate) fn call(self) -> Option<Box<Call>> {
        match self {
            Carried::Call { body } => Some(body),
            Carried::Answer { .. } | Carried::Served { .. } | Carried::Report { .. } => None,
        }
    }

    pub(crate) fn served(self) -> Option<Box<Served>> {
        match self {
            Carried::Served { served } => Some(served),
            Carried::Answer { .. } | Carried::Call { .. } | Carried::Report { .. } => None,
        }
    }

    pub(crate) fn report(self) -> Option<(Kind, Box<[u8]>)> {
        match self {
            Carried::Report { kind, content } => Some((kind, content)),
            Carried::Answer { .. } | Carried::Call { .. } | Carried::Served { .. } => None,
        }
    }
}
