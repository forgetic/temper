//! Frozen legacy engine; see docs/plans/next-domain/README.md, section 3.4.
//! The domain layer of the temper engine (programming-model.md, section 4;
//! engine-domain.md, section 3): the engine loop's entry point.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Domain`] they are given. Time and randomness are
//! inputs; every effect, from a call to the forge to an assignment to a
//! worker, a reply to a person or an operation of the store, is a
//! [`Request`] the layers below carry out, and its outcome comes back later
//! as an [`Event`].
//!
//! It is the root domain over a tree of child domains (4.5), and the only one
//! that faces the protocol layer:
//!
//! ```text
//! temper-legacy-engine-domain                  faces the protocol: forge, workers, people, store; routes; translates
//! ├── temper-legacy-engine-domain-work         the hub: items' lifecycle
//! ├── temper-legacy-engine-domain-plan         policy: steps, what is due, wakes, what an outcome writes
//! ├── temper-legacy-engine-domain-rules        policy: the deployment's rules over runs and writes
//! ├── temper-legacy-engine-domain-forge        capability: the working set and the client
//! ├── temper-engine-domain-fleet        capability: workers, placement, attempts, relaying
//! ├── temper-engine-domain-brief        capability: a run's brief, sections within budgets
//! ├── temper-engine-domain-notes        capability: scopes, entries, indexes
//! └── temper-engine-domain-views        capability: reports in; streams and traces out
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
//! child domains name by token are its own: records, outcomes and pages filled
//! in as the forge's calls go out, answers, calls and reports carried for
//! the fleet ([`Event`], [`Request`] carry them typed).
//!
//! Decisions the child domains left to it, settled here:
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
//!   their meaning across restarts, and the top level's own past them:
//!   zero, the record carries no step; seven, the rules want a person to
//!   accept the run due; eight, the rules refuse it.
//! - **Where a change's branch is** is in its record, as a run's answer
//!   says it pushed. A late answer, its attempt presumed lost, says so too,
//!   while the record names no branch; and before a change is made again
//!   with no branch recorded, the branch is read on the forge, where an
//!   attempt whose answer never came may have pushed it.
//! - **A merge refused for a conflict** marks the head it was refused at as
//!   conflicting, whatever the working set read of it, for as long as it is
//!   the pull request's head: the plan sends the change back for a rebase.
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
//! Not built yet, against engine-domain.md: a run's `note` the rules want a
//! person to accept (one of a wide scope, section 10) is refused, not held
//! for a person, as a call that waited on one could outlive its run, and
//! nothing yet holds a call that waits so.
//!
//! What happens is also told as content-free [`Fact`]s, the child domains' and
//! its own, gathered into one bounded queue the loop drains
//! ([`Domain::pop_fact`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod config;
mod credentials;
mod domain;
mod facts;
mod items;
mod jobs;
mod limits;
mod people;
mod route;
mod runs;
mod serve;
#[cfg(test)]
mod tests;
mod translate;
mod waits;

pub use temper_engine_domain_accounts as accounts;

pub use boundary::{
    Answer, Ask, Assignment, Call, Charter, Checkout, Chunk, Decoded, Event, Failure, Hello, Hosted, Inbound, Item,
    Landed, Model, Outcome, Payload, Phase, Posted, Record, Refusal, Related, Relations, Reply, Request, Served, Start,
    Store, Stored, Trace, Unserved, Watched, Work, Workspace,
};
pub use config::{Account, Config};
pub use domain::{Domain, fire, max_out, resume, step};
pub use facts::Fact;
pub use limits::{Limits, accepts, worst_case};
// The payloads are the children's where they meet the protocol as they are:
// a parent may use its children's types.
pub use temper_engine_domain_brief as brief;
pub use temper_engine_domain_fleet as fleet;
pub use temper_engine_domain_notes as notes;
pub use temper_engine_domain_views as views;
pub use temper_legacy_engine_domain_forge as forge;
pub use temper_legacy_engine_domain_plan as plan;
pub use temper_legacy_engine_domain_rules as rules;
pub use temper_legacy_engine_domain_work as work;
