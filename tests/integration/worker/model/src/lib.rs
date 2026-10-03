//! A simulated world for the whole worker (programming-style.md, 11;
//! worker-model.md, section 9; testing-pyramid.md, 2.3): the worker's
//! top-level model (`temper_worker_model`) run against the engine's
//! (`temper_engine_model`), each with everything around it, with no protocol
//! and no io. One loop drives both, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for:
//!
//! - **the protocol layers** between the engine and the worker
//!   ([`protocol`]): the engine's boundary toward its workers and the
//!   worker's toward its engine, each in its own terms, translated as they
//!   would be, with what the worker passes through opaque encoded by the
//!   engine world's codecs (charters, outcomes) and the world's own (relayed
//!   calls and their answers);
//! - **the network** between them: one channel at a time, which the worker
//!   dials, with a latency each way, that drops at drawn moments, after which
//!   the engine is out of reach for a drawn while, shorter or longer than the
//!   worker's grace; what is in flight on a channel that drops is lost; a
//!   frame its receiver takes again harmlessly is sent again now and then,
//!   right behind it or late (once the attempt it is for has answered, or
//!   behind the next channel's hello), and the channel stalls now and then;
//! - **the forge:** the fake forge (`temper_forge_model`), as the engine's
//!   world sets it up: protected default branches, CI cued by content, a
//!   repository whose CI never reports; reached by the engine through its
//!   protocol layer as the engine's world plays it (each call with a
//!   deadline, webhooks as hints), by people, and by the worker's git, whose
//!   pushes it observes as any change;
//! - **people** (the engine world's [`temper_engine_model_tests::people`]),
//!   who hand issues in, open sessions and message them, review what CI
//!   passed, correct notes, release what is held and close what they gave
//!   up on; and a person who now and then stops a run;
//! - **the engine's store**: snapshots and traces, in memory;
//! - **agent processes:** the agent world's process trees and scripted agents
//!   (`temper_worker_model_agent_tests`), which work, call the host, push,
//!   wait for inbound events, park, end, fail and misbehave as their script
//!   draws; what their words say is their run's, drawn from their charter as
//!   the engine world's runs draw theirs: their relayed calls, what their
//!   pushes write in the file CI reads, and the outcome they end with; and
//!   they edit the working trees of their workspace before they ask to push,
//!   and now and then as they go, so that what the worker commits is what
//!   they wrote;
//! - **git and files:** the fake disk (`temper_checkout_fake`) and the forge,
//!   through the checkout world's translation of the checkout's operations,
//!   after a latency, racing their deadlines and the cancels of an aborted
//!   prepare, with the faults the world scripts: unreachable repositories,
//!   refused pushes and branch creations, and another party moving a push
//!   branch, or deleting one between attempts; and, in some worlds, a base
//!   the changes land into that the forge does not have until a change's
//!   first checkout creates it. The engine starts a checkout only from a
//!   base or from its item's branch, never from saved work or a commit
//!   (worker-model.md, 4.1), so no preparation here starts from either;
//! - **the shell:** which drains the facts and the run's facts for the
//!   engine, and, in some worlds, tells the worker to shut down at a drawn
//!   moment, stops it once it is done, and starts a new worker, cold, a
//!   while later, which may take more than the last (an upgrade).
//!
//! It checks the worker's contracts as it goes: one dial at a time, and one
//! terminal per git operation; every hello on a channel the worker holds
//! open, offering no slot once it is shutting down, and listing exactly the
//! runs it was given whose answers the engine has not acknowledged, but
//! those it gave up, with those it answered followed by their answers; an
//! answer sent only on a channel open, the same each time it is sent again;
//! a run cancelled for contact only once the worker has been out of reach
//! past its grace, by the engine only once the engine has cancelled its
//! attempt, and for a shutdown only once told to; an agent spawned in a
//! workspace io has, and started for an attempt not answered, with the
//! attempt's snapshot, in the tree each repository was checked out at; an
//! inbound event only for its agent's attempt; nothing an agent hears
//! holding the forge identity; no relayed answer to a run after the cancel
//! its attempt's cancel sent down to it; no git operation in a workspace
//! while its run's agent may be running, but the push it asked for, and none
//! at all by a hold whose run has answered; a run answered only once its
//! agent has gone and nothing of git runs for it; every operation's deadline
//! and identity; a branch the worker moves moved only by a fast-forward;
//! what landed exactly the tree the agent left when it asked to push, and
//! saved work exactly the tree it left. Two referees hold the rest, from
//! outside: the engine world's ([`temper_engine_model_tests::referee`]),
//! over what the forge did and what the engine assigned (nothing lands on a
//! protected branch without green CI and a person's approval, keyed
//! creations made once, attempts that only grow, one live run per item,
//! a person's message reaching a run, where its agent hears it), and this
//! world's ([`referee`]), over what crosses the channel (nothing answered
//! for an attempt the engine never made, an acknowledgement only for an
//! answer it took, every answer it took acknowledged). Once it settles: every story's item closed;
//! nothing in flight; every open item the engine tracks held; every process
//! tree gone and read to its end; and the worker with nothing live in any
//! sub-model, no alarm armed, every slot free and every answer acknowledged
//! or given up. Its own state is bounded too: a world that grows its trace
//! or its deliveries past their bounds, or does not settle in the
//! iterations it is given, fails with its seed.

pub mod protocol;
pub mod referee;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, ENGINE_LIMITS, Git, LIMITS, Network, RELEASE, Settings, Stats, World};
