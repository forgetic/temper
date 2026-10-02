//! The session sub-model of the temper coding agent's model layer
//! (programming-model.md, 4.5): one conversation with an LLM, driven turn by
//! turn, running the tools the LLM asks for, until the LLM finishes, a limit
//! ends it, or it expires.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change nothing
//! but the [`Model`] they are given. Time and randomness are inputs; every
//! effect, from calling an LLM to answering a caller, is a [`Request`] that its
//! parent, the top-level model (`temper-agent-model`), routes on, and its
//! outcome comes back later through the parent as an [`Event`].
//!
//! The conversation is provider-neutral ([`llm`]): the protocol layer speaks
//! each provider's wire format.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod limits;
pub mod llm;
mod model;
mod session;
#[cfg(test)]
mod tests;

pub use boundary::{Event, Outcome, Report, Request, Task, ToolCall};
pub use limits::{Limits, worst_case};
pub use model::{MAX_OUT, Model, fire, step};
