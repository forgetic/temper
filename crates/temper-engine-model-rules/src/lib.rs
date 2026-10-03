//! The rules sub-model of the temper engine's model layer
//! (programming-model.md, 4.5; engine-model.md, sections 3 and 7): the
//! deployment's rules, which every run the engine starts and every write it
//! makes must satisfy, whatever a plan says. A plan's gates add to them, and
//! never take from them.
//!
//! It is policy: decisions over data, keeping nothing between calls but its
//! configuration ([`Rules`], from a vocabulary fixed here). Its entry points
//! are pure functions: [`check_run`] before a run starts (where it may push,
//! its budget against what was spent per run, per goal and per deployment),
//! [`check_write`] before a write (landing on a protected branch, the
//! deployment's repositories, a plan past a size or an estimate or landing
//! on a protected branch, a note of wide scope), and [`check_request`]
//! before a person's request is acted on. Each answers a [`Decision`]
//! (allow; wait for facts the forge has yet to report; a person's acceptance
//! and at what permission; or refuse), and writes why into a bounded queue
//! of [`Finding`]s its caller provides.
//!
//! Sans-io: the facts a check reads (spend so far, CI and reviews on a pull
//! request, a person's permission, who accepted the step or the proposal)
//! come in what it is asked; its parent, the engine's top-level model
//! (`temper-engine-model`), gathers them from the working set and translates
//! the plan's gates, the outcome's writes and people's requests into the
//! rules' terms. The rules know nothing of plans, items or the forge's API:
//! when a write waits, for facts or for a person, the parent holds it and
//! asks again once they change; when one is refused, the parent says why.
//! Plan's own gates are the rules' too: `Approvals` and `Accepted` are
//! checked here as well as followed there.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod check;
mod limits;
mod rules;
#[cfg(test)]
mod tests;

pub use boundary::{
    Act, Bound, Ci, Decision, Finding, Gate, Goal, Grant, Landing, Oversized, Permission, Plan, Repository, Request,
    Review, Run, Scope, Stance, Target, Write,
};
pub use check::{check_request, check_run, check_write};
pub use limits::{Limits, max_out, worst_case};
pub use rules::{Acts, Branch, Rules};
