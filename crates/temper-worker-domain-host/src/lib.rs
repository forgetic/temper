//! The host child domain of the temper worker's domain layer
//! (programming-model.md, 4.5; worker-domain.md, sections 3 and 4): the hub of
//! the worker, as the run is in the agent. It hosts runs in a fixed number of
//! slots, from the engine's assignment to the one answer the engine gets: it
//! has each run's workspace prepared and its agent started, relays between the
//! run and the engine while the run is live, serves its pushes, and once the
//! run parks, ends, fails or is cancelled, has its agent stopped, its
//! unfinished work saved and its workspace released before it answers.
//!
//! Sans-io: [`step`] and [`resume`] turn events into requests and change
//! nothing but the [`Domain`] they are given. Every effect, from preparing a
//! workspace to answering the engine, is a [`Request`] that its parent, the
//! worker's root domain (`temper-worker-domain`), routes on, and its
//! outcome comes back later through the parent as an [`Event`]. The host owns
//! no timers: every wait it does is bounded below it, by the deadlines of git
//! operations, the agent's watchdog and wall time, and the grace of cancel,
//! then kill.
//!
//! The host knows a hosted run's lifecycle and nothing of git, processes or
//! channels: workspaces are the checkout child domain's and agents the agent
//! child domain's, which its parent translates to and from, and it names no
//! type of theirs (siblings share none, 4.5). What it passes through (a
//! charter, a snapshot, inbound events, the bodies of relayed calls and their
//! answers, an outcome) is opaque bytes, bounded at the entrance; what it acts
//! on is typed. The engine link, its grace on losing contact and the reconnect
//! are the top level's; the host cancels every run when told to
//! ([`Event::CancelAll`]) and reports what it hosts when asked
//! ([`Event::Report`]).
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Domain::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the host decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod assignment;
mod boundary;
mod call;
mod domain;
mod facts;
mod hosted;
mod limits;
mod push;
#[cfg(test)]
mod tests;

pub use boundary::{
    Access, AgentFailure, Answer, Ask, Assignment, Bounce, Event, Failure, Finish, Hosting, Invalid, Landed, Landing,
    Missing, Phase, Preparation, Push, Reason, Refusal, Reply, Repository, Request, RunFailure, Start, Work, Workspace,
};
pub use domain::{Domain, max_out, resume, step};
pub use facts::Fact;
pub use limits::{Limits, worst_case};

pub use push::{PushDiagnostic, PushFailure, PushReason};
