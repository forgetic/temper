//! The building blocks every step crate shares (programming-model.md, 10.2):
//! typed handles and the slabs that issue them, bounded queues and lists, the
//! per-layer deadline table, time, randomness, the tokens that cross layer
//! boundaries, and the environment a step reads.
//!
//! Application code does not hand-roll data structures: what is missing goes
//! here, written once and tested hard.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bytes;
mod deadlines;
mod env;
mod id;
mod list;
mod queue;
mod rng;
mod slab;
mod time;
mod token;

pub use deadlines::Deadlines;
pub use env::Env;
pub use id::Id;
pub use list::List;
pub use queue::Queue;
pub use rng::Rng;
pub use slab::Slab;
pub use time::{Duration, Time};
pub use token::{ReplyTo, Token};
