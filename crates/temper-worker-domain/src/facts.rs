//! What the worker tells whoever watches it (worker-domain.md, section 7): the
//! child domains' facts, content-free, gathered after every entry point into
//! one bounded queue the loop drains at its own pace, with the engine link's
//! comings and goings. What does not fit is dropped and counted, and nothing
//! the domain decides depends on it.
//!
//! The host names a run by the engine's names for it, and its capabilities by
//! their clients' tokens, which are the host's: the host's for its run with
//! the agent child domain, the top level's for its workspace with the checkout.
//! What the run itself tells is not among them: it is the engine's, and goes
//! to it as it is ([`crate::Told`]).

use temper_worker_domain_agent as agent;
use temper_worker_domain_checkout as checkout;
use temper_worker_domain_host as host;

/// Something that happened in a child domain, or to the engine link.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// In the host child domain.
    Host { fact: host::Fact },
    /// In the checkout child domain.
    Checkout { fact: checkout::Fact },
    /// In the agent child domain.
    Agent { fact: agent::Fact },
    /// The channel to the engine opened.
    Connected,
    /// The channel to the engine closed.
    Lost,
    /// The channel to the engine stayed down past the grace: every run is
    /// cancelled.
    Grace,
}
