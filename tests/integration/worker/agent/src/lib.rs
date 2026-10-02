//! A simulated world for the worker's agent sub-model (programming-model.md,
//! 4.5 and 11; worker-model.md, sections 6 and 9): the agent sub-model, with
//! the world as its parent, a scripted client spawning agents ([`client`]),
//! and process trees standing in for io ([`tree`]), each running a scripted
//! agent process that speaks the channel ([`script`]), well or badly. One
//! loop drives them, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the agent sub-model: the top level and, through it, the host that
//! is its client; the protocol layer that speaks the channel over the pipes
//! ([`translate`]); and io's contained process trees. It checks the
//! contracts as it goes: every request gets exactly one terminal event, and
//! every spawn exactly one end, refused at once only when there is no slot or
//! it is beyond the limits; an agent is named before anything is said of it,
//! and its client hears at most one finish or fault, a fault only while the
//! run is live; a call reusing a name in flight is never passed on; "stopped"
//! comes only once the process has exited and its tree is empty, with
//! nothing asked of io in flight, and in time: a stopping agent is
//! terminated, then killed, within the grace; the watchdog never fires while
//! the run waits for a call's answer or an inbound event, nor before it has
//! been silent past the deadline or a long operation's deadline, so a run
//! that keeps making progress is never stopped by it, and the wall time only
//! once it is up; every breach of the channel's rules is caught, stops the
//! agent, and is told as such while the run is live, and nothing else is;
//! and inbound events reach the agent once each, in the order sent. Once it
//! settles: nothing in flight, no agent left, no alarm armed, every process
//! tree empty, waited for, reaped and read to its end, every call answered,
//! and facts that add up to what crossed the boundary unless some were
//! dropped.

pub mod client;
pub mod script;
pub mod translate;
pub mod tree;
mod world;

pub use temper_world::Span;
pub use world::{Settings, Stats, World};
