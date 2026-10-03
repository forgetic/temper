//! The plan sub-model of the temper engine's model layer
//! (programming-style.md, 4.5; engine-model.md, sections 3, 5 and 6): the
//! engine's plan policy. It knows the primitives (agent steps, changes, waits
//! and sessions), plans as graphs of steps under a goal, gates, wake rules,
//! envelopes and templates, and decides:
//!
//! - whether a plan may exist ([`check`]), what accepting it makes
//!   ([`accept`]), and what growing it makes and whether that needs a
//!   person's acceptance ([`grow`]) (5.2);
//! - what is due for an item, from its step and the facts about it: nothing
//!   yet, a run and the parts of its charter, an engine action, done, or a
//!   hold for a person ([`due`], 5.3 and 4.5);
//! - whether the events in a session's inbox wake it ([`wake`], 5.4);
//! - what a run's outcome writes, or that it is stale or invalid
//!   ([`apply`], 4.4);
//! - what a person's release of a held item writes ([`release`]), and their
//!   rejection of one of its proposals ([`rejected`]);
//! - whether a goal's part of a record, or a step's, as read, is one it
//!   could have written ([`check_goal`], [`check_record`]): records are forge
//!   data, and a person may edit them.
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

mod accept;
mod apply;
mod check;
mod config;
mod due;
mod facts;
mod limits;
mod plan;
mod record;
#[cfg(test)]
mod tests;
mod wake;
mod write;

pub use accept::{Growing, Grown, accept, grow};
pub use apply::{Accept, Applied, Outcome, Stale, Then, apply, rejected, release};
pub use check::{Problem, Problems, check, check_goal, check_record};
pub use config::{Config, Repo, Template};
pub use due::{Action, Due, Finish, Hold, Repair, Run, Sections, Waits, Why, due};
pub use facts::{Ci, Decided, Decision, Facts, Mergeable, Pull, PullState, Relations};
pub use limits::{Limits, max_out, worst_case};
pub use plan::{
    AgentSpec, Batch, Budget, ChangeSpec, Charter, Commit, Envelope, Gate, Grants, Growth, Plan, Repository, Resume,
    Review, SessionSpec, Sources, Step, Target, WaitSpec, Wake, Work,
};
pub use record::{Entry, Goal, Progress, Record, Reviewed, Verdict};
pub use wake::{Inbound, Source, Woken, wake};
pub use write::{Key, Write};
