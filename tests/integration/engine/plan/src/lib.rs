//! A simulated world for the engine's plan sub-model (programming-model.md,
//! 4.5 and 11; engine-model.md, sections 4 to 6): the plan, with a scripted
//! engine as its parent, which holds the items of a few goals on an abstract
//! forge ([`forge`]), asks the plan what is due for each, runs what is due
//! with outcomes drawn from the seed ([`script`]), applies the writes the plan
//! asks for, and moves time on. One loop drives them, deterministically.
//!
//! The world stands in for the rest of the engine (the item lifecycle's
//! mechanics, the rules, briefs and the fleet, reduced to what the plan
//! needs: a run is due, it is claimed, it ends; a failed run is tried again
//! after a backoff and held past its attempts; a plan proposed, and growth
//! beyond an envelope, wait for a person's acceptance, and a rejection is the
//! plan's to count), for the forge (issues carrying records, branches, pull
//! requests found by their branch, CI on exact heads that may never report,
//! reviews, merges that conflict, by the engine or by hand, and bases that
//! move), for workers and agents (runs that report, push, review, propose,
//! grow plans, make tasks, release held steps, finish sessions, escalate and
//! fail), and for people (who review, approve, ask for changes, decide,
//! accept and reject proposals, release held items, close pull requests,
//! push to bases and to branches, and write to sessions, some of whose
//! inboxes fill first with events their rules do not name). It translates
//! between the forge's terms and the plan's ([`translate`]).
//!
//! It checks the contracts as it goes: every run ends once; a run's feedback,
//! once taken, gives an outcome that fits; and an outcome the engine
//! restarted while applying (at any of its writes, between the goal's record
//! and the step's too, and having lost what it kept only in memory) applies
//! again from the record as it landed, finding what it made by its keys. The
//! scenario's expectations are the [`referee`]'s: safety on every
//! observation, liveness as its own deadlines. Once it settles: nothing in
//! flight, no item running or waiting for a person's answer, and every goal
//! and task ended, done or held. Its own state is bounded: a seed whose trace
//! or deliveries run away fails fast, naming the seed.

pub mod forge;
pub mod referee;
pub mod script;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{Settings, Stats, World};
