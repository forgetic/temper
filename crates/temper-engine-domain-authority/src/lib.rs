//! What a task may do, as an owned value with a preorder, its carved funding,
//! and the rules each action passes (domain/authority.md, sections 3–9;
//! programming-model.md, 4.5).
//!
//! Pure policy over values supplied by the engine's root: resource coverage,
//! authority order and accounting over snapshots. [`Domain`] holds only the
//! deployment's rules, project policies and their admission limits. It keeps
//! no task, funding numbers, connector or clock. The tasks child keeps the
//! numbers; accounting answers say what they become. Connector kind orders
//! are supplied as validated [`Implies`] data. Checks allocate nothing,
//! refuse oversized inputs at admission and write every independent reason
//! into caller-reserved bounded queues. [`needs`] constructs an owned value;
//! policy events update the bounded table and emit one lifecycle fact.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod check;
mod domain;
mod limits;
mod numbers;
mod order;
mod rules;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_numbers;
#[cfg(test)]
mod tests_policy;
mod value;

pub use boundary::{
    Action, Answer, BatchAsk, Call, CallAsk, Checked, Delegate, Effect, EffectAsk, Fact, Finding, Holder, PersonAsk,
    PersonRequest, RunAsk, Source, Status, Write, Writer,
};
pub use check::{check_batch, check_call, check_effect, check_request, check_run, covers, needs};
pub use domain::{Domain, Event, POLICY_MAX_OUT, PolicyFact, PolicyRefusal, step};
pub use limits::{Limits, max_out, worst_case};
pub use numbers::{Charged, Funder, Funding, Moved, Numbers, carve, charge, left, move_funding, settle};
pub use order::{FITS_MAX_OUT, Lack, Lacks, fits};
pub use order::{Implication, Implies, at_most, grant_at_most, grant_covers, pattern_at_most, pattern_covers};
pub use rules::{Policy, ProposalKind, Proposals, RequestKind, Requests, Requirement, Role, Rules};
pub use value::{Authority, Budget, Delegation, Executor, Grant, Last, Name, Pattern, Scopes, Tools};
