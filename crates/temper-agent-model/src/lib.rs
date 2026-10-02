//! The model layer of the temper coding agent (programming-model.md, section 4):
//! the agent loop's entry point.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change nothing
//! but the [`Model`] they are given. Time and randomness are inputs; every
//! effect, from calling an LLM to telling an opener its session has ended, is
//! a [`Request`] the layers below carry out, and its outcome comes back later
//! as an [`Event`].
//!
//! It is the top-level model over a tree of sub-models (4.5), and the only one
//! that faces the protocol layer: it owns its children's state, and routes each
//! event to the child it is for and each child's requests back out. Today it
//! holds only the session sub-model (`temper-agent-model-session`), one
//! conversation with an LLM; the run sub-model will sit between the protocol
//! layer and the sessions.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod limits;
mod model;
mod route;
#[cfg(test)]
mod tests;

pub use boundary::{Event, Request};
pub use limits::{Limits, worst_case};
pub use model::{MAX_OUT, Model, fire, step};
// The payloads are the session's: a parent may use its children's types.
pub use temper_agent_model_session::{End, Spec, ToolCall, Yield, llm};
