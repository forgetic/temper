//! A simulated world for the agent's model (programming-style.md, 11;
//! agent-model.md, section 3), where it meets the worker's (worker-model.md,
//! section 9): the worker's top-level model hosting runs, each in an agent
//! process that is a fresh agent model, with the run and the sessions beneath
//! it and the tools beneath those; and fakes for every other neighbour,
//! driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for:
//!
//! - **the engine:** the fake engine's model ([`temper_fake_engine_model`]),
//!   which assigns runs on charters it draws, in workspaces it draws, retries,
//!   overbooks and cancels; over a channel that keeps its order and never
//!   drops, through the whole worker's world's translation of its api
//!   (`temper_worker_model_tests::translate`), which frames each charter with
//!   its attempt;
//! - **io's agent processes:** each spawn is an agent model sized for one
//!   run, which the world drives as its process's shell would; its pipes
//!   carry the channel between the worker and the run, which the agent's
//!   protocol layer translates on its side ([`channel`]), taking the world's
//!   frame off the charter and adding where io put each repository; a
//!   terminate or a kill ends the process at once, with what it had in flight,
//!   and its exit, its reap and the end of its pipe follow; one that has
//!   answered exits of itself;
//! - **git and files:** the fake forge (`temper_forge_model`), each job's
//!   repository on it seeded to cue its script ([`fixture`]), and one disk
//!   ([`temper_checkout_fake`]), through the checkout world's translation of
//!   the checkout's operations and its route to the forge
//!   (`temper_worker_model_checkout_tests::translate` and `forge`): the
//!   working trees the worker prepares are the roots the agents' tools read
//!   and write, so what the worker commits is what the agent left; with
//!   another party moving a push branch, and repositories that refuse
//!   pushes; and the forge's observations of every branch moved, which the
//!   referee sees;
//! - **the agents' other neighbours:** a fake LLM provider's model, which plays
//!   scripted jobs, or wanders at random ([`temper_llm_model`], [`script`]),
//!   with the protocol layers on both sides, which only the world sees both
//!   vocabularies of ([`translate`]); and io, running the tools' file
//!   operations and commands and the runs' own looks and checks on the disk.
//!
//! It aims at the paths that cross the models, which their own worlds cannot
//! see: a run's conversations as sessions, their tools at work in the
//! checkout the worker prepared, the run's asks and answers as tool calls and
//! results, its checks and pushes, sub-agents nested in their askers' calls,
//! a budget spent across sessions, cancels and deadlines cascading down the
//! tree, and how each run ends, through the worker, at the engine. It checks
//! the contracts as it goes: the agent's (one answer per start, given once the
//! run's conversations have all ended; one terminal per request; a push only
//! once the checks the run found passed; an answer that fits what happened to
//! its run and adds up what its conversations used; no conversation opened
//! past the budget, and no more than one completion each after it; only main
//! offered `finish`, and a sub-agent the families it was asked with); the
//! worker's (an agent spawned in a workspace io has, one at a time; every
//! git operation's deadline and identity, and none in a workspace while its
//! agent runs but the push it asked for; a branch moved only by a
//! fast-forward; each attempt answered once). And the invariants once it
//! settles: the engine took one answer for each attempt and has nothing out,
//! the worker holds nothing, every agent process exited, was reaped and read
//! to its end, each agent that exited of itself held nothing, nothing is in
//! flight, and facts that add up to what crossed the boundary unless some
//! were dropped or an agent was killed.
//!
//! What the scenarios expect of the worker and the agent together is held by
//! a referee (testing-pyramid.md, 5.2; [`referee`]), which sees only what the
//! fakes see and ends each run with its verdict: the worker commits exactly
//! the tree the agent left; the engine records the outcome the run accepted,
//! or how it failed, a cancel being the worker's to report; what landed is on
//! the forge; and every assignment is answered within the wall time the
//! worker's watchdog gives a run, and a margin.
//!
//! Not exercised, as the agent's side does not do it yet: inbound events and
//! a run's waiting for them, parking and snapshots, and relayed calls (forge
//! reads and outlets).

pub mod channel;
pub mod fixture;
pub mod referee;
pub mod script;
pub mod translate;
mod world;

pub use script::{JOBS, Job};
pub use temper_world::Span;
pub use world::{Allowed, BUDGET, LIMITS, Run, Settings, Stats, TIGHT, Told, WORKER, World};
