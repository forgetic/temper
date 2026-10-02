//! The model layer of a fake forge, for simulations.
//!
//! A forge seen from the inside (testing-pyramid.md, 4.2), Forgejo-shaped:
//! repositories with their users' permissions, labels, issues and pull
//! requests sharing one numbering, comments, reviews, CI statuses, merges, a
//! wiki, and the git a worker clones, fetches and pushes, all in one store,
//! so a branch a worker pushes is the head the engine reads, and a merge the
//! engine makes moves the branch the next fetch sees (engine-model.md,
//! sections 12 and 14).
//!
//! Calls come up from its protocol layer as [`Event::Call`], each by a user
//! (the engine, a person, CI: the forge treats them alike) about one
//! repository, and each is answered with exactly one [`Request::Reply`],
//! after a latency drawn from the configuration, sometimes late. A call is
//! decided when it arrives: refused for its user's rate, failed by chance
//! before it is made or timed out after, or made. It checks its client: it
//! refuses what Forgejo refuses (too little permission, what is missing, a
//! label not defined, someone else's comment, a stale head on merge, a merge
//! its base's protection does not allow, a push that is not a fast-forward,
//! what is past its limits), and it keeps what the API guarantees: one answer
//! per call, numbers, comment ids and revisions that only grow, and listings
//! paged as Forgejo pages them, by an updated time kept in seconds that
//! moves for what moves Forgejo's (the `reads` module).
//!
//! The forge moves on its own too. CI reports on every commit that becomes a
//! head, by chance or as its content cues (the `ci` module); a repository's
//! subscriber hears of each change by webhook, as [`Request::Hook`], after a
//! drawn latency, late, or never (the `hooks` module); and a world can have
//! another party advance a branch ([`advance`]). Everything that changes is
//! observed, content and all, for a referee (testing-pyramid.md, 5.2): a
//! bounded queue a world drains ([`Model::pop_observation`]); and
//! [`Model::inspect`] reads the store at settle.
//!
//! Its vocabulary ([`api`]) is its own: it shares nothing with the engine's
//! model, the worker's, or the git a working tree keeps. Between them sits a
//! protocol layer on each side, or a world translating. It follows the same
//! programming style as any other step crate, bounded by its [`Limits`].

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod boundary;
mod ci;
mod faults;
mod git;
mod hooks;
mod issues;
mod limits;
mod model;
mod observe;
mod pulls;
mod reads;
mod scenario;
mod store;
mod wiki;

#[cfg(test)]
mod tests;

pub use boundary::{Event, Request};
pub use git::{Object, Tree};
pub use limits::{Limits, worst_case};
pub use model::{Config, MAX_OUT, Model, Tally, fire, step};
pub use observe::{Branches, Observation, Operation};
pub use scenario::{advance, commit, grant, repository, set_reachable, set_refusing};
