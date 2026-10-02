//! The checkout sub-model of the temper worker's model layer
//! (programming-model.md, 4.5; worker-model.md, 5): the workspaces on the
//! worker's disk, a cache keyed by workstream, and the git and file operations
//! that prepare them, commit what they hold and push it.
//!
//! A client prepares a workspace for a spec (a workstream and its
//! repositories, each with where to start, and where a change is pushed if it
//! may be written) and holds it until it releases it: it may push what the
//! workspace holds, as each repository's run left it, or save it to a
//! saved-work branch, and abort what is in flight. Its clients are opaque
//! owners, named by their tokens: it knows nothing of runs, attempts, agents or
//! the engine.
//!
//! Sans-io: [`step`] turns events into requests and changes nothing but the
//! [`Model`] it is given. Its parent is the worker's top-level model
//! (`temper-worker-model`), which routes its clients' requests in and its
//! answers out, and its operations ([`git`]) down to the protocol layer, which
//! runs each as one contained git invocation, or a file operation, and parses
//! its output into a typed terminal event. The checkout arms no timers: every
//! operation carries its deadline as data, and io runs the race (5.3).
//!
//! No credentials reach the disk: every operation that reaches the forge, or
//! commits, names the identity it acts as, and the protocol layer maps the
//! name to credentials for that invocation only.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the checkout decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod cache;
mod facts;
pub mod git;
mod hold;
mod limits;
mod model;
#[cfg(test)]
mod tests;

pub use boundary::{Event, Failure, Landing, Message, Outcome, Prepared, Refusal, Repository, Request, Spec, Start};
pub use facts::{Cached, Fact, Tally, Target};
pub use limits::{Limits, worst_case};
pub use model::{MAX_OUT, Model, step};
