//! The run child domain of the temper coding agent's domain layer
//! (programming-model.md, 4.5; agent-domain.md, section 4): one agent instance.
//! A run takes its charter from the worker, opens the conversation that does
//! the work, accounts what it spends against one budget, and answers once.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change nothing
//! but the [`Domain`] they are given. Time is an input; every effect, from
//! opening a conversation to answering the worker, is a [`Request`] that its
//! parent, the root domain (`temper-agent-domain`), routes on, and its
//! outcome comes back later through the parent as an [`Event`].
//!
//! A run has three faces, all through its parent: the worker's and io's, which
//! the parent routes to and from the protocol layer, and its conversations',
//! which the parent translates to and from the session child domain's
//! vocabulary. The run names no session type: siblings share none (4.5).
//!
//! What a run is given is policy as data ([`charter`]): the run interprets no
//! workflow vocabulary, and compares the labels in it byte for byte. So is what
//! it may finish with ([`outcome`]), which [`outcome::judge`] checks a declared
//! outcome against.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod agent;
mod boundary;
mod budget;
mod call;
pub mod charter;
mod domain;
pub mod facts;
mod land;
mod limits;
pub mod outcome;
mod prepare;
mod prompt;
mod run;
#[cfg(test)]
mod tests;

pub use boundary::{
    Answer, Ask, AskRefusal, End, Event, Exit, Failure, Fault, Invalid, Opening, Place, Policy, Push, Ran, Read,
    Refusal, Request, Returned, Stop,
};
pub use budget::{Budget, Exhausted, Spend};
pub use charter::Charter;
pub use domain::{Domain, MAX_OUT, fire, step};
pub use limits::{Limits, worst_case};
