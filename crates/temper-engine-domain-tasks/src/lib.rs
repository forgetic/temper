//! The tasks child domain: atomic batches, lifecycle and durable inboxes
//! (programming-model.md, 4.5; domain/tasks.md, sections 2, 4, 5 and 7).
//! Carries its own authority values and never judges them. The root checks
//! authority and issues all task, attempt, message and subscription numbers. Save/Erase and outward
//! outputs are emitted together; durability is the parent's (engine.md, 5.6).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod admission;
mod amend;
mod batch;
mod boundary;
mod closing;
mod domain;
mod facts;
mod failures;
mod funders;
mod inbox;
mod limits;
mod message;
mod moving;
mod refs;
mod run;
mod stored;
#[cfg(test)]
mod tests;
mod value;
mod wake;
pub use admission::{Admission, AdmissionKey};
pub use amend::{Amendment, AuthorityChange, Authorization, Change, Control, History};
pub use boundary::*;
pub use domain::{Domain, fire, live_task, max_out, step, task_stub};
pub use facts::Fact;
pub use failures::{Class, Retries, Retry, Tries};
pub use funders::{Balance, Closure, FundingRecord};
pub use limits::{Limits, worst_case};
pub use moving::{Movement, Transfer};
pub use value::{
    Authority, AuthorityExecutor, Budget, Delegation, Funder, Grant, Last, Numbers, Pattern, Scopes, Tools,
};

pub use message::{
    Envelope, Interest, Message, MessageKey, NewsClass, Notice, Offer, Question, Receipt, ResultsWake, Rule,
    Subscription, SubscriptionKind, UserMessage, WakePolicy,
};
