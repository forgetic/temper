//! A simulated world for the whole worker (programming-style.md, 11;
//! worker-model.md, section 9): the worker's top-level model
//! (`temper_worker_model`) run against everything around it, with no protocol
//! and no io. One loop drives it, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for:
//!
//! - **the engine:** the fake engine (`temper_fake_engine_model`), a step
//!   model of its own on a stage of its own, which assigns runs, sends inbound
//!   events, cancels, answers relayed calls, and checks the worker as it goes;
//!   the world translates between its api and the worker's boundary as the
//!   protocol layers would ([`translate`]);
//! - **the network** between them: one channel at a time, which the worker
//!   dials, with a latency each way, that drops at drawn moments, after which
//!   the engine is out of reach for a drawn while, shorter or longer than the
//!   worker's grace; what is in flight on a channel that drops is lost;
//! - **agent processes:** the agent world's process trees and scripted agents
//!   (`temper_worker_model_agent_tests`), which work, call the host, push,
//!   wait for inbound events, park, end, fail and misbehave; and which edit
//!   the working trees of their workspace before they ask to push, and now
//!   and then as they go, so that what the worker commits is what they wrote;
//! - **git and files:** the fake disk (`temper_checkout_fake`) and the fake
//!   forge (`temper_forge_model`), through the checkout world's translation
//!   of the checkout's operations and its route to the forge, after a
//!   latency, racing their deadlines and the cancels of an aborted
//!   prepare; with remotes seeded from the fake engine's names, and the
//!   faults the world scripts: unreachable repositories, refused pushes,
//!   starting points the forge does not have, and another party moving a
//!   push branch;
//! - **the shell:** which drains the facts and the run's facts for the
//!   engine, and, in some worlds, tells the worker to shut down at a drawn
//!   moment and stops once it is done.
//!
//! It checks the contracts as it goes: the fake engine's own (nothing for an
//! attempt it never made, or from a worker it did not assign it to); one dial
//! at a time, and one terminal per git operation; every hello on a channel
//! the worker holds open, offering no slot once it is shutting down, and
//! listing exactly the runs it was given whose answers the engine has not
//! acknowledged, but those it gave up, with those it answered followed by
//! their answers; an answer sent only on a channel open, the same each time
//! it is sent again; a run cancelled for contact only once the worker has
//! been out of reach past its grace, by the engine only once the engine has
//! cancelled its attempt, and for a shutdown only once told to; an agent
//! spawned in a workspace io has, and started for an attempt not answered,
//! with the attempt's snapshot, in the tree each repository was checked out
//! at, which for saved work is the last save that landed; an inbound event
//! only for its agent's attempt; nothing an agent hears holding the forge
//! identity; no relayed answer to a run after the cancel its attempt's
//! cancel sent down to it (behind what waited for it); no git
//! operation in a workspace while its run's agent may be running, but the
//! push it asked for, and none at all by a hold whose run has answered; a run
//! answered only once its agent has gone and nothing of git runs for it;
//! every operation's deadline and identity; a branch moved only by a
//! fast-forward; what landed exactly the tree the agent left when it asked to
//! push, and saved work exactly the tree it left. Once it settles: nothing in
//! flight, every process tree gone and read to its end; the engine with
//! nothing outstanding and no alarm, every item closed unless the worker shut
//! down, and each attempt's answer taken once, however often it was sent,
//! every one having reached it but those the worker gave up and refusals lost
//! in flight (a refusal goes once); and the worker with nothing live in any
//! sub-model, no alarm armed but a dial of a worker shut, every slot free and
//! every answer acknowledged or given up.

pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{Git, Network, Settings, Stats, World};
