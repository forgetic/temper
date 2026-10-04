//! Agent protocol translation and client stacks (llm.md; channel.md, 6).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod channel;
pub mod grants;
pub mod limits;
pub mod payload;
pub mod render;
pub mod tools;
pub mod translate;
pub mod worker;

pub use limits::{Error, Limits};
