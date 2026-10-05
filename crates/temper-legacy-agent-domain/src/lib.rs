//! The domain layer of the temper coding agent (programming-model.md, section
//! 4; agent-domain.md, section 3): the agent loop's entry point.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Domain`] they are given. Time and randomness are
//! inputs; every effect, from calling an LLM to answering the worker, is a
//! [`Request`] the layers below carry out, and its outcome comes back later
//! as an [`Event`].
//!
//! It is the root domain over a tree of child domains (4.5), and the only one
//! that faces the protocol layer:
//!
//! ```text
//! temper-legacy-agent-domain                 faces the protocol; routes; translates run <-> session
//! ├── temper-legacy-agent-domain-run         one agent instance: charter, conversations, outcome
//! └── temper-legacy-agent-domain-session     one conversation with an LLM
//!     └── temper-legacy-agent-domain-tools   read, list, search, write, edit, shell
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
//! ready list, which the loop drains with [`resume`] (the `domain` module).
//!
//! Its records toward the protocol layer ([`Event`], [`Request`]) carry the
//! children's types where the children meet the protocol as they are (the
//! worker's records and the run's io, the run's; the tools' io, the tools'),
//! and its own conversation vocabulary ([`llm`]): the session's, with the
//! run's typed values in place of tickets.
//!
//! What happens is also told as content-free [`Fact`]s, the child domains',
//! gathered into one bounded queue the loop drains ([`Domain::pop_fact`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod domain;
mod facts;
mod limits;
pub mod llm;
mod peer;
mod route;
#[cfg(test)]
mod tests;
mod translate;

pub use boundary::{Event, Grant, GrantName, Request};
pub use domain::{Domain, fire, max_out, resume, step};
pub use facts::{Content, Fact};
pub use limits::{Limits, worst_case};
// The payloads are the children's: a parent may use its children's types.
pub use temper_legacy_agent_domain_run as run;
pub use temper_legacy_agent_domain_session as session;
pub use temper_legacy_agent_domain_tools as tools;
