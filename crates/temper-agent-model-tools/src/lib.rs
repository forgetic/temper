//! The tools sub-model of the temper coding agent's model layer
//! (programming-model.md, 4.5; agent-model.md, 6): what a session does to the
//! checkout. It reads, lists and writes files for the calls a session's LLM
//! makes; editing, searching and running commands come next.
//!
//! Sans-io: [`step`] turns events into requests and changes nothing but the
//! [`Model`] it is given. Its parent is the session sub-model, which opens a
//! kit for each session, hands it the LLM's calls, and routes the file
//! operations the tools ask for out to io and their terminal events back. The
//! tools arm no timers: every operation carries its deadline as data, and io
//! runs the race (5.3).
//!
//! Both directions are typed: a [`Call`] arrives decoded by the protocol layer
//! from the JSON the LLM wrote, its paths split into parts ([`Path`]), and an
//! [`Outcome`] goes back for the protocol layer to render as the text the LLM
//! reads.
//!
//! The checkout is shared, knowledge is not. Files are world state, reached
//! through io and shared by every session; what a session's LLM has read, and
//! at which version, belongs to its kit, along with the [`Authority`] the
//! session was opened with. A file may be changed only if its current version
//! was read; creating one needs no read.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod authority;
mod boundary;
mod call;
mod job;
mod kit;
mod knowledge;
mod limits;
mod model;
mod path;
#[cfg(test)]
mod tests;
mod window;

pub use authority::{Authority, Grants, Repo};
pub use boundary::{Done, Event, Expect, Op, Refusal, Request, Version};
pub use call::{Call, Effect, Entry, Fault, Kind, Outcome, effect};
pub use limits::{Limits, worst_case};
pub use model::{Model, max_out, step};
pub use path::{Name, Part, Path, Place};
