//! The model layer of the temper coding agent (programming-style.md, section 4;
//! agent-model.md, section 3): the agent loop's entry point.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Model`] they are given. Time and randomness are
//! inputs; every effect, from calling an LLM to answering the worker, is a
//! [`Request`] the layers below carry out, and its outcome comes back later
//! as an [`Event`].
//!
//! It is the top-level model over a tree of sub-models (4.5), and the only one
//! that faces the protocol layer:
//!
//! ```text
//! temper-agent-model                 faces the protocol; routes; translates run <-> session
//! ├── temper-agent-model-run         one agent instance: charter, conversations, outcome
//! └── temper-agent-model-session     one conversation with an LLM
//!     └── temper-agent-model-tools   read, list, search, write, edit, shell
//! ```
//!
//! It owns its children's state and routes each event to the child it is for
//! and each child's requests out: the worker's records and the run's own io to
//! and from the run, the LLM providers' and the tools' io to and from the
//! sessions. The run and the sessions are siblings that share no types: the
//! run opens conversations, and the top level opens a session for each,
//! translating between the two vocabularies through small total functions,
//! and keeping, for each conversation, the values its session carries as
//! tickets. Hand-offs between them that could chain within a step wait on a
//! ready list, which the loop drains with [`resume`] (the `model` module).
//!
//! Its records toward the protocol layer ([`Event`], [`Request`]) carry the
//! children's types where the children meet the protocol as they are (the
//! worker's records and the run's io, the run's; the tools' io, the tools'),
//! and its own conversation vocabulary ([`llm`]): the session's, with the
//! run's typed values in place of tickets.
//!
//! What happens is also told as content-free [`Fact`]s, the sub-models',
//! gathered into one bounded queue the loop drains ([`Model::pop_fact`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod facts;
mod limits;
pub mod llm;
mod model;
mod peer;
mod route;
#[cfg(test)]
mod tests;
mod translate;

pub use boundary::{Event, Request};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
pub use model::{Model, fire, max_out, resume, step};
// The payloads are the children's: a parent may use its children's types.
pub use temper_agent_model_run as run;
pub use temper_agent_model_session as session;
pub use temper_agent_model_tools as tools;
