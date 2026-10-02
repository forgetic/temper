//! Host calls in flight: a run's push, served through its workspace, or a
//! forge read or outlet, relayed to the engine (worker-model.md, 4.2, step 4).
//!
//! A call's transition table, as the host keeps it:
//!
//! ```text
//! state     event                  next      emits
//! -         called: push           Pushing   push
//!           called: relay          Relayed   relay
//! Pushing   pushed                 Closed    reply: how it went
//!           its run leaves live    Orphaned  reply: unavailable
//! Orphaned  pushed                 Closed
//! Relayed   relayed                Closed    reply: the engine's answer
//!           its run leaves live    Closed    reply: unavailable
//! ```
//!
//! A push cannot be abandoned half way, and it touches the workspace, so it
//! is waited for even once its run has been answered: an orphaned push keeps
//! its run stopping until it settles, and what it landed still counts. A
//! relayed call is the engine's to answer, which a cancelled attempt never
//! is: once its run leaves live the host answers it, and drops whatever the
//! engine sends for it after. A call is retired as it closes.

use temper_lib::{Id, Token};

use crate::hosted::Hosted;

#[derive(Debug)]
pub(crate) struct Call {
    /// The run that made it.
    pub(crate) hosted: Id<Hosted>,
    pub(crate) state: State,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum State {
    /// A push in flight, for the call `call` of the agent `agent`.
    Pushing { agent: Token, call: Token },
    /// A push in flight whose call was answered as its run left live.
    Orphaned,
    /// Relayed to the engine, for the call `call` of the agent `agent`.
    Relayed { agent: Token, call: Token },
    /// Terminal: holds nothing.
    Closed,
}
