//! Secret-free account policy (credentials.md, sections 5 and 6). Token values
//! stay in the protocol layer, including a rotation waiting to be saved.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod boundary;
mod domain;
mod limits;

pub use boundary::{Event, Fact, Failure, Grant, Request, State};
pub use domain::{Domain, MAX_OUT, fire, step};
pub use limits::{Limits, worst_case};
