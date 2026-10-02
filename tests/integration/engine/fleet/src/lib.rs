//! A simulated world for the engine's fleet sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, section 8): the fleet, with the world as its
//! parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the fleet: the protocol layer and the network, as channels that
//! carry messages in order after a latency and lose what is in flight when
//! they close; workers that dial in, say hello with their slots, the
//! workstreams they hold and what they host, host runs that end, park or
//! fail after a drawn time, refuse some assignments as invalid, relay host
//! calls and wait for their answers, bounce inbound events and tell facts,
//! wind down when cancelled, keep their answers until they are acknowledged
//! and send them again after every hello, and, out of contact past their own
//! grace, cancel their runs themselves; and the parent, as the engine's top
//! level would be, which starts runs, keeps their claims across restarts and
//! adopts them after one, cancels some and replaces others with a newer
//! attempt, gives up on those that take too long, sends inbound events,
//! answers relayed calls after a latency, and retries what fails.
//!
//! It checks the contracts as it goes: every start and adoption ends exactly
//! once, every relayed call is answered exactly once, and every payload the
//! parent hands the fleet is echoed exactly once. Its referee ([`referee`])
//! holds the fleet to what the scenarios expect, from what the workers and
//! the parent see, and injects channels that drop and engine restarts. And
//! the invariants once it settles: nothing in flight, nothing tracked by the
//! fleet, nothing hosted or held by a worker in contact, every item done, and
//! the referee's verdict passed.

pub mod referee;
mod workers;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, LIMITS, Settings, Stats, World};
