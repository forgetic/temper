//! The model layer of the temper engine (programming-style.md, section 4;
//! engine-model.md, section 3): the engine loop's entry point.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Model`] they are given. Time and randomness are
//! inputs; every effect, from a call to the forge to an assignment to a
//! worker, a reply to a person or an operation of the store, is a
//! [`Request`] the layers below carry out, and its outcome comes back later
//! as an [`Event`].
//!
//! It is the top-level model over a tree of sub-models (4.5), and the only one
//! that faces the protocol layer:
//!
//! ```text
//! temper-engine-model                  faces the protocol: forge, workers, people, store; routes; translates
//! ├── temper-engine-model-work         the hub: items' lifecycle
//! ├── temper-engine-model-plan         policy: steps, what is due, wakes, what an outcome writes
//! ├── temper-engine-model-rules        policy: the deployment's rules over runs and writes
//! ├── temper-engine-model-forge        capability: the working set and the client
//! ├── temper-engine-model-fleet        capability: workers, placement, attempts, relaying
//! ├── temper-engine-model-brief        capability: a run's brief, sections within budgets
//! ├── temper-engine-model-notes        capability: scopes, entries, indexes
//! └── temper-engine-model-views        capability: reports in; streams and traces out
//! ```
//!
//! It owns its children's state, routes each event to the child it is for,
//! and completes the hand-offs between them before it returns, translating
//! between their vocabularies through small total functions (the `route`
//! and `translate` modules). `work` is the hub; `plan` and `rules` are pure,
//! and called in place; the others are capabilities.
//!
//! It keeps a table of the items it holds (the `items` module): for each,
//! the parts of its record (the hub's lifecycle, the plan's step, its own
//! relations), the job the hub asked for, the run in flight and its inbox.
//! It composes the record from its owners' parts as a write goes out and
//! splits it as one is read; nothing durable is ever a token. Payloads the
//! sub-models name by token are its own: records, outcomes and pages filled
//! in as the forge's calls go out, answers, calls and reports carried for
//! the fleet ([`Event`], [`Request`] carry them typed).
//!
//! Decisions the sub-models left to it, settled here:
//!
//! - **Starting cold** (the restart contract): a restart rebuilds from the
//!   forge and the store everything a decision reads, and nothing is made
//!   twice. Every item announced is taken into the hub as its record says,
//!   once the record is found sound (within the limits, its goal's plan one
//!   the plan could have made); one that is not is held as mangled. Its
//!   claim is adopted at once, with the grants its step's charter gives the
//!   run its record says is running; the pull request its record names is
//!   linked in the working set, so the plan reads it again. The fleet hears
//!   `Loaded` only once the cold read is done and every claim it read has
//!   reached the fleet; nothing new starts before (an item that did not fit
//!   the working set is not waited for: a worker still hosting its run
//!   keeps it as a stray, past the grace it is retried). Every item read
//!   waiting is asked what is due afresh: a relation found done is news
//!   again. What an earlier life may have made is looked for before it is
//!   made: an adopted attempt's outcome and comments after its claim's
//!   inbox position, an outcome read back after its comment, an engine
//!   action's creations and a person's keyed request anywhere. An adopted
//!   attempt takes nothing of the new life's inbox, so what it may not have
//!   seen goes again to the next run; nor does a run its worker lists as
//!   ending. A person who opened a session hears so only once its first
//!   record is written. A run's call that reaches no live claim is answered
//!   at once, unserved: busy before the cold start is done, failed after.
//! - **Hold reasons** are the plan's, coded as small integers that keep
//!   their meaning across restarts (zero: the record carries no step).
//! - **Relations** (dependencies, children, the goal, the pull request) are
//!   in the record, each marked done as it closes; one not held and not
//!   known done is read afresh before the plan decides.
//! - **Growth's two record writes** (the goal's and the growing step's): the
//!   goal's goes first, on the side, tried again while the forge fails for
//!   a while, and the growing step's, the commit point, waits for it to
//!   land; one that cannot be written fails the application, which is held
//!   with its outcome. Applied again after a restart, the plan recognises
//!   a growth it made.
//! - **People's messages** are written on their behalf, attributed to them,
//!   and are news from them.
//! - **A brief whose item stopped** runs to its end, and its answer is
//!   dropped; its reads are bounded by the brief's own deadline.
//!
//! What happens is also told as content-free [`Fact`]s, the sub-models' and
//! its own, gathered into one bounded queue the loop drains
//! ([`Model::pop_fact`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod config;
mod facts;
mod items;
mod jobs;
mod limits;
mod model;
mod people;
mod route;
mod runs;
mod serve;
#[cfg(test)]
mod tests;
mod translate;
mod waits;

pub use boundary::{
    Answer, Ask, Assignment, Call, Charter, Checkout, Chunk, Decoded, Event, Failure, Hello, Hosted, Inbound, Item,
    Landed, Outcome, Payload, Phase, Posted, Record, Refusal, Related, Relations, Reply, Request, Served, Start, Store,
    Stored, Trace, Unserved, Watched, Work, Workspace,
};
pub use config::Config;
pub use facts::Fact;
pub use limits::{Limits, accepts, worst_case};
pub use model::{Model, fire, max_out, resume, step};
// The payloads are the children's where they meet the protocol as they are:
// a parent may use its children's types.
pub use temper_engine_model_brief as brief;
pub use temper_engine_model_fleet as fleet;
pub use temper_engine_model_forge as forge;
pub use temper_engine_model_notes as notes;
pub use temper_engine_model_plan as plan;
pub use temper_engine_model_rules as rules;
pub use temper_engine_model_views as views;
pub use temper_engine_model_work as work;
