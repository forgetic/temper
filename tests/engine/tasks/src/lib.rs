//! Tasks with agent executors, always-allow authority and a durable parent.
//! Store decisions and requester result messages are committed atomically;
//! executor work and replies are withheld until that decision is durable.
pub mod accounting_referee;
pub mod inbox_referee;
pub mod referee;
mod world;
pub use world::{LIMITS, RETRY, Reply, World, authority, run_story, run_story_facts, task};
