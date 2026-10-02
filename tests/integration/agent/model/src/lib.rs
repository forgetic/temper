//! A simulated world for the agent's model (programming-model.md, 11;
//! agent-model.md, section 3): the top level, with the run and the sessions
//! beneath it and the tools beneath those, and fakes for every neighbour,
//! driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the agent: a fake worker's model, which starts runs on charters it
//! draws, cancels some, and plays the pushes they ask for
//! ([`temper_fake_worker_model`]); a fake LLM provider's model, which plays
//! scripted jobs, or wanders at random ([`temper_llm_model`], [`script`]); the
//! protocol layers on both sides of each, which only the world sees both
//! vocabularies of ([`translate`]); and io, running the tools' file
//! operations and commands and the runs' own looks and checks on a fake
//! checkout ([`temper_checkout_fake`]), a directory for each repository of
//! each run ([`fixture`]).
//!
//! It aims at the paths that cross the sub-models, which their own worlds
//! cannot see: a run's conversations as sessions, their tools at work in the
//! run's checkout, the run's asks and answers as tool calls and results, its
//! checks and pushes, sub-agents nested in their askers' calls, a budget
//! spent across sessions, cancels and deadlines cascading down the tree. It
//! checks the boundary contracts as it goes (one answer per start, given once
//! the run's conversations have all ended; one terminal per request; a push
//! only once the checks the run found passed; an answer that fits what
//! happened to its run and adds up what its conversations used; no
//! conversation opened past the budget, and no more than one completion each
//! after it; only main offered `finish`, and a sub-agent the families it was
//! asked with) and the universal invariants once it settles (no live
//! entities, every ticket freed, the ready list drained, nothing in flight,
//! and facts that add up to what crossed the boundary unless some were
//! dropped).

pub mod fixture;
pub mod script;
pub mod translate;
mod world;

pub use script::{JOBS, Job};
pub use temper_world::Span;
pub use world::{Allowed, BUDGET, LIMITS, Run, Settings, Stats, TIGHT, Told, World};
