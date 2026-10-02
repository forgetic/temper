//! The session sub-model of the temper coding agent's model layer
//! (programming-model.md, 4.5): one conversation with an LLM, driven turn by
//! turn, running the tools the LLM asks for. When the LLM stops calling tools
//! the session yields to its opener, which continues it with a new message or
//! closes it; a failure, a limit or its budget (turns, tokens and time, given
//! by the opener) ends it on its own.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change nothing
//! but the [`Model`] they are given. Time and randomness are inputs; every
//! effect, from calling an LLM to telling the opener the session has ended, is
//! a [`Request`] that its parent, the top-level model (`temper-agent-model`),
//! routes on, and its outcome comes back later through the parent as an
//! [`Event`].
//!
//! The conversation is provider-neutral ([`llm`]): the protocol layer speaks
//! each provider's wire format.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the session decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod facts;
mod limits;
pub mod llm;
mod model;
mod session;
#[cfg(test)]
mod tests;

pub use boundary::{Budget, Dimension, End, Event, Request, Spec, ToolCall, Yield};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
pub use model::{MAX_OUT, Model, fire, step};
