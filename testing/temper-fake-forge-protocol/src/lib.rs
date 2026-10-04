//! Forgejo's HTTP face over the fake forge domain. Documents remain in the
//! domain-independent provider crate; this crate translates stored fake rows.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod config;
pub mod connection;
pub mod service;
mod thread;
pub mod translate;
