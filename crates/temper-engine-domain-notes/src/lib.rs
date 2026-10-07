//! Notes learned by tasks and corrected by parties, in the store (jig's
//! domain/engine.md, section 10). Entries are loaded on demand and every
//! entry's scope has a durable index line. A write compares the recalled
//! revision, then emits both records in one decision. The parent commits them
//! together. Notes know neither a connector's system nor the parent's store
//! encoding. [`step`] is the only entry point; there are no timers.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod domain;
mod limits;
#[cfg(test)]
mod tests;

pub use boundary::{
    Author, Change, Entry, Event, Key, Line, New, Pattern, Range, Record, Refusal, Request, Rows, Scope,
};
pub use domain::{Domain, MAX_OUT, max_out, step};
pub use limits::{Limits, worst_case};
