//! The plan sub-model of the temper engine's model layer
//! (programming-style.md, 4.5; engine-model.md, sections 3 and 5): the
//! engine's plan policy. It knows the primitives (agent steps, changes, waits
//! and sessions), plans as graphs of steps under a goal, gates, wake rules,
//! envelopes and templates. It checks a plan before it exists.
//!
//! It knows nothing of the forge's API, the workers, the record's encoding or
//! the mechanics of an item's lifecycle, and it never checks the rules: it
//! says what a step wants, and its parent checks that against the rules.
//!
//! Policy: it keeps nothing between calls but its configuration ([`Config`])
//! and its [`Limits`]. Its entry points are pure functions over its own
//! types, which return their decisions, and write the writes they ask for
//! into a bounded queue the caller gives, with room for [`max_out`] of them.
//!
//! Sans-io: time is an input, in the [`Env`](temper_lib::Env) every entry
//! point reads. Its parent is the top-level model (`temper-engine-model`),
//! which gathers the facts a decision reads from the working set and
//! translates what the plan decides into the vocabularies of its siblings.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod check;
mod config;
mod limits;
mod plan;
#[cfg(test)]
mod tests;

pub use check::{Problem, Problems, check};
pub use config::{Config, Repo, Template};
pub use limits::{Limits, max_out, worst_case};
pub use plan::{
    AgentSpec, Batch, Budget, ChangeSpec, Charter, Commit, Envelope, Gate, Grants, Growth, Plan, Repository, Resume,
    Review, SessionSpec, Sources, Step, Target, WaitSpec, Wake, Work,
};
