//! The engine's root domain: the charged chat walking story beside the frozen
//! legacy root (domain/engine.md, sections 3–7). [`engine::Domain`] keeps real
//! tasks, authority, people, fleet, brief and account children, with bounded
//! root-owned commits, loads, fresh-number counters and unfinished handoffs.
//! The root translates between children; no sibling talks to another directly.
//!
//! [`engine::step`] admits authenticated sign-ins and keyed chats, worker
//! hellos/losses, numbered turns, priced answers, historical result reads and
//! store/account terminals. Actual held chats also route authenticated escalation
//! reads and keyed Release/Reject/Pass acceptance decisions . [`engine::resume`] releases one durable delivery
//! or routes pending work; [`engine::fire`] drives child timers through the
//! same decision barrier. A writing decision is one ordered atomic commit;
//! claims, accepted turns, answers and people's replies leave only after the
//! commit they follow is durable. Store failures stop further release.
//!
//! [`Decision`] and [`Journal`] expose the commit seam separately; [`loads`]
//! owns fenced one-terminal page reads. Root-owned [`RunProof`] rows hold one
//! current claim's latest turn metadata and typed terminal; authentic funding
//! numbers stay in tasks. Startup pages and validates current proof rows before
//! tasks restoration consequences and fleet adoption. [`TerminalRecord`] and
//! transcript archives stay outside the bounded live proof map. Immutable escalation
//! decision history uses named single-row reads through the finite shared query
//! slots, never a restored live history map . The shell reserves entry-point output
//! room and calls [`engine::Domain::reclaim`] after each iteration. The store
//! protocol encodes typed [`Record`]s and supplies bounded pages; the root
//! never knows file descriptors, wire formats, secret credential bytes,
//! repositories or kernel completion mechanics (domain/engine.md, 2 and 5.5).
//!
//! This root currently routes person Report chats and their required task brief
//! only, with the configured charter and no delegate/dependency/input-result
//! route. Unsupported restored shapes stop startup rather than reaching a
//! dormant consumer.
//! Connector execution, notes and view routes remain later increments.
//! Child facts are bounded observations drained by
//! [`engine::Domain::drain_facts`]; dropping them changes no decision.
//! [`engine::Domain::quiescent`] is an idle fence, not a final story result;
//! assigned workers and future task/account timers may remain (5.7). The
//! first-turn and final Report durable/lost-completion restart cuts are covered
//! by the walking world; broader deployment recovery remains later work

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod decision;
pub mod engine;
pub mod loads;
mod store;
#[cfg(test)]
mod tests;
pub use decision::{
    Decision, Delivery, Journal, Limits as JournalLimits, Output, ResultEntry, accept, committed, fresh, resume, takes,
    uncommitted, worst_case,
};
pub use store::{
    CallAnswer, CallKey, CallRecord, Deployment, EscalationDecisionRecord, Family, Key, Range, Record, RunProof,
    TerminalRecord, TurnProof, TurnRecord, Write, record_bytes,
};
