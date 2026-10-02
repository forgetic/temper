//! The model layer of a fake LLM provider, for simulations.
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
//! model. Between the two sits a protocol layer on each side, or a simulator
//! standing in for both.
//!
//! It follows the same programming style as any other step crate.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod model;
mod respond;

pub use model::{Config, Event, MAX_OUT, Model, Request, fire, step};
