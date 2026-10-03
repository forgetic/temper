//! A simulated world for the engine's model (engine-model.md, section 14;
//! testing-pyramid.md, 2.2): the engine's top-level model, with all its
//! sub-models beneath it, in one loop, deterministically from a seed, against
//! fakes for everything else.
//!
//! The world owns the clock and the seeds, and stands in for:
//!
//! - **the forge:** the fake forge (`temper_forge_model`), its repositories
//!   protecting their default branch, CI cued by content (a file that says
//!   green), and a second repository whose CI never reports; through the
//!   engine's forge protocol layer as the world plays it ([`translate`]):
//!   the forge sub-model's world's translation of each call, with a
//!   deadline, the payloads the engine names written by the [`codec`]s and
//!   what it wrote found again and decoded, webhooks as hints; with the
//!   fake's faults on as the settings say (late, failing and late-landing
//!   calls, rate limits, lost and late webhooks, a skewed clock);
//! - **workers** ([`workers`]), scripted: they dial in, host runs on
//!   charters carried as bytes, play each run's [`script`] (facts, relayed
//!   calls, pushes through the fake forge's git, waits for inbound events,
//!   parks with snapshots, outcomes as bytes), keep answers until they are
//!   acknowledged, and lose their channel and come back;
//! - **people** ([`people`]), scripted, on the forge and the web: they hand
//!   issues in, open sessions and message them, accept, reject and decide
//!   on plans, review what CI passed, correct notes in the wiki, release
//!   what is held, and close what they gave up on;
//! - **the store** ([`store`]): snapshots and traces, in memory, slow or
//!   failing;
//! - **the engine restarting**, injected by the referee: a new model,
//!   starting cold from the forge and the store.
//!
//! Workers and people read the forge as observed ([`mirror`]): what the fake
//! forge reports it did, never its store, decoded with the codecs. The
//! system worlds reuse the codecs, and the [`names`] their protocol layers
//! give runs and attempts on a worker's channel.
//!
//! It checks the contracts as it goes: every forge call ended once, in the
//! life of the engine that made it; every person's ask answered once, by
//! the engine it went to; every store operation ended once. Its referee
//! ([`referee`]) holds the engine to what the stories expect, from outside:
//! nothing lands on a protected branch without green CI on its exact head
//! and a person's approval of it; writes only to the deployment's
//! repositories; keyed creations and outcomes made once; attempts that
//! only grow, one live run per item, dependencies done before a run;
//! nothing of a plan made before a person accepts it; a person's message
//! reaching a run or its item ending; every story ending within a bound. And
//! the invariants once it settles: nothing in flight, the workers idle,
//! every open item the engine tracks held, and the referee's verdict
//! passed. Its own state is bounded too: a world that grows its trace,
//! its deliveries or the store's traces past their bounds, or does not
//! settle in the iterations it is given, fails with its seed.

pub mod codec;
pub mod deployment;
pub mod mirror;
pub mod names;
pub mod people;
pub mod referee;
pub mod script;
pub mod store;
pub mod translate;
pub mod workers;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, Settings, Stats, World};
