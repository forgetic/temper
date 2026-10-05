//! The domain layer of a fake LLM provider, for the worlds that test temper.
//!
//! A provider seen from the inside: calls come up from its protocol layer as
//! [`Event::Call`], and each is answered with exactly one [`Request::Reply`]
//! after a latency drawn from the configuration. What it answers follows a
//! script (see the `respond` module): configured chances of failing, a
//! configured number of tool rounds, then a final answer; or, for a
//! conversation a world scripted ([`api::Script`]), the answers it wrote,
//! one after another. It rejects conversations a real provider would reject,
//! so it also checks its clients.
//!
//! Its vocabulary ([`api`]) is its own: it shares nothing with the agent's
//! domain. Between the two sits a protocol layer on each side, or a world
//! translating (testing-strategy.md, section 4).
//!
//! It follows the programming model as any other step crate does.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod domain;
mod limits;
mod respond;

pub use limits::worst_case;

pub use domain::{Config, Domain, Event, MAX_OUT, Request, fire, step};
