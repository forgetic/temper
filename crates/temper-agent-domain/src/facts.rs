//! What the agent tells whoever watches it (agent-domain.md, section 7): the
//! child domains' facts, content-free, gathered after every entry point into one
//! bounded queue the loop drains at its own pace. What does not fit is dropped
//! and counted, and nothing the domain decides depends on it.
//!
//! The run names a conversation by its token for it, and the sessions name
//! theirs by their opener's, which is the same token: the facts of a
//! conversation and of its session go together.

use temper_agent_domain_run::facts as run;
use temper_agent_domain_session as session;

/// Something that happened in a child domain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// In the run child domain.
    Run { fact: run::Fact },
    /// In the session child domain, or its tools.
    Session { fact: session::Fact },
}
