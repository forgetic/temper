//! The tasks child domain: atomic batches, lifecycle and durable inboxes
//! (programming-model.md, 4.5; domain/tasks.md, sections 2, 4, 5 and 7).
//! Carries its own authority values and never judges them. The root checks
//! authority and issues all task, attempt, message and subscription numbers. Save/Erase and outward
//! outputs are emitted together; durability is the parent's (domain/engine.md, 5.6).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod admission;
mod batch;
mod boundary;
mod closing;
mod domain;
mod facts;
mod failures;
mod funders;
mod limits;
mod owned;
mod run;
mod stored;
#[cfg(test)]
mod tests;
mod value;
pub use boundary::{
    Accepted, Active, Cause, Closing, Contract, End, Ending, Event, Executor, Hold, Key, New, Parameter, Party, Phase,
    Problem, Refusal, Request, RunContext, Spec, Stage, Status, Stored, TaskRecord, TaskResult, Verdict, Was,
};
pub use domain::{Domain, fire, max_out, step};
pub use facts::Fact;
pub use failures::{Class, Retries, Retry, Tries};
pub use funders::{Closure, FundingRecord};
pub use limits::{Limits, worst_case};
pub use owned::stored_bytes;
pub use value::{
    Authority, AuthorityExecutor, Budget, Delegation, Funder, Grant, Last, Numbers, Pattern, Scopes, Tools,
};
