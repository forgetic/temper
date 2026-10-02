//! A simulated world for the engine's plan sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, sections 4 to 6): the plan, with a scripted
//! engine as its parent, which holds the items of a few goals on an abstract
//! forge ([`forge`]), asks the plan what is due for each, runs what is due
//! with outcomes drawn from the seed ([`script`]), applies the writes the plan
//! asks for, and moves time on. One loop drives them, deterministically.
//!
//! The world stands in for the rest of the engine (the item lifecycle's
//! mechanics, the rules, briefs and the fleet, reduced to what the plan
//! needs: a run is due, it ends; a failed run is tried again after a backoff
//! and held past its attempts; a plan proposed, and growth beyond an
//! envelope, wait for a person's acceptance), for the forge (issues carrying
//! records, branches, pull requests, CI on exact heads, reviews, merges that
//! conflict and bases that move), for workers and agents (runs that report,
//! push, review, propose, grow plans, make tasks, escalate and fail), and for
//! people (who review, approve, ask for changes, decide, accept and reject
//! proposals, close pull requests and push to bases). It translates between
//! the forge's terms and the plan's ([`translate`]).
//!
//! It checks the contracts as it goes: every run ends once; a run's feedback,
//! once taken, gives an outcome that fits; an outcome a person accepted
//! applies again as it did; and a restart's second application finds what the
//! first made. The scenario's expectations are the [`referee`]'s: safety on
//! every observation, liveness as its own deadlines. Once it settles: nothing
//! in flight, no item running or waiting for a person's answer, and every goal
//! and task ended, done or held.

pub mod forge;
pub mod referee;
pub mod script;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{Settings, Stats, World};
