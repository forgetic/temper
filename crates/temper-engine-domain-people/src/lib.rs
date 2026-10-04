//! People as parties (programming-model.md, 4.5; domain/people.md).
//! Keeps identities, secret-free sign-ins, project roles and keyed answers.
//! Knows tasks only by number; the parent checks authority and makes tasks.
//! Save/Erase join the parent's decision; the parent holds replies until
//! durable (domain/engine.md, 5.6). Inboxes and adoption follow later.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod boundary;
mod domain;
mod facts;
mod limits;
#[cfg(test)]
mod tests;
pub use boundary::{
    Ask, Event, Holding, Identity, IdentityKey, InitialOwner, Key, Outcome, Refusal, Reply, Request, RequestKey, Role,
    Stored,
};
pub use domain::{Domain, fire, max_out, step};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
