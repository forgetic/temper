//! The browser's typed state and decisions (web/architecture.md, sections 3–4).
//! Holds the page, frame, drafts, requests, reads and watches. The shell owns
//! storage and IO; the view owns all wording and rendering.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod action;
mod address;
mod ask;
mod boundary;
mod domain;
mod drafts;
mod facts;
mod frame;
mod limits;
mod link;
mod notices;
mod pages;
mod reads;
mod requests;
mod saved;
mod streams;
#[cfg(test)]
mod tests;

pub use action::{Action, FieldRef, Form};
pub use address::{Address, Section};
pub use ask::{Ask, Key, Outcome, Refusal};
pub use boundary::{
    Answer, Change, Cursor, Event, Offset, PersonSnapshot, Query, ReadResult, Request, Snapshot, StreamEnd,
    StreamEvent, Watch,
};
pub use domain::{Domain, fire, max_out, step};
pub use drafts::Field;
pub use facts::Fact;
pub use frame::{Frame, Person, Project};
pub use limits::{Backoff, Limits, worst_case};
pub use link::LinkState;
pub use notices::{Notice, NoticeKind};
pub use pages::{ChatLine, Chats, Page};
pub use saved::{Saved, SavedDraft, SavedPending};
