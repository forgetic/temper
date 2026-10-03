//! The notes sub-model's state and its entry points (programming-model.md,
//! section 3).

use temper_lib::{Env, Id, Map, Queue, Slab, Token};

use crate::boundary::{Event, Request, Scope};
use crate::call::{self, Call};
use crate::facts::{Fact, Facts};
use crate::kept::{self, Kept};
use crate::limits::{self, Limits};

/// The most requests a step or a resume emits: the first operation on each
/// of the three scopes a call needs kept; or a call's answer and the next
/// operation on the scope that answered it.
pub const MAX_OUT: u32 = 3;

/// The notes sub-model's state.
#[derive(Debug)]
pub struct Model {
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

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let ops = limits::ops(limits).expect("worst_case accepted the limits");
        Model {
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

    /// How many facts were dropped for want of room since the model was made.
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
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Index { reply_to, scopes, budget } => call::index(model, env, reply_to, scopes, budget, out),
        Event::Search { reply_to, scopes, query, most } => call::search(model, env, reply_to, scopes, query, most, out),
        Event::Recall { reply_to, recall } => call::recall(model, env, reply_to, recall, out),
        Event::Note { reply_to, scope, name, change } => call::note(model, env, reply_to, scope, name, change, out),
        Event::Refresh { scope } => kept::refresh(model, scope, out),
        Event::Changed { scope, name } => kept::changed(model, env, scope, name, out),
        // A terminal that names no operation in flight is dropped.
        Event::Listed { owner, pages } => match end(model, owner) {
            Some(Op::Scope(kept)) => kept::listed(model, env, kept, pages, out),
            Some(Op::Recall(_)) => unreachable!("a recall lists nothing"),
            None => {}
        },
        Event::Fetched { owner, fetched } => match end(model, owner) {
            Some(Op::Scope(kept)) => kept::fetched(model, env, kept, fetched, out),
            Some(Op::Recall(call)) => call::fetched(model, env, call, fetched, out),
            None => {}
        },
        Event::Wrote { owner, wrote } => match end(model, owner) {
            Some(Op::Scope(kept)) => kept::wrote(model, kept, wrote, out),
            Some(Op::Recall(_)) => unreachable!("a recall writes nothing"),
            None => {}
        },
    }
}

/// Answers, or goes on with, the next call whose scopes have all been read
/// once, emitting at most [`MAX_OUT`] requests.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(call) = model.ready.pop() {
        call::resume(model, env, call, out);
    }
}

/// Starts an operation for `op`, and names it.
pub(crate) fn start(model: &mut Model, op: Op) -> Id<Op> {
    model.ops.insert(op).expect("room for an operation for each scope and each call, twice over")
}

/// Ends the operation `owner` names: whom it was for, or `None` if it names
/// none in flight.
fn end(model: &mut Model, owner: Token) -> Option<Op> {
    let id = Id::from_token(owner);
    let op = *model.ops.get(id)?;
    model.ops.retire(id);
    Some(op)
}
