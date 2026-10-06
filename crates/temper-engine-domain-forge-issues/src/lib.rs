//! Goal issue projection (domain/forge.md, section 12).
//!
//! The top stores `Projected` and commits each returned record with its keyed
//! effect. This child knows goal text and accepted milestones, never forge
//! issue numbers or API calls. `project` schedules at most one write each
//! step: open, a changed body after the interval, each milestone once, then
//! the finished comment and close. Repeating a committed record emits no
//! duplicate key.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod boundary;
mod domain;
#[cfg(test)]
mod tests;

pub use boundary::{Decision, Effect, GoalView, Key, Limits, Milestone, MilestoneKey, PlanItem, Projected, Projection};
pub use domain::project;
