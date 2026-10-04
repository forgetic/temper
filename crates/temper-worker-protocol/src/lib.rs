//! The worker's channel translation and connections (channel.md, section 13).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod agent;
pub mod credentials;
pub mod limits;
pub mod link;
pub mod relays;
pub mod translate;
pub use limits::{Limits, worst_case};
