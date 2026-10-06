//! The browser-independent view of the web client (web/architecture.md,
//! section 4). It reads the client domain and builds a bounded tree. The
//! browser sees node ids and patches; bindings remain here, so a person acts
//! through [`decode`] and a stale node simply produces no action.
//!
//! The view knows no DOM, URL codec or protocol. [`render`] is its entry
//! point; [`View::tree`] is the tree face used by worlds and the fake person.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod binding;
mod builder;
mod cards;
mod confirm;
mod diff;
mod frame;
mod limits;
pub mod markdown;
mod pages;
mod render;
mod tree;
mod words;

#[cfg(test)]
mod tests;

pub use binding::{Binding, DomEvent, decode};
pub use builder::Builder;
pub use diff::{Attribute, Made, Patch};
pub use limits::{Limits, worst_case};
pub use render::{View, render};
pub use tree::{Class, Classes, Element, InputKind, Level, Node, NodeId, NodeKey, Role, State, States, Tree, Value};
