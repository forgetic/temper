//! The engine's channel connections and translation (channel.md, section 13).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod connection;
pub mod limits;
pub mod listener;
mod memory;
pub mod names;
pub mod payload;
pub mod translate;

pub use limits::{Limits, worst_case};
pub use memory::worst_case as translation_worst_case;
