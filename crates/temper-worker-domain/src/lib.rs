//! The domain layer of the temper worker (programming-model.md, section 4;
//! worker-domain.md, section 3): the worker loop's entry point.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Domain`] they are given. Time and randomness are
//! inputs; every effect, from dialling the engine to spawning an agent or
//! running git, is a [`Request`] the layers below carry out, and its outcome
//! comes back later as an [`Event`].
//!
//! It is the root domain over a tree of child domains (4.5), and the only one
//! that faces the protocol layer:
//!
//! ```text
//! temper-worker-domain                 faces the protocol; the engine link; routes; translates
//! ├── temper-worker-domain-host        hosted runs: admit, prepare, start, relay, park or end
//! ├── temper-worker-domain-checkout    workspaces: prepare, commit, push, save; the cache
//! └── temper-worker-domain-agent       agent processes: spawn, channel, watchdog, cancel then kill
//! ```
//!
//! It owns its children's state and routes each event to the child it is for
//! and each child's requests out: the engine's messages to and from the host,
//! the git and file io to and from the checkout, the agent processes' io to and
//! from the agent child domain. The host is the hub, and the checkout and the
//! agent child domain are its capabilities; the three share no types, so the
//! top level translates the host's workspace requests into the checkout's and
//! its outcomes back, and the host's agent requests into the agent child
//! domain's and what the run says back, through small total functions, keeping
//! for each workspace what each side names it by. The hand-offs are short and
//! acyclic, so each entry point completes them before it returns (the `domain`
//! module).
//!
//! It owns the engine link (worker-domain.md, section 2): it dials the engine,
//! says hello on every channel, with its slots, the workstreams its checkouts
//! hold and the runs it hosts, and delivers each run's one answer, keeping it
//! until the engine acknowledges it and sending it again right after every
//! hello, which lists the run as answered. Past a grace without a channel it
//! cancels every run itself, which saves their work first. Told to shut down,
//! it cancels every run and is done once the engine has every answer, or the
//! answers were given up past the grace.
//!
//! Its records toward the protocol layer ([`Event`], [`Request`]) carry the
//! children's types where the children meet the protocol as they are: the
//! engine's messages are the host's (an assignment, an answer, a bounce), the
//! agent processes' io the agent child domain's, and the git and file io the
//! checkout's.
//!
//! What happens is also told as content-free [`Fact`]s, the child domains' and
//! the link's, gathered into one bounded queue the loop drains
//! ([`Domain::pop_fact`]). What a run tells of itself goes to the engine,
//! opaque, through a bounded queue of its own ([`Domain::pop_told`]), best
//! effort: what does not fit is dropped and counted.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod domain;
mod facts;
mod limits;
mod link;
mod route;
#[cfg(test)]
mod tests;
mod translate;
mod workspace;

pub use boundary::{Event, Hello, Hosted, Phase, Request, Told};
pub use domain::{Domain, fire, max_out, resume, step};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
// The payloads are the children's: a parent may use its children's types.
pub use temper_worker_domain_agent as agent;
pub use temper_worker_domain_checkout as checkout;
pub use temper_worker_domain_host as host;
