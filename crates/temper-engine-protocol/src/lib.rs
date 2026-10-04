//! The engine's channel connections and translation (channel.md, section 13).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod memory;
pub mod names;
pub mod payload;
pub mod translate;

pub use memory::worst_case;
