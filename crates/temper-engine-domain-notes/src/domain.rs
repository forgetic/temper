//! The notes child domain's state and its entry points (programming-model.md,
//! section 3).

use skein_lib::{Env, Id, Map, Queue, Slab, Token};

use crate::boundary::{Event, Request, Scope};
use crate::call::{self, Call};
use crate::facts::{Fact, Facts};
use crate::kept::{self, Kept};
use crate::limits::{self, Limits};

/// The most requests a step or a resume emits: the first operation on each
/// of the three scopes a call needs kept; or a call's answer and the next
/// operation on the scope that answered it.
pub const MAX_OUT: u32 = 3;

/// The notes child domain's state.
#[derive(Debug)]
pub struct Domain {
    /// The scopes kept, by their scope.
    pub(crate) kept: Slab<Kept>,
    pub(crate) scopes: Map<Scope, Id<Kept>>,
    pub(crate) calls: Slab<Call>,
    /// The wiki operations in flight, each for a scope kept or a recall.
    pub(crate) ops: Slab<Op>,
    /// Calls whose scopes have all been read once, to answer or go on with.
    pub(crate) ready: Queue<Id<Call>>,
    /// Counts the uses of scopes, which says which was used least recently.
    pub(crate) uses: u64,
    pub(crate) facts: Facts,
}

/// A wiki operation in flight, and whom it is for: its `owner` token is its
/// handle's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Op {
    Scope(Id<Kept>),
    Recall(Id<Call>),
}

impl Domain {
    /// A domain with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Domain {
        let ops = limits::ops(limits).expect("worst_case accepted the limits");
        Domain {
            kept: Slab::with_capacity(limits.scopes),
            scopes: Map::with_capacity(limits.scopes),
            calls: Slab::with_capacity(limits.calls),
            ops: Slab::with_capacity(ops),
            ready: Queue::with_capacity(limits.calls),
            uses: 0,
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Scopes kept.
    #[must_use]
    pub fn scopes(&self) -> u32 {
        self.scopes.len()
    }

    /// Calls in flight, answered ones included until they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// Wiki operations in flight, ended ones included until they are
    /// reclaimed.
    #[must_use]
    pub fn ops(&self) -> u32 {
        self.ops.len()
    }

    /// Whether a call waits to be answered or gone on with by [`resume`].
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room since the domain was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what ended in this iteration.
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
        self.ops.reclaim();
    }
}

/// The most requests one step or resume emits under `limits`: the room the
/// parent reserves in `out`.
#[must_use]
pub const fn max_out(_limits: &Limits) -> u32 {
    MAX_OUT
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Index { reply_to, scopes, budget } => call::index(domain, env, reply_to, scopes, budget, out),
        Event::Search { reply_to, scopes, query, most } => {
            call::search(domain, env, reply_to, scopes, query, most, out);
        }
        Event::Recall { reply_to, recall } => call::recall(domain, env, reply_to, recall, out),
        Event::Note { reply_to, scope, name, change } => call::note(domain, env, reply_to, scope, name, change, out),
        Event::Refresh { scope } => kept::refresh(domain, scope, out),
        Event::Changed { scope, name } => kept::changed(domain, env, scope, name, out),
        // A terminal that names no operation in flight is dropped.
        Event::Listed { owner, pages } => match end(domain, owner) {
            Some(Op::Scope(kept)) => kept::listed(domain, env, kept, pages, out),
            Some(Op::Recall(_)) => unreachable!("a recall lists nothing"),
            None => {}
        },
        Event::Fetched { owner, fetched } => match end(domain, owner) {
            Some(Op::Scope(kept)) => kept::fetched(domain, env, kept, fetched, out),
            Some(Op::Recall(call)) => call::fetched(domain, env, call, fetched, out),
            None => {}
        },
        Event::Wrote { owner, wrote } => match end(domain, owner) {
            Some(Op::Scope(kept)) => kept::wrote(domain, kept, wrote, out),
            Some(Op::Recall(_)) => unreachable!("a recall writes nothing"),
            None => {}
        },
    }
}

/// Answers, or goes on with, the next call whose scopes have all been read
/// once, emitting at most [`MAX_OUT`] requests.
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(call) = domain.ready.pop() {
        call::resume(domain, env, call, out);
    }
}

/// Starts an operation for `op`, and names it.
pub(crate) fn start(domain: &mut Domain, op: Op) -> Id<Op> {
    domain.ops.insert(op).expect("room for an operation for each scope and each call, twice over")
}

/// Ends the operation `owner` names: whom it was for, or `None` if it names
/// none in flight.
fn end(domain: &mut Domain, owner: Token) -> Option<Op> {
    let id = Id::from_token(owner);
    let op = *domain.ops.get(id)?;
    domain.ops.retire(id);
    Some(op)
}
