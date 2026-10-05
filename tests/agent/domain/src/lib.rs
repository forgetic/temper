//! The agent's top-level world (testing.md, 2.1; agent-domain.md,
//! section 3), where it meets the worker's (worker-domain.md, section 9) and
//! the engine's (engine-domain.md, section 14): the engine's root domain, with
//! every child domain beneath it, assigning the steps of the issues people hand
//! in; the worker's root domain hosting their runs, each in an agent process
//! that is a fresh agent domain, with the run and the sessions beneath it and
//! the tools beneath those; and fakes for every other neighbour, driven by one
//! loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for:
//!
//! - **the forge:** one fake forge (`temper_fake_forge_domain`) for everyone,
//!   its repositories the deployment's, each holding the [`fixture`]'s code,
//!   protecting its default branch, CI cued by a file the runs leave green
//!   ([`forge`]); the engine reaches it through its protocol layer as the
//!   engine's world plays it (`temper_legacy_engine_domain_world::translate`, its
//!   payloads written and read by the engine's codecs), each call with a
//!   deadline, with the forge's faults as the settings say (late and failing
//!   calls, rate limits, lost and late webhooks, a skewed clock); io's git
//!   reaches it directly, through the checkout world's translation of the
//!   checkout's operations (`temper_worker_checkout_world::translate`), failing
//!   to reach it as the settings say; with another party moving a push branch,
//!   and repositories that refuse pushes;
//! - **the channel between the engine and the worker:** the protocol layers
//!   on both sides ([`protocol`]), which keep its order; a charter goes as the
//!   engine's codec writes it, and an outcome comes back the same way;
//! - **people** ([`desk`], [`people`]): they hand in issues whose step is an
//!   agent step or a change, as the engine finds them tracked with a record
//!   of its own; a reviewer approves each pull request the engine opens once
//!   CI passed on its exact head; and a person stops a run now and then,
//!   through the engine's web;
//! - **the engine's store:** the engine's world's, in memory, slow or
//!   failing;
//! - **io's agent processes:** each spawn is an agent domain sized for one
//!   run, which the world drives as its process's shell would; its pipes
//!   carry the channel between the worker and the run, which the agent's
//!   protocol layer translates on its side ([`channel`]), taking the world's
//!   frame off the charter, decoding the engine's charter into the run's and
//!   adding where io put each repository; a terminate or a kill ends the
//!   process at once, with what it had in flight, and its exit, its reap and
//!   the end of its pipe follow; one that has answered exits of itself;
//! - **the disk** ([`temper_fake_checkout`]): the working trees the worker
//!   prepares are the roots the agents' tools read and write, so what the
//!   worker commits is what the agent left;
//! - **the agents' other neighbours:** a fake LLM provider's domain, which
//!   plays scripted jobs, cued by the guidance of the step a run is for, or
//!   wanders at random ([`temper_legacy_fake_llm_domain`], [`script`]), with the
//!   protocol layers on both sides, which only the world sees both vocabularies
//!   of ([`translate`]); and io, running the tools' file operations and
//!   commands and the runs' own looks and checks on the disk.
//!
//! It aims at the paths that cross the domains, which their own worlds cannot
//! see: a step the engine assigns reaching an agent as a charter, a run's
//! conversations as sessions, their tools at work in the checkout the worker
//! prepared, the run's asks and answers as tool calls and results, its
//! checks and pushes, sub-agents nested in their askers' calls, a budget
//! spent across sessions, cancels and deadlines cascading down the tree, how
//! each run ends, through the worker, at the engine, and what the engine
//! makes of it on the forge: a pull request reviewed and merged, a verdict or
//! a report posted, a step retried or held. It checks the contracts as it
//! goes: the agent's (one answer per start, given once the run's
//! conversations have all ended; one terminal per request; a push only once
//! the checks the run found passed; an answer that fits what happened to its
//! run and adds up what its conversations used; no conversation opened past
//! the budget, and no more than one completion each after it; only main
//! offered `finish`, and a sub-agent the families it was asked with); the
//! worker's (an agent spawned in a workspace io has, one at a time; every
//! git operation's deadline and identity, and none in a workspace while its
//! agent runs but the push it asked for; a branch moved only by a
//! fast-forward; each attempt answered once); the engine's (every forge call
//! ended once, every person's ask answered once, every store operation ended
//! once). And the invariants once it settles: every issue handed in ended or
//! held for a person, every attempt answered once, the worker holds nothing,
//! every agent process exited, was reaped and read to its end, each agent
//! that exited of itself held nothing, nothing is in flight, and facts that
//! add up to what crossed the boundary unless some were dropped or an agent
//! was killed. Its own state is bounded too: a world that grows its trace or
//! its deliveries past their bounds, or does not settle in the iterations it
//! is given, fails with its seed.
//!
//! What the scenarios expect is held by two referees (testing.md,
//! 5.2), which see only what the fakes see and end each run with their
//! verdicts. Where the worker and the agent meet ([`referee`]): the worker
//! commits exactly the tree the agent left; the engine hears the outcome the
//! run accepted, or how it failed, a cancel being the worker's to report, and
//! posts on its item, within a bound, exactly the outcome a run ended with,
//! and no other; what landed is on the forge; what the engine merges is what
//! a run landed, keeping every file the run changed; an issue is held for a
//! person only for its runs' failures, a person's stop or its plan's reasons,
//! and for its writes or its record only where the forge or the store were
//! scripted to fail; every assignment is answered within the wall time the
//! worker's watchdog gives a run, and a margin; and every issue handed in
//! ends, closed or held, within a bound. The engine's
//! (`temper_legacy_engine_domain_world::referee`): nothing lands on a protected
//! branch without green CI on its exact head and a person's approval of it,
//! writes only to the deployment's repositories, keyed creations and outcomes
//! made once, attempts that only grow and one live run per item. Its stories
//! are kept by the first referee instead, as an issue held for a person ends
//! here as surely as one closed, and the engine's referee meets a story only
//! on a close.
//!
//! Not exercised, as the agent's side does not do it yet: sessions, inbound
//! events and a run's waiting for them, parking and snapshots, and relayed
//! calls (forge reads and outlets). Nor, on this world's channel, a worker
//! that loses its channel or an engine that restarts: the engine's and the
//! whole worker's worlds drive those. Nor the engine's bounds on spend: what
//! a run spent does not cross the channel, so the engine's rules over spend
//! (per run, per goal, per deployment) are not enforced here; the agent
//! bounds each run's spend by its charter's budget, its tokens split across
//! the kinds ([`channel::split`]).

pub mod channel;
pub mod desk;
pub mod fixture;
pub mod forge;
pub mod people;
pub mod protocol;
pub mod referee;
pub mod script;
pub mod translate;
mod world;

pub use script::{JOBS, Job};
pub use temper_world::Span;
pub use world::{
    Allowed, BUDGET, CALM, ENGINE, FORGE, LIMITS, MOST, Run, Settings, Stats, TIGHT, Told, WORKER, World, config,
};
